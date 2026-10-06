// SPDX-FileCopyrightText: 2023 Foundation Devices Inc.
//
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::error::update_last_error;
use arti::proxy::{run_proxy_with_listeners, ListenProtocols};
use arti_client::config::CfgPath;
use arti_client::{DormantMode, TorClient, TorClientConfig};
use lazy_static::lazy_static;
use std::ffi::{c_char, c_void, CStr};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::{io, ptr};
use tokio::runtime::{Builder, Runtime};
use tokio::task::JoinHandle;
use tor_rtcompat::tokio::TokioNativeTlsRuntime;
use tor_rtcompat::{NetStreamListener, NetStreamProvider, TcpListenOptions, ToplevelBlockOn};

pub use crate::error::tor_last_error_message;
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

/// How many more OS-assigned ports to try when another process holds the
/// IPv6 twin of the assigned port, before failing.
const IPV6_BIND_RETRIES: u32 = 3;

#[repr(C)]
pub struct Tor {
    client: *mut c_void,
    proxy: *mut c_void,
    port: u16,
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
        port: 0,
    };

    let state_dir = unwrap_or_return!(CStr::from_ptr(state_dir).to_str(), err_ret);
    let cache_dir = unwrap_or_return!(CStr::from_ptr(cache_dir).to_str(), err_ret);

    let runtime = unwrap_or_return!(TokioNativeTlsRuntime::create(), err_ret);

    // Reserve the port before initializing storage or bootstrapping. On any
    // later failure these listeners are dropped and the port is released.
    let rt = unwrap_or_return!(proxy_runtime(), err_ret);
    let (listeners, socks_port) =
        unwrap_or_return!(bind_localhost(rt, &runtime, socks_port), err_ret);

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
    let client_box = Box::new(client);

    Tor {
        client: Box::into_raw(client_box) as *mut c_void,
        proxy: Box::into_raw(proxy_handle_box) as *mut c_void,
        port: socks_port,
    }
}

/// Borrow the client handle produced by [`tor_start`] without taking ownership.
///
/// Ownership stays with the caller until [`tor_client_free`] is called, so the
/// handle can be passed to any number of FFI calls in between.
unsafe fn client_ref<'a>(client: *mut c_void) -> &'a Arc<TorClient<TokioNativeTlsRuntime>> {
    assert!(!client.is_null());
    &*(client as *const Arc<TorClient<TokioNativeTlsRuntime>>)
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
/// The handle must not be used after this call. The proxy task keeps its own
/// reference to the underlying client, so stopping the proxy and freeing the
/// handle can happen in either order.
///
/// # Safety
/// `client` must be null or a live handle returned by [`tor_start`].
/// No other call may use the handle concurrently with or after this call.
#[no_mangle]
pub unsafe extern "C" fn tor_client_free(client: *mut c_void) {
    if client.is_null() {
        return;
    }

    drop(Box::from_raw(
        client as *mut Arc<TorClient<TokioNativeTlsRuntime>>,
    ));
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
    use std::path::PathBuf;
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

    /// Build a client the same way `tor_start` does, minus bootstrapping,
    /// and hand it out as the opaque pointer Dart would receive.
    fn unbootstrapped_client_handle() -> *mut c_void {
        let runtime = TokioNativeTlsRuntime::create().unwrap();
        let cfg =
            TorClientConfigBuilder::from_directories(scratch_dir("state"), scratch_dir("cache"))
                .build()
                .unwrap();
        let client = TorClient::with_runtime(runtime)
            .config(cfg)
            .create_unbootstrapped()
            .unwrap();
        Box::into_raw(Box::new(client)) as *mut c_void
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
        let cache_dir = scratch_dir("cache");
        let cfg = TorClientConfigBuilder::from_directories(scratch_dir("state"), &cache_dir)
            .build()
            .unwrap();
        let client = TorClient::with_runtime(TokioNativeTlsRuntime::create().unwrap())
            .config(cfg)
            .create_unbootstrapped()
            .unwrap();
        for entry in std::fs::read_dir(&cache_dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() && path.file_name() != Some("dir.lock".as_ref()) {
                std::fs::write(&path, [0x5a; 8192]).unwrap();
            }
        }
        let handle = Box::into_raw(Box::new(client)) as *mut c_void;

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
