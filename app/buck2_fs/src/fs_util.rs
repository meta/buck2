/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

/// Buck2 having full control over how FS IO works is beneficial for implementing
/// IO counters and retry policies that are optimized for Buck2 and the EdenFS
/// virtualized file system.
use std::env;
use std::fs;
use std::fs::File;
use std::io;
use std::io::Read;
use std::io::Write;
use std::ops::Deref;
#[cfg(unix)]
use std::os::fd::AsFd;
#[cfg(unix)]
use std::os::fd::BorrowedFd;
use std::path::Path;
use std::path::PathBuf;

use buck2_env::soft_error::soft_error_or_panic;
use buck2_error::BuckErrorContext;
use buck2_error::ErrorTag;
use buck2_error::buck2_error;
#[cfg(unix)]
use buck2_error::internal_error;

use crate::cwd::assert_cwd_is_not_set;
pub use crate::error::IoError;
pub use crate::error::IoErrorSource;
use crate::error::IoResultExt;
use crate::io_counters::IoCounterGuard;
use crate::io_counters::IoCounterKey;
use crate::paths::abs_norm_path::AbsNormPath;
use crate::paths::abs_norm_path::AbsNormPathBuf;
use crate::paths::abs_path::AbsPath;
use crate::paths::abs_path::AbsPathBuf;
use crate::paths::forward_rel_path::ForwardRelativePath;

fn is_retryable(err: &io::Error) -> bool {
    cfg!(target_os = "macos")
        && (err.kind() == io::ErrorKind::TimedOut
            || err.kind() == io::ErrorKind::StaleNetworkFileHandle)
}

static MAX_IO_ATTEMPTS: u32 = 3;

fn with_retries<T>(mut func: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let mut attempts = 0;
    let mut last_error_kind: Option<std::io::ErrorKind> = None;

    loop {
        match func() {
            Ok(v) => {
                if let Some(err) = last_error_kind {
                    // Counts how often the retry policy earns its keep; an `io::Result` has no
                    // room for a `buck2_error`, so under the hard-error policy this panics.
                    soft_error_or_panic!(
                        "fs_io_succeeded_after_retry",
                        buck2_error!(buck2_error::ErrorTag::Input, "{}", err.to_string())
                    );
                }
                return Ok(v);
            }
            Err(e) if is_retryable(&e) => {
                last_error_kind = Some(e.kind());
                attempts += 1;
                if attempts >= MAX_IO_ATTEMPTS {
                    return Err(e);
                }
            }
            Err(e) => return Err(e),
        }
    }
}

fn if_exists<T>(r: io::Result<T>) -> io::Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn symlink<P, Q>(original: P, link: Q) -> Result<(), IoError>
where
    P: AsRef<Path>,
    Q: AsRef<AbsPath>,
{
    let _guard = IoCounterKey::Symlink.guard();
    let original_ref = original.as_ref();
    let link_ref = link.as_ref();
    symlink_impl(original_ref, link_ref).map_err(|e| {
        e.context(format!(
            "symlink(original={}, link={})",
            original_ref.display(),
            link_ref.display(),
        ))
        .check_eden(link_ref)
    })
}

#[cfg(unix)]
fn symlink_impl(original: &Path, link: &AbsPath) -> Result<(), IoError> {
    std::os::unix::fs::symlink(original, link.as_maybe_relativized()).map_err(IoError::new)
}

/// Create symlink on Windows.
#[cfg(windows)]
fn symlink_impl(original: &Path, link: &AbsPath) -> Result<(), IoError> {
    use std::borrow::Cow;
    use std::io::ErrorKind;

    use buck2_error::ErrorTag;
    use common_path::common_path;

    fn permission_check(result: io::Result<()>) -> Result<(), IoError> {
        match result {
            // Standard issue on Windows machines, so hint at the resolution, as it is not obvious.
            // Unfortunately this doesn't have an `ErrorKind`, so have to do it with substring matching.
            Err(e) if e.to_string().contains("privilege is not held") => Err(IoError::new(e)
                .context(
                    "Perhaps you need to turn on 'Developer Mode' in Windows to enable symlinks.",
                )
                .tag(ErrorTag::Environment)),
            Err(e) => Err(IoError::new(e)),
            Ok(_) => Ok(()),
        }
    }

    let link = link.as_path();

    // If original is a relative path, fix it up to be absolute
    let target_abspath = if original.is_absolute() {
        Cow::Borrowed(original)
    } else {
        Cow::Owned(
            link.parent()
                .ok_or_else(|| {
                    IoError::new(io::Error::new(
                        io::ErrorKind::NotFound,
                        "Expected path with a parent in symlink target",
                    ))
                    .tag(ErrorTag::SymlinkParentMissing)
                })?
                .join(original),
        )
    };
    let target_abspath = std::path::absolute(&target_abspath).map_err(IoError::new)?;

    // Relative symlinks in Windows are relative from the real/canonical path of the link,
    // so it can get messy when the symlink lives in buck-out. For Windows, we'll canonicalize,
    // or if the target doesn't exist yet simulate canonicalize() by canonicalizing the common
    // ancestor between the target and link and appending the rest. The common ancestor should
    // live somewhere between repo root / buck-out and link's directory, which should both exist.
    // Canonicalize() will also handle adding the verbatim prefix \\?\, which is required for
    // supporting paths longer than 260
    // In general, it should be OK to opt for absolute / canonical paths when possible as
    // buck will not read any of these paths.
    let target_canonical = if let Ok(path) = target_abspath.canonicalize() {
        path
    } else {
        // target doesn't exist yet, try to guess the canonical path
        if let Some(common_path) = common_path(&target_abspath, link) {
            let from_common = target_abspath
                .strip_prefix(&common_path)
                .map_err(|e| IoError::new(io::Error::other(e)))?;
            let common_canonicalized = common_path.canonicalize().map_err(IoError::new)?;
            common_canonicalized.join(from_common)
        } else {
            target_abspath
        }
    };

    let target_metadata = target_canonical.metadata();
    match target_metadata {
        Ok(meta) if meta.is_dir() => {
            permission_check(std::os::windows::fs::symlink_dir(&target_canonical, link))
        }
        Err(e) if e.kind() != ErrorKind::NotFound => Err(IoError::new(e)),
        _ => {
            // Either file or not existent. Default to file.
            // TODO(T144443238): This will cause issues if the file type turns out to be directory, fix this
            permission_check(std::os::windows::fs::symlink_file(&target_canonical, link))
        }
    }
}

pub fn set_current_dir<P: AsRef<AbsPath>>(path: P) -> Result<(), IoError> {
    assert_cwd_is_not_set().map_err(IoError::internal)?;
    env::set_current_dir(path.as_ref())
        .map_err(|e| IoError::new_with_path("set_current_dir", path, e))
}

pub fn create_dir_all<P: AsRef<AbsPath>>(path: P) -> buck2_error::Result<()> {
    let _guard = IoCounterKey::MkDir.guard();
    with_retries(|| fs::create_dir_all(path.as_ref().as_maybe_relativized()))
        .map_err(|e| IoError::new_with_path("create_dir_all", path, e).categorize_internal())
}

pub fn create_dir<P: AsRef<AbsPath>>(path: P) -> Result<(), IoError> {
    let _guard = IoCounterKey::MkDir.guard();

    with_retries(|| fs::create_dir(path.as_ref().as_maybe_relativized()))
        .map_err(|e| IoError::new_with_path("create_dir", path, e))
}

/// Create directory if not exists.
///
/// Fail if exists but is not a directory or creation failed.
pub fn create_dir_if_not_exists<P: AsRef<AbsPath>>(path: P) -> Result<(), IoError> {
    let path = path.as_ref();
    let _guard = IoCounterKey::MkDir.guard();
    with_retries(|| {
        let e = match fs::create_dir(path.as_maybe_relativized()) {
            Ok(()) => return Ok(()),
            Err(e) => e,
        };

        match symlink_metadata(path) {
            Ok(metadata) => {
                if metadata.is_dir() {
                    Ok(())
                } else {
                    // File exists but not a directory, return original error.
                    Err(e)
                }
            }
            Err(_) => {
                // `lstat` failed, means something like permission denied, return original error.
                Err(e)
            }
        }
    })
    .map_err(|e| IoError::new_with_path("create_dir", path, e))
}

/// `DirEntry` which is known to contain absolute path.
pub struct DirEntry {
    dir_entry: fs::DirEntry,
}

impl DirEntry {
    pub fn path(&self) -> AbsNormPathBuf {
        // This is safe, because `read_dir` is called with absolute path,
        // and filename is not dot or dot-dot.
        AbsNormPathBuf::unchecked_new(self.dir_entry.path())
    }
}

impl Deref for DirEntry {
    type Target = fs::DirEntry;

    fn deref(&self) -> &Self::Target {
        &self.dir_entry
    }
}

pub struct ReadDir {
    read_dir: fs::ReadDir,
    /// We store guard in the iterator, so if an iteration does non-trivial operations,
    /// these non-trivial operations will be considered part of read dir in IO counters.
    _guard: IoCounterGuard,
}

impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        self.read_dir
            .next()
            .map(|res| res.map(|dir_entry| DirEntry { dir_entry }))
    }
}

pub fn read_dir<P: AsRef<AbsNormPath>>(path: P) -> Result<ReadDir, IoError> {
    let _guard = IoCounterKey::ReadDir.guard();
    let path = path.as_ref();
    with_retries(|| fs::read_dir(path))
        .map_err(|e| IoError::new_with_path("read_dir", path, e))
        .map(|read_dir| ReadDir { read_dir, _guard })
}

pub fn read_dir_if_exists<P: AsRef<AbsNormPath>>(path: P) -> buck2_error::Result<Option<ReadDir>> {
    let _guard = IoCounterKey::ReadDir.guard();
    let path = path.as_ref();
    with_retries(|| if_exists(fs::read_dir(path)))
        .map_err(|e| IoError::new_with_path("read_dir_if_exists", path, e).categorize_internal())
        .map(|opt| opt.map(|read_dir| ReadDir { read_dir, _guard }))
}

pub fn try_exists<P: AsRef<AbsPath>>(path: P) -> buck2_error::Result<bool> {
    let _guard = IoCounterKey::Stat.guard();
    with_retries(|| path.as_ref().as_maybe_relativized().try_exists())
        .map_err(|e| IoError::new_with_path("try_exists", path, e).categorize_internal())
}

pub fn remove_file<P: AsRef<AbsPath>>(path: P) -> Result<(), IoError> {
    let _guard = IoCounterKey::Remove.guard();
    with_retries(|| remove_file_impl(path.as_ref().as_maybe_relativized()))
        .map_err(|e| IoError::new_with_path("remove_file", path, e))
}

#[cfg(unix)]
fn remove_file_impl(path: &Path) -> io::Result<()> {
    fs::remove_file(path)
}

#[cfg(windows)]
#[allow(clippy::permissions_set_readonly_false)]
fn remove_file_impl(path: &Path) -> io::Result<()> {
    use std::io::ErrorKind;
    use std::os::windows::fs::FileTypeExt;

    let file_type = path.symlink_metadata()?.file_type();
    if !file_type.is_symlink() || file_type.is_symlink_file() {
        match fs::remove_file(path) {
            Ok(_) => Ok(()),
            Err(e) => {
                // Some tools may set readonly attribute on files in buck-out
                // which causes `Access is denied` error on Windows.
                // Try to remove readonly attribute and retry.
                if e.kind() == ErrorKind::PermissionDenied {
                    let mut perms = fs::metadata(path)?.permissions();
                    if perms.readonly() {
                        perms.set_readonly(false);
                        fs::set_permissions(path, perms)?;
                        fs::remove_file(path)
                    } else {
                        Err(e)
                    }
                } else {
                    Err(e)
                }
            }
        }
    } else {
        fs::remove_dir(path)
    }
}

