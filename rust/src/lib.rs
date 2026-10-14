// SPDX-FileCopyrightText: 2023 Foundation Devices Inc.
//
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::error::update_last_error;
use arti::proxy::{run_proxy_with_listeners, ListenProtocols};
use arti_client::config::CfgPath;
use arti_client::{DormantMode, TorClient, TorClientConfig};
use lazy_static::lazy_static;
use std::ffi::{c_char, c_void, CStr};
use std::future::Future;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::{io, ptr};
use tokio::runtime::{Builder, Runtime};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tor_rtcompat::tokio::TokioNativeTlsRuntime;
use tor_rtcompat::{NetStreamListener, NetStreamProvider, TcpListenOptions, ToplevelBlockOn};

pub use crate::error::{tor_last_error_message, tor_string_free};
#[cfg(not(target_os = "windows"))]
pub use crate::util::tor_get_nofile_limit;
#[cfg(not(target_os = "windows"))]
pub use crate::util::tor_set_nofile_limit;

#[macro_use]
mod error;
mod util;

lazy_static! {
    static ref RUNTIME: io::Result<Runtime> = Builder::new_multi_thread().enable_all().build();
}

/// How long freeing a client waits for its background tasks to finish.
const CLIENT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// How many more OS-assigned ports to try when another process holds the
/// IPv6 twin of the assigned port, before failing.
const IPV6_BIND_RETRIES: u32 = 3;

#[repr(C)]
pub struct Tor {
    client: *mut c_void,
    proxy: *mut c_void,
    port: u16,
}

/// The tokio runtime a single client and its background tasks run on.
///
/// Arti's background tasks hold the client's runtime, so a runtime the client
/// owns is never dropped and the tasks keep the directory cache and state
/// files open after the client is freed. Each client instead runs on a
/// runtime owned here and shut down when this is dropped.
struct ClientRuntime(Option<Runtime>);

impl ClientRuntime {
    fn new() -> io::Result<Self> {
        Ok(Self(Some(
            Builder::new_multi_thread().enable_all().build()?,
        )))
    }

    /// An arti runtime that spawns onto this runtime without owning it.
    fn arti(&self) -> io::Result<TokioNativeTlsRuntime> {
        let _guard = self.0.as_ref().expect("runtime is live").enter();
        TokioNativeTlsRuntime::current()
    }
}

impl Drop for ClientRuntime {
    fn drop(&mut self) {
        if let Some(runtime) = self.0.take() {
            let started = Instant::now();
            runtime.shutdown_timeout(CLIENT_SHUTDOWN_TIMEOUT);
            if started.elapsed() >= CLIENT_SHUTDOWN_TIMEOUT {
                // Blocking tasks cannot be interrupted, so a slow storage
                // write may still hold its file open after this returns.
                log::warn!(
                    "Tor client runtime did not shut down within {:?}; files may remain open",
                    CLIENT_SHUTDOWN_TIMEOUT
                );
            }
        }
    }
}

/// What a client handle passed across FFI points to.
struct ClientHandle {
    // Fields drop in order, so the client is released before its runtime
    // shuts down.
    client: Arc<TorClient<TokioNativeTlsRuntime>>,
    cancel_bootstrap: watch::Sender<bool>,
    _runtime: ClientRuntime,
}

/// A reserved SOCKS listener that becomes active only after bootstrap.
enum ProxyHandle {
    /// Bound but idle until `start`.
    Reserved {
        client: Arc<TorClient<TokioNativeTlsRuntime>>,
        listeners: Vec<Listener>,
    },
    /// The accept loop is running.
    Running(JoinHandle<anyhow::Result<()>>),
}

impl ProxyHandle {
    fn new(client: Arc<TorClient<TokioNativeTlsRuntime>>, listeners: Vec<Listener>) -> Self {
        Self::Reserved { client, listeners }
    }

    fn start(&mut self, rt: &Runtime) {
        if let Self::Reserved { client, listeners } = self {
            let task = start_proxy(rt, Arc::clone(client), std::mem::take(listeners));
            *self = Self::Running(task);
        }
    }
}

fn into_client_handle(
    client: Arc<TorClient<TokioNativeTlsRuntime>>,
    runtime: ClientRuntime,
) -> *mut c_void {
    let (cancel_bootstrap, _) = watch::channel(false);
    Box::into_raw(Box::new(ClientHandle {
        client,
        cancel_bootstrap,
        _runtime: runtime,
    })) as *mut c_void
}

