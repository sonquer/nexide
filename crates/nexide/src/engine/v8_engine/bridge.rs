//! Per-isolate bridge state - the single point of contact between
//! `__nexide` ops and the host runtime.
//!
//! [`BridgeState`] is parked in V8's isolate slot so any
//! [`v8::FunctionCallback`] can pull it out without indirection. Every
//! op in [`super::ops_bridge`] reads / mutates this struct, never the
//! enclosing [`super::V8Engine`] directly - the engine merely owns
//! the state's lifetime.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;

use tokio::sync::mpsc;

use super::async_ops::{Completion, CompletionChannel};
use super::handle_table::HandleTable;
use crate::engine::cjs::CjsResolver;
use crate::ops::{DispatchTable, EnvOverlay, FsHandle, ProcessConfig, RequestQueue, WorkerId};

pub(crate) type NapiWorkItem = Box<dyn FnOnce(&mut v8::PinScope<'_, '_>) + Send + 'static>;

/// TCP socket entry: shared, async-locked stream so reads and writes
/// from JS land on the same FD without racing. `Rc` because each
/// isolate is single-threaded; `tokio::sync::Mutex` because read /
/// write futures must be awaited.
pub(super) type TcpStreamSlot = std::rc::Rc<tokio::sync::Mutex<tokio::net::TcpStream>>;

/// TCP listener entry: shared so multiple `accept` ops can target
/// the same listener concurrently if the JS code chooses to.
pub(super) type TcpListenerSlot = std::rc::Rc<tokio::net::TcpListener>;

/// TLS-wrapped client stream slot. Same shared+locked pattern as
/// [`TcpStreamSlot`] so reads and writes serialise on the same FD.
pub(super) type TlsStreamSlot =
    std::rc::Rc<tokio::sync::Mutex<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>>;

/// Streaming HTTP response body slot. The receiver is held under a
/// `Mutex` because successive `read` ops poll it across awaits; only
/// one read may be in flight at a time per response which matches the
/// Node Readable contract.
pub(super) type HttpResponseSlot = std::rc::Rc<
    tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<Result<Vec<u8>, crate::ops::NetError>>>,
>;

/// Tracked child process plus optional captured pipes. Wrapped in a
/// `Mutex` so concurrent stdin writes / stdout reads from JS do not
/// race on the underlying handles.
pub(super) struct ChildSlot {
    pub child: tokio::sync::Mutex<tokio::process::Child>,
    pub stdin: tokio::sync::Mutex<Option<tokio::process::ChildStdin>>,
    pub stdout: tokio::sync::Mutex<Option<tokio::process::ChildStdout>>,
    pub stderr: tokio::sync::Mutex<Option<tokio::process::ChildStderr>>,
}

pub(super) type ChildSlotHandle = std::rc::Rc<ChildSlot>;

/// Streaming zlib slot. JS feeds chunks one at a time through
/// `op_zlib_feed` and finalises with `op_zlib_finish`. The
/// underlying `ZlibStream::finish` consumes `self`, so the slot is
/// removed from the handle table before finalisation.
pub(super) type ZlibSlot = std::rc::Rc<std::cell::RefCell<Option<crate::ops::ZlibStream>>>;