/// Like `remove_file`, but on Unix, if permission is denied, add read, write,
/// and execute permission for the owner of the containing directory and try
/// again.
/// Unlinking a file requires write and execute permission on the containing
/// directory, and action outputs can contain read-only directories (e.g. OS
/// images, where distributions give their top level directories and
/// directories in /etc restrictive permissions). The permission change is not
/// reverted, so only use this when the containing directory is being removed
/// too.
pub fn remove_file_harder<P: AsRef<AbsPath>>(path: P) -> Result<(), IoError> {
    let _guard = IoCounterKey::Remove.guard();
    with_retries(|| remove_file_harder_impl(path.as_ref()))
        .map_err(|e| IoError::new_with_path("remove_file_harder", path, e))
}

#[cfg(not(unix))]
fn remove_file_harder_impl(path: &AbsPath) -> io::Result<()> {
    // On Windows removal is blocked by the readonly attribute of the file
    // itself, not by the permissions of the containing directory, and
    // remove_file_impl already clears that attribute and retries.
    remove_file_impl(path.as_maybe_relativized())
}

#[cfg(unix)]
fn remove_file_harder_impl(path: &AbsPath) -> io::Result<()> {
    remove_with_writable_parent(path, remove_file_impl)
}

#[cfg(unix)]
fn remove_with_writable_parent(
    path: &AbsPath,
    remove: impl Fn(&Path) -> io::Result<()>,
) -> io::Result<()> {
    match remove(path.as_maybe_relativized()) {
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            let Some(parent) = path.parent() else {
                return Err(e);
            };
            // Retry even if the permissions were already sufficient: another
            // removal in the same directory may have added them just now.
            match open_dir_adding_owner_rwx(nix::fcntl::AT_FDCWD, parent.as_maybe_relativized()) {
                Ok(_) => remove(path.as_maybe_relativized()),
                Err(repair_error) => {
                    let repair_error = io::Error::from(repair_error);
                    if is_retryable(&repair_error) {
                        Err(repair_error)
                    } else {
                        Err(e)
                    }
                }
            }
        }
        r => r,
    }
}

/// Opens the directory `name` relative to `parent` and adds read, write, and
/// execute permission for its owner. Symlinks are not followed, and the mode is
/// changed on the opened directory, so a directory replaced by a symlink in the
/// meantime is never followed. Failing to add the permissions is not an error:
/// the caller's retry reports whatever is still denied.
#[cfg(unix)]
fn open_dir_adding_owner_rwx<P: ?Sized + nix::NixPath>(
    parent: BorrowedFd<'_>,
    name: &P,
) -> nix::Result<nix::dir::Dir> {
    use nix::dir::Dir;
    use nix::errno::Errno;
    use nix::fcntl::OFlag;
    use nix::sys::stat::Mode;

    let flags = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    match Dir::openat(parent, name, flags, Mode::empty()) {
        Ok(dir) => {
            let mode = nix::sys::stat::fstat(&dir)?.st_mode;
            if mode & 0o700 != 0o700 {
                let _ignored = nix::sys::stat::fchmod(&dir, owner_rwx_mode(mode));
            }
            Ok(dir)
        }
        // The owner can't read the directory, so it can't be opened to change
        // its mode.
        Err(Errno::EACCES) => {
            add_owner_rwx_to_unreadable_dir(parent, name)?;
            Dir::openat(parent, name, flags, Mode::empty())
        }
        Err(e) => Err(e),
    }
}

#[cfg(unix)]
fn owner_rwx_mode(mode: libc::mode_t) -> nix::sys::stat::Mode {
    nix::sys::stat::Mode::from_bits_truncate((mode | 0o700) & 0o7777)
}

#[cfg(target_os = "linux")]
fn add_owner_rwx_to_unreadable_dir<P: ?Sized + nix::NixPath>(
    parent: BorrowedFd<'_>,
    name: &P,
) -> nix::Result<()> {
    use std::os::fd::AsRawFd;

    use nix::fcntl::OFlag;
    use nix::sys::stat::FchmodatFlags;
    use nix::sys::stat::Mode;

    // An `O_PATH` descriptor doesn't need read permission. `fchmod` doesn't
    // accept it, but `chmod` on its `/proc` entry changes exactly the
    // directory it refers to.
    let fd = nix::fcntl::openat(
        parent,
        name,
        OFlag::O_PATH | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )?;
    let mode = nix::sys::stat::fstat(&fd)?.st_mode;
    nix::sys::stat::fchmodat(
        nix::fcntl::AT_FDCWD,
        format!("/proc/self/fd/{}", fd.as_raw_fd()).as_str(),
        owner_rwx_mode(mode),
        FchmodatFlags::FollowSymlink,
    )
}

#[cfg(all(unix, not(target_os = "linux")))]
fn add_owner_rwx_to_unreadable_dir<P: ?Sized + nix::NixPath>(
    parent: BorrowedFd<'_>,
    name: &P,
) -> nix::Result<()> {
    use nix::errno::Errno;
    use nix::fcntl::AtFlags;
    use nix::sys::stat::FchmodatFlags;
    use nix::sys::stat::SFlag;

    let stat = nix::sys::stat::fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW)?;
    if SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT != SFlag::S_IFDIR {
        return Err(Errno::ENOTDIR);
    }
    // If `name` has been replaced by a symlink since, this changes the mode
    // of the symlink itself.
    nix::sys::stat::fchmodat(
        parent,
        name,
        owner_rwx_mode(stat.st_mode),
        FchmodatFlags::NoFollowSymlink,
    )
}

pub fn copy<P: AsRef<AbsPath>, Q: AsRef<AbsPath>>(from: P, to: Q) -> Result<u64, IoError> {
    let _guard = IoCounterKey::Copy.guard();
    let from_ref = from.as_ref();
    let to_ref = to.as_ref();
    with_retries(|| {
        copy_impl(
            from_ref.as_maybe_relativized(),
            to_ref.as_maybe_relativized(),
        )
    })
    .map_err(|e| {
        IoError::new(e)
            .context(format!(
                "copy(from={}, to={})",
                from_ref.display(),
                to_ref.display(),
            ))
            .check_eden(from_ref)
            .check_eden(to_ref)
    })
}

/// `fs::copy` on Linux uses `copy_file_range`, which keeps holes only on a filesystem that can
/// share extents (btrfs, XFS). Elsewhere (ext4, tmpfs) sparse files get inflated to their full
/// size. Copy only the data ranges of a sparse file instead.
#[cfg(target_os = "linux")]
fn copy_impl(from: &Path, to: &Path) -> io::Result<u64> {
    use std::io::Seek;
    use std::io::SeekFrom;
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::fs::PermissionsExt;

    use nix::errno::Errno;
    use nix::unistd::Whence;
    use nix::unistd::lseek;

    let src = File::open(from)?;
    let metadata = src.metadata()?;
    let len = metadata.len();
    // `blocks()` counts 512 byte units, whatever the filesystem's block size.
    if !metadata.is_file() || metadata.blocks() * 512 >= len {
        return fs::copy(from, to);
    }

    let permissions = metadata.permissions();
    let mut dst = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(permissions.mode())
        .open(to)?;
    let mut offset = 0;
    while offset < len {
        let data = match lseek(&src, offset as i64, Whence::SeekData) {
            Ok(data) => data as u64,
            // Nothing but a hole from `offset` on.
            Err(Errno::ENXIO) => break,
            Err(e) => return Err(e.into()),
        };
        let hole = lseek(&src, data as i64, Whence::SeekHole)? as u64;
        (&src).seek(SeekFrom::Start(data))?;
        dst.seek(SeekFrom::Start(data))?;
        // Like `fs::copy`, this goes through `copy_file_range` where possible, which still shares
        // the extents on a filesystem that can.
        io::copy(&mut (&src).take(hole - data), &mut dst)?;
        offset = hole;
    }
    // Covers a trailing hole, which no copy above reaches.
    dst.set_len(len)?;
    // As `fs::copy` does: the mode given to `open` only applies to a new file, minus the umask.
    dst.set_permissions(permissions)?;
    Ok(len)
}

#[cfg(not(target_os = "linux"))]
fn copy_impl(from: &Path, to: &Path) -> io::Result<u64> {
    fs::copy(from, to)
}

/// Clones `from` to `to` where the filesystem shares blocks between files (btrfs, XFS with
/// reflinks, APFS), after which the two are independent. `Ok(false)` when the filesystem, or the
/// pair of them, has no such thing, in which case nothing is left at `to`; any other failure is
/// an error with the OS error intact. `from` must be a regular file and `to` must not exist. The
/// clone gets the source's permission bits, as a copy does.
#[cfg(target_os = "linux")]
fn reflink(from: &AbsPath, to: &AbsPath) -> io::Result<bool> {
    use std::os::fd::AsRawFd;

    let src = fs::File::open(from.as_maybe_relativized())?;
    let dst = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to.as_maybe_relativized())?;
    // SAFETY: `FICLONE` takes the source descriptor as its argument; both descriptors are open.
    let ret = unsafe { libc::ioctl(dst.as_raw_fd(), libc::FICLONE as _, src.as_raw_fd()) };
    if ret == 0 {
        return dst
            .set_permissions(src.metadata()?.permissions())
            .map(|()| true);
    }
    let error = io::Error::last_os_error();
    drop(dst);
    let _ignored = fs::remove_file(to.as_maybe_relativized());
    match error.raw_os_error() {
        // No reflinks on this filesystem, across these two, for this file, or on this kernel.
        Some(libc::EOPNOTSUPP | libc::EXDEV | libc::EINVAL | libc::ENOTTY | libc::ENOSYS) => {
            Ok(false)
        }
        _ => Err(error),
    }
}

#[cfg(target_os = "macos")]
fn reflink(from: &AbsPath, to: &AbsPath) -> io::Result<bool> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;

    /// Give the clone to the caller rather than copying the source's owner.
    const CLONE_NOOWNERCOPY: u32 = 0x0002;
    let src = fs::File::open(from.as_maybe_relativized())?;
    let dst = CString::new(to.as_maybe_relativized().as_os_str().as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    // SAFETY: an open descriptor and a NUL-terminated path. `clonefile` creates `to` whole or
    // not at all, and copies the source's mode.
    let ret = unsafe {
        libc::fclonefileat(
            src.as_raw_fd(),
            libc::AT_FDCWD,
            dst.as_ptr(),
            CLONE_NOOWNERCOPY,
        )
    };
    if ret == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ENOTSUP | libc::EXDEV | libc::EINVAL) => Ok(false),
        _ => Err(error),
    }
}

/// Windows clones blocks on ReFS only, through an API nothing here needs yet.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn reflink(_from: &AbsPath, _to: &AbsPath) -> io::Result<bool> {
    Ok(false)
}

/// Copies `from` to `to`, which must not exist, with the source's permission bits; nothing is
/// left at `to` on failure.
fn copy_new(from: &AbsPath, to: &AbsPath) -> io::Result<()> {
    let mut src = fs::File::open(from.as_maybe_relativized())?;
    let mut dst = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to.as_maybe_relativized())?;
    let copied = io::copy(&mut src, &mut dst)
        .and_then(|_| dst.set_permissions(src.metadata()?.permissions()));
    if let Err(e) = copied {
        drop(dst);
        let _ignored = fs::remove_file(to.as_maybe_relativized());
        return Err(e);
    }
    Ok(())
}

