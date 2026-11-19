/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::sync::OnceLock;

use buck2_error::buck2_error;

pub fn sc_page_size() -> buck2_error::Result<usize> {
    static PAGE_SIZE: OnceLock<usize> = OnceLock::new();
    PAGE_SIZE
        .get_or_try_init(|| unsafe {
            let size = libc::sysconf(libc::_SC_PAGESIZE);
            if size <= 0 {
                return Err(buck2_error!(
                    buck2_error::ErrorTag::Environment,
                    "Invalid page size from sysconf: {}",
                    size
                ));
            }
            Ok(size as usize)
        })
        .copied()
}

#[cfg(test)]
mod tests {
    use crate::os::unix_like::sc_page_size::sc_page_size;

    #[test]
    fn test_page_size() {
        let size = sc_page_size().unwrap();
        // Page size should be a power of two and at least 4096.
        assert!(size >= 4096);
        assert!(size.is_power_of_two());
    }
}
