/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::Error;

use buck2_error::buck2_error;
use windows_sys::Win32::Foundation::FALSE;
use windows_sys::core::BOOL;

pub(crate) fn result_bool(ret: BOOL) -> buck2_error::Result<()> {
    if ret == FALSE {
        Err(buck2_error!(
            buck2_error::ErrorTag::Tier0,
            "{}",
            format!("{}", Error::last_os_error())
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn result_dword(ret: u32) -> buck2_error::Result<()> {
    if ret == u32::MAX {
        Err(buck2_error!(
            buck2_error::ErrorTag::Tier0,
            "{}",
            format!("{}", Error::last_os_error())
        ))
    } else {
        Ok(())
    }
}