/// Copy `from` to `to`, sharing the underlying blocks with the source where the filesystem
/// supports it and falling back to a full copy where it does not. The two paths are independent
/// either way: writing to one never changes the other.
///
/// `from` must be a regular file and `to` must not exist. Use [`reflink_or_copy_all`] to clone a
/// whole tree.
pub fn reflink_or_copy<P: AsRef<AbsPath>, Q: AsRef<AbsPath>>(
    from: P,
    to: Q,
) -> Result<(), IoError> {
    let _guard = IoCounterKey::Copy.guard();
    let from_ref = from.as_ref();
    let to_ref = to.as_ref();
    with_retries(|| {
        if reflink(from_ref, to_ref)? {
            Ok(())
        } else {
            copy_new(from_ref, to_ref)
        }
    })
    .map_err(|e| {
        IoError::new(e)
            .context(format!(
                "reflink_or_copy(from={}, to={})",
                from_ref.display(),
                to_ref.display(),
            ))
            .check_eden(from_ref)
            .check_eden(to_ref)
    })
}

/// Makes `link` another name for the file at `original`: one inode, so a write through either
/// path is seen through the other. `link` must not exist.
pub fn hard_link<P: AsRef<AbsPath>, Q: AsRef<AbsPath>>(
    original: P,
    link: Q,
) -> Result<(), IoError> {
    let _guard = IoCounterKey::Hardlink.guard();
    let original_ref = original.as_ref();
    let link_ref = link.as_ref();
    with_retries(|| {
        fs::hard_link(
            original_ref.as_maybe_relativized(),
            link_ref.as_maybe_relativized(),
        )
    })
    .map_err(|e| {
        IoError::new(e)
            .context(format!(
                "hard_link(original={}, link={})",
                original_ref.display(),
                link_ref.display(),
            ))
            .check_eden(original_ref)
            .check_eden(link_ref)
    })
}

/// How [`reflink_or_hardlink_or_copy`] put the content in place.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Linked {
    /// A clone: independent of the source, sharing its blocks until one of them is written.
    Reflinked,
    /// The source's inode under a second name: a write through either path is seen through
    /// the other.
    Hardlinked,
    /// A full copy.
    Copied,
}

/// Puts the content of `from` at `to` as cheaply as the filesystem allows: a reflink where it
/// has them, else a hardlink, else a full copy. Unlike [`reflink_or_copy`], the result may share
/// an inode with the source, which is for callers that can bear a write through one path being
/// seen through the other. `from` must be a regular file and `to` must not exist.
pub fn reflink_or_hardlink_or_copy<P: AsRef<AbsPath>, Q: AsRef<AbsPath>>(
    from: P,
    to: Q,
) -> Result<Linked, IoError> {
    let from_ref = from.as_ref();
    let to_ref = to.as_ref();
    let context = |e: io::Error| {
        IoError::new(e)
            .context(format!(
                "reflink_or_hardlink_or_copy(from={}, to={})",
                from_ref.display(),
                to_ref.display(),
            ))
            .check_eden(from_ref)
            .check_eden(to_ref)
    };
    {
        let _guard = IoCounterKey::Copy.guard();
        if with_retries(|| reflink(from_ref, to_ref)).map_err(context)? {
            return Ok(Linked::Reflinked);
        }
    }
    if hard_link(from_ref, to_ref).is_ok() {
        return Ok(Linked::Hardlinked);
    }
    let _guard = IoCounterKey::Copy.guard();
    with_retries(|| copy_new(from_ref, to_ref))
        .map(|()| Linked::Copied)
        .map_err(context)
}

/// Recursively copy `from` to `to`, sharing the underlying blocks with the source where the
/// filesystem supports it and falling back to a full copy where it does not. The two trees are
/// independent either way: writing to one never changes the other.
///
/// Symlinks are recreated with their target string unchanged, so a relative symlink that points
/// outside the copied tree resolves elsewhere at the destination - the same thing that happens
/// when a tree is renamed.
///
/// `to` must not exist.
pub fn reflink_or_copy_all<P: AsRef<AbsNormPath>, Q: AsRef<AbsNormPath>>(
    from: P,
    to: Q,
) -> buck2_error::Result<()> {
    let from = from.as_ref();
    let to = to.as_ref();
    reflink_or_copy_all_impl(from, to)
        .with_buck_error_context(|| format!("reflink_or_copy_all(from={from}, to={to})"))
}

fn reflink_or_copy_all_impl(from: &AbsNormPath, to: &AbsNormPath) -> buck2_error::Result<()> {
    let file_type = symlink_metadata(from).categorize_internal()?.file_type();
    if file_type.is_dir() {
        create_dir_all(to)?;
        for entry in read_dir(from).categorize_internal()? {
            let entry = entry
                .map_err(|e| IoError::new_with_path("read_dir", from, e).categorize_internal())?;
            let file_name = entry.file_name();
            let name = ForwardRelativePath::new(&file_name)?;
            reflink_or_copy_all_impl(&entry.path(), &to.join(name))?;
        }
        Ok(())
    } else if file_type.is_symlink() {
        let target = read_link(from).categorize_internal()?;
        symlink(target, to).categorize_internal()
    } else {
        reflink_or_copy(from, to).categorize_internal()
    }
}

pub fn read_link<P: AsRef<AbsPath>>(path: P) -> Result<PathBuf, IoError> {
    let _guard = IoCounterKey::ReadLink.guard();
    with_retries(|| fs::read_link(path.as_ref().as_maybe_relativized()))
        .map_err(|e| IoError::new_with_path("read_link", path, e))
}

/// Renames `from` to `to`, replacing a file there. A read-only file is replaced on Windows too,
/// which `std::fs::rename` refuses.
pub fn rename<P: AsRef<AbsPath>, Q: AsRef<AbsPath>>(from: P, to: Q) -> Result<(), IoError> {
    let _guard = IoCounterKey::Rename.guard();
    let from_ref = from.as_ref();
    let to_ref = to.as_ref();
    with_retries(|| {
        rename_impl(
            from_ref.as_maybe_relativized(),
            to_ref.as_maybe_relativized(),
        )
    })
    .map_err(|e| {
        IoError::new(e)
            .context(format!(
                "rename(from={}, to={})",
                from_ref.display(),
                to_ref.display()
            ))
            .check_eden(from_ref)
            .check_eden(to_ref)
    })
}

#[cfg(unix)]
fn rename_impl(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

#[cfg(windows)]
#[allow(clippy::permissions_set_readonly_false)]
fn rename_impl(from: &Path, to: &Path) -> io::Result<()> {
    use std::io::ErrorKind;

    match fs::rename(from, to) {
        // The files the disk-state materializer tracks are read-only under
        // `buck2.readonly_outputs`, and a write action's output is published by renaming a temp
        // file over the previous one. Unix needs only the directory for that; Windows wants the
        // attribute cleared first, as `remove_file_impl` does to delete such a file.
        Err(e) if e.kind() == ErrorKind::PermissionDenied => {
            let Ok(metadata) = fs::symlink_metadata(to) else {
                return Err(e);
            };
            let mut perms = metadata.permissions();
            if !metadata.is_file() || !perms.readonly() {
                return Err(e);
            }
            perms.set_readonly(false);
            fs::set_permissions(to, perms)?;
            fs::rename(from, to)
        }
        r => r,
    }
}

pub fn write<P: AsRef<AbsPath>, C: AsRef<[u8]>>(path: P, contents: C) -> Result<(), IoError> {
    let _guard = IoCounterKey::Write.guard();
    with_retries(|| fs::write(path.as_ref().as_maybe_relativized(), &contents))
        .map_err(|e| IoError::new_with_path("write", path, e))
}

/// Write `contents` to `path` and apply the executable bit when opening / creating the file.
pub fn write_with_executable_bit<P: AsRef<AbsPath>, C: AsRef<[u8]>>(
    path: P,
    contents: C,
    executable: bool,
) -> Result<(), IoError> {
    let _guard = IoCounterKey::Write.guard();
    with_retries(|| {
        let mut file = fs::File::create(path.as_ref().as_maybe_relativized())?;
        #[cfg(unix)]
        if executable {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o755))?;
        }
        #[cfg(not(unix))]
        let _ = executable;
        file.write_all(contents.as_ref())
    })
    .map_err(|e| IoError::new_with_path("write_with_executable_bit", path, e))
}

pub fn metadata<P: AsRef<AbsPath>>(path: P) -> Result<fs::Metadata, IoError> {
    let _guard = IoCounterKey::Stat.guard();
    with_retries(|| fs::metadata(path.as_ref().as_maybe_relativized()))
        .map_err(|e| IoError::new_with_path("metadata", path, e))
}

pub fn symlink_metadata<P: AsRef<AbsPath>>(path: P) -> Result<fs::Metadata, IoError> {
    let _guard = IoCounterKey::Stat.guard();
    with_retries(|| fs::symlink_metadata(path.as_ref().as_maybe_relativized()))
        .map_err(|e| IoError::new_with_path("symlink_metadata", path, e))
}

pub fn set_permissions<P: AsRef<AbsPath>>(path: P, perm: fs::Permissions) -> Result<(), IoError> {
    let _guard = IoCounterKey::Chmod.guard();
    with_retries(|| fs::set_permissions(path.as_ref().as_maybe_relativized(), perm.clone()))
        .map_err(|e| {
            IoError::new(e)
                .context(format!("set_permissions({}, _)", path.as_ref().display()))
                .check_eden(path.as_ref())
        })
}

pub fn set_executable<P: AsRef<AbsPath>>(path: P, executable: bool) -> Result<(), IoError> {
    let path = path.as_ref();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        // Unix permission bits
        let mut perms = metadata(path)?.permissions();
        if executable {
            // Add ugo+x
            perms.set_mode(perms.mode() | 0o111);
        } else {
            // Remove ugo+x
            perms.set_mode(perms.mode() & !0o111);
        }
        set_permissions(path, perms)?;
    }
    #[cfg(not(unix))]
    {
        // Nothing to do
        let _ignore = path;
        let _ignore_executable = executable;
    }

    Ok(())
}

/// Gives or takes the permission to write `path`, a file: with `false` nobody may write it
/// (`0o444`, or `0o555` for an executable); with `true` its owner may again. On Windows this is
/// the read-only attribute. Follows symlinks.
pub fn set_writable<P: AsRef<AbsPath>>(path: P, writable: bool) -> Result<(), IoError> {
    let path = path.as_ref();
    let mut perms = metadata(path)?.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if writable {
            perms.set_mode(perms.mode() | 0o200);
        } else {
            perms.set_mode(perms.mode() & !0o222);
        }
    }
    #[cfg(not(unix))]
    perms.set_readonly(!writable);
    set_permissions(path, perms)
}

/// Removes a directory tree. On Unix, if permission is denied, adds read,
/// write, and execute permission for the owner on every directory in the tree
/// (but not on the parent of `path`) and tries again. Those permission changes
/// remain if the removal still fails.
pub fn remove_dir_all<P: AsRef<AbsPath>>(path: P) -> Result<(), IoError> {
    let _guard = IoCounterKey::RmDirAll.guard();
    with_retries(|| remove_dir_all_impl(path.as_ref().as_maybe_relativized()))
        .map_err(|e| IoError::new_with_path("remove_dir_all", path, e))
}