/// Owner of the host-side state every op closes over.
///
/// Stored in a single `Rc<RefCell<…>>` so V8 callbacks (which receive
/// only an `Isolate*`) can fetch it via `isolate.get_slot::<BridgeStateHandle>()`.
/// Interior mutability is required because callbacks are entered with
/// only `&Isolate`, never `&mut`.
pub(super) struct BridgeState {
    pub queue: Rc<RequestQueue>,
    pub dispatch_table: DispatchTable,
    pub worker_id: WorkerId,
    pub process: Option<ProcessConfig>,
    pub env_overlay: EnvOverlay,
    pub fs: Option<FsHandle>,
    pub cjs: Option<Arc<dyn CjsResolver>>,
    pub cjs_root: String,
    pub exit_requested: Option<i32>,
    pub pending_pop: VecDeque<v8::Global<v8::PromiseResolver>>,
    pub pending_pop_batch: VecDeque<(v8::Global<v8::PromiseResolver>, u32)>,
    pub async_completions_tx: mpsc::UnboundedSender<Completion>,
    pub async_completions_rx: Rc<RefCell<mpsc::UnboundedReceiver<Completion>>>,
    pub napi_work_tx: mpsc::UnboundedSender<NapiWorkItem>,
    pub napi_work_rx: Rc<RefCell<mpsc::UnboundedReceiver<NapiWorkItem>>>,
    pub napi_wakeup: Arc<tokio::sync::Notify>,
    pub net_streams: HandleTable<TcpStreamSlot>,
    pub net_listeners: HandleTable<TcpListenerSlot>,
    pub tls_streams: HandleTable<TlsStreamSlot>,
    pub http_responses: HandleTable<HttpResponseSlot>,
    pub child_processes: HandleTable<ChildSlotHandle>,
    pub zlib_streams: HandleTable<ZlibSlot>,
}

impl Default for BridgeState {
    fn default() -> Self {
        let channel = CompletionChannel::new();
        let (napi_tx, napi_rx) = mpsc::unbounded_channel();
        Self {
            queue: Rc::new(RequestQueue::new()),
            dispatch_table: DispatchTable::default(),
            worker_id: WorkerId::new(0, true),
            process: None,
            env_overlay: EnvOverlay::default(),
            fs: None,
            cjs: None,
            cjs_root: crate::engine::cjs::ROOT_PARENT.to_owned(),
            exit_requested: None,
            pending_pop: VecDeque::new(),
            pending_pop_batch: VecDeque::new(),
            async_completions_tx: channel.sender(),
            async_completions_rx: channel.receiver(),
            napi_work_tx: napi_tx,
            napi_work_rx: Rc::new(RefCell::new(napi_rx)),
            napi_wakeup: Arc::new(tokio::sync::Notify::new()),
            net_streams: HandleTable::default(),
            net_listeners: HandleTable::default(),
            tls_streams: HandleTable::default(),
            http_responses: HandleTable::default(),
            child_processes: HandleTable::default(),
            zlib_streams: HandleTable::default(),
        }
    }
}

/// Handle parked in the isolate slot. Cheap to clone (`Rc`).
#[derive(Clone)]
pub(crate) struct BridgeStateHandle(pub(super) Rc<RefCell<BridgeState>>);

impl Default for BridgeStateHandle {
    fn default() -> Self {
        Self(Rc::new(RefCell::new(BridgeState::default())))
    }
}

impl BridgeStateHandle {
    /// Wraps a fully populated state.
    #[must_use]
    pub(super) fn new(state: BridgeState) -> Self {
        Self(Rc::new(RefCell::new(state)))
    }
}

/// Returns the bridge state attached to `isolate`.
///
/// # Panics
///
/// Panics when the engine layer forgot to install a [`BridgeStateHandle`]
/// before evaluating JavaScript. Real ops only fire after `boot_internal`
/// has finished, so reaching this branch is a programmer error.
pub(super) fn from_isolate(isolate: &v8::Isolate) -> BridgeStateHandle {
    isolate
        .get_slot::<BridgeStateHandle>()
        .cloned()
        .expect("BridgeStateHandle must be installed before any op fires")
}

/// Returns a clonable sender for the per-isolate N-API work channel.
pub(crate) fn napi_work_sender(
    isolate: &v8::Isolate,
) -> mpsc::UnboundedSender<NapiWorkItem> {
    from_isolate(isolate).0.borrow().napi_work_tx.clone()
}

/// Returns the cross-thread wake-up handle used to notify the engine
/// pump that new N-API work is available.
pub(crate) fn napi_wakeup(isolate: &v8::Isolate) -> Arc<tokio::sync::Notify> {
    from_isolate(isolate).0.borrow().napi_wakeup.clone()
}