/// Create a Tor client and a localhost SOCKS proxy.
///
/// # Safety
/// `state_dir` and `cache_dir` must point to valid, NUL-terminated strings
/// for the duration of the call. Each returned non-null handle must be
/// released exactly once with its corresponding cleanup function.
#[no_mangle]
pub unsafe extern "C" fn tor_start(
    socks_port: u16,
    state_dir: *const c_char,
    cache_dir: *const c_char,
) -> Tor {
    let err_ret = Tor {
        client: ptr::null_mut(),
        proxy: ptr::null_mut(),
        port: 0,
    };

    let state_dir = unwrap_or_return!(CStr::from_ptr(state_dir).to_str(), err_ret);
    let cache_dir = unwrap_or_return!(CStr::from_ptr(cache_dir).to_str(), err_ret);

    let client_runtime = unwrap_or_return!(ClientRuntime::new(), err_ret);
    let runtime = unwrap_or_return!(client_runtime.arti(), err_ret);

    // Reserve the port before initializing storage or bootstrapping. On any
    // later failure these listeners are dropped and the port is released.
    let rt = unwrap_or_return!(proxy_runtime(), err_ret);
    let (listeners, socks_port) =
        unwrap_or_return!(bind_localhost(rt, &runtime, socks_port), err_ret);

    let mut cfg_builder = TorClientConfig::builder();
    cfg_builder
        .storage()
        // The paths come from the caller as-is; `CfgPath::new` would expand
        // `$VAR` and `~` and reject directories whose names contain them.
        .state_dir(CfgPath::new_literal(state_dir))
        .cache_dir(CfgPath::new_literal(cache_dir));
    cfg_builder.address_filter().allow_onion_addrs(true);

    let cfg = unwrap_or_return!(cfg_builder.build(), err_ret);

    let client = unwrap_or_return!(
        runtime.block_on(async {
            TorClient::with_runtime(runtime.clone())
                .config(cfg)
                .create_unbootstrapped()
        }),
        err_ret
    );

    let proxy_handle_box = Box::new(ProxyHandle::new(Arc::clone(&client), listeners));

    Tor {
        client: into_client_handle(client, client_runtime),
        proxy: Box::into_raw(proxy_handle_box) as *mut c_void,
        port: socks_port,
    }
}

/// Borrow the client handle produced by [`tor_start`] without taking ownership.
///
/// Ownership stays with the caller until [`tor_client_free`] is called, so the
/// handle can be passed to any number of FFI calls in between. Null yields
/// `None` so callers can report it instead of aborting the process.
unsafe fn client_handle_ref<'a>(client: *mut c_void) -> Option<&'a ClientHandle> {
    (client as *const ClientHandle).as_ref()
}

fn null_client_error() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "client handle is null")
}

/// The outcome of [`tor_client_bootstrap`].
///
/// Every code other than `Ok` also sets the message returned by
/// [`tor_last_error_message`].
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TorBootstrapStatus {
    /// The client bootstrapped.
    Ok = 0,
    /// [`tor_client_cancel_bootstrap`] was called on the handle.
    Cancelled = 1,
    /// The deadline passed before bootstrap finished.
    TimedOut = 2,
    /// Arti reported an error.
    Failed = 3,
    /// The client handle was null.
    NullClient = 4,
}

/// Bootstrap the client, returning on cancellation or after `timeout_millis`.
/// A null `client` returns `NullClient`.
///
/// # Safety
/// `client` must be null or a live handle returned by [`tor_start`].
/// It must not be freed while this call is in progress.
#[no_mangle]
pub unsafe extern "C" fn tor_client_bootstrap(
    client: *mut c_void,
    timeout_millis: u64,
) -> TorBootstrapStatus {
    let Some(handle) = client_handle_ref(client) else {
        update_last_error(null_client_error());
        return TorBootstrapStatus::NullClient;
    };
    let client = &handle.client;
    let cancelled = handle.cancel_bootstrap.subscribe();

    let result = client.runtime().block_on(controlled_bootstrap(
        client.bootstrap(),
        cancelled,
        Duration::from_millis(timeout_millis),
    ));

    match result {
        Ok(()) => TorBootstrapStatus::Ok,
        Err(error) => {
            let status = error.status();
            update_last_error(error);
            status
        }
    }
}

