//! Per-isolate FIFO of [`RequestId`]s waiting for the JS pump.
//!
//! Bridges the dispatcher (Rust → push) with the JS request pump
//! (JS → async pop via [`op_nexide_pop_request`]). Living entirely
//! inside the V8 isolate's `OpState`, the queue is single-threaded
//! by construction - `Rc<RefCell<...>>` access is fine and no
//! cross-thread synchronisation is required.
//!
//! The queue keeps two pieces of state:
//! * `pending`: a `VecDeque<RequestId>` consumed in-order by the
//!   pump.
//! * `notify`: a [`tokio::sync::Notify`] woken on every push so a
//!   parked pump future resumes immediately.
//!
//! [`op_nexide_pop_request`]: super::extension::op_nexide_pop_request

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use tokio::sync::Notify;

use super::dispatch_table::RequestId;

/// Cheap-to-clone handle to the per-isolate request queue.
///
/// Cloning the handle shares the underlying buffer; the cost is a
/// pair of [`Rc`] refcount bumps. The queue is intentionally
/// `!Send + !Sync` - it lives on the isolate's local thread and is
/// never observed from elsewhere.
#[derive(Debug, Clone, Default)]
pub struct RequestQueue {
    pending: Rc<RefCell<VecDeque<RequestId>>>,
    notify: Rc<Notify>,
}

impl RequestQueue {
    /// Constructs an empty queue.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Pushes `id` and wakes the pump (Command).
    ///
    /// Notifies a single waiter - the pump is the only consumer by
    /// design (rubber-duck blocker: enforce single-waiter semantics
    /// in [`Self::pop`]).
    pub fn push(&self, id: RequestId) {
        self.pending.borrow_mut().push_back(id);
        self.notify.notify_one();
    }

    /// Awaits the next id (Command).
    ///
    /// Resolves immediately when the queue is non-empty; otherwise
    /// suspends on the queue's [`Notify`] until a producer calls
    /// [`Self::push`]. The loop tolerates spurious wake-ups.
    #[allow(clippy::future_not_send)]
    pub async fn pop(&self) -> RequestId {
        loop {
            if let Some(id) = self.pending.borrow_mut().pop_front() {
                return id;
            }
            self.notify.notified().await;
        }
    }

    /// Awaits at least one id and drains up to `max` (Command).
    ///
    /// Resolves with a non-empty `Vec` of all currently-queued ids
    /// (capped at `max`). When the queue is empty the future
    /// suspends on [`Notify`] exactly like [`Self::pop`]; the loop
    /// tolerates spurious wake-ups. `max == 0` is a programming
    /// error and is treated as `1` so the caller never receives an
    /// empty `Vec`.
    ///
    /// Designed for the batched pump strategy: the JS
    /// pump issues `__dispatch` for every id in the batch within a
    /// single microtask cycle, amortising the per-pop op crossing
    /// cost when the queue is hot.
    #[allow(clippy::future_not_send)]
    pub async fn pop_batch(&self, max: usize) -> Vec<RequestId> {
        let cap = max.max(1);
        loop {
            {
                let mut pending = self.pending.borrow_mut();
                if !pending.is_empty() {
                    let take = pending.len().min(cap);
                    let mut out = Vec::with_capacity(take);
                    for _ in 0..take {
                        if let Some(id) = pending.pop_front() {
                            out.push(id);
                        }
                    }
                    return out;
                }
            }
            self.notify.notified().await;
        }
    }

