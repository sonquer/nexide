//! Generational, per-isolate table of in-flight HTTP request slots.
//!
//! Multiplexed dispatch (TASK Plan-B / B-3) requires that every
//! `op_nexide_*` op be parameterised by a `RequestId` so that several
//! handlers can run concurrently inside the same V8 isolate without
//! stomping on each other's [`RequestSlot`] / [`ResponseSlot`].
//!
//! The table uses an arena-style backing store with explicit
//! generation counters so a stale id (e.g. an op invoked after a
//! request has already been settled and recycled) is rejected as
//! `Stale` instead of silently routing to the wrong handler — the
//! "ABA" failure mode rubber-duck flagged for raw `u32` ids.
//!
//! The store is intentionally `!Send` and `!Sync` — it lives inside a
//! V8 isolate's `OpState`, which is single-threaded by construction.
//! No internal locking is needed; access is serialised by the
//! `Rc<RefCell<OpState>>` style table held in the engine.

use thiserror::Error;
use tokio::sync::oneshot;

use super::request::RequestSlot;
use super::response::{ResponsePayload, ResponseSlot};

/// Outcome returned to the dispatcher's awaiter when a request
/// completes.
///
/// A multiplexed dispatcher (B-3) hands the [`oneshot::Sender`] to the
/// table at insertion time and receives one of these variants when JS
/// finishes (`sendEnd`) or fails (`finishError`).
pub type CompletionResult = Result<ResponsePayload, RequestFailure>;

/// Reason why a request never produced a successful response.
///
/// Variants are kept narrow so the HTTP shield can map them to
/// distinct status codes (`502` vs `504`) without inspecting strings.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum RequestFailure {
    /// The JS handler reported an error via `op_nexide_finish_error`.
    #[error("handler error: {0}")]
    Handler(String),

    /// The dispatcher cancelled the request (timeout or client gone).
    #[error("request cancelled")]
    Cancelled,

    /// The hosting isolate's pump task died (panic, event-loop error
    /// or worker recycle) before the request could settle. Mapped to
    /// a `502 Bad Gateway` by the HTTP shield.
    #[error("pump died before request completed: {0}")]
    PumpDied(String),
}

/// Sender half handed to the table at request insertion.
///
/// Wrapped in `Option` so the table can `take()` the sender once the
/// request settles even when the entry is still kept around for late
/// op cleanup (rubber-duck: split `response_ready` from
/// `handler_settled`).
type Completion = oneshot::Sender<CompletionResult>;

/// Per-request state stored in the [`DispatchTable`].
///
/// Bundles the request and response slots with the dispatcher's
/// completion sender so any op layer can route work without a
/// secondary lookup. Owning the sender here lets `op_nexide_send_end`
/// resolve the awaiter without touching channel internals.
#[derive(Debug)]
pub struct InFlight {
    request: RequestSlot,
    response: ResponseSlot,
    completion: Option<Completion>,
}

impl InFlight {
    /// Returns the read-only request facing JS.
    #[must_use]
    pub const fn request(&self) -> &RequestSlot {
        &self.request
    }

    /// Returns the request slot for body-read ops (Command).
    pub const fn request_mut(&mut self) -> &mut RequestSlot {
        &mut self.request
    }

    /// Returns the response sink for `send_*` ops (Command).
    pub const fn response_mut(&mut self) -> &mut ResponseSlot {
        &mut self.response
    }

    /// Removes the completion sender so the caller can resolve the
    /// awaiter exactly once. Subsequent calls return `None`.
    pub const fn take_completion(&mut self) -> Option<Completion> {
        self.completion.take()
    }

    /// Builds a fresh in-flight entry for a newly-arrived request.
    fn new(request: RequestSlot, completion: Completion) -> Self {
        Self {
            request,
            response: ResponseSlot::new(),
            completion: Some(completion),
        }
    }
}

/// Generational identifier for an entry in [`DispatchTable`].
///
/// `index` is the slot offset; `generation` rolls forward each time
/// a slot is reused, so a stale id (held by a late op invocation
/// after the request was settled and the slot recycled) is detected
/// instead of routing to a different handler.
///
/// The struct is `Copy` so it can be passed cheaply through op call
/// boundaries and through V8 ↔ Rust glue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId {
    index: u32,
    generation: u32,
}

