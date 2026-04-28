//! Generic infrastructure for asynchronous V8 ops.
//!
//! V8 callbacks are synchronous: they receive a `&mut PinScope` and
//! must return immediately. To expose Node-style async APIs (DNS,
//! TCP, child process spawning, …) we follow a two-stage pattern:
//!
//! 1. **Stage 1 - schedule.** The op callback creates a fresh
//!    [`v8::PromiseResolver`], stashes its [`v8::Global`] handle, and
//!    spawns a `tokio` task that performs the I/O off-isolate. The
//!    callback returns the resolver's promise to JavaScript while the
//!    work is still in flight.
//! 2. **Stage 2 - settle.** When the I/O finishes the spawned task
//!    pushes a [`Completion`] onto the per-isolate channel held in
//!    [`super::bridge::BridgeState`]. The completion bundles the
//!    resolver handle with a [`Settler`] closure that - once the
//!    isolate thread re-enters the engine pump - receives the active
//!    `PinScope` and either resolves or rejects the promise with a
//!    fully-typed `v8::Value`.
//!
//! All marshalling from Rust types to V8 values therefore happens on
//! the isolate thread, where `v8::Local` values are valid. The
//! `tokio` task only needs to carry around the `Send` payload of the
//! I/O result; this keeps the interface free of unsafe pointer
//! gymnastics and works equally well for `Send` and `!Send` work via
//! [`tokio::task::spawn_local`].

use std::cell::RefCell;
use std::rc::Rc;

use tokio::sync::mpsc;

/// Resolves or rejects `resolver` from inside the isolate thread.
///
/// The closure runs after the spawned `tokio` task completes and the
/// engine pump re-enters JavaScript. It is given the active scope so
/// it can construct fresh `v8::Local` values to hand back to V8.
pub(super) type Settler =
    Box<dyn for<'s, 'a> FnOnce(&mut v8::PinScope<'s, 'a>, v8::Local<'s, v8::PromiseResolver>)>;

/// One pending async-op completion ready to be delivered to V8.
pub(super) struct Completion {
    resolver: v8::Global<v8::PromiseResolver>,
    settler: Settler,
}

impl Completion {
    /// Pairs `resolver` with the `settler` that will fulfil it.
    pub(super) fn new(resolver: v8::Global<v8::PromiseResolver>, settler: Settler) -> Self {
        Self { resolver, settler }
    }
}

/// Per-isolate channel used by async ops to schedule completions.
///
/// `Sender` is cloned into every spawned op task; the matching
/// receiver is drained on each pump tick by [`drain`].
pub(super) struct CompletionChannel {
    tx: mpsc::UnboundedSender<Completion>,
    rx: Rc<RefCell<mpsc::UnboundedReceiver<Completion>>>,
}

impl CompletionChannel {
    /// Builds a fresh, empty channel.
    pub(super) fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            tx,
            rx: Rc::new(RefCell::new(rx)),
        }
    }

    /// Returns a clonable sender for spawned op tasks.
    pub(super) fn sender(&self) -> mpsc::UnboundedSender<Completion> {
        self.tx.clone()
    }

    /// Returns a shared handle to the receiver.
    ///
    /// Cloned into the bridge state at boot so [`drain`] can borrow
    /// it on every pump tick without reaching into [`super::engine::V8Engine`].
    pub(super) fn receiver(&self) -> Rc<RefCell<mpsc::UnboundedReceiver<Completion>>> {
        Rc::clone(&self.rx)
    }
}

impl Default for CompletionChannel {
    fn default() -> Self {
        Self::new()
    }
}

/// Drains every ready completion and applies its settler.
///
/// Designed to be cheap when the queue is empty (a single
/// `try_recv` returning `Empty`). Called by the engine pump before
/// the microtask checkpoint so freshly-resolved promises see their
/// `.then` continuations on the same tick.
pub(super) fn drain<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    rx: &Rc<RefCell<mpsc::UnboundedReceiver<Completion>>>,
) {
    loop {
        let next = rx.borrow_mut().try_recv();
        let completion = match next {
            Ok(c) => c,
            Err(_) => return,
        };
        let resolver = v8::Local::new(scope, &completion.resolver);
        (completion.settler)(scope, resolver);
    }
}

/// Builds a [`Settler`] that rejects the promise with a Node-style
/// `Error` whose `.code` property is set to `code`.
///
/// Both `message` and `code` are captured by value so the closure is
/// `'static` and can be carried across `tokio` task boundaries.
pub(super) fn reject_with_code(message: String, code: &'static str) -> Settler {
    Box::new(move |scope, resolver| {
        let msg = v8::String::new(scope, &message).unwrap_or_else(|| v8::String::empty(scope));
        let err = v8::Exception::error(scope, msg);
        if let Ok(err_obj) = TryInto::<v8::Local<v8::Object>>::try_into(err) {
            let code_key =
                v8::String::new(scope, "code").unwrap_or_else(|| v8::String::empty(scope));
            let code_val = v8::String::new(scope, code).unwrap_or_else(|| v8::String::empty(scope));
            err_obj.set(scope, code_key.into(), code_val.into());
        }
        resolver.reject(scope, err);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `CompletionChannel::new` must hand back a sender and receiver
    /// that observe one another. We do not exercise actual V8
    /// resolution here - that requires a fully-booted isolate and is
    /// covered by the per-op integration tests.
    #[test]
    fn channel_starts_empty() {
        let ch = CompletionChannel::new();
        let _tx = ch.sender();
        let rx = ch.receiver();
        assert!(matches!(
            rx.borrow_mut().try_recv(),
            Err(mpsc::error::TryRecvError::Empty),
        ));
    }
}
