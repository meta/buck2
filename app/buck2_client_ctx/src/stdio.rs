/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! This module provides {e,}print{ln,} macros that return errors when they fail, unlike the std
//! macros, which yield panics. The errors returned by those methods don't make sense to handle in
//! place, and should usually just be propagated in order to lead to a quick exit.

use std::fmt::Arguments;
use std::fs::File;
use std::io;
use std::io::LineWriter;
use std::io::Stdout;
use std::io::Write;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use buck2_error::ErrorTag;
use buck2_error::internal_error;

use crate::exit_result::ClientIoError;

static HAS_WRITTEN_TO_STDOUT: AtomicBool = AtomicBool::new(false);

pub fn has_written_to_stdout() -> bool {
    HAS_WRITTEN_TO_STDOUT.load(Ordering::Relaxed)
}

static STDOUT_LOCKED: AtomicBool = AtomicBool::new(false);

fn stdout() -> buck2_error::Result<io::Stdout> {
    if STDOUT_LOCKED.load(Ordering::Relaxed) {
        return Err(internal_error!("stdout is already locked"));
    }
    HAS_WRITTEN_TO_STDOUT.store(true, Ordering::Relaxed);
    Ok(io::stdout())
}

#[macro_export]
macro_rules! print {
    () => {
        $crate::stdio::_print(std::format_args!(""))
    };
    ($($arg:tt)*) => {
        $crate::stdio::_print(std::format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! println {
    () => {
        $crate::stdio::_print(std::format_args!("\n"))
    };
    ($fmt:tt) => {
        $crate::stdio::_print(std::format_args!(concat!($fmt, "\n")))
    };
    ($fmt:tt, $($arg:tt)*) => {
        $crate::stdio::_print(std::format_args!(concat!($fmt, "\n"), $($arg)*))
    };
}

#[macro_export]
macro_rules! eprint {
    () => {
        $crate::stdio::_eprint(std::format_args!(""))
    };
    ($($arg:tt)*) => {
        $crate::stdio::_eprint(std::format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! eprintln {
    () => {
        $crate::stdio::_eprint(std::format_args!("\n"))
    };
    ($fmt:expr) => {
        $crate::stdio::_eprint(std::format_args!(concat!($fmt, "\n")))
    };
    ($fmt:expr, $($arg:tt)*) => {
        $crate::stdio::_eprint(std::format_args!(concat!($fmt, "\n"), $($arg)*))
    };
}

pub fn _print(fmt: Arguments) -> buck2_error::Result<()> {
    stdout()?
        .lock()
        .write_fmt(fmt)
        .map_err(|e| ClientIoError::from(e).into())
}

pub fn _eprint(fmt: Arguments) -> buck2_error::Result<()> {
    io::stderr()
        .lock()
        .write_fmt(fmt)
        .map_err(|e| ClientIoError::from(e).into())
}

pub fn print_bytes(bytes: &[u8]) -> buck2_error::Result<()> {
    stdout()?
        .lock()
        .write_all(bytes)
        .map_err(|e| ClientIoError::from(e).into())
}

pub fn flush() -> buck2_error::Result<()> {
    stdout()?.flush().map_err(|e| ClientIoError::from(e).into())
}

fn stdout_to_file(stdout: &Stdout) -> buck2_error::Result<File> {
    #[cfg(not(windows))]
    {
        use std::os::fd::AsFd;
        Ok(File::from(stdout.as_fd().try_clone_to_owned()?))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsHandle;
        Ok(File::from(stdout.as_handle().try_clone_to_owned()?))
    }
}

pub async fn print_with_writer<E, F>(f: F) -> buck2_error::Result<()>
where
    E: Into<buck2_error::Error>,
    F: AsyncFnOnce(&mut (dyn Write + Send)) -> Result<(), E>,
{
    let stdout = stdout()?;

    struct StdoutLockedGuard;

    impl Drop for StdoutLockedGuard {
        fn drop(&mut self) {
            assert!(
                STDOUT_LOCKED
                    .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
            );
        }
    }

    assert!(
        STDOUT_LOCKED
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    );
    let _guard = StdoutLockedGuard;

    let _guard = stdout.lock();
    let file = stdout_to_file(&stdout)?;
    let mut w = LineWriter::new(file);
    match f(&mut w).await {
        Ok(()) => {}
        Err(e) => {
            return Err(as_client_io_error(e.into()));
        }
    }
    w.flush().map_err(ClientIoError::from)?;
    Ok(())
}

/// Assumes a broken pipe error in F passed to print_with_writer is from writing to the (stdout/stderr) writer
/// and tags it as `IoClientBrokenPipe` (treated as a cancelled command).
fn as_client_io_error(e: buck2_error::Error) -> buck2_error::Error {
    if e.has_tag(ErrorTag::IoBrokenPipe) {
        e.tag([ErrorTag::IoClientBrokenPipe])
    } else {
        e
    }
}