#[cfg(not(unix))]
fn remove_dir_all_impl(path: &Path) -> io::Result<()> {
    fs::remove_dir_all(path)
}

#[cfg(unix)]
fn remove_dir_all_impl(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            // Removing a directory tree requires read, write, and execute
            // permission on every directory in it, regardless of the
            // permissions of the files.
            add_owner_rwx_recursively(nix::fcntl::AT_FDCWD, path);
            fs::remove_dir_all(path)
        }
        r => r,
    }
}

/// Best-effort `chmod u+rwx` on the directory `path` and every directory below
/// it. Each directory is opened relative to its parent's descriptor, so
/// symlinks are not followed and path length doesn't matter. Errors are
/// ignored: whatever couldn't be fixed makes the retried removal fail with a
/// more useful error.
#[cfg(unix)]
fn add_owner_rwx_recursively<P: ?Sized + nix::NixPath>(parent: BorrowedFd<'_>, name: &P) {
    use std::ffi::CString;

    use nix::dir::Type;

    let Ok(mut dir) = open_dir_adding_owner_rwx(parent, name) else {
        return;
    };
    let subdirs: Vec<CString> = dir
        .iter()
        .flatten()
        // The type is unknown on some file systems; opening a non-directory
        // fails harmlessly.
        .filter(|entry| entry.file_type().is_none_or(|t| t == Type::Directory))
        .map(|entry| entry.file_name().to_owned())
        .filter(|name| name.as_bytes() != b"." && name.as_bytes() != b"..")
        .collect();
    for subdir in subdirs {
        add_owner_rwx_recursively(dir.as_fd(), subdir.as_c_str());
    }
}

fn symlink_metadata_if_exists_impl<P: AsRef<AbsPath>>(
    path: P,
) -> Result<Option<fs::Metadata>, IoError> {
    let _guard = IoCounterKey::Stat.guard();
    with_retries(|| if_exists(fs::symlink_metadata(path.as_ref().as_maybe_relativized())))
        .map_err(|e| IoError::new_with_path("symlink_metadata", path, e))
}

/// `None` if file does not exist.
pub fn symlink_metadata_if_exists<P: AsRef<AbsPath>>(
    path: P,
) -> buck2_error::Result<Option<fs::Metadata>> {
    symlink_metadata_if_exists_impl(path).categorize_internal()
}

/// Remove whatever exists at `path`, be it a file, directory, pipe, broken symlink, etc.
/// Do nothing if `path` does not exist.
pub fn remove_all<P: AsRef<AbsPath>>(path: P) -> Result<(), IoError> {
    // There are no counters because every function called here has its own counter.
    let metadata = match symlink_metadata_if_exists_impl(&path) {
        Ok(None) => return Ok(()),
        Ok(Some(s)) => Ok(s),
        // `NotADirectory` means we are trying to delete a path (e.g. "/foo/bar") that has a subpath
        // pointing to a regular file (e.g. "/foo"). In this case do not fail and behave similarly to as
        // when path we are trying to delete does not exist.
        Err(e) if e.io_error_kind() == Some(io::ErrorKind::NotADirectory) => {
            return Ok(());
        }
        Err(e) => Err(e),
    }?;

    let r = if metadata.is_dir() {
        remove_dir_all(&path)
    } else {
        remove_file(&path)
    };
    if let Err(e) = &r {
        // If the remove itself returned NotFound, something else removed the path between
        // our metadata check and our remove call. Either way the prior content is gone,
        // which is what we came here to achieve — so report success. Do NOT re-check the
        // filesystem after the fact: a concurrent writer may have recreated content there
        // by the time of the re-check, and that content isn't ours to delete.
        if e.io_error_kind() == Some(io::ErrorKind::NotFound) {
            return Ok(());
        }
    }
    r
}

pub fn read<P: AsRef<AbsPath>>(path: P) -> Result<Vec<u8>, IoError> {
    let _guard = IoCounterKey::Read.guard();
    with_retries(|| fs::read(path.as_ref().as_maybe_relativized()))
        .map_err(|e| IoError::new_with_path("read", path, e))
}

pub fn read_to_string<P: AsRef<AbsPath>>(path: P) -> Result<String, IoError> {
    let _guard = IoCounterKey::Read.guard();
    with_retries(|| fs::read_to_string(path.as_ref().as_maybe_relativized()))
        .map_err(|e| IoError::new_with_path("read_to_string", path, e))
}

/// Read a file, if it exists. Returns `None` when the file does not exist.
pub fn read_to_string_if_exists<P: AsRef<AbsPath>>(path: P) -> buck2_error::Result<Option<String>> {
    let _guard = IoCounterKey::Read.guard();
    with_retries(|| if_exists(fs::read_to_string(path.as_ref().as_maybe_relativized()))).map_err(
        |e| IoError::new_with_path("read_to_string_if_exists", path, e).categorize_internal(),
    )
}

/// Read a file, if it exists. Returns `None` when the file does not exist.
pub fn read_if_exists<P: AsRef<AbsPath>>(path: P) -> buck2_error::Result<Option<Vec<u8>>> {
    let _guard = IoCounterKey::Read.guard();
    with_retries(|| if_exists(fs::read(path.as_ref().as_maybe_relativized())))
        .map_err(|e| IoError::new_with_path("read_if_exists", path, e).categorize_internal())
}

/// `std::env::current_dir`, reported like every other filesystem operation.
///
/// Worth going through here rather than calling `std::env` directly: this is the call that fails
/// when the working directory has been deleted, so it is the only place that can attribute that,
/// and on its own `std::env::current_dir` produces a bare `No such file or directory (os error 2)`
/// with nothing to say which path it meant. The path is always the working directory, so the tag
/// belongs here rather than at each caller.
pub fn current_dir() -> buck2_error::Result<AbsPathBuf> {
    let _guard = IoCounterKey::Stat.guard();
    env::current_dir()
        .map_err(IoErrorSource::from)
        .and_then(|path| AbsPathBuf::new(path).map_err(IoErrorSource::Internal))
        .map_err(|e| IoError::new(e).context("current_dir()"))
        .categorize_tagged(ErrorTag::MissingWorkingDir)
}

pub fn canonicalize<P: AsRef<AbsPath>>(path: P) -> Result<AbsNormPathBuf, IoError> {
    let _guard = IoCounterKey::Canonicalize.guard();
    with_retries(|| dunce::canonicalize(path.as_ref()).into())
        .map_err(IoErrorSource::from)
        .and_then(|path| AbsNormPathBuf::new(path).map_err(IoErrorSource::Internal))
        .map_err(|e| IoError::new_with_path("canonicalize", path, e))
}

pub fn canonicalize_if_exists<P: AsRef<AbsPath>>(
    path: P,
) -> buck2_error::Result<Option<AbsNormPathBuf>> {
    let _guard = IoCounterKey::Canonicalize.guard();
    with_retries(|| if_exists(dunce::canonicalize(path.as_ref())))
        .map_err(IoErrorSource::from)
        .and_then(|path| {
            path.map(|path| AbsNormPathBuf::new(path).map_err(IoErrorSource::Internal))
                .transpose()
        })
        .map_err(|e| {
            IoError::new_with_path("canonicalize_if_exists", path, e).categorize_internal()
        })
}

/// Convert Windows UNC path to regular path.
pub fn simplified(path: &AbsPath) -> buck2_error::Result<&AbsPath> {
    let path = dunce::simplified(path.as_ref());
    // This should not fail, but better not panic.
    AbsPath::new(path)
}

pub fn remove_dir<P: AsRef<AbsPath>>(path: P) -> Result<(), IoError> {
    let _guard = IoCounterKey::RmDir.guard();
    with_retries(|| fs::remove_dir(path.as_ref().as_maybe_relativized()))
        .map_err(|e| IoError::new_with_path("remove_dir", path, e))
}

/// Like `remove_dir`, but on Unix, if permission is denied, add read, write, and
/// execute permission for the owner of the containing directory and try again.
/// As with `remove_file_harder`, only use this when the containing directory is
/// being removed too.
pub fn remove_dir_harder<P: AsRef<AbsPath>>(path: P) -> Result<(), IoError> {
    let _guard = IoCounterKey::RmDir.guard();
    with_retries(|| remove_dir_harder_impl(path.as_ref()))
        .map_err(|e| IoError::new_with_path("remove_dir_harder", path, e))
}

#[cfg(unix)]
fn remove_dir_harder_impl(path: &AbsPath) -> io::Result<()> {
    remove_with_writable_parent(path, |path| fs::remove_dir(path))
}

#[cfg(not(unix))]
fn remove_dir_harder_impl(path: &AbsPath) -> io::Result<()> {
    fs::remove_dir(path.as_maybe_relativized())
}

pub struct DiskSpaceStats {
    pub free_space: u64,
    pub total_space: u64,
}

/// Free and total disk space on given path. Path does not have to be disk root.
/// When the path does not exist, the behavior is not specified.
pub fn disk_space_stats<P: AsRef<AbsPath>>(path: P) -> buck2_error::Result<DiskSpaceStats> {
    #[cfg(not(windows))]
    fn disk_space_stats_impl(path: &AbsPath) -> buck2_error::Result<DiskSpaceStats> {
        use std::ffi::CString;
        use std::mem::MaybeUninit;
        use std::os::unix::ffi::OsStrExt;

        let path_c = CString::new(path.as_os_str().as_bytes())
            .map_err(buck2_error::Error::from)
            .with_buck_error_context(|| format!("Failed to convert path to CString: {path:?}"))?;
        let mut statvfs = unsafe { MaybeUninit::<libc::statvfs>::zeroed().assume_init() };
        unsafe {
            let r = libc::statvfs(path_c.as_ptr(), &mut statvfs);
            if r != 0 {
                let e = io::Error::last_os_error();
                return Err(IoError::new_with_path("statvfs", path, e).categorize_internal());
            }
        }
        // These casts are required because statvfs fields are u32 on macOS but u64 on Linux.
        #[allow(clippy::unnecessary_cast)]
        let fr_size = statvfs.f_frsize as u64;
        #[allow(clippy::unnecessary_cast)]
        let free_space = (statvfs.f_bavail as u64)
            .checked_mul(fr_size)
            .ok_or_else(|| {
                internal_error!(
                    "Multiplication overflow for statvfs free space for `{}`",
                    path.display()
                )
            })?;

        #[allow(clippy::unnecessary_cast)]
        let total_space = (statvfs.f_blocks as u64)
            .checked_mul(fr_size)
            .ok_or_else(|| {
                internal_error!(
                    "Multiplication overflow for statvfs total space for `{}`",
                    path.display()
                )
            })?;
        Ok(DiskSpaceStats {
            free_space,
            total_space,
        })
    }

    #[cfg(windows)]
    fn disk_space_stats_impl(path: &AbsPath) -> buck2_error::Result<DiskSpaceStats> {
        use std::ptr;

        use buck2_util::os::win::os_str::os_str_to_wide_null_term;

        let path_c = os_str_to_wide_null_term(path.as_os_str());

        unsafe {
            let mut free_bytes: u64 = 0;
            let mut total_bytes: u64 = 0;
            let r = windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                path_c.as_ptr(),
                &mut free_bytes,  // lpFreeBytesAvailableToCaller
                &mut total_bytes, // lpTotalNumberOfBytes
                ptr::null_mut(),  // lpTotalNumberOfFreeBytes
            );
            if r == 0 {
                let e = io::Error::last_os_error();
                return Err(
                    IoError::new_with_path("GetDiskFreeSpaceExW", path, e).categorize_internal()
                );
            }
            Ok(DiskSpaceStats {
                free_space: free_bytes,
                total_space: total_bytes,
            })
        }
    }

    let _guard = IoCounterKey::Stat.guard();
    disk_space_stats_impl(path.as_ref())
}

