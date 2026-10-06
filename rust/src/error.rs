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
    let mut error_message = last_error.to_string().replace('\0', "");
    let mut cause = last_error.source();
    while let Some(error) = cause {
        let message = error.to_string().replace('\0', "");
        // Some Display implementations already include their source. Only
        // omit an exact, colon-delimited suffix, never a substring match.
        if error_message != message && !error_message.ends_with(&format!(": {message}")) {
            error_message.push_str(": ");
            error_message.push_str(&message);
        }
        cause = error.source();
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;
    use std::{fmt, io};

    #[derive(Debug)]
    struct BootstrapError(io::Error);

    impl fmt::Display for BootstrapError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "bootstrap failed")
        }
    }

    impl Error for BootstrapError {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(&self.0)
        }
    }

    fn take_message() -> String {
        unsafe {
            let message = tor_last_error_message();
            let text = CStr::from_ptr(message).to_string_lossy().into_owned();
            tor_string_free(message.cast_mut());
            text
        }
    }

    #[test]
    fn error_message_includes_causes_in_order() {
        update_last_error(BootstrapError(io::Error::other("storage unavailable")));
        assert_eq!(take_message(), "bootstrap failed: storage unavailable");

        update_last_error(io::Error::other("standalone error"));
        assert_eq!(take_message(), "standalone error");
    }
    #[derive(Debug)]
    struct Context {
        message: &'static str,
        source: Box<dyn Error>,
        display_source: bool,
    }

    impl fmt::Display for Context {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.message)?;
            if self.display_source {
                write!(f, ": {}", self.source)?;
            }
            Ok(())
        }
    }

    impl Error for Context {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(self.source.as_ref())
        }
    }

    #[test]
    fn error_message_does_not_repeat_embedded_causes() {
        for outer_embeds in [false, true] {
            update_last_error(Context {
                message: "top",
                source: Box::new(Context {
                    message: "cause",
                    source: Box::new(io::Error::other("root")),
                    display_source: true,
                }),
                display_source: outer_embeds,
            });
            assert_eq!(take_message(), "top: cause: root");
        }

        // Shared words are not evidence that a cause is already embedded.
        update_last_error(Context {
            message: "root failed during bootstrap",
            source: Box::new(io::Error::other("root failed")),
            display_source: false,
        });
        assert_eq!(take_message(), "root failed during bootstrap: root failed");
    }
}