/// Why [`controlled_bootstrap`] returned without bootstrapping.
#[derive(Debug)]
enum BootstrapError {
    Cancelled,
    TimedOut(Duration),
    Failed(io::Error),
}

impl BootstrapError {
    fn status(&self) -> TorBootstrapStatus {
        match self {
            BootstrapError::Cancelled => TorBootstrapStatus::Cancelled,
            BootstrapError::TimedOut(_) => TorBootstrapStatus::TimedOut,
            BootstrapError::Failed(_) => TorBootstrapStatus::Failed,
        }
    }
}

impl std::fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BootstrapError::Cancelled => write!(f, "Tor bootstrap cancelled"),
            BootstrapError::TimedOut(timeout) => write!(
                f,
                "Tor bootstrap timed out after {} ms",
                timeout.as_millis()
            ),
            BootstrapError::Failed(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for BootstrapError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BootstrapError::Failed(error) => error.source(),
            _ => None,
        }
    }
}

async fn controlled_bootstrap<F, E>(
    bootstrap: F,
    mut cancelled: watch::Receiver<bool>,
    timeout: Duration,
) -> Result<(), BootstrapError>
where
    F: Future<Output = Result<(), E>>,
    E: std::error::Error + Send + Sync + 'static,
{
    if *cancelled.borrow() {
        return Err(BootstrapError::Cancelled);
    }

    tokio::select! {
        biased;
        result = cancelled.changed() => {
            result.map_err(|_| {
                BootstrapError::Failed(io::Error::other("Tor bootstrap cancellation channel closed"))
            })?;
            Err(BootstrapError::Cancelled)
        }
        // Keep the error itself, not its message, so its source chain reaches
        // update_last_error.
        result = bootstrap => result.map_err(|error| BootstrapError::Failed(io::Error::other(error))),
        _ = tokio::time::sleep(timeout) => Err(BootstrapError::TimedOut(timeout)),
    }
}

/// Ask an in-progress bootstrap to return promptly. Null is accepted.
///
/// Cancellation is sticky: once this is called, every later
/// [`tor_client_bootstrap`] on the same handle returns `Cancelled` at once.
/// To bootstrap again, free the handle with [`tor_client_free`] and create a
/// new one with [`tor_start`].
///
/// # Safety
/// `client` must be null or a live handle returned by [`tor_start`]. It may be
/// called concurrently with [`tor_client_bootstrap`], but not with or after
/// [`tor_client_free`].
#[no_mangle]
pub unsafe extern "C" fn tor_client_cancel_bootstrap(client: *mut c_void) {
    if let Some(handle) = client_handle_ref(client) {
        handle.cancel_bootstrap.send_replace(true);
    }
}

/// Change the client's dormant mode. A null `client` is ignored and sets the
/// message returned by [`tor_last_error_message`].
///
/// # Safety
/// `client` must be null or a live handle returned by [`tor_start`].
/// It must not be freed while this call is in progress.
#[no_mangle]
pub unsafe extern "C" fn tor_client_set_dormant(client: *mut c_void, soft_mode: bool) {
    let Some(handle) = client_handle_ref(client) else {
        update_last_error(null_client_error());
        return;
    };

    let dormant_mode = if soft_mode {
        DormantMode::Soft
    } else {
        DormantMode::Normal
    };
    handle.client.set_dormant(dormant_mode);
}

/// Release the client handle returned by [`tor_start`].
///
/// This shuts down the client's background tasks and closes its state and
/// cache files, which can block for a few seconds. Stop the proxy first: it
/// keeps its own reference to the client, but cannot serve connections once
/// the client is freed.
///
/// # Safety
/// `client` must be null or a live handle returned by [`tor_start`].
/// No other call may use the handle concurrently with or after this call.
/// It must not be called from within an async runtime.
#[no_mangle]
pub unsafe extern "C" fn tor_client_free(client: *mut c_void) {
    if client.is_null() {
        return;
    }

    drop(Box::from_raw(client as *mut ClientHandle));
}