pub struct FileWriteGuard {
    file: File,
    _guard: IoCounterGuard,
}

impl Write for FileWriteGuard {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

pub fn create_file<P: AsRef<AbsPath>>(path: P) -> Result<FileWriteGuard, IoError> {
    let guard = IoCounterKey::Write.guard();
    let file = with_retries(|| File::create(path.as_ref().as_maybe_relativized()))
        .map_err(|e| IoError::new_with_path("create_file", path, e))?;
    Ok(FileWriteGuard {
        file,
        _guard: guard,
    })
}

fn create_file_if_not_exists_impl<P: AsRef<AbsPath>>(
    path: P,
) -> Result<Option<FileWriteGuard>, io::Error> {
    let guard = IoCounterKey::Write.guard();
    match File::create_new(path.as_ref().as_maybe_relativized()) {
        Ok(file) => Ok(Some(FileWriteGuard {
            file,
            _guard: guard,
        })),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn create_file_if_not_exists<P: AsRef<AbsPath>>(
    path: P,
) -> buck2_error::Result<Option<FileWriteGuard>> {
    with_retries(|| create_file_if_not_exists_impl(&path)).map_err(|e| {
        IoError::new_with_path("create_file_if_not_exists", path, e).categorize_internal()
    })
}

pub struct FileReadGuard {
    file: File,
    _guard: IoCounterGuard,
}

impl Read for FileReadGuard {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file.read(buf)
    }
}

pub fn open_file<P: AsRef<AbsPath>>(path: P) -> Result<FileReadGuard, IoError> {
    let guard = IoCounterKey::Read.guard();
    let file = with_retries(|| File::open(path.as_ref().as_maybe_relativized()))
        .map_err(|e| IoError::new_with_path("open_file", path, e))?;
    Ok(FileReadGuard {
        file,
        _guard: guard,
    })
}

pub fn open_file_if_exists<P: AsRef<AbsPath>>(
    path: P,
) -> buck2_error::Result<Option<FileReadGuard>> {
    let guard = IoCounterKey::Read.guard();
    let Some(file) =
        with_retries(|| if_exists(File::open(path.as_ref().as_maybe_relativized())))
            .map_err(|e| IoError::new_with_path("open_file", path, e).categorize_internal())?
    else {
        return Ok(None);
    };
    Ok(Some(FileReadGuard {
        file,
        _guard: guard,
    }))
}

/// Wrapper for fs_util functions that convert to buck2_error::Result without categorization.
/// Should only be used in tests.
pub mod uncategorized {
    use std::fs;
    use std::path::Path;
    use std::path::PathBuf;

    pub use super::Linked;
    pub use super::canonicalize_if_exists;
    pub use super::create_dir_all;
    pub use super::create_file_if_not_exists;
    pub use super::disk_space_stats;
    pub use super::open_file_if_exists;
    pub use super::read_dir_if_exists;
    pub use super::read_if_exists;
    pub use super::read_to_string_if_exists;
    pub use super::reflink_or_copy_all;
    pub use super::simplified;
    pub use super::symlink_metadata_if_exists;
    pub use super::try_exists;
    use crate::error::IoResultExt;
    use crate::fs_util::FileReadGuard;
    use crate::fs_util::FileWriteGuard;
    use crate::fs_util::ReadDir;
    use crate::paths::abs_norm_path::AbsNormPath;
    use crate::paths::abs_norm_path::AbsNormPathBuf;
    use crate::paths::abs_path::AbsPath;

    pub fn symlink<P, Q>(original: P, link: Q) -> buck2_error::Result<()>
    where
        P: AsRef<Path>,
        Q: AsRef<AbsPath>,
    {
        super::symlink(original, link).uncategorized()
    }

    pub fn set_current_dir<P: AsRef<AbsPath>>(path: P) -> buck2_error::Result<()> {
        super::set_current_dir(path).uncategorized()
    }

    pub fn create_dir<P: AsRef<AbsPath>>(path: P) -> buck2_error::Result<()> {
        super::create_dir(path).uncategorized()
    }

    pub fn create_dir_if_not_exists<P: AsRef<AbsPath>>(path: P) -> buck2_error::Result<()> {
        super::create_dir_if_not_exists(path).uncategorized()
    }

    pub fn read_dir<P: AsRef<AbsNormPath>>(path: P) -> buck2_error::Result<ReadDir> {
        super::read_dir(path).uncategorized()
    }

    pub fn remove_file<P: AsRef<AbsPath>>(path: P) -> buck2_error::Result<()> {
        super::remove_file(path).uncategorized()
    }

    pub fn remove_file_harder<P: AsRef<AbsPath>>(path: P) -> buck2_error::Result<()> {
        super::remove_file_harder(path).uncategorized()
    }

    pub fn copy<P: AsRef<AbsPath>, Q: AsRef<AbsPath>>(from: P, to: Q) -> buck2_error::Result<u64> {
        super::copy(from, to).uncategorized()
    }

    pub fn reflink_or_copy<P: AsRef<AbsPath>, Q: AsRef<AbsPath>>(
        from: P,
        to: Q,
    ) -> buck2_error::Result<()> {
        super::reflink_or_copy(from, to).uncategorized()
    }

    pub fn hard_link<P: AsRef<AbsPath>, Q: AsRef<AbsPath>>(
        original: P,
        link: Q,
    ) -> buck2_error::Result<()> {
        super::hard_link(original, link).uncategorized()
    }

    pub fn reflink_or_hardlink_or_copy<P: AsRef<AbsPath>, Q: AsRef<AbsPath>>(
        from: P,
        to: Q,
    ) -> buck2_error::Result<Linked> {
        super::reflink_or_hardlink_or_copy(from, to).uncategorized()
    }

    pub fn read_link<P: AsRef<AbsPath>>(path: P) -> buck2_error::Result<PathBuf> {
        super::read_link(path).uncategorized()
    }

    pub fn rename<P: AsRef<AbsPath>, Q: AsRef<AbsPath>>(from: P, to: Q) -> buck2_error::Result<()> {
        super::rename(from, to).uncategorized()
    }

    pub fn write<P: AsRef<AbsPath>, C: AsRef<[u8]>>(
        path: P,
        contents: C,
    ) -> buck2_error::Result<()> {
        super::write(path, contents).uncategorized()
    }

    pub fn write_with_executable_bit<P: AsRef<AbsPath>, C: AsRef<[u8]>>(
        path: P,
        contents: C,
        executable: bool,
    ) -> buck2_error::Result<()> {
        super::write_with_executable_bit(path, contents, executable).uncategorized()
    }

    pub fn metadata<P: AsRef<AbsPath>>(path: P) -> buck2_error::Result<fs::Metadata> {
        super::metadata(path).uncategorized()
    }

    pub fn symlink_metadata<P: AsRef<AbsPath>>(path: P) -> buck2_error::Result<fs::Metadata> {
        super::symlink_metadata(path).uncategorized()
    }

    pub fn set_permissions<P: AsRef<AbsPath>>(
        path: P,
        perm: fs::Permissions,
    ) -> buck2_error::Result<()> {
        super::set_permissions(path, perm).uncategorized()
    }

    pub fn set_executable<P: AsRef<AbsPath>>(path: P, executable: bool) -> buck2_error::Result<()> {
        super::set_executable(path, executable).uncategorized()
    }

    pub fn set_writable<P: AsRef<AbsPath>>(path: P, writable: bool) -> buck2_error::Result<()> {
        super::set_writable(path, writable).uncategorized()
    }

    pub fn remove_dir_all<P: AsRef<AbsPath>>(path: P) -> buck2_error::Result<()> {
        super::remove_dir_all(path).uncategorized()
    }

    pub fn remove_all<P: AsRef<AbsPath>>(path: P) -> buck2_error::Result<()> {
        super::remove_all(path).uncategorized()
    }

    pub fn read<P: AsRef<AbsPath>>(path: P) -> buck2_error::Result<Vec<u8>> {
        super::read(path).uncategorized()
    }

    pub fn read_to_string<P: AsRef<AbsPath>>(path: P) -> buck2_error::Result<String> {
        super::read_to_string(path).uncategorized()
    }

    pub fn canonicalize<P: AsRef<AbsPath>>(path: P) -> buck2_error::Result<AbsNormPathBuf> {
        super::canonicalize(path).uncategorized()
    }

    pub fn remove_dir<P: AsRef<AbsPath>>(path: P) -> buck2_error::Result<()> {
        super::remove_dir(path).uncategorized()
    }

    pub fn remove_dir_harder<P: AsRef<AbsPath>>(path: P) -> buck2_error::Result<()> {
        super::remove_dir_harder(path).uncategorized()
    }

    pub fn create_file<P: AsRef<AbsPath>>(path: P) -> buck2_error::Result<FileWriteGuard> {
        super::create_file(path).uncategorized()
    }

    pub fn open_file<P: AsRef<AbsPath>>(path: P) -> buck2_error::Result<FileReadGuard> {
        super::open_file(path).uncategorized()
    }
}

// Exercises the real filesystem, which Miri's isolation has none of.
#[cfg(all(test, not(miri)))]
mod tests {
    use std::fs;
    use std::fs::File;
    use std::io;
    use std::path::Path;
    use std::path::PathBuf;

    use assert_matches::assert_matches;
    use buck2_error::ErrorTag;
    use buck2_hash::BuckMutMap;

    use crate::error::IoResultExt;
    use crate::fs_util::IoError;
    use crate::fs_util::MAX_IO_ATTEMPTS;
    use crate::fs_util::uncategorized as fs_util;
    use crate::fs_util::with_retries;
    use crate::paths::abs_norm_path::AbsNormPath;
    use crate::paths::abs_path::AbsPath;
    use crate::paths::forward_rel_path::ForwardRelativePath;

    #[test]
    fn if_exists_read_dir() -> buck2_error::Result<()> {
        let binding = std::env::temp_dir();
        let existing_path = AbsNormPath::new(&binding)?;
        let res = fs_util::read_dir_if_exists(existing_path)?;
        assert!(res.is_some());
        let not_existing_dir = existing_path.join(ForwardRelativePath::unchecked_new("dir"));
        let res = fs_util::read_dir_if_exists(not_existing_dir)?;
        assert!(res.is_none());
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn copy_keeps_holes() -> buck2_error::Result<()> {
        use std::io::Seek;
        use std::io::SeekFrom;
        use std::io::Write;
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::fs::PermissionsExt;

        const LEN: u64 = 64 << 20;
        let tempdir = tempfile::tempdir()?;
        let src = AbsPath::new(tempdir.path())?.join("src");
        let dst = AbsPath::new(tempdir.path())?.join("dst");

        // Data at the start and in the middle, and a trailing hole.
        let mut file = File::create(&src)?;
        file.write_all(b"head")?;
        file.seek(SeekFrom::Start(LEN / 2))?;
        file.write_all(b"middle")?;
        file.set_len(LEN)?;
        file.set_permissions(fs::Permissions::from_mode(0o751))?;
        drop(file);
        let src_blocks = fs::metadata(&src)?.blocks();
        if src_blocks * 512 >= LEN {
            // The temporary directory is on a filesystem without holes.
            return Ok(());
        }

        assert_eq!(fs_util::copy(&src, &dst)?, LEN);
        assert_eq!(fs::read(&src)?, fs::read(&dst)?);
        let metadata = fs::metadata(&dst)?;
        assert_eq!(metadata.permissions().mode() & 0o7777, 0o751);
        assert!(metadata.blocks() * 512 < LEN / 2);
        Ok(())
    }

    #[test]
    fn create_and_remove_symlink_dir() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = tempdir.path().join("root");
        let root = AbsPath::new(&root)?;
        fs_util::create_dir_all(root)?;
        let dir1 = root.join("dir1");
        let symlink_dir1 = root.join("symlink_dir1");

        // Create dir1 and link symlink_dir1 to dir1
        fs_util::create_dir_all(&dir1)?;
        assert!(fs_util::symlink_metadata(&dir1)?.is_dir());
        fs_util::symlink(&dir1, &symlink_dir1)?;
        assert!(fs_util::symlink_metadata(&symlink_dir1)?.is_symlink());

        // Remove the symlink, dir1 should still be in tact
        fs_util::remove_file(&symlink_dir1)?;
        assert!(fs_util::symlink_metadata(&dir1)?.is_dir());
        assert_matches!(fs_util::symlink_metadata(&symlink_dir1), Err(..));

        // Clean up
        fs_util::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn create_and_remove_symlink_file() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = tempdir.path().join("root");
        let root = AbsPath::new(&root)?;
        fs_util::create_dir_all(root)?;
        let file1 = root.join("file1");
        let symlink_file1 = root.join("symlink_file1");

        // Create file1 and link symlink_file1 to file1
        File::create(&file1)?;
        assert!(fs_util::symlink_metadata(&file1)?.is_file());
        fs_util::symlink(&file1, &symlink_file1)?;
        assert!(fs_util::symlink_metadata(&symlink_file1)?.is_symlink());

        // Remove the symlink, file1 should still be in tact
        fs_util::remove_file(&symlink_file1)?;
        assert!(fs_util::symlink_metadata(&file1)?.is_file());
        assert_matches!(fs_util::symlink_metadata(&symlink_file1), Err(..));

        // Clean up
        fs_util::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn test_symlink_with_target_length_over_max_path() -> buck2_error::Result<()> {
        // In Windows, the maximum length of a path is 260.
        // To allow extended path lengths, canonicalize the paths
        // so that they are prefixed with '\\?'
        let max_path = 260;

        // 1. Test a case where we create a simple simlink to a long file path
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let symlink1 = root.join("symlink1");

        // Create a path that looks like <tmp>/subdir/subdir/.../subdir/simple_file
        let mut long_sub_path = "subdir/".repeat(max_path / 7);
        long_sub_path.push_str("simple_file");
        let target_path1 = root.join(long_sub_path);
        assert!(target_path1.to_str().unwrap().len() > max_path);

        fs_util::create_dir_all(target_path1.parent().unwrap())?;
        fs_util::symlink(&target_path1, &symlink1)?;
        fs_util::write(&target_path1, b"This is File1")?;
        assert_eq!(fs_util::read_to_string(&symlink1)?, "This is File1");

        // 2. Test a case where we create a symlink to an absolute path target with some relative ../../
        let symlink2 = root.join("symlink2");

        // Create a path that looks like <tmp>/subdir/subdir/.../subdir/../abs_with_relative
        let long_sub_path = "subdir/".repeat(max_path / 7);
        let target_path2 = root.join(long_sub_path).join("../abs_with_relative");
        assert!(target_path2.is_absolute());
        assert!(target_path2.to_str().unwrap().len() > max_path);
        fs_util::create_dir_all(&target_path2)?;
        let target_path2 = target_path2.join("file2");
        fs_util::symlink(&target_path2, &symlink2)?;
        fs_util::write(&target_path2, b"This is File2")?;
        assert_eq!(fs_util::read_to_string(&symlink2)?, "This is File2");
        Ok(())
    }

    #[test]
    fn symlink_to_file_which_doesnt_exist() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let symlink_path = root.join("symlink");
        let target_path = root.join("file");
        fs_util::symlink(&target_path, &symlink_path)?;
        fs_util::write(&target_path, b"File content")?;
        assert_eq!(fs_util::read_to_string(&symlink_path)?, "File content");
        Ok(())
    }

    #[test]
    fn symlink_with_missing_parent() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let symlink_path = root.join("missing/symlink");
        let target_path = root.join("file");
        let res =
            crate::fs_util::symlink(&target_path, &symlink_path).map_err(|e| e.categorize_input());
        let err = buck2_error::Error::from(res.unwrap_err());
        assert!(err.has_tag(ErrorTag::MissingInputPath));
        Ok(())
    }

    #[test]
    fn symlink_to_symlinked_dir() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let dir_path = root.join("dir");
        let file_path = dir_path.join("file");
        fs_util::create_dir_all(&dir_path)?;
        fs_util::write(file_path, b"Content")?;
        let symlink1_path = dir_path.join("symlink1");
        let symlink2_path = dir_path.join("symlink2");
        fs_util::symlink(&dir_path, &symlink1_path)?;
        fs_util::symlink(&symlink1_path, &symlink2_path)?;
        assert_eq!(
            fs_util::read_to_string(symlink2_path.join("file"))?,
            "Content"
        );
        assert!(fs_util::metadata(&symlink1_path)?.is_dir());
        assert!(fs_util::metadata(symlink2_path)?.is_dir());
        Ok(())
    }

