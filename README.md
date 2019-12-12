# broadcaster

<!-- cargo-sync-readme start -->

broadcaster provides a wrapper for any Stream and Sink implementing the mpsc pattern to enable
broadcasting items. This means that any item sent will be received by every receiver, not just
the first to check (like most mpmc streams). As an example:
```rust
use broadcaster::BroadcastChannel;
use futures_util::StreamExt;

let mut chan = BroadcastChannel::new();
chan.send(&5i32).await?;
assert_eq!(chan.next().await, Some(5));

let mut chan2 = chan.clone();
chan2.send(&6i32).await?;
assert_eq!(chan.next().await, Some(6));
assert_eq!(chan2.next().await, Some(6));
```

<!-- cargo-sync-readme end -->
