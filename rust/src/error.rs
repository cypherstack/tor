// SPDX-FileCopyrightText: 2023 Foundation Devices Inc.
//
// SPDX-License-Identifier: GPL-3.0-or-later

use log::{error, warn};
use std::cell::RefCell;
use std::error::Error;
use std::ffi::{c_char, CString};
//pub(crate) use crate::unwrap_or_return;

thread_local! {
    static LAST_ERROR: RefCell<Option<Box<dyn Error>>> = RefCell::new(None);
}

/// Take the current thread's last error as a newly allocated C string.
///
/// # Safety
/// This function has no preconditions. The returned string is owned by the
/// caller and must be released exactly once with [`tor_string_free`].
#[no_mangle]
pub unsafe extern "C" fn tor_last_error_message() -> *const c_char {
    let last_error = match crate::error::take_last_error() {
        Some(err) => err,
        None => return CString::new("").unwrap().into_raw(),
    };

    // CString::new rejects interior NULs, and a panic here would abort the
    // process at the FFI boundary.
    let error_message = last_error.to_string().replace('\0', "");
    CString::new(error_message).unwrap().into_raw()
}

/// Release a string returned by [`tor_last_error_message`].
///
/// # Safety
/// `message` must be null or a pointer returned by `tor_last_error_message`
/// that has not been released yet. It must not be used after this call.
#[no_mangle]
pub unsafe extern "C" fn tor_string_free(message: *mut c_char) {
    if message.is_null() {
        return;
    }

    drop(CString::from_raw(message));
}

macro_rules! unwrap_or_return {
    ($a:expr,$b:expr) => {
        match $a {
            Ok(x) => x,
            Err(e) => {
                update_last_error(e);
                return $b;
            }
        }
    };
}

/// Update the most recent error, clearing whatever may have been there before.
pub fn update_last_error<E: Error + 'static>(err: E) {
    error!("Setting LAST_ERROR: {}", err);

    {
        // Print a pseudo-backtrace for this error, following back each error's
        // cause until we reach the root error.
        let mut cause = err.source();
        while let Some(parent_err) = cause {
            warn!("Caused by: {}", parent_err);
            cause = parent_err.source();
        }
    }

    LAST_ERROR.with(|prev| {
        *prev.borrow_mut() = Some(Box::new(err));
    });
}

/// Retrieve the most recent error, clearing it in the process.
pub fn take_last_error() -> Option<Box<dyn Error>> {
    LAST_ERROR.with(|prev| prev.borrow_mut().take())
}