/// Start accepting SOCKS connections on the listeners reserved by [`tor_start`].
/// Repeated calls are harmless.
///
/// # Safety
/// `proxy` must be a live, non-null proxy handle returned by [`tor_start`]. It
/// must not be used concurrently or after [`tor_proxy_stop`].
#[no_mangle]
pub unsafe extern "C" fn tor_proxy_start(proxy: *mut c_void) -> bool {
    if proxy.is_null() {
        update_last_error(io::Error::new(
            io::ErrorKind::InvalidInput,
            "proxy handle is null",
        ));
        return false;
    }
    let rt = unwrap_or_return!(proxy_runtime(), false);
    let proxy = &mut *(proxy as *mut ProxyHandle);
    proxy.start(rt);
    true
}

/// Stop the proxy, or release its reserved listeners if it was not started.
/// Null is accepted.
///
/// This waits for the accept loop to exit, so the listeners and the loop's
/// reference to the client are released by the time it returns. Call it
/// before [`tor_client_free`].
///
/// # Safety
/// `proxy` must be null or a live proxy handle returned by [`tor_start`].
/// A non-null handle must be passed to this function only once.
/// It must not be called from within an async runtime.
#[no_mangle]
pub unsafe extern "C" fn tor_proxy_stop(proxy: *mut c_void) {
    if proxy.is_null() {
        return;
    }

    if let ProxyHandle::Running(task) = *Box::from_raw(proxy as *mut ProxyHandle) {
        task.abort();
        // Aborting only requests cancellation; the task's future, which owns
        // the listeners and an `Arc` of the client, is dropped by a runtime
        // worker. Join it so teardown is complete when this returns.
        if let Ok(rt) = proxy_runtime() {
            let _ = rt.block_on(task);
        }
    }
}

type Listener = <TokioNativeTlsRuntime as NetStreamProvider>::Listener;

/// The runtime that drives the SOCKS accept loop.
fn proxy_runtime() -> io::Result<&'static Runtime> {
    RUNTIME
        .as_ref()
        .map_err(|e| io::Error::new(e.kind(), e.to_string()))
}

fn start_proxy(
    rt: &Runtime,
    client: Arc<TorClient<TokioNativeTlsRuntime>>,
    listeners: Vec<Listener>,
) -> JoinHandle<anyhow::Result<()>> {
    rt.spawn(run_proxy_with_listeners(
        client,
        listeners,
        ListenProtocols::SocksOnly,
        None,
    ))
}

/// Bind `port` on every localhost address family that is available.
///
/// IPv4 is authoritative: the IPv4 listener's port is the published port, and
/// IPv6 loopback is added on the same port. IPv4 alone is served only when
/// IPv6 is unavailable on the host. When the OS picks the port and another
/// process already holds its IPv6 twin, a fresh port is tried a few times
/// before failing, since a client resolving `localhost` to `::1` would reach
/// that process.
///
/// This blocks on `rt` so the listeners are registered with the reactor that
/// later runs the accept loop, and so the caller sees a bind failure before
/// the port is published.
fn bind_localhost(
    rt: &Runtime,
    runtime: &TokioNativeTlsRuntime,
    requested_port: u16,
) -> io::Result<(Vec<Listener>, u16)> {
    rt.block_on(async {
        let options = TcpListenOptions::default();
        let mut retries_left = if requested_port == 0 {
            IPV6_BIND_RETRIES
        } else {
            0
        };
        loop {
            let ipv4 = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), requested_port);
            let ipv4_listener = runtime
                .listen(&ipv4, &options)
                .await
                .map_err(|e| io::Error::new(e.kind(), format!("Can't listen on {ipv4}: {e}")))?;
            let port = ipv4_listener.local_addr()?.port();

            let ipv6 = SocketAddr::new(Ipv6Addr::LOCALHOST.into(), port);
            let error = match runtime.listen(&ipv6, &options).await {
                Ok(listener) => return Ok((vec![ipv4_listener, listener], port)),
                Err(error) => error,
            };
            match ipv6_bind_fallback(requested_port, retries_left, &error) {
                Ipv6Fallback::Ipv4Only => return Ok((vec![ipv4_listener], port)),
                Ipv6Fallback::Retry => {
                    retries_left -= 1;
                    drop(ipv4_listener);
                }
                Ipv6Fallback::Fail => return Err(ipv6_bind_error(requested_port, ipv6, error)),
            }
        }
    })
}

/// The error reported when IPv6 loopback could not be bound on `ipv6`.
fn ipv6_bind_error(requested_port: u16, ipv6: SocketAddr, error: io::Error) -> io::Error {
    let reason = if requested_port == 0 && error.kind() == io::ErrorKind::AddrInUse {
        format!(
            "the IPv6 twin of the assigned port was occupied after {} attempts ({error})",
            IPV6_BIND_RETRIES + 1
        )
    } else {
        error.to_string()
    };
    io::Error::new(error.kind(), format!("Can't listen on {ipv6}: {reason}"))
}