    /// Returns the count of currently-queued ids. Pure (Query) -
    /// telemetry / tests only.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pending.borrow().len()
    }

    /// Drains up to `max` ids without ever suspending (Command).
    ///
    /// Returns an empty `Vec` when the queue is empty - the caller
    /// is expected to fall back to [`Self::pop_batch`] (which awaits)
    /// in that case. `max == 0` is a programming error and is
    /// treated as `1` for symmetry with [`Self::pop_batch`].
    ///
    /// Used by the JS pump's sync-drain hot path: while the queue is
    /// non-empty the pump dispatches batches without crossing back
    /// to Tokio, avoiding a per-batch `await` even under burst load.
    #[must_use]
    pub fn try_pop_batch(&self, max: usize) -> Vec<RequestId> {
        let cap = max.max(1);
        let mut pending = self.pending.borrow_mut();
        if pending.is_empty() {
            return Vec::new();
        }
        let take = pending.len().min(cap);
        let mut out = Vec::with_capacity(take);
        for _ in 0..take {
            if let Some(id) = pending.pop_front() {
                out.push(id);
            }
        }
        out
    }

    /// Returns `true` iff the queue is empty. Pure (Query).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.borrow().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(i: u32, g: u32) -> RequestId {
        RequestId::from_parts(i, g)
    }

    #[tokio::test]
    async fn pop_returns_pushed_ids_in_order() {
        let queue = RequestQueue::new();
        queue.push(id(1, 0));
        queue.push(id(2, 0));
        queue.push(id(3, 0));

        assert_eq!(queue.pop().await, id(1, 0));
        assert_eq!(queue.pop().await, id(2, 0));
        assert_eq!(queue.pop().await, id(3, 0));
        assert!(queue.is_empty());
    }

    #[tokio::test]
    async fn pop_resumes_when_push_arrives() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let queue = RequestQueue::new();
                let q2 = queue.clone();
                let popper = tokio::task::spawn_local(async move { q2.pop().await });

                tokio::task::yield_now().await;
                queue.push(id(7, 1));

                let observed = popper.await.expect("popper completed");
                assert_eq!(observed, id(7, 1));
            })
            .await;
    }

    #[tokio::test]
    async fn len_tracks_push_pop_balance() {
        let queue = RequestQueue::new();
        assert_eq!(queue.len(), 0);
        queue.push(id(0, 0));
        queue.push(id(1, 0));
        assert_eq!(queue.len(), 2);
        let _ = queue.pop().await;
        assert_eq!(queue.len(), 1);
    }

    #[tokio::test]
    async fn pop_batch_drains_up_to_cap() {
        let queue = RequestQueue::new();
        for i in 0..5 {
            queue.push(id(i, 0));
        }
        let batch = queue.pop_batch(3).await;
        assert_eq!(batch.len(), 3);
        assert_eq!(batch[0], id(0, 0));
        assert_eq!(batch[2], id(2, 0));
        assert_eq!(queue.len(), 2);
    }

    #[tokio::test]
    async fn pop_batch_returns_all_when_below_cap() {
        let queue = RequestQueue::new();
        queue.push(id(9, 0));
        queue.push(id(10, 0));
        let batch = queue.pop_batch(64).await;
        assert_eq!(batch.len(), 2);
        assert!(queue.is_empty());
    }

    #[tokio::test]
    async fn pop_batch_treats_zero_cap_as_one() {
        let queue = RequestQueue::new();
        queue.push(id(1, 0));
        queue.push(id(2, 0));
        let batch = queue.pop_batch(0).await;
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0], id(1, 0));
    }

    #[tokio::test]
    async fn pop_batch_resumes_when_push_arrives() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let queue = RequestQueue::new();
                let q2 = queue.clone();
                let popper = tokio::task::spawn_local(async move { q2.pop_batch(8).await });

                tokio::task::yield_now().await;
                queue.push(id(7, 1));
                queue.push(id(8, 1));

                let batch = popper.await.expect("popper completed");
                assert_eq!(batch.len(), 2);
            })
            .await;
    }

    #[tokio::test]
    async fn try_pop_batch_returns_empty_on_empty_queue() {
        let queue = RequestQueue::new();
        let batch = queue.try_pop_batch(8);
        assert!(batch.is_empty());
    }

    #[tokio::test]
    async fn try_pop_batch_drains_up_to_cap_without_blocking() {
        let queue = RequestQueue::new();
        for i in 0..5 {
            queue.push(id(i, 0));
        }
        let batch = queue.try_pop_batch(3);
        assert_eq!(batch.len(), 3);
        assert_eq!(batch[0], id(0, 0));
        assert_eq!(queue.len(), 2);
    }

    #[tokio::test]
    async fn try_pop_batch_treats_zero_cap_as_one() {
        let queue = RequestQueue::new();
        queue.push(id(1, 0));
        queue.push(id(2, 0));
        let batch = queue.try_pop_batch(0);
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0], id(1, 0));
    }
}
