/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![cfg(windows)]

use std::sync::OnceLock;

use windows_sys::Win32::System::SystemInformation::GetSystemInfo;
use windows_sys::Win32::System::SystemInformation::SYSTEM_INFO;

pub fn page_size() -> buck2_error::Result<usize> {
    static PAGE_SIZE: OnceLock<usize> = OnceLock::new();
    PAGE_SIZE
        .get_or_try_init(|| {
            let info: SYSTEM_INFO = unsafe {
                let mut info: SYSTEM_INFO = std::mem::zeroed();
                GetSystemInfo(&mut info);
                info
            };
            let size = info.dwPageSize as usize;
            if size == 0 || !size.is_power_of_two() {
                return Err(buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Environment,
                    "Invalid page size from GetSystemInfo: {}",
                    size
                ));
            }
            Ok(size)
        })
        .copied()
}

#[cfg(test)]
mod tests {
    use super::page_size;

    #[test]
    fn test_page_size() {
        let size = page_size().unwrap();
        // Page size should be a power of two and at least 4096.
        assert!(size >= 4096);
        assert!(size.is_power_of_two());
    }
}