/// What to do when IPv6 loopback could not be bound on the IPv4 port.
#[derive(Debug, PartialEq, Eq)]
enum Ipv6Fallback {
    /// Serve IPv4 loopback only.
    Ipv4Only,
    /// Release the IPv4 listener and ask the OS for another port.
    Retry,
    /// Report the error. This includes an OS-assigned port whose IPv6 twin
    /// stayed occupied after every retry.
    Fail,
}

fn ipv6_bind_fallback(requested_port: u16, retries_left: u32, error: &io::Error) -> Ipv6Fallback {
    if ipv6_is_unavailable(error) {
        return Ipv6Fallback::Ipv4Only;
    }
    // Only an OS-assigned port can be swapped for another one. A port the
    // caller asked for is reported as occupied, as it is for IPv4. Serving
    // IPv4 alone would leave `[::1]` on the published port to another process.
    if requested_port == 0 && error.kind() == io::ErrorKind::AddrInUse && retries_left > 0 {
        return Ipv6Fallback::Retry;
    }
    Ipv6Fallback::Fail
}

/// Whether IPv6 loopback is unavailable on this host. IPv4 remains usable in
/// this case, so the SOCKS proxy should still start.
fn ipv6_is_unavailable(error: &io::Error) -> bool {
    if matches!(
        error.kind(),
        io::ErrorKind::AddrNotAvailable | io::ErrorKind::Unsupported
    ) {
        return true;
    }

    #[cfg(unix)]
    if error.raw_os_error() == Some(libc::EAFNOSUPPORT) {
        return true;
    }

    // std maps only WSAEADDRNOTAVAIL to a typed kind on Windows. A disabled
    // IPv6 stack fails socket creation with WSAEAFNOSUPPORT (10047) or
    // WSAEPROTONOSUPPORT (10043), which arrive uncategorized.
    #[cfg(windows)]
    if matches!(error.raw_os_error(), Some(10047 | 10043)) {
        return true;
    }

    false
}

// Due to its simple signature this dummy function is the one added (unused) to iOS swift codebase to force Xcode to link the lib
/// Print a greeting to verify that the library is linked.
///
/// # Safety
/// This function has no additional safety requirements.
#[no_mangle]
pub unsafe extern "C" fn tor_hello() {
    println!("HELLO THERE");
}