impl RequestId {
    /// Constructs a `RequestId` from raw `(index, generation)` parts.
    ///
    /// Used at the op boundary, where JS supplies the two halves as
    /// separate primitives. Production code should always obtain ids
    /// through [`DispatchTable::insert`] — round-tripping is fine
    /// (the table validates both halves on lookup).
    #[must_use]
    pub const fn from_parts(index: u32, generation: u32) -> Self {
        Self { index, generation }
    }

    /// Returns the slot index. Pure (Query).
    #[must_use]
    pub const fn index(self) -> u32 {
        self.index
    }

    /// Returns the slot generation counter. Pure (Query).
    #[must_use]
    pub const fn generation(self) -> u32 {
        self.generation
    }
}

/// Internal slot states for the arena.
#[derive(Debug)]
enum Slot {
    /// Free slot ready to receive a new request. `generation` is the
    /// generation that the next inserted [`InFlight`] will be tagged
    /// with; `next_free` chains free slots into a singly-linked list.
    Vacant {
        generation: u32,
        next_free: Option<u32>,
    },
    /// Slot currently hosting an in-flight request. `generation`
    /// matches the [`RequestId`] handed to JS — any op call carrying
    /// a different generation is [`DispatchError::Stale`].
    Occupied {
        generation: u32,
        inflight: InFlight,
    },
}

/// Failure modes for [`DispatchTable`] lookups.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DispatchError {
    /// `index` is past the end of the arena. Indicates JS sent us a
    /// fabricated id; treated as a hard error.
    #[error("dispatch table: index {0} out of range")]
    OutOfRange(u32),

    /// Slot exists but its generation no longer matches the supplied
    /// [`RequestId`]. The id was reused for a newer request — treat
    /// the late op call as a no-op-with-error, never panic.
    #[error("dispatch table: id (index {0}, gen {1}) is stale")]
    Stale(u32, u32),

    /// Slot exists and the generation matches, but the arena itself
    /// has nothing in flight at that slot. Means the request was
    /// already settled and removed; usually paired with a JS-side
    /// race after `sendEnd`.
    #[error("dispatch table: slot {0} has been released")]
    Released(u32),
}

/// Single-threaded arena keyed by [`RequestId`].
///
/// Lives in the V8 isolate's `OpState`. Insertion grows the arena on
/// demand or reuses a vacant slot via the free list; removal pushes
/// the slot back onto the free list and bumps its generation, so the
/// next insertion at the same index hands out a fresh
/// [`RequestId`] that compares unequal to any prior id.
#[derive(Debug, Default)]
pub struct DispatchTable {
    slots: Vec<Slot>,
    free_head: Option<u32>,
    in_flight: u32,
}