    #[test]
    fn relative_symlink_to_nonexistent_file() -> buck2_error::Result<()> {
        // tmp -- dir1 (exists) -- file1 (doesn't exist)
        //     \
        //      \ symlink1 to dir1/file1
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let dir_path = root.join("dir1");
        fs_util::create_dir_all(dir_path)?;
        let relative_symlink1_path = root.join("relative_symlink1");
        fs_util::symlink("dir1/file1", &relative_symlink1_path)?;
        fs_util::write(root.join("dir1/file1"), b"File content")?;
        assert_eq!(
            fs_util::read_to_string(&relative_symlink1_path)?,
            "File content"
        );
        Ok(())
    }

    #[test]
    fn relative_symlink_to_nonexistent_dir() -> buck2_error::Result<()> {
        // tmp -- dir1 (doesn't exists) -- file1 (doesn't exist)
        //     \
        //      \ dir2 -- relative_symlink1 to ../dir1/file1
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let dir1 = root.join("dir1");
        let dir2 = root.join("dir2");
        // Only create dir2 for the symlink creation
        fs_util::create_dir_all(&dir2)?;
        let relative_symlink1_path = dir2.join("relative_symlink1");
        // Symlink creation should still work even if dir1 doesn't exist yet
        fs_util::symlink("../dir1/file1", &relative_symlink1_path)?;
        // Create dir1, test that we can write into file1 and symlink1
        fs_util::create_dir_all(dir1)?;
        fs_util::write(root.join("dir1/file1"), b"File content")?;
        assert_eq!(
            fs_util::read_to_string(&relative_symlink1_path)?,
            "File content"
        );
        fs_util::write(&relative_symlink1_path, b"File content 2")?;
        assert_eq!(
            fs_util::read_to_string(root.join("dir1/file1"))?,
            "File content 2"
        );
        Ok(())
    }

    #[test]
    fn relative_symlink_from_symlinked_dir_windows() -> buck2_error::Result<()> {
        if !cfg!(windows) {
            return Ok(());
        }

        // tmp1 -- dir1 -- subdir1 -- file1
        // tmp2 -- symlink_to_subdir1 (points to /tmp1/dir1/subdir1) -- (file1)             (points to /tmp1/dir1/subdir1/file1)
        //     \                                                    \-- symlink_to_file2    (want to symlink to ../dir2/file2, which is actually /tmp2/dir2/file2)
        //      \                                                   \-- symlink_to_file3    (want to symlink to ../dir2/file3, which is actually /tmp2/dir2/file3)
        //       \                                                  \-- symlink_to_symlink1 (want to symlink to file1, which is actually /tmp/2/dir1/subdir1/file1)
        //        \ dir2 -- file2
        //              \-- file3 (doesn't exist yet)

        // Construct the directory structure
        let tempdir1 = tempfile::tempdir()?;
        let tempdir2 = tempfile::tempdir()?;
        let tempdir1_canonicalized = tempdir1.path().canonicalize()?;
        let tempdir2_canonicalized = tempdir2.path().canonicalize()?;
        let root1 = AbsPath::new(&tempdir1_canonicalized)?;
        let root2 = AbsPath::new(&tempdir2_canonicalized)?;
        let dir1 = root1.join("dir1");
        let subdir1 = dir1.join("subdir1");
        let file1 = subdir1.join("file1");
        let dir2 = root2.join("dir2");
        let file2 = dir2.join("file2");
        let file3 = dir2.join("file3");
        fs_util::create_dir_all(&subdir1)?;
        fs_util::create_dir_all(&dir2)?;
        fs_util::write(&file1, b"File content 1")?;
        fs_util::write(&file2, b"File content 2")?;

        // Simple symlink to a directory
        let symlink_to_subdir1 = root2.join("symlink_to_subdir1");
        fs_util::symlink(&subdir1, &symlink_to_subdir1)?;
        assert_eq!(
            fs_util::read_link(&symlink_to_subdir1)?.canonicalize()?,
            subdir1.as_path()
        );
        assert_eq!(
            fs_util::read_to_string(symlink_to_subdir1.join("file1"))?,
            "File content 1"
        );

        // Test1: A relative symlink that needs to get converted to the realpath correctly
        // /tmp2/symlink_to_subdir1/symlink_to_file2 would live in /tmp1/dir1/subdir1/file2, which means the relative symlink is incorrect
        // Test that symlink properly converts to canonicalized target path
        let symlink_to_file2 = symlink_to_subdir1.join("symlink_to_file2");
        fs_util::symlink("../dir2/file2", &symlink_to_file2)?;
        assert_eq!(
            fs_util::read_link(&symlink_to_file2)?.canonicalize()?,
            file2.as_path()
        );
        assert_eq!(
            fs_util::read_to_string(&symlink_to_file2)?,
            "File content 2"
        );

        // Test2: Same case as test1, but target file doesn't exist yet
        let symlink_to_file3 = symlink_to_subdir1.join("symlink_to_file3");
        fs_util::symlink("../dir2/file3", &symlink_to_file3)?;
        fs_util::write(&file3, b"File content 3")?;
        assert_eq!(
            fs_util::read_link(&symlink_to_file3)?.canonicalize()?,
            file3.as_path()
        );
        assert_eq!(fs_util::read_to_string(&file3)?, "File content 3");
        assert_eq!(
            fs_util::read_to_string(&symlink_to_file3)?,
            "File content 3"
        );

        // Test3: Create a symlink from a symlinked directory to another symlink in the same directory
        let symlink_to_symlink1 = symlink_to_subdir1.join("symlink_to_symlink1");
        fs_util::symlink("../symlink_to_subdir1/file1", &symlink_to_symlink1)?;
        assert_eq!(
            fs_util::read_link(&symlink_to_symlink1)?.canonicalize()?,
            file1.as_path()
        );
        assert_eq!(
            fs_util::read_to_string(&symlink_to_symlink1)?,
            "File content 1"
        );
        Ok(())
    }