#[cfg(test)]
mod tests {
    use super::*;
    use arti_client::config::TorClientConfigBuilder;
    use std::ffi::CString;
    use std::net::{Ipv4Addr, TcpListener};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn scratch_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "tor_ffi_plugin-test-{}-{n}-{label}",
            std::process::id()
        ));
        // Process ids are reused, so a directory from an earlier run (such as
        // a deliberately corrupted cache) may already exist under this name.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Build a client under `root` the same way `tor_start` does, minus
    /// bootstrapping.
    fn unbootstrapped_client(
        root: &Path,
    ) -> (Arc<TorClient<TokioNativeTlsRuntime>>, ClientRuntime) {
        let runtime = ClientRuntime::new().unwrap();
        let cfg = TorClientConfigBuilder::from_directories(root.join("state"), root.join("cache"))
            .build()
            .unwrap();
        let client = TorClient::with_runtime(runtime.arti().unwrap())
            .config(cfg)
            .create_unbootstrapped()
            .unwrap();
        (client, runtime)
    }

    /// An unbootstrapped client as the opaque pointer Dart would receive.
    fn unbootstrapped_client_handle() -> *mut c_void {
        let (client, runtime) = unbootstrapped_client(&scratch_dir("client"));
        into_client_handle(client, runtime)
    }

    #[test]
    fn freeing_the_client_closes_its_files() {
        let root = scratch_dir("free");
        let (client, runtime) = unbootstrapped_client(&root);
        // Leave a bootstrap running, as Tor.stop() does with a live client, so
        // background tasks are using the directory cache. The result does not
        // matter, so this works offline too.
        let bootstrapping = Arc::clone(&client);
        runtime
            .0
            .as_ref()
            .unwrap()
            .spawn(async move { drop(bootstrapping.bootstrap().await) });
        std::thread::sleep(Duration::from_secs(1));
        unsafe { tor_client_free(into_client_handle(client, runtime)) };

        // Windows refuses to delete files that are still open. Before the fix
        // the client's background tasks kept the directory cache open forever.
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn client_handle_survives_repeated_calls_until_freed() {
        let handle = unbootstrapped_client_handle();

        // Before the fix every call through the handle reconstructed and
        // dropped the owning Box, so the second call here was a use after
        // free and the explicit free a double free.
        unsafe {
            tor_client_set_dormant(handle, true);
            tor_client_set_dormant(handle, false);
            tor_client_free(handle);
        }
    }

    #[test]
    fn cancelled_bootstrap_returns_without_network_and_preserves_the_handle() {
        let handle = unbootstrapped_client_handle();

        unsafe {
            tor_client_cancel_bootstrap(handle);
            assert_eq!(
                tor_client_bootstrap(handle, 60_000),
                TorBootstrapStatus::Cancelled
            );
            tor_client_set_dormant(handle, true);
            tor_client_set_dormant(handle, false);
            tor_client_free(handle);
        }
        let error = crate::error::take_last_error().unwrap().to_string();
        assert!(error.contains("cancelled"), "{error}");
    }

    #[test]
    fn controlled_bootstrap_honors_its_deadline() {
        let (_cancel, cancelled) = tokio::sync::watch::channel(false);
        let result = proxy_runtime().unwrap().block_on(controlled_bootstrap(
            std::future::pending::<io::Result<()>>(),
            cancelled,
            Duration::ZERO,
        ));

        let error = result.unwrap_err();
        assert_eq!(error.status(), TorBootstrapStatus::TimedOut);
        assert!(error.to_string().contains("timed out"), "{error}");
    }

    #[test]
    fn controlled_bootstrap_keeps_the_error_chain() {
        #[derive(Debug)]
        struct Inner;
        impl std::fmt::Display for Inner {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("no consensus")
            }
        }
        impl std::error::Error for Inner {}

        #[derive(Debug)]
        struct Outer(Inner);
        impl std::fmt::Display for Outer {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("bootstrap failed")
            }
        }
        impl std::error::Error for Outer {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }

        let (_cancel, cancelled) = tokio::sync::watch::channel(false);
        let error = proxy_runtime()
            .unwrap()
            .block_on(controlled_bootstrap(
                async { Err::<(), _>(Outer(Inner)) },
                cancelled,
                Duration::from_secs(60),
            ))
            .unwrap_err();

        assert_eq!(error.status(), TorBootstrapStatus::Failed);
        assert_eq!(error.to_string(), "bootstrap failed");
        let source = std::error::Error::source(&error).expect("source chain was dropped");
        assert_eq!(source.to_string(), "no consensus");
    }

    #[test]
    fn bootstrap_status_codes_are_stable() {
        let handle = unbootstrapped_client_handle();

        unsafe {
            // A zero deadline expires before an offline bootstrap can finish.
            assert_eq!(tor_client_bootstrap(handle, 0) as i32, 2);
            let error = crate::error::take_last_error().unwrap().to_string();
            assert!(error.contains("timed out"), "{error}");

            tor_client_cancel_bootstrap(handle);
            assert_eq!(tor_client_bootstrap(handle, 60_000) as i32, 1);
            let error = crate::error::take_last_error().unwrap().to_string();
            assert!(error.contains("cancelled"), "{error}");

            tor_client_free(handle);
        }
        assert_eq!(TorBootstrapStatus::Ok as i32, 0);
        assert_eq!(TorBootstrapStatus::Failed as i32, 3);
        assert_eq!(TorBootstrapStatus::NullClient as i32, 4);
    }

    #[test]
    fn error_message_is_released_by_tor_string_free() {
        update_last_error(io::Error::other("boom"));

        unsafe {
            let message = tor_last_error_message();
            assert_eq!(CStr::from_ptr(message).to_str().unwrap(), "boom");
            tor_string_free(message as *mut c_char);
            tor_string_free(ptr::null_mut());
        }
    }

    #[test]
    fn error_message_drops_interior_nul() {
        update_last_error(io::Error::other("bo\0om"));

        unsafe {
            let message = tor_last_error_message();
            assert_eq!(CStr::from_ptr(message).to_str().unwrap(), "boom");
            tor_string_free(message as *mut c_char);
        }
    }

    #[test]
    fn client_free_ignores_null() {
        unsafe { tor_client_free(ptr::null_mut()) };
    }

    #[test]
    fn client_bootstrap_reports_null() {
        let status = unsafe { tor_client_bootstrap(ptr::null_mut(), 1_000) };

        assert_eq!(status, TorBootstrapStatus::NullClient);
        let error = crate::error::take_last_error().unwrap().to_string();
        assert!(error.contains("null"), "{error}");
    }

    #[test]
    fn client_set_dormant_ignores_null() {
        unsafe { tor_client_set_dormant(ptr::null_mut(), true) };

        let error = crate::error::take_last_error().unwrap().to_string();
        assert!(error.contains("null"), "{error}");
    }

    #[test]
    fn bind_localhost_reports_an_occupied_port() {
        let occupied = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = occupied.local_addr().unwrap().port();
        let arti_rt = TokioNativeTlsRuntime::create().unwrap();

        let err = match bind_localhost(proxy_runtime().unwrap(), &arti_rt, port) {
            Ok(_) => panic!("bound port {port} while it was occupied"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("Can't listen on"), "{err}");
    }

    #[test]
    fn tor_start_rejects_an_occupied_port_before_opening_storage() {
        let occupied = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let root = scratch_dir("busy-port");
        // This cannot be a storage directory. A late bind would fail on storage
        // initialization instead of reporting the occupied port.
        let state = root.join("state-file");
        std::fs::write(&state, b"not a directory").unwrap();
        let state = CString::new(state.to_str().unwrap()).unwrap();
        let cache = CString::new(root.join("cache").to_str().unwrap()).unwrap();

        let result = unsafe {
            tor_start(
                occupied.local_addr().unwrap().port(),
                state.as_ptr(),
                cache.as_ptr(),
            )
        };
        assert!(result.client.is_null());
        assert!(result.proxy.is_null());
        let error = crate::error::take_last_error().unwrap().to_string();
        std::fs::remove_dir_all(root).unwrap();
        assert!(error.contains("Can't listen on"), "{error}");
    }

    #[test]
    fn unstarted_proxy_holds_its_port_and_can_be_released() {
        let root = scratch_dir("reserved-proxy");
        let state = CString::new(root.join("state").to_str().unwrap()).unwrap();
        let cache = CString::new(root.join("cache").to_str().unwrap()).unwrap();

        let result = unsafe { tor_start(0, state.as_ptr(), cache.as_ptr()) };
        assert!(!result.client.is_null());
        assert!(!result.proxy.is_null());
        assert_ne!(result.port, 0);
        assert!(TcpListener::bind((Ipv4Addr::LOCALHOST, result.port)).is_err());

        unsafe {
            tor_proxy_stop(result.proxy);
            tor_client_free(result.client);
        }
        TcpListener::bind((Ipv4Addr::LOCALHOST, result.port)).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bind_localhost_holds_the_port_until_dropped() {
        let arti_rt = TokioNativeTlsRuntime::create().unwrap();
        let rt = proxy_runtime().unwrap();

        // A requested port must be honored and held. Another test may take the
        // freed port first, so retry with a fresh one.
        let (listeners, port) = (0..8)
            .find_map(|_| {
                let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
                    .unwrap()
                    .local_addr()
                    .unwrap()
                    .port();
                match bind_localhost(rt, &arti_rt, port) {
                    Ok((listeners, bound_port)) => {
                        assert_eq!(bound_port, port);
                        Some((listeners, port))
                    }
                    Err(e) if e.kind() == io::ErrorKind::AddrInUse => None,
                    Err(e) => panic!("bind_localhost failed: {e}"),
                }
            })
            .expect("no free port after 8 attempts");

        assert!(
            TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_err(),
            "port {port} was published without being held"
        );
        drop(listeners);
        TcpListener::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
    }

    #[test]
    fn stopping_the_proxy_releases_its_listeners_and_client_reference() {
        let (client, runtime) = unbootstrapped_client(&scratch_dir("proxy-stop"));
        let arti_rt = runtime.arti().unwrap();
        let rt = proxy_runtime().unwrap();
        let (listeners, port) = bind_localhost(rt, &arti_rt, 0).unwrap();
        let mut proxy = Box::new(ProxyHandle::new(Arc::clone(&client), listeners));
        proxy.start(rt);
        // Let the accept loop start polling its listeners.
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(Arc::strong_count(&client), 2);

        unsafe { tor_proxy_stop(Box::into_raw(proxy) as *mut c_void) };

        // Before the fix the aborted task still owned the listeners and the
        // client until a runtime worker got around to dropping it.
        assert_eq!(Arc::strong_count(&client), 1);
        TcpListener::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
        drop(client);
        drop(runtime);
    }

    #[test]
    fn tor_start_uses_the_data_directories_literally() {
        let root = scratch_dir("literal-paths");
        // `CfgPath::new` treated this as a variable reference and failed.
        let state_path = root.join("$state");
        let cache_path = root.join("~cache");
        let state = CString::new(state_path.to_str().unwrap()).unwrap();
        let cache = CString::new(cache_path.to_str().unwrap()).unwrap();

        let result = unsafe { tor_start(0, state.as_ptr(), cache.as_ptr()) };
        let error = crate::error::take_last_error().map(|e| e.to_string());
        assert!(!result.client.is_null(), "{error:?}");
        unsafe {
            tor_proxy_stop(result.proxy);
            tor_client_free(result.client);
        }

        assert!(state_path.is_dir(), "state dir was not created literally");
        assert!(cache_path.is_dir(), "cache dir was not created literally");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ipv6_bind_fallback_retries_only_os_assigned_ports() {
        let in_use = io::Error::from(io::ErrorKind::AddrInUse);
        let unavailable = io::Error::from(io::ErrorKind::AddrNotAvailable);
        let denied = io::Error::from(io::ErrorKind::PermissionDenied);

        assert_eq!(ipv6_bind_fallback(0, 2, &in_use), Ipv6Fallback::Retry);
        // Out of retries, the IPv6 twin still belongs to another process.
        assert_eq!(ipv6_bind_fallback(0, 0, &in_use), Ipv6Fallback::Fail);
        assert_eq!(ipv6_bind_fallback(9050, 0, &in_use), Ipv6Fallback::Fail);
        assert_eq!(
            ipv6_bind_fallback(9050, 0, &unavailable),
            Ipv6Fallback::Ipv4Only
        );
        assert_eq!(ipv6_bind_fallback(0, 2, &denied), Ipv6Fallback::Fail);

        #[cfg(windows)]
        {
            let no_ipv6 = io::Error::from_raw_os_error(10047);
            assert_eq!(ipv6_bind_fallback(0, 0, &no_ipv6), Ipv6Fallback::Ipv4Only);
            assert_eq!(
                ipv6_bind_fallback(9050, 0, &no_ipv6),
                Ipv6Fallback::Ipv4Only
            );
        }
    }

    #[test]
    fn ipv6_bind_error_names_the_exhausted_retries() {
        let ipv6 = SocketAddr::new(Ipv6Addr::LOCALHOST.into(), 41000);
        let in_use = || io::Error::from(io::ErrorKind::AddrInUse);

        let exhausted = ipv6_bind_error(0, ipv6, in_use());
        assert_eq!(exhausted.kind(), io::ErrorKind::AddrInUse);
        let message = exhausted.to_string();
        assert!(
            message.contains("IPv6 twin of the assigned port was occupied after 4 attempts"),
            "{message}"
        );

        let requested = ipv6_bind_error(41000, ipv6, in_use()).to_string();
        assert!(
            requested.starts_with("Can't listen on [::1]:41000"),
            "{requested}"
        );
        assert!(!requested.contains("attempts"), "{requested}");
    }

    #[test]
    #[cfg(windows)]
    fn windows_reports_a_disabled_ipv6_stack_as_unavailable() {
        // WSAEAFNOSUPPORT and WSAEPROTONOSUPPORT have no typed ErrorKind.
        assert!(ipv6_is_unavailable(&io::Error::from_raw_os_error(10047)));
        assert!(ipv6_is_unavailable(&io::Error::from_raw_os_error(10043)));
    }

    #[test]
    fn bind_localhost_allocates_and_reports_one_port() {
        let arti_rt = TokioNativeTlsRuntime::create().unwrap();

        let (listeners, port) = bind_localhost(proxy_runtime().unwrap(), &arti_rt, 0).unwrap();

        assert_ne!(port, 0);
        assert!(
            TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_err(),
            "reported port {port} was not held by the proxy listener"
        );
        drop(listeners);
        TcpListener::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
    }
}