impl DispatchTable {
    /// Creates an empty table.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            slots: Vec::new(),
            free_head: None,
            in_flight: 0,
        }
    }

    /// Inserts a new in-flight request and returns a fresh
    /// [`RequestId`].
    ///
    /// Reuses a previously-released slot if the free list is
    /// non-empty; otherwise extends the arena by one element. The
    /// returned id is unique even after slot reuse thanks to the
    /// per-slot generation counter.
    ///
    /// # Panics
    ///
    /// Panics if the table has already grown past
    /// `u32::MAX as usize` slots — that would mean the same isolate
    /// has handled `~4` billion concurrent requests without any
    /// settling, which is structurally impossible while the
    /// per-isolate semaphore caps in-flight work at
    /// [`crate::DEFAULT_MAX_INFLIGHT_PER_ISOLATE`] (or its env
    /// override).
    pub fn insert(&mut self, request: RequestSlot, completion: Completion) -> RequestId {
        let inflight = InFlight::new(request, completion);
        if let Some(index) = self.free_head {
            self.free_head = self.advance_free_list(index);
            let generation = self.activate_vacant(index, inflight);
            self.in_flight = self.in_flight.saturating_add(1);
            return RequestId { index, generation };
        }

        let index = u32::try_from(self.slots.len()).expect("dispatch table exceeds u32 indices");
        self.slots.push(Slot::Occupied {
            generation: 0,
            inflight,
        });
        self.in_flight = self.in_flight.saturating_add(1);
        RequestId {
            index,
            generation: 0,
        }
    }

    /// Removes the entry at `id`, returning the [`InFlight`] so the
    /// caller can drain its completion sender.
    ///
    /// # Errors
    ///
    /// See [`DispatchError`] — covers out-of-range indices, stale
    /// generations and already-released slots.
    pub fn remove(&mut self, id: RequestId) -> Result<InFlight, DispatchError> {
        let prev_free_head = self.free_head;
        let slot = self.slot_mut(id)?;
        let Slot::Occupied {
            generation,
            inflight,
        } = std::mem::replace(
            slot,
            Slot::Vacant {
                generation: 0,
                next_free: None,
            },
        )
        else {
            unreachable!("slot_mut already verified the slot is occupied");
        };

        let next_generation = generation.wrapping_add(1);
        *slot = Slot::Vacant {
            generation: next_generation,
            next_free: prev_free_head,
        };
        self.free_head = Some(id.index);
        self.in_flight = self.in_flight.saturating_sub(1);
        Ok(inflight)
    }

    /// Returns a shared reference to the entry at `id`.
    ///
    /// # Errors
    ///
    /// See [`DispatchError`].
    pub fn get(&self, id: RequestId) -> Result<&InFlight, DispatchError> {
        let slot = self
            .slots
            .get(id.index as usize)
            .ok_or(DispatchError::OutOfRange(id.index))?;
        match slot {
            Slot::Occupied {
                generation,
                inflight,
            } if *generation == id.generation => Ok(inflight),
            Slot::Occupied { generation, .. } => {
                Err(DispatchError::Stale(id.index, *generation))
            }
            Slot::Vacant { .. } => Err(DispatchError::Released(id.index)),
        }
    }

    /// Returns a mutable reference to the entry at `id`.
    ///
    /// # Errors
    ///
    /// See [`DispatchError`].
    pub fn get_mut(&mut self, id: RequestId) -> Result<&mut InFlight, DispatchError> {
        let slot = self.slot_mut(id)?;
        let Slot::Occupied { inflight, .. } = slot else {
            unreachable!("slot_mut already verified the slot is occupied");
        };
        Ok(inflight)
    }

    /// Returns the count of currently-occupied slots. Pure (Query).
    #[must_use]
    pub const fn in_flight(&self) -> u32 {
        self.in_flight
    }

    /// Returns the total number of slots ever allocated. Pure (Query)
    /// — exposed for telemetry and tests, not for hot-path routing.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Drains every still-occupied slot and sends `failure` to each
    /// pending completion sender.
    ///
    /// Used by the worker loop on isolate shutdown / pump death:
    /// every in-flight request must be failed (typically with
    /// [`RequestFailure::PumpDied`]) so callers awaiting the
    /// dispatcher's oneshot do not hang until their own client
    /// timeout. After draining the table is logically empty —
    /// every slot is recycled and its generation bumped, defeating
    /// any ABA race where a late JS op tries to address a settled
    /// id.
    ///
    /// Returns the number of failed requests (for telemetry).
    /// `O(capacity)` — intended for shutdown paths only.
    ///
    /// # Panics
    ///
    /// Panics if the table holds more than `u32::MAX` slots, which
    /// is impossible in practice (slot allocation is gated by the
    /// per-isolate semaphore, capped well below 1 M).
    pub fn fail_all<F>(&mut self, mut failure: F) -> usize
    where
        F: FnMut() -> RequestFailure,
    {
        let mut failed = 0_usize;
        for (raw_index, slot) in self.slots.iter_mut().enumerate() {
            let Slot::Occupied { generation, .. } = *slot else {
                continue;
            };
            let placeholder = Slot::Vacant {
                generation: 0,
                next_free: None,
            };
            let Slot::Occupied {
                generation: _,
                inflight,
            } = std::mem::replace(slot, placeholder)
            else {
                unreachable!("slot was Occupied immediately above");
            };
            let next_generation = generation.wrapping_add(1);
            *slot = Slot::Vacant {
                generation: next_generation,
                next_free: self.free_head,
            };
            self.free_head = Some(u32::try_from(raw_index).expect("slot index fits in u32"));
            let mut inflight = inflight;
            if let Some(completion) = inflight.take_completion() {
                let _ = completion.send(Err(failure()));
                failed += 1;
            }
        }
        self.in_flight = 0;
        failed
    }

    /// Validates `id` and returns a mutable handle to the underlying
    /// `Slot`. Centralises all the error checking so [`Self::remove`]
    /// and [`Self::get_mut`] share one source of truth.
    fn slot_mut(&mut self, id: RequestId) -> Result<&mut Slot, DispatchError> {
        let slot = self
            .slots
            .get_mut(id.index as usize)
            .ok_or(DispatchError::OutOfRange(id.index))?;
        match slot {
            Slot::Occupied { generation, .. } if *generation == id.generation => Ok(slot),
            Slot::Occupied { generation, .. } => {
                Err(DispatchError::Stale(id.index, *generation))
            }
            Slot::Vacant { .. } => Err(DispatchError::Released(id.index)),
        }
    }

    /// Removes `index` from the head of the free list and returns the
    /// new head pointer.
    fn advance_free_list(&self, index: u32) -> Option<u32> {
        let Slot::Vacant { next_free, .. } = self
            .slots
            .get(index as usize)
            .expect("free list always points at a real slot")
        else {
            unreachable!("free list never points at an occupied slot");
        };
        *next_free
    }

    /// Replaces the `Vacant` slot at `index` with an `Occupied` entry
    /// inheriting the slot's pending generation. Returns the
    /// generation that the resulting [`RequestId`] must carry.
    fn activate_vacant(&mut self, index: u32, inflight: InFlight) -> u32 {
        let slot = self
            .slots
            .get_mut(index as usize)
            .expect("free list always points at a real slot");
        let Slot::Vacant { generation, .. } = *slot else {
            unreachable!("free list never points at an occupied slot");
        };
        *slot = Slot::Occupied {
            generation,
            inflight,
        };
        generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::request::RequestMeta;
    use bytes::Bytes;

    fn slot() -> RequestSlot {
        RequestSlot::new(
            RequestMeta::try_new("GET", "/").unwrap(),
            Vec::new(),
            Bytes::new(),
        )
    }

    fn completion() -> oneshot::Sender<CompletionResult> {
        let (tx, _rx) = oneshot::channel();
        tx
    }

    #[test]
    fn fresh_table_reports_zero_in_flight() {
        let table = DispatchTable::new();
        assert_eq!(table.in_flight(), 0);
        assert_eq!(table.capacity(), 0);
    }

    #[test]
    fn insert_returns_unique_ids_and_grows_capacity() {
        let mut table = DispatchTable::new();
        let id1 = table.insert(slot(), completion());
        let id2 = table.insert(slot(), completion());
        assert_ne!(id1, id2);
        assert_eq!(id1.generation(), 0);
        assert_eq!(id2.generation(), 0);
        assert_eq!(table.in_flight(), 2);
        assert_eq!(table.capacity(), 2);
    }

    #[test]
    fn remove_recycles_slot_and_bumps_generation() {
        let mut table = DispatchTable::new();
        let first = table.insert(slot(), completion());
        let _ = table.remove(first).unwrap();
        let second = table.insert(slot(), completion());
        assert_eq!(second.index(), first.index());
        assert_eq!(second.generation(), first.generation().wrapping_add(1));
        assert_eq!(table.capacity(), 1);
    }

    #[test]
    fn stale_id_after_recycle_is_rejected() {
        let mut table = DispatchTable::new();
        let stale = table.insert(slot(), completion());
        let _ = table.remove(stale).unwrap();
        let _fresh = table.insert(slot(), completion());
        assert_eq!(
            table.get(stale).unwrap_err(),
            DispatchError::Stale(stale.index(), stale.generation().wrapping_add(1))
        );
    }

    #[test]
    fn released_id_is_rejected_until_reuse() {
        let mut table = DispatchTable::new();
        let id = table.insert(slot(), completion());
        let _ = table.remove(id).unwrap();
        assert_eq!(table.get(id).unwrap_err(), DispatchError::Released(id.index()));
    }

    #[test]
    fn get_on_unknown_index_is_out_of_range() {
        let table = DispatchTable::new();
        let phantom = RequestId {
            index: 42,
            generation: 0,
        };
        assert_eq!(table.get(phantom).unwrap_err(), DispatchError::OutOfRange(42));
    }

    #[test]
    fn take_completion_is_consumed_once() {
        let mut table = DispatchTable::new();
        let id = table.insert(slot(), completion());
        assert!(table.get_mut(id).unwrap().take_completion().is_some());
        assert!(table.get_mut(id).unwrap().take_completion().is_none());
    }

    #[test]
    fn free_list_chains_multiple_releases_lifo() {
        let mut table = DispatchTable::new();
        let a = table.insert(slot(), completion());
        let b = table.insert(slot(), completion());
        let c = table.insert(slot(), completion());

        let _ = table.remove(a).unwrap();
        let _ = table.remove(b).unwrap();
        let _ = table.remove(c).unwrap();
        assert_eq!(table.in_flight(), 0);

        let next = table.insert(slot(), completion());
        assert_eq!(next.index(), c.index());
        let next2 = table.insert(slot(), completion());
        assert_eq!(next2.index(), b.index());
        let next3 = table.insert(slot(), completion());
        assert_eq!(next3.index(), a.index());
    }

    #[test]
    fn double_remove_reports_stale_after_recycle() {
        let mut table = DispatchTable::new();
        let id = table.insert(slot(), completion());
        let _ = table.remove(id).unwrap();
        let _ = table.insert(slot(), completion());
        let err = table.remove(id).unwrap_err();
        assert!(matches!(err, DispatchError::Stale(_, _)));
    }

    #[test]
    fn capacity_does_not_shrink_after_remove() {
        let mut table = DispatchTable::new();
        let a = table.insert(slot(), completion());
        let b = table.insert(slot(), completion());
        let _ = table.remove(a).unwrap();
        let _ = table.remove(b).unwrap();
        assert_eq!(table.capacity(), 2);
        assert_eq!(table.in_flight(), 0);
    }

    #[test]
    fn request_failure_handler_carries_message() {
        let failure = RequestFailure::Handler("boom".to_owned());
        assert_eq!(failure.to_string(), "handler error: boom");
    }

    #[test]
    fn request_failure_cancelled_displays_canonical_message() {
        assert_eq!(RequestFailure::Cancelled.to_string(), "request cancelled");
    }

    #[test]
    fn request_failure_pump_died_includes_reason() {
        let failure = RequestFailure::PumpDied("v8 event loop error".to_owned());
        assert_eq!(
            failure.to_string(),
            "pump died before request completed: v8 event loop error"
        );
    }

    #[test]
    fn fail_all_drains_every_occupied_slot() {
        let mut table = DispatchTable::new();
        let (tx_a, mut rx_a) = oneshot::channel();
        let (tx_b, mut rx_b) = oneshot::channel();
        let id_a = table.insert(slot(), tx_a);
        let id_b = table.insert(slot(), tx_b);
        assert_eq!(table.in_flight(), 2);

        let drained = table.fail_all(|| RequestFailure::PumpDied("boom".to_owned()));

        assert_eq!(drained, 2);
        assert_eq!(table.in_flight(), 0);
        assert!(matches!(
            rx_a.try_recv(),
            Ok(Err(RequestFailure::PumpDied(ref msg))) if msg == "boom"
        ));
        assert!(matches!(
            rx_b.try_recv(),
            Ok(Err(RequestFailure::PumpDied(ref msg))) if msg == "boom"
        ));
        assert!(matches!(
            table.get(id_a),
            Err(DispatchError::Stale(_, _) | DispatchError::Released(_))
        ));
        assert!(matches!(
            table.get(id_b),
            Err(DispatchError::Stale(_, _) | DispatchError::Released(_))
        ));
    }

    #[test]
    fn fail_all_bumps_generation_so_old_ids_are_stale() {
        let mut table = DispatchTable::new();
        let (tx, _rx) = oneshot::channel();
        let old_id = table.insert(slot(), tx);

        let _ = table.fail_all(|| RequestFailure::Cancelled);
        let (tx_new, _rx_new) = oneshot::channel();
        let new_id = table.insert(slot(), tx_new);

        assert_eq!(new_id.index, old_id.index, "slot must be reused");
        assert_ne!(
            new_id.generation, old_id.generation,
            "generation must advance to defeat ABA"
        );
        assert!(matches!(table.get(old_id), Err(DispatchError::Stale(_, _))));
    }

    #[test]
    fn fail_all_on_empty_table_is_noop() {
        let mut table = DispatchTable::new();
        assert_eq!(table.fail_all(|| RequestFailure::Cancelled), 0);
        assert_eq!(table.in_flight(), 0);
    }

    #[test]
    fn fail_all_skips_slots_whose_completion_was_taken() {
        let mut table = DispatchTable::new();
        let (tx, _rx) = oneshot::channel();
        let id = table.insert(slot(), tx);
        let inflight = table.get_mut(id).expect("freshly inserted");
        let _ = inflight.take_completion();

        assert_eq!(table.fail_all(|| RequestFailure::Cancelled), 0);
        assert_eq!(table.in_flight(), 0);
        assert!(matches!(
            table.get(id),
            Err(DispatchError::Stale(_, _) | DispatchError::Released(_))
        ));
    }
}
