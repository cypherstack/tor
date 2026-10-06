// SPDX-FileCopyrightText: 2023 Foundation Devices Inc.
//
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::error::update_last_error;
use arti::proxy::{run_proxy_with_listeners, ListenProtocols};
use arti_client::config::CfgPath;
use arti_client::{DormantMode, TorClient, TorClientConfig};
use lazy_static::lazy_static;
use std::ffi::{c_char, c_void, CStr};
use std::sync::Arc;
use std::time::Duration;
use std::{io, ptr};
use tokio::runtime::{Builder, Runtime};
use tokio::task::JoinHandle;
use tor_config::Listen;
use tor_rtcompat::tokio::TokioNativeTlsRuntime;
use tor_rtcompat::{NetStreamProvider, TcpListenOptions, ToplevelBlockOn};

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

#[repr(C)]
pub struct Tor {
    client: *mut c_void,
    proxy: *mut c_void,
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
            runtime.shutdown_timeout(CLIENT_SHUTDOWN_TIMEOUT);
        }
    }
}

/// What a client handle passed across FFI points to.
struct ClientHandle {
    // Fields drop in order, so the client is released before its runtime
    // shuts down.
    client: Arc<TorClient<TokioNativeTlsRuntime>>,
    _runtime: ClientRuntime,
}

fn into_client_handle(
    client: Arc<TorClient<TokioNativeTlsRuntime>>,
    runtime: ClientRuntime,
) -> *mut c_void {
    Box::into_raw(Box::new(ClientHandle {
        client,
        _runtime: runtime,
    })) as *mut c_void
}

/// Start a bootstrapped Tor client and a localhost SOCKS proxy.
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
    };

    let state_dir = unwrap_or_return!(CStr::from_ptr(state_dir).to_str(), err_ret);
    let cache_dir = unwrap_or_return!(CStr::from_ptr(cache_dir).to_str(), err_ret);

    let client_runtime = unwrap_or_return!(ClientRuntime::new(), err_ret);
    let runtime = unwrap_or_return!(client_runtime.arti(), err_ret);

    // Reserve the port before initializing storage or bootstrapping. On any
    // later failure these listeners are dropped and the port is released.
    let rt = unwrap_or_return!(proxy_runtime(), err_ret);
    let listeners = unwrap_or_return!(bind_localhost(rt, &runtime, socks_port), err_ret);

    let mut cfg_builder = TorClientConfig::builder();
    cfg_builder
        .storage()
        .state_dir(CfgPath::new(state_dir.to_owned()))
        .cache_dir(CfgPath::new(cache_dir.to_owned()));
    cfg_builder.address_filter().allow_onion_addrs(true);

    let cfg = unwrap_or_return!(cfg_builder.build(), err_ret);

    let client = unwrap_or_return!(
        runtime.block_on(async {
            TorClient::with_runtime(runtime.clone())
                .config(cfg)
                .create_bootstrapped()
                .await
        }),
        err_ret
    );

    let proxy_handle_box = Box::new(start_proxy(rt, Arc::clone(&client), listeners));

    Tor {
        client: into_client_handle(client, client_runtime),
        proxy: Box::into_raw(proxy_handle_box) as *mut c_void,
    }
}

/// Borrow the client handle produced by [`tor_start`] without taking ownership.
///
/// Ownership stays with the caller until [`tor_client_free`] is called, so the
/// handle can be passed to any number of FFI calls in between.
unsafe fn client_ref<'a>(client: *mut c_void) -> &'a Arc<TorClient<TokioNativeTlsRuntime>> {
    assert!(!client.is_null());
    &(*(client as *const ClientHandle)).client
}

/// Ensure the client has bootstrapped.
///
/// # Safety
/// `client` must be a live, non-null handle returned by [`tor_start`].
/// It must not be freed while this call is in progress.
#[no_mangle]
pub unsafe extern "C" fn tor_client_bootstrap(client: *mut c_void) -> bool {
    let client = client_ref(client);

    unwrap_or_return!(client.runtime().block_on(client.bootstrap()), false);
    true
}

