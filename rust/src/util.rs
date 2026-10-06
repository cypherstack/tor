// SPDX-FileCopyrightText: 2024 Foundation Devices Inc.
//
// SPDX-License-Identifier: GPL-3.0-or-later
#[cfg(not(target_os = "windows"))]
use crate::error::update_last_error;

/// Read the current open-file limit.
///
/// # Safety
/// This function has no additional safety requirements.
#[no_mangle]
#[cfg(not(target_os = "windows"))]
pub unsafe extern "C" fn tor_get_nofile_limit() -> u64 {
    let nofile_limit = unwrap_or_return!(rlimit::getrlimit(rlimit::Resource::NOFILE), 0);
    nofile_limit.0
}

/// Increase the open-file limit, up to the hard limit.
///
/// # Safety
/// This function has no additional safety requirements.
#[no_mangle]
#[cfg(not(target_os = "windows"))]
pub unsafe extern "C" fn tor_set_nofile_limit(limit: u64) -> u64 {
    unwrap_or_return!(rlimit::increase_nofile_limit(limit), 0)
}
