/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io;

use buck2_data::error::ErrorTag;

// https://learn.microsoft.com/en-us/windows/win32/debug/system-error-codes--0-499-
// "The process cannot access the file because it is being used by another process."
const ERROR_SHARING_VIOLATION: i32 = 32;
const ERROR_FILE_SYSTEM_VIRTUALIZATION_UNAVAILABLE: i32 = 369;
const ERROR_INTERNAL_ERROR: i32 = 1359;
const ERROR_PRIVILEGE_NOT_HELD: i32 = 1314;
const ERROR_NO_SYSTEM_RESOURCES: i32 = 1450;

// https://github.com/apple/darwin-xnu/blob/main/bsd/sys/errno.h#L200
// "Stale NFS file handle (os error 70)"
const ERROR_STALE_NFS_HANDLE: i32 = 70;

pub(crate) fn io_error_kind_to_error_tag(kind: io::ErrorKind) -> ErrorTag {
    match kind {
        io::ErrorKind::NotFound => ErrorTag::IoNotFound,
        io::ErrorKind::NotADirectory => ErrorTag::IoNotADirectory,
        io::ErrorKind::PermissionDenied => ErrorTag::IoPermissionDenied,
        io::ErrorKind::TimedOut => ErrorTag::IoTimeout,
        io::ErrorKind::ExecutableFileBusy => ErrorTag::IoExecutableFileBusy,
        io::ErrorKind::BrokenPipe => ErrorTag::IoBrokenPipe,
        io::ErrorKind::StorageFull => ErrorTag::IoStorageFull,
        io::ErrorKind::ConnectionAborted => ErrorTag::IoConnectionAborted,
        io::ErrorKind::ReadOnlyFilesystem => ErrorTag::IoReadOnlyFilesystem,
        _ => ErrorTag::IoSystem,
    }
}

fn raw_os_error_tag(code: i32) -> Option<ErrorTag> {
    // POSIX error codes
    let tag = match code {
        libc::ENOTCONN => Some(ErrorTag::IoNotConnected),
        libc::ECONNABORTED => Some(ErrorTag::IoConnectionAborted),
        libc::EIO => Some(ErrorTag::IoInputOutputError),
        libc::EFAULT => Some(ErrorTag::IoBadAddress),
        _ => None,
    };
    if tag.is_some() {
        return tag;
    }

    if cfg!(windows) {
        match code {
            ERROR_SHARING_VIOLATION => Some(ErrorTag::IoWindowsSharingViolation),
            ERROR_FILE_SYSTEM_VIRTUALIZATION_UNAVAILABLE => {
                Some(ErrorTag::IoWindowsVirtualizationUnavailable)
            }
            ERROR_INTERNAL_ERROR => Some(ErrorTag::IoWindowsInternalError),
            ERROR_PRIVILEGE_NOT_HELD => Some(ErrorTag::IoWindowsPrivilegeNotHeld),
            ERROR_NO_SYSTEM_RESOURCES => Some(ErrorTag::IoWindowsNoSystemResources),
            _ => None,
        }
    } else if cfg!(target_os = "macos") {
        match code {
            ERROR_STALE_NFS_HANDLE => Some(ErrorTag::IoStaleNfsHandle),
            _ => None,
        }
    } else {
        None
    }
}

fn io_error_kind_tag(e: &io::Error) -> ErrorTag {
    let kind_tag = io_error_kind_to_error_tag(e.kind());
    if kind_tag != ErrorTag::IoSystem {
        return kind_tag;
    }

    if let Some(os_error_code) = e.raw_os_error() {
        if let Some(tag) = raw_os_error_tag(os_error_code) {
            return tag;
        }
    }

    ErrorTag::IoSystem
}

impl From<io::Error> for crate::Error {
    #[cold]
    #[track_caller]
    fn from(value: std::io::Error) -> Self {
        let error_tag = io_error_kind_tag(&value);
        crate::conversion::from_any_with_tag(value, error_tag)
    }
}
