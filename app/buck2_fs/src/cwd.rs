/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::Path;
use std::sync::OnceLock;

use allocative::Allocative;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;

use crate::IoResultExt;
use crate::fs_util;
use crate::paths::abs_norm_path::AbsNormPath;
use crate::paths::abs_norm_path::AbsNormPathBuf;
use crate::paths::abs_path::AbsPathBuf;

#[derive(Allocative)]
pub struct WorkingDirectoryGen<T: WorkingDirectoryImpl> {
    path: AbsNormPathBuf,
    imp: T,
}

impl<T: WorkingDirectoryImpl> WorkingDirectoryGen<T> {
    pub fn open(path: AbsNormPathBuf) -> buck2_error::Result<Self> {
        let imp = T::open(&path)?;
        Ok(Self { path, imp })
    }

    pub fn chdir_and_promise_it_will_not_change(&self) -> buck2_error::Result<()> {
        self.imp
            .chdir(&self.path)
            .buck_error_context("Failed to chdir")?;
        cwd_will_not_change(&self.path).buck_error_context("Failed to set working dir")?;
        Ok(())
    }

    pub fn is_stale(&self) -> buck2_error::Result<bool> {
        self.imp.is_stale(&self.path)
    }
}

/// Used to make sure our UNIX and portable implementations are identical.
pub trait WorkingDirectoryImpl: Sized {
    fn open(path: &AbsNormPath) -> buck2_error::Result<Self>;
    fn chdir(&self, _path: &AbsNormPath) -> buck2_error::Result<()>;
    fn is_stale(&self, path: &AbsNormPath) -> buck2_error::Result<bool>;
}

#[cfg(unix)]
mod unix_impl {

    use std::os::fd::OwnedFd;

    use allocative::Allocative;
    use buck2_error::BuckErrorContext;
    use nix::fcntl::OFlag;
    use nix::sys::stat::Mode;

    use super::WorkingDirectoryImpl;
    use crate::paths::abs_norm_path::AbsNormPath;

    #[derive(Allocative)]
    pub struct UnixWorkingDirectoryImpl {
        #[allocative(skip)]
        fd: OwnedFd,
    }

    impl WorkingDirectoryImpl for UnixWorkingDirectoryImpl {
        fn open(path: &AbsNormPath) -> buck2_error::Result<Self> {
            let fd = nix::fcntl::open(
                path.as_path(),
                OFlag::O_RDONLY | OFlag::O_CLOEXEC | OFlag::O_DIRECTORY,
                Mode::empty(),
            )
            .with_buck_error_context(|| format!("Failed to open: `{path}`"))?;

            Ok(Self { fd })
        }

        fn chdir(&self, _path: &AbsNormPath) -> buck2_error::Result<()> {
            nix::unistd::fchdir(&self.fd)?;
            Ok(())
        }

        fn is_stale(&self, path: &AbsNormPath) -> buck2_error::Result<bool> {
            let cwd = nix::sys::stat::fstat(&self.fd).buck_error_context("Failed to stat cwd")?;
            let path = nix::sys::stat::stat(path.as_path())
                .with_buck_error_context(|| format!("Failed to stat `{path}`"))?;
            Ok(cwd.st_dev != path.st_dev || cwd.st_ino != path.st_ino)
        }
    }
}

#[derive(Allocative)]
pub struct PortableWorkingDirectoryImpl;

impl WorkingDirectoryImpl for PortableWorkingDirectoryImpl {
    fn open(_path: &AbsNormPath) -> buck2_error::Result<Self> {
        Ok(Self)
    }

    fn chdir(&self, path: &AbsNormPath) -> buck2_error::Result<()> {
        fs_util::set_current_dir(path).categorize_internal()?;
        Ok(())
    }

    /// Not supported for PortableWorkingDirectoryImpl
    fn is_stale(&self, _path: &AbsNormPath) -> buck2_error::Result<bool> {
        Ok(false)
    }
}

#[cfg(unix)]
pub type WorkingDirectory = WorkingDirectoryGen<unix_impl::UnixWorkingDirectoryImpl>;

#[cfg(not(unix))]
pub type WorkingDirectory = WorkingDirectoryGen<PortableWorkingDirectoryImpl>;

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum CwdError {
    #[error("cwd is already set to `{}`", _0.display())]
    CwdAlreadySet(AbsPathBuf),
}

static CWD: OnceLock<AbsPathBuf> = OnceLock::new();

/// Promise the cwd will not change going forward. This should only be called once.
fn cwd_will_not_change(cwd: &AbsNormPath) -> buck2_error::Result<()> {
    CWD.set(cwd.as_abs_path().to_owned())
        .ok()
        .internal_error("cwd_will_not_change was called twice")?;
    Ok(())
}

pub(crate) fn assert_cwd_is_not_set() -> buck2_error::Result<()> {
    if let Some(cwd) = CWD.get() {
        return Err(CwdError::CwdAlreadySet(cwd.clone()).into());
    }
    Ok(())
}

pub(crate) fn maybe_relativize(path: &Path) -> &Path {
    maybe_relativize_impl(path, &CWD)
}

fn maybe_relativize_impl<'a>(path: &'a Path, cell: &'_ OnceLock<AbsPathBuf>) -> &'a Path {
    if let Some(cwd) = cell.get() {
        if let Ok(path) = path.strip_prefix(cwd) {
            if path == Path::new("") {
                return Path::new(".");
            }
            return path;
        }
    }

    path
}

pub(crate) fn maybe_relativize_str(path: &str) -> &str {
    maybe_relativize_str_impl(path, &CWD)
}

fn maybe_relativize_str_impl<'a>(path: &'a str, cell: &'_ OnceLock<AbsPathBuf>) -> &'a str {
    if let Some(path) = maybe_relativize_impl(Path::new(path), cell).to_str() {
        return path;
    }

    path
}

#[cfg(test)]
mod tests {
    use super::*;

    fn foo_bar() -> AbsPathBuf {
        if cfg!(windows) {
            AbsPathBuf::new("C:\\foo\\bar").unwrap()
        } else {
            AbsPathBuf::new("/foo/bar").unwrap()
        }
    }

    #[test]
    fn test_maybe_relativize() {
        let cell = OnceLock::new();
        let path = foo_bar().join("baz").into_path_buf();
        assert_eq!(maybe_relativize_impl(&path, &cell), path);
        cell.set(foo_bar()).unwrap();
        assert_eq!(maybe_relativize_impl(&path, &cell), Path::new("baz"));
        assert_eq!(maybe_relativize_impl(&foo_bar(), &cell), Path::new("."));
    }

    #[test]
    fn test_maybe_relativize_str() {
        let cell = OnceLock::new();
        let path = foo_bar().join("baz").to_str().unwrap().to_owned();
        assert_eq!(maybe_relativize_str_impl(&path, &cell), path);
        cell.set(foo_bar()).unwrap();
        assert_eq!(maybe_relativize_str_impl(&path, &cell), "baz");
        assert_eq!(
            maybe_relativize_str_impl(foo_bar().to_str().unwrap(), &cell),
            "."
        );
    }
}
