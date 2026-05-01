//! JS ↔ Rust op bridge for nexide HTTP requests.
//!
//! Two-way binding between the Axum shield (Rust) and the App Router
//! handler (JavaScript): [`RequestSlot`] is planted by Rust before
//! the handler runs and the [`ResponseSlot`] is populated by
//! JavaScript and resolved back through the per-request completion
//! channel held in [`DispatchTable`]. The traits ([`RequestSource`],
//! [`ResponseSink`]) keep the op-layer code independent of the
//! concrete buffers (Dependency Inversion).

mod dispatch_table;
mod dns;
mod fs_sync;
mod http_client;
mod log;
mod net;
mod process;
mod process_spawn;
mod queue;
mod request;
mod response;
mod tls;
mod zlib_stream;

pub use dispatch_table::{
    CompletionResult, DispatchError, DispatchTable, InFlight, RequestFailure, RequestId,
};
pub use dns::{
    DnsError, LookupFamily, LookupResult, MxRecord, SrvRecord, lookup as dns_lookup,
    resolve_cname as dns_resolve_cname, resolve_mx as dns_resolve_mx, resolve_ns as dns_resolve_ns,
    resolve_srv as dns_resolve_srv, resolve_txt as dns_resolve_txt, resolve4 as dns_resolve4,
    resolve6 as dns_resolve6, reverse as dns_reverse,
};
pub use fs_sync::{
    DirEntry, FsBackend, FsError, FsHandle, FsStat, MemoryFs, PathSandbox, RealFs, Sandbox,
};
pub use http_client::{
    HttpHeader, HttpRequest, ResponseHandle as HttpResponseHandle, request as http_request,
};
pub use log::WorkerId;
pub use net::{
    AddressInfo, NetError, accept as net_accept, connect as net_connect, listen as net_listen,
    read_chunk as net_read_chunk, write_all as net_write_all,
};
pub use process::{
    EnvOverlay, EnvSource, ExitRequested, MapEnv, OsEnv, ProcessConfig, ProcessConfigBuilder,
};
pub use process_spawn::{
    ChildHandle, ExitInfo, SpawnRequest, StdioMode, kill as proc_kill, read_pipe as proc_read_pipe,
    spawn as proc_spawn, wait as proc_wait, write_pipe as proc_write_pipe,
};
pub use queue::RequestQueue;
pub use request::{
    HeaderPair, REQUEST_META_MAX_LEN, RequestMeta, RequestMetaError, RequestSlot, RequestSource,
};
pub use response::{ResponseError, ResponseHead, ResponsePayload, ResponseSink, ResponseSlot};
pub use tls::{
    connect as tls_connect, read_chunk as tls_read_chunk, shutdown as tls_shutdown,
    upgrade as tls_upgrade, write_all as tls_write_all,
};
pub use zlib_stream::{ZlibKind, ZlibStream, parse_kind as parse_zlib_kind};

/// JavaScript bridge installed on `globalThis.__nexide`. The script
/// wires the V8 callbacks to a small ergonomic façade that the
/// Next.js handler can consume.
pub const JS_BRIDGE: &str = include_str!("../../runtime/polyfills/nexide_bridge.js");