/// Change the client's dormant mode.
///
/// # Safety
/// `client` must be a live, non-null handle returned by [`tor_start`].
/// It must not be freed while this call is in progress.
#[no_mangle]
pub unsafe extern "C" fn tor_client_set_dormant(client: *mut c_void, soft_mode: bool) {
    let client = client_ref(client);

    let dormant_mode = if soft_mode {
        DormantMode::Soft
    } else {
        DormantMode::Normal
    };
    client.set_dormant(dormant_mode);
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

/// Stop the proxy and release its handle. Null is accepted.
///
/// # Safety
/// `proxy` must be null or a live proxy handle returned by [`tor_start`].
/// A non-null handle must be passed to this function only once.
#[no_mangle]
pub unsafe extern "C" fn tor_proxy_stop(proxy: *mut c_void) {
    if proxy.is_null() {
        return;
    }

    let proxy = Box::from_raw(proxy as *mut JoinHandle<anyhow::Result<()>>);

    proxy.abort();
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
    println!("Starting proxy!");
    rt.spawn(run_proxy_with_listeners(
        client,
        listeners,
        ListenProtocols::SocksOnly,
        None,
    ))
}

/// Bind `port` on every localhost address family that is available.
///
/// This blocks on `rt` so the listeners are registered with the reactor that
/// later runs the accept loop, and so the caller sees a bind failure before
/// the port is published.
fn bind_localhost(
    rt: &Runtime,
    runtime: &TokioNativeTlsRuntime,
    port: u16,
) -> io::Result<Vec<Listener>> {
    rt.block_on(async {
        let listen = Listen::new_localhost(port);
        let mut listeners = Vec::new();
        for addr in listen
            .ip_addrs()
            .map_err(|e| io::Error::other(e.to_string()))?
            .flatten()
        {
            match runtime.listen(&addr, &TcpListenOptions::default()).await {
                Ok(listener) => listeners.push(listener),
                #[cfg(unix)]
                Err(ref e) if e.raw_os_error() == Some(libc::EAFNOSUPPORT) => {}
                Err(e) => {
                    return Err(io::Error::new(
                        e.kind(),
                        format!("Can't listen on {addr}: {e}"),
                    ))
                }
            }
        }
        if listeners.is_empty() {
            return Err(io::Error::other("Couldn't open SOCKS listeners"));
        }
        Ok(listeners)
    })
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
    fn client_handle_survives_a_failed_bootstrap() {
        // Overwrite the directory cache after the client has opened it, so
        // bootstrap fails at once instead of reaching for the network. The
        // client holds a lock on dir.lock, which Windows enforces, so skip it.
        let root = scratch_dir("failed-bootstrap");
        let (client, runtime) = unbootstrapped_client(&root);
        for entry in std::fs::read_dir(root.join("cache")).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() && path.file_name() != Some("dir.lock".as_ref()) {
                std::fs::write(&path, [0x5a; 8192]).unwrap();
            }
        }
        let handle = into_client_handle(client, runtime);

        // Before the fix tor_client_bootstrap() freed the handle on every
        // return, so the calls after it were a use after free and the final
        // free a double free.
        unsafe {
            assert!(!tor_client_bootstrap(handle));
            tor_client_set_dormant(handle, true);
            tor_client_set_dormant(handle, false);
            tor_client_free(handle);
        }
        assert!(crate::error::take_last_error().is_some());
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
    fn bind_localhost_holds_the_port_until_dropped() {
        let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let arti_rt = TokioNativeTlsRuntime::create().unwrap();

        let listeners = bind_localhost(proxy_runtime().unwrap(), &arti_rt, port).unwrap();

        assert!(
            TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_err(),
            "port {port} was published without being held"
        );
        drop(listeners);
        TcpListener::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
    }
}
