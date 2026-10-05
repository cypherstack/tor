// SPDX-FileCopyrightText: 2023 Foundation Devices Inc.
//
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::error::update_last_error;
use arti::socks;
use arti_client::config::CfgPath;
use arti_client::{DormantMode, TorClient, TorClientConfig};
use lazy_static::lazy_static;
use std::ffi::{c_char, c_void, CStr};
use std::{io, ptr};
use tokio::runtime::{Builder, Runtime};
use tokio::task::JoinHandle;
use tor_config::Listen;
use tor_rtcompat::tokio::TokioNativeTlsRuntime;
use tor_rtcompat::ToplevelBlockOn;

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

#[repr(C)]
pub struct Tor {
    client: *mut c_void,
    proxy: *mut c_void,
}

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

    let runtime = unwrap_or_return!(TokioNativeTlsRuntime::create(), err_ret);

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

    let proxy_handle_box = Box::new(start_proxy(socks_port, client.clone()));
    let client_box = Box::new(client.clone());

    Tor {
        client: Box::into_raw(client_box) as *mut c_void,
        proxy: Box::into_raw(proxy_handle_box) as *mut c_void,
    }
}

/// Borrow the client handle produced by [`tor_start`] without taking ownership.
///
/// Ownership stays with the caller until [`tor_client_free`] is called, so the
/// handle can be passed to any number of FFI calls in between.
unsafe fn client_ref<'a>(client: *mut c_void) -> &'a TorClient<TokioNativeTlsRuntime> {
    assert!(!client.is_null());
    &*(client as *const TorClient<TokioNativeTlsRuntime>)
}

#[no_mangle]
pub unsafe extern "C" fn tor_client_bootstrap(client: *mut c_void) -> bool {
    let client = client_ref(client);

    unwrap_or_return!(client.runtime().block_on(client.bootstrap()), false);
    true
}

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
/// clone of the underlying client, so stopping the proxy and freeing the
/// handle can happen in either order.
#[no_mangle]
pub unsafe extern "C" fn tor_client_free(client: *mut c_void) {
    if client.is_null() {
        return;
    }

    drop(Box::from_raw(client as *mut TorClient<TokioNativeTlsRuntime>));
}

#[no_mangle]
pub unsafe extern "C" fn tor_proxy_stop(proxy: *mut c_void) {
    if proxy.is_null() {
        return;
    }

    let proxy = Box::from_raw(proxy as *mut JoinHandle<anyhow::Result<()>>);

    proxy.abort();
}

fn start_proxy(
    port: u16,
    client: TorClient<TokioNativeTlsRuntime>,
) -> JoinHandle<anyhow::Result<()>> {
    println!("Starting proxy!");
    let rt = RUNTIME.as_ref().unwrap();
    rt.spawn(socks::run_socks_proxy(
        client.runtime().clone(),
        client.clone(),
        Listen::new_localhost(port),
        None,
    ))
}

// Due to its simple signature this dummy function is the one added (unused) to iOS swift codebase to force Xcode to link the lib
#[no_mangle]
pub unsafe extern "C" fn tor_hello() {
    println!("HELLO THERE");
}

#[cfg(test)]
mod tests {
    use super::*;
    use arti_client::config::TorClientConfigBuilder;
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
}
