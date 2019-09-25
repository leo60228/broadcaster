#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! broadcaster provides a wrapper for any Stream and Sink implementing the mpsc pattern to enable
//! broadcasting items. This means that any item sent will be received by every receiver, not just
//! the first to check (like most mpmc streams). As an example:
//! ```rust
//! use broadcaster::BroadcastChannel;
//!
//! # use futures_executor::block_on;
//! # block_on(async {
//! let mut chan = BroadcastChannel::new();
//! chan.send(&5i32).await?;
//! assert_eq!(chan.recv().await, Some(5));
//!
//! let mut chan2 = chan.clone();
//! chan2.send(&6i32).await?;
//! assert_eq!(chan.recv().await, Some(6));
//! assert_eq!(chan2.recv().await, Some(6));
//! # Ok::<(), futures_channel::mpsc::SendError>(())
//! # }).unwrap();
//! ```

use futures_core::{future::*, stream::*};
use futures_sink::Sink;
use futures_util::sink::SinkExt;
use futures_util::stream::StreamExt;
use futures_util::try_future::try_join_all;
use slab::Slab;
use std::sync::Arc;
use parking_lot::RwLock;

#[cfg(feature = "default-channels")]
use futures_channel::mpsc::*;

/// A broadcast channel, wrapping any clonable Stream and Sink to have every message sent to every
/// receiver.
pub struct BroadcastChannel<
    T,
    #[cfg(feature = "default-channels")] S = UnboundedSender<T>,
    #[cfg(feature = "default-channels")] R = UnboundedReceiver<T>,
    #[cfg(not(feature = "default-channels"))] S,
    #[cfg(not(feature = "default-channels"))] R,
> where
    T: Send + Clone + 'static,
    S: Send + Sync + Unpin + Clone + Sink<T>,
    R: Unpin + Stream<Item = T>,
{
    senders: Arc<RwLock<Slab<S>>>,
    sender_key: usize,
    receiver: R,
    ctor: Arc<dyn Fn() -> (S, R) + Send + Sync>,
}

#[cfg(feature = "default-channels")]
impl<T: Send + Clone> BroadcastChannel<T> {
    /// Create a new unbounded channel. Requires the `default-channels` feature.
    pub fn new() -> Self {
        let (tx, rx) = unbounded();
        let mut slab = Slab::new();
        let sender_key = slab.insert(tx);
        Self {
            senders: Arc::new(RwLock::new(slab)),
            sender_key,
            receiver: rx,
            ctor: Arc::new(unbounded),
        }
    }
}

#[cfg(feature = "default-channels")]
impl<T: Send + Clone> BroadcastChannel<T, Sender<T>, Receiver<T>> {
    /// Create a new bounded channel with a specific capacity. Requires the `default-channels` feature.
    pub fn with_cap(cap: usize) -> Self {
        let (tx, rx) = channel(cap);
        let mut slab = Slab::new();
        let sender_key = slab.insert(tx);
        Self {
            senders: Arc::new(RwLock::new(slab)),
            sender_key,
            receiver: rx,
            ctor: Arc::new(move || channel(cap)),
        }
    }
}

impl<T, S, R> BroadcastChannel<T, S, R>
where
    T: Send + Clone + 'static,
    S: Send + Sync + Unpin + Clone + Sink<T>,
    R: Unpin + Stream<Item = T>,
{
    /// Construct a new channel from any Sink and Stream. For proper functionality, cloning a
    /// Sender will create a new sink that also sends data to Receiver.
    pub fn with_ctor(ctor: Arc<dyn Fn() -> (S, R) + Send + Sync>) -> Self {
        let (tx, rx) = ctor();
        let mut slab = Slab::new();
        let sender_key = slab.insert(tx);
        Self {
            senders: Arc::new(RwLock::new(slab)),
            sender_key,
            receiver: rx,
            ctor,
        }
    }

    /// Send an item to all receivers in the channel, including this one. This is because
    /// futures-channel does not support comparing a sender and receiver. If this is not the
    /// desired behavior, you must handle it yourself.
    pub async fn send(&self, item: &T) -> Result<(), S::Error> {
        // can't be split up because of how async/await works
        let mut senders: Slab<S> =
            Slab::clone(&*self.senders.read());

        try_join_all(senders.iter_mut().map(|(_, s)| s.send(item.clone()))).await?;
        Ok(())
    }

    /// Receive a single value from the channel.
    pub fn recv(&mut self) -> impl Future<Output = Option<T>> + '_ {
        self.receiver.next()
    }
}

impl<T, S, R> Clone for BroadcastChannel<T, S, R>
where
    T: Send + Clone + 'static,
    S: Send + Sync + Unpin + Clone + Sink<T>,
    R: Unpin + Stream<Item = T>,
{
    fn clone(&self) -> Self {
        let (tx, rx) = (self.ctor)();
        let sender_key = self
            .senders
            .write()
            .insert(tx);

        Self {
            senders: self.senders.clone(),
            sender_key,
            receiver: rx,
            ctor: self.ctor.clone(),
        }
    }
}

impl<T, S, R> Drop for BroadcastChannel<T, S, R>
where
    T: Send + Clone + 'static,
    S: Send + Sync + Unpin + Clone + Sink<T>,
    R: Unpin + Stream<Item = T>,
{
    fn drop(&mut self) {
        self.senders
            .write()
            .remove(self.sender_key);
    }
}

#[cfg(all(feature = "default-channels", test))]
mod test {
    use super::BroadcastChannel;
    use futures_executor::block_on;
    use futures_util::future::FutureExt;

    #[test]
    fn send_recv() {
        let mut chan = BroadcastChannel::new();
        block_on(chan.send(&5)).unwrap();
        assert_eq!(block_on(chan.recv()), Some(5));
    }

    #[test]
    fn now_or_never() {
        let fut = async {
            let mut chan = BroadcastChannel::new();
            chan.send(&5i32).await?;
            assert_eq!(chan.recv().await, Some(5));

            let mut chan2 = chan.clone();
            chan2.send(&6i32).await?;
            assert_eq!(chan.recv().await, Some(6));
            assert_eq!(chan2.recv().await, Some(6));
            Ok::<(), futures_channel::mpsc::SendError>(())
        };
        fut.now_or_never().unwrap().unwrap();
    }

    fn assert_impl_send<T: Send>() {}
    fn assert_impl_sync<T: Sync>() {}
    fn assert_val_impl_send<T: Send>(_val: &T) {}
    fn assert_val_impl_sync<T: Sync>(_val: &T) {}

    #[test]
    fn recv_two() {
        let fut = async {
            let mut chan = BroadcastChannel::new();
            chan.send(&5i32).await?;
            assert_eq!(chan.recv().await, Some(5));

            let mut chan2 = chan.clone();
            chan2.send(&6i32).await?;
            assert_eq!(chan.recv().await, Some(6));
            assert_eq!(chan2.recv().await, Some(6));
            Ok::<(), futures_channel::mpsc::SendError>(())
        };
        assert_val_impl_send(&fut);
        assert_val_impl_sync(&fut);
        block_on(fut).unwrap();
    }

    #[test]
    fn send_sync() {
        assert_impl_send::<BroadcastChannel<i32>>();
        assert_impl_sync::<BroadcastChannel<i32>>();
    }
}