    #[test]
    fn absolute_symlink_to_nonexistent_file_in_nonexistent_dir() -> buck2_error::Result<()> {
        // tmp -- dir1 (doesn't exists) -- file1 (doesn't exist)
        //     \
        //      \ symlink1 to /tmp/dir1/file1
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let abs_symlink1_path = root.join("abs_symlink1");
        let target_abs_path = root.join("dir1/file1");
        fs_util::symlink(&target_abs_path, &abs_symlink1_path)?;
        fs_util::create_dir_all(root.join("dir1"))?;
        fs_util::write(&target_abs_path, b"File content")?;
        assert_eq!(fs_util::read_to_string(&abs_symlink1_path)?, "File content");
        Ok(())
    }

    #[test]
    fn remove_file_removes_symlink_to_directory() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let symlink_path = root.join("symlink_dir");
        let dir_path = root.join("dir");
        let file_path = dir_path.join("file");
        fs_util::create_dir_all(&dir_path)?;
        fs_util::write(file_path, b"File content")?;
        fs_util::symlink(&dir_path, &symlink_path)?;
        let symlinked_path = symlink_path.join("file");
        assert_eq!(fs_util::read_to_string(symlinked_path)?, "File content");
        fs_util::remove_file(&symlink_path)?;
        assert_eq!(
            io::ErrorKind::NotFound,
            fs::metadata(&symlink_path).unwrap_err().kind()
        );
        Ok(())
    }

    #[test]
    fn remove_file_does_not_remove_directory() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let dir_path = root.join("dir");
        fs_util::create_dir_all(AbsPath::new(&dir_path)?)?;
        assert_matches!(fs_util::remove_file(&dir_path), Err(..));
        assert!(dir_path.try_exists()?);
        Ok(())
    }

    #[test]
    fn remove_file_broken_symlink() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let symlink_path = tempdir.path().join("symlink");
        let symlink_path = AbsPath::new(&symlink_path)?;
        fs_util::symlink("path_which_doesnt_exist", symlink_path)?;
        fs_util::remove_file(symlink_path)?;
        assert_eq!(
            io::ErrorKind::NotFound,
            fs::symlink_metadata(symlink_path).unwrap_err().kind()
        );
        Ok(())
    }

    #[test]
    fn remove_file_non_existing_file() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let file_path = root.join("file_doesnt_exist");
        assert_matches!(fs_util::remove_file(file_path), Err(..));
        Ok(())
    }

    /// Gives the owner full permissions on the paths when dropped, so that a
    /// failing test doesn't leave a temp dir behind that can't be deleted.
    #[cfg(unix)]
    struct RestorePermissions(Vec<PathBuf>);

    #[cfg(unix)]
    impl Drop for RestorePermissions {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;

            for path in &self.0 {
                drop(fs::set_permissions(path, fs::Permissions::from_mode(0o755)));
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn remove_file_harder_in_readonly_directory() -> buck2_error::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let dir = root.join("dir");
        let file = dir.join("file");
        fs::create_dir(&dir)?;
        fs::write(&file, b"content")?;
        let _restore = RestorePermissions(vec![dir.to_path_buf()]);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555))?;

        fs_util::remove_file_harder(&file)?;
        assert!(!file.try_exists()?);
        Ok(())
    }

    #[test]
    fn remove_dir_harder_does_not_remove_nonempty_directory() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let dir = root.join("dir");
        let file = dir.join("file");
        fs::create_dir(&dir)?;
        fs::write(&file, b"content")?;

        assert_matches!(fs_util::remove_dir_harder(&dir), Err(..));
        assert_eq!(fs::read(&file)?, b"content");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn remove_dir_all_readonly_tree() -> buck2_error::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let dir = root.join("dir");
        let subdir = dir.join("subdir");
        fs::create_dir_all(&subdir)?;
        fs::write(subdir.join("file"), b"content")?;
        let _restore = RestorePermissions(vec![dir.to_path_buf(), subdir.to_path_buf()]);
        // No permissions at all on the inner directory, read-only on the outer.
        fs::set_permissions(&subdir, fs::Permissions::from_mode(0o000))?;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555))?;

        fs_util::remove_dir_all(&dir)?;
        assert!(!dir.try_exists()?);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn remove_dir_all_readonly_tree_does_not_follow_symlinks() -> buck2_error::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let target = root.join("target");
        let target_file = target.join("file");
        fs::create_dir(&target)?;
        fs::write(&target_file, b"content")?;
        let dir = root.join("dir");
        fs::create_dir(&dir)?;
        fs_util::symlink(&target, dir.join("symlink"))?;
        let _restore = RestorePermissions(vec![dir.to_path_buf(), target.to_path_buf()]);
        fs::set_permissions(&target, fs::Permissions::from_mode(0o500))?;
        // Unlinking the symlink fails until the directory is made writable.
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555))?;

        fs_util::remove_dir_all(&dir)?;
        assert!(!dir.try_exists()?);
        assert!(target_file.try_exists()?);
        let mode = fs::symlink_metadata(&target)?.permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o500,
            "the symlink target keeps its permissions"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn remove_dir_all_readonly_tree_longer_than_path_max() -> buck2_error::Result<()> {
        use nix::fcntl::OFlag;
        use nix::sys::stat::Mode;

        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let dir = root.join("dir");
        fs::create_dir(&dir)?;

        // Paths below `dir` exceed `PATH_MAX`, so create them relative to
        // their parent's descriptor.
        let name = "d".repeat(200);
        let open_dir = OFlag::O_RDONLY | OFlag::O_DIRECTORY;
        let mut fd =
            nix::fcntl::openat(nix::fcntl::AT_FDCWD, dir.as_path(), open_dir, Mode::empty())
                .map_err(io::Error::from)?;
        for _ in 0..30 {
            nix::sys::stat::mkdirat(&fd, name.as_str(), Mode::from_bits_truncate(0o755))
                .map_err(io::Error::from)?;
            fd = nix::fcntl::openat(&fd, name.as_str(), open_dir, Mode::empty())
                .map_err(io::Error::from)?;
        }
        drop(
            nix::fcntl::openat(
                &fd,
                "file",
                OFlag::O_CREAT | OFlag::O_WRONLY,
                Mode::from_bits_truncate(0o644),
            )
            .map_err(io::Error::from)?,
        );
        nix::sys::stat::fchmod(&fd, Mode::from_bits_truncate(0o555)).map_err(io::Error::from)?;

        fs_util::remove_dir_all(&dir)?;
        assert!(!dir.try_exists()?);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn remove_dir_all_does_not_modify_parent_permissions() -> buck2_error::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let parent = root.join("parent");
        let dir = parent.join("dir");
        fs::create_dir_all(&dir)?;
        let _restore = RestorePermissions(vec![parent.to_path_buf()]);
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o555))?;

        // Depending on capabilities (e.g. CAP_DAC_OVERRIDE) the removal may
        // succeed or fail; either way the parent's permissions must be left
        // untouched.
        drop(fs_util::remove_dir_all(&dir));
        let mode = fs::symlink_metadata(&parent)?.permissions().mode();
        assert_eq!(mode & 0o777, 0o555);
        Ok(())
    }

    #[test]
    fn remove_dir_all_does_not_follow_symlinks() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let target = root.join("target");
        let target_file = target.join("file");
        fs::create_dir(&target)?;
        fs::write(&target_file, b"content")?;
        let dir = root.join("dir");
        fs::create_dir(&dir)?;
        fs_util::symlink(&target, dir.join("symlink"))?;

        fs_util::remove_dir_all(&dir)?;
        assert!(!dir.try_exists()?);
        assert!(target_file.try_exists()?);
        Ok(())
    }

    #[test]
    fn remove_all_nonexistent() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        fs_util::remove_all(root.join("nonexistent"))?;
        Ok(())
    }

    #[test]
    fn remove_all_regular() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let path = root.join("file");
        fs::write(&path, b"regular")?;
        fs_util::remove_all(&path)?;
        assert!(!path.try_exists()?);
        Ok(())
    }

    #[test]
    fn remove_all_dir() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let path = root.join("dir");
        fs::create_dir(&path)?;
        fs::write(path.join("file"), b"regular file in a dir")?;
        fs_util::remove_all(&path)?;
        assert!(!path.try_exists()?);
        Ok(())
    }

    #[test]
    fn remove_all_broken_symlink() -> buck2_error::Result<()> {
        fn ls(path: &Path) -> buck2_error::Result<Vec<PathBuf>> {
            let mut entries = fs::read_dir(path)?
                .map(|entry| Ok(entry.map(|entry| entry.path())?))
                .collect::<buck2_error::Result<Vec<_>>>()?;
            entries.sort();
            Ok(entries)
        }

        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let target = root.join("non-existent-target");
        let path = root.join("symlink");
        fs_util::symlink(target, &path)?;

        assert_eq!(vec![path.as_path()], ls(tempdir.path())?, "Sanity check");

        fs_util::remove_all(&path)?;

        // We cannot use `exists` here because it does not report what we need on broken symlink.
        assert_eq!(Vec::<PathBuf>::new(), ls(tempdir.path())?);

        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn remove_all_path_contains_regular_file() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let regular_file = root.join("foo");
        fs_util::write(&regular_file, b"data")?;
        let path = root.join("foo/bar");
        assert!(fs_util::remove_all(&path).is_ok());
        Ok(())
    }

    #[test]
    fn remove_dir_all_does_not_remove_file() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let file_path = root.join("file");
        fs::write(&file_path, b"File content")?;
        assert!(fs_util::remove_dir_all(&file_path).is_err());
        assert!(file_path.try_exists()?);
        Ok(())
    }

    #[test]
    fn test_create_dir_if_not_exists() {
        let tempdir = tempfile::tempdir().unwrap();
        let tempdir = AbsPath::new(tempdir.path()).unwrap();
        fs_util::create_dir_if_not_exists(tempdir.join("dir1")).unwrap();
        assert!(
            fs_util::symlink_metadata(tempdir.join("dir1"))
                .unwrap()
                .is_dir()
        );
        fs_util::create_dir_if_not_exists(tempdir.join("dir1")).unwrap();
        assert!(
            fs_util::symlink_metadata(tempdir.join("dir1"))
                .unwrap()
                .is_dir()
        );

        assert!(fs_util::create_dir_if_not_exists(tempdir.join("dir2/file")).is_err());
        assert!(!fs_util::try_exists(tempdir.join("dir2")).unwrap());

        fs_util::write(tempdir.join("file"), b"rrr").unwrap();
        assert!(fs_util::create_dir_if_not_exists(tempdir.join("file")).is_err());
    }

    #[test]
    fn test_read_to_string_if_exists() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let f1 = root.join("f1");
        let f2 = root.join("f2");

        fs_util::write(&f1, b"data")?;
        assert_eq!(
            fs_util::read_to_string_if_exists(&f1)?.as_deref(),
            Some("data")
        );
        assert_eq!(fs_util::read_to_string_if_exists(f2)?, None);

        Ok(())
    }

    #[test]
    fn test_read_if_exists() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let f1 = root.join("f1");
        let f2 = root.join("f2");

        fs_util::write(&f1, b"data")?;
        assert_eq!(
            fs_util::read_if_exists(&f1)?.as_deref(),
            Some(b"data".as_slice())
        );
        assert_eq!(fs_util::read_if_exists(f2)?, None);

        Ok(())
    }

    #[test]
    fn test_set_executable() {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;

        let tempdir = tempfile::tempdir().unwrap();
        let root = AbsPath::new(tempdir.path()).unwrap();
        let path = root.join("file");
        fs_util::write(&path, b"rrr").unwrap();

        #[cfg(unix)]
        {
            let mode = fs_util::metadata(&path).unwrap().permissions().mode();
            // Default behavior: executable bits are not set
            assert_eq!(0, mode & 0o111);
        }

        fs_util::set_executable(&path, true).unwrap();

        #[cfg(unix)]
        {
            let mode = fs_util::metadata(&path).unwrap().permissions().mode();
            // executable bits should now be set
            assert_eq!(0o111, mode & 0o111);
        }

        fs_util::set_executable(&path, false).unwrap();
        #[cfg(unix)]
        {
            let mode = fs_util::metadata(&path).unwrap().permissions().mode();
            // executable bits should now be unset
            assert_eq!(0, mode & 0o111);
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_remove_all_removes_readonly_path() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let path = root.join("foo/bar/link");
        fs_util::create_dir_all(path.parent().unwrap())?;
        fs_util::write(&path, b"data")?;
        let mut perm = fs_util::metadata(&path)?.permissions();
        perm.set_readonly(true);
        fs_util::set_permissions(&path, perm)?;
        fs_util::remove_all(&path)?;
        assert!(!path.exists());
        Ok(())
    }

    #[test]
    fn test_set_writable() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let path = root.join("file");
        fs_util::write(&path, b"data")?;
        fs_util::set_executable(&path, true)?;

        fs_util::set_writable(&path, false)?;
        let perms = fs_util::metadata(&path)?.permissions();
        assert!(perms.readonly());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(perms.mode() & 0o777, 0o555, "the executable bits stay");
        }
        assert!(
            std::fs::OpenOptions::new().write(true).open(&path).is_err(),
            "nobody writes a read-only file"
        );

        fs_util::set_writable(&path, true)?;
        assert!(!fs_util::metadata(&path)?.permissions().readonly());
        fs_util::write(&path, b"again")?;
        assert_eq!(fs_util::read_to_string(&path)?, "again");
        Ok(())
    }

    #[test]
    fn test_rename_replaces_a_read_only_file() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let from = root.join("from");
        let to = root.join("to");
        fs_util::write(&from, b"new")?;
        fs_util::write(&to, b"old")?;
        fs_util::set_writable(&to, false)?;

        fs_util::rename(&from, &to)?;
        assert_eq!(fs_util::read_to_string(&to)?, "new");
        assert!(!from.exists());
        Ok(())
    }

    #[test]
    fn test_create_file_if_not_exists() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let path = root.join("foo.txt");
        let _file = fs_util::create_file_if_not_exists(&path)?.unwrap();
        assert!(fs_util::create_file_if_not_exists(path)?.is_none());
        Ok(())
    }

    #[test]
    fn test_reflink_or_copy_file() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let src = root.join("src");
        let dest = root.join("dest");

        fs_util::write(&src, b"original")?;
        fs_util::reflink_or_copy(&src, &dest)?;
        assert_eq!(fs_util::read_to_string(&dest)?, "original");

        // Whether the copy reflinked or not, the two files must be independent.
        fs_util::write(&src, b"modified")?;
        assert_eq!(fs_util::read_to_string(&dest)?, "original");
        fs_util::write(&dest, b"dest only")?;
        assert_eq!(fs_util::read_to_string(&src)?, "modified");

        Ok(())
    }

    #[test]
    fn test_hard_link_shares_the_inode() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let src = root.join("src");
        let link = root.join("link");

        fs_util::write(&src, b"content")?;
        fs_util::hard_link(&src, &link)?;
        assert_eq!(fs_util::read_to_string(&link)?, "content");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                fs_util::metadata(&src)?.ino(),
                fs_util::metadata(&link)?.ino()
            );
        }
        assert_matches!(fs_util::hard_link(&src, &link), Err(..));

        Ok(())
    }

    #[test]
    fn test_reflink_or_hardlink_or_copy_puts_the_content_and_refuses_an_existing_dest()
    -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let src = root.join("src");
        let dest = root.join("dest");

        fs_util::write(&src, b"content")?;
        let linked = fs_util::reflink_or_hardlink_or_copy(&src, &dest)?;
        assert_eq!(fs_util::read_to_string(&dest)?, "content");
        assert_matches!(fs_util::reflink_or_hardlink_or_copy(&src, &dest), Err(..));
        assert_eq!(fs_util::read_to_string(&dest)?, "content");

        // Which leg ran is the filesystem's business; only a hardlink lets a write via one path
        // show up via the other.
        fs_util::write(&src, b"modified")?;
        let shared = fs_util::read_to_string(&dest)? == "modified";
        assert_eq!(shared, linked == fs_util::Linked::Hardlinked, "{linked:?}");

        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn test_reflink_or_copy_keeps_the_mode() -> buck2_error::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let src = root.join("src");
        let dest = root.join("dest");

        fs_util::write(&src, b"run me")?;
        fs_util::set_executable(&src, true)?;
        fs_util::reflink_or_copy(&src, &dest)?;
        assert_ne!(fs_util::metadata(&dest)?.permissions().mode() & 0o111, 0);

        Ok(())
    }

    #[test]
    fn test_reflink_or_copy_rejects_existing_dest() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let src = root.join("src");
        let dest = root.join("dest");

        fs_util::write(&src, b"src")?;
        fs_util::write(&dest, b"dest")?;
        assert_matches!(fs_util::reflink_or_copy(&src, &dest), Err(..));
        assert_eq!(fs_util::read_to_string(&dest)?, "dest");

        Ok(())
    }

    #[test]
    fn test_reflink_or_copy_all_directory() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let canonical = tempdir.path().canonicalize()?;
        let root = AbsNormPath::new(&canonical)?;
        let src = root.join(ForwardRelativePath::new("src")?);
        let dest = root.join(ForwardRelativePath::new("dest")?);

        fs_util::create_dir_all(src.join(ForwardRelativePath::new("sub/subsub")?))?;
        fs_util::write(src.join(ForwardRelativePath::new("top")?), b"top")?;
        fs_util::write(src.join(ForwardRelativePath::new("sub/mid")?), b"mid")?;
        fs_util::write(
            src.join(ForwardRelativePath::new("sub/subsub/leaf")?),
            b"leaf",
        )?;

        fs_util::reflink_or_copy_all(&src, &dest)?;

        assert_eq!(
            fs_util::read_to_string(dest.join(ForwardRelativePath::new("top")?))?,
            "top"
        );
        assert_eq!(
            fs_util::read_to_string(dest.join(ForwardRelativePath::new("sub/mid")?))?,
            "mid"
        );
        assert_eq!(
            fs_util::read_to_string(dest.join(ForwardRelativePath::new("sub/subsub/leaf")?))?,
            "leaf"
        );

        // The trees must not share storage that a write can be observed through.
        fs_util::write(src.join(ForwardRelativePath::new("sub/mid")?), b"changed")?;
        assert_eq!(
            fs_util::read_to_string(dest.join(ForwardRelativePath::new("sub/mid")?))?,
            "mid"
        );

        Ok(())
    }

    #[test]
    fn test_reflink_or_copy_all_preserves_symlinks() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let canonical = tempdir.path().canonicalize()?;
        let root = AbsNormPath::new(&canonical)?;
        let src = root.join(ForwardRelativePath::new("src")?);
        let dest = root.join(ForwardRelativePath::new("dest")?);

        fs_util::create_dir_all(&src)?;
        fs_util::write(src.join(ForwardRelativePath::new("file")?), b"content")?;
        fs_util::symlink("file", src.join(ForwardRelativePath::new("link")?))?;

        fs_util::reflink_or_copy_all(&src, &dest)?;

        let dest_link = dest.join(ForwardRelativePath::new("link")?);
        assert!(fs_util::symlink_metadata(&dest_link)?.is_symlink());
        assert_eq!(fs_util::read_to_string(&dest_link)?, "content");

        Ok(())
    }

    #[test]
    fn test_disk_space_stats() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let disk_space = fs_util::disk_space_stats(root)?;
        assert!(disk_space.total_space > disk_space.free_space);
        Ok(())
    }

    static TEST_FILE_CONTENT: &str = "test";

    fn check_io_with_retry(
        path: &Path,
        error_kind: std::io::ErrorKind,
        expected_attempts: u32,
        should_succeed: bool,
    ) {
        let mut attempts: u32 = 0;
        let mut open_fn = |p: &Path| -> io::Result<File> {
            attempts += 1;
            if attempts >= MAX_IO_ATTEMPTS {
                std::fs::File::open(p)
            } else {
                Err(io::Error::new(error_kind, error_kind.to_string()))
            }
        };
        let io_result = with_retries(|| open_fn(path))
            .map_err(|e| IoError::new(e).context("test123".to_owned()));

        if should_succeed {
            let mut file = io_result.unwrap();
            assert_eq!(attempts, expected_attempts);
            let mut buf = String::new();
            io::Read::read_to_string(&mut file, &mut buf).unwrap();
            assert_eq!(buf, TEST_FILE_CONTENT);
        } else {
            assert_eq!(
                io_result.err().and_then(|e| e.io_error_kind()),
                Some(error_kind)
            );
            assert_eq!(attempts, expected_attempts);
        }
    }

    fn get_test_path(name: &str, tempdir: &tempfile::TempDir) -> std::path::PathBuf {
        let path = tempdir.path().join(name);
        std::fs::write(&path, TEST_FILE_CONTENT).unwrap();
        path
    }

    #[test]
    fn test_retry_io() -> buck2_error::Result<()> {
        use std::io::ErrorKind;

        let tempdir = tempfile::tempdir().unwrap();
        let mut test_cases = BuckMutMap::default();
        // The behavior of these test cases varies by platform
        let should_succeed = cfg!(target_os = "macos");
        let expected_attempts = if should_succeed { MAX_IO_ATTEMPTS } else { 1 };
        test_cases.insert(
            get_test_path("test_timeout", &tempdir),
            (ErrorKind::TimedOut, expected_attempts, should_succeed),
        );
        test_cases.insert(
            get_test_path("test_stale", &tempdir),
            (
                ErrorKind::StaleNetworkFileHandle,
                expected_attempts,
                should_succeed,
            ),
        );

        // These test cases should behave the same on all platforms
        test_cases.insert(
            get_test_path("test_too_many_args", &tempdir),
            (ErrorKind::ArgumentListTooLong, 1, false),
        );
        test_cases.insert(
            get_test_path("test_permission_denied", &tempdir),
            (ErrorKind::PermissionDenied, 1, false),
        );

        for (test_path, results) in test_cases {
            check_io_with_retry(&test_path, results.0, results.1, results.2);
        }

        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn read_dir_on_file_tags_not_a_directory() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let canonical = tempdir.path().canonicalize()?;
        let root = AbsNormPath::new(&canonical)?;
        let file_path = root.join(ForwardRelativePath::unchecked_new("a_file"));
        fs_util::write(&file_path, b"content")?;
        let file_path = AbsNormPath::new(&file_path)?;
        let err = crate::fs_util::read_dir(file_path)
            .categorize_input()
            .err()
            .expect("Error expected");
        assert!(err.has_tag(ErrorTag::InputPathNotADirectory));
        Ok(())
    }

    #[test]
    fn test_io_error_tag() -> buck2_error::Result<()> {
        let fail_fn = || -> io::Result<File> {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "test error, always fails",
            ))
        };

        let file =
            with_retries(fail_fn).map_err(|e| IoError::new(e).context("should fail".to_owned()));
        let buck2_error = file.uncategorized().err().unwrap();

        assert_eq!(
            buck2_error.tags(),
            &[ErrorTag::IoPermissionDenied, ErrorTag::IoSystem]
        );
        Ok(())
    }
}
