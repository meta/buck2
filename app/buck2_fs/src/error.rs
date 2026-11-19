/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Error types for filesystem operations.

use std::io;
#[cfg(target_os = "linux")]
use std::path::Path;
#[cfg(any(fbcode_build, target_os = "linux"))]
use std::path::PathBuf;

use buck2_error::ErrorTag;
use tokio::task::JoinError;

use crate::paths::abs_path::AbsPath;

/// Hint attached to `EIO` errors raised for paths on an EdenFS mount.
///
/// EdenFS answers a FUSE request with `EIO` whenever it cannot produce the data: a tree or blob
/// fetch from the source control server failed (network, certificates, load shedding), the
/// overlay is corrupt, or the mount is shutting down. None of those are problems with the path
/// being read or with buck2.
pub const EDEN_EIO_HINT: &str = "EdenFS returned an I/O error for this path. This usually means \
    EdenFS could not fetch data from the source control server (network, certificates or server \
    load shedding) or the mount is unhealthy; it is not a problem with the build files themselves. \
    Check your network connection and run `eden doctor`";

/// Hint attached to `ENOTCONN` errors raised for paths on an EdenFS mount.
pub const EDEN_NOT_CONNECTED_HINT: &str = "The EdenFS mount for this path is not connected. \
    EdenFS is probably not running or the checkout is no longer mounted. Run `eden doctor`";

/// The hint to attach to `e` if it is an I/O error that says the EdenFS mount itself is unhealthy.
pub fn eden_io_error_hint(e: &buck2_error::Error) -> Option<&'static str> {
    if e.has_tag(ErrorTag::IoNotConnected) {
        Some(EDEN_NOT_CONNECTED_HINT)
    } else if e.has_tag(ErrorTag::IoInputOutputError) {
        Some(EDEN_EIO_HINT)
    } else {
        None
    }
}

/// Classify `e` as an EdenFS error, and attach the matching hint, if it is an `EIO` or
/// `ENOTCONN` error that was raised for `path`, which the caller knows lives in an EdenFS
/// checkout.
///
/// Callers that know at construction time that the project is on EdenFS (the I/O provider, which
/// detected it while the mount was healthy) should use this rather than relying on
/// [`IoError::check_eden`]: probing the mount at error time fails in the same way the read did.
/// The mount table is still consulted where available, so a path that is inside the checkout but
/// on a different filesystem (a `buck-out` redirection onto local disk, for example) is not blamed
/// on Eden. Errors that already carry `IoEden` were classified, and hinted, when the `IoError`
/// was converted and are returned unchanged.
pub fn tag_eden_io_error(path: &AbsPath, e: buck2_error::Error) -> buck2_error::Error {
    tag_eden_io_error_impl(is_on_eden_mount(path), e)
}

fn tag_eden_io_error_impl(
    on_eden_mount: Option<bool>,
    e: buck2_error::Error,
) -> buck2_error::Error {
    if e.has_tag(ErrorTag::IoEden) {
        return e;
    }
    let Some(hint) = eden_io_error_hint(&e) else {
        return e;
    };
    if on_eden_mount == Some(false) {
        return e;
    }
    e.tag([ErrorTag::IoEden]).context(hint)
}

/// Whether `path` is on an EdenFS mount according to the mount table, without touching the path
/// itself (which may be on a mount that no longer answers).
///
/// `Some(_)` on Linux, from `/proc/self/mountinfo`; `None` where no mount table is consulted.
pub fn is_on_eden_mount(path: &AbsPath) -> Option<bool> {
    #[cfg(target_os = "linux")]
    {
        let mountinfo = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
        Some(mount_is_eden(&mountinfo, path.as_path()))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        None
    }
}

/// Whether the mount that `path` is on, per the `/proc/self/mountinfo` contents in `mountinfo`,
/// is an EdenFS mount. EdenFS mounts show up as `fuse` (or `nfs`) with the source `edenfs:`.
#[cfg(target_os = "linux")]
fn mount_is_eden(mountinfo: &str, path: &Path) -> bool {
    let mut best: Option<(PathBuf, &str, &str)> = None;
    for line in mountinfo.lines() {
        // `<id> <parent> <maj:min> <root> <mount point> <options> [<optional>...] - <fstype> <source> <super options>`
        let Some((before, after)) = line.split_once(" - ") else {
            continue;
        };
        let mut before = before.split(' ');
        let Some(mount_point) = before.nth(4) else {
            continue;
        };
        let mut after = after.split(' ');
        let (Some(fstype), Some(source)) = (after.next(), after.next()) else {
            continue;
        };
        let mount_point = PathBuf::from(unescape_mountinfo(mount_point));
        if !path.starts_with(&mount_point) {
            continue;
        }
        if best.as_ref().is_none_or(|(best_point, _, _)| {
            mount_point.as_os_str().len() >= best_point.as_os_str().len()
        }) {
            best = Some((mount_point, fstype, source));
        }
    }
    best.is_some_and(|(_, fstype, source)| {
        source.starts_with("edenfs") || fstype.contains("edenfs")
    })
}

/// Mount points in `/proc/self/mountinfo` have space, tab, newline and backslash octal-escaped,
/// one escape per byte, so a non-ASCII path arrives as a run of `\\ooo` escapes that must be
/// reassembled as bytes, not as chars.
#[cfg(target_os = "linux")]
fn unescape_mountinfo(s: &str) -> std::ffi::OsString {
    use std::os::unix::ffi::OsStringExt;

    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && bytes.len() - i >= 4 && s.is_char_boundary(i + 4) {
            if let Ok(code) = u8::from_str_radix(&s[i + 1..i + 4], 8) {
                out.push(code);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    std::ffi::OsString::from_vec(out)
}

/// Decide whether the parent directory of a failed path is on EdenFS, from the result of a strict
/// `.eden` marker lookup, the mount table, and the lenient marker walk.
///
/// - The marker was found or ruled out: that is the answer.
/// - The marker lookup itself failed with `EIO` or `ENOTCONN`: the filesystem is not answering
///   the way a broken EdenFS mount does not answer, and a healthy local filesystem never does for
///   a `stat`. The lenient walk reports "not Eden" in exactly that situation, which made EdenFS
///   failures look like buck2 failures precisely when EdenFS was broken. Since a dying local disk
///   or a hung network mount can fail the same way, confirm with the mount table when there is
///   one; without a mount table the probe failure is taken at face value.
/// - Any other failure (`EACCES` on an ACL-restricted directory, for instance): fall back to the
///   lenient walk, which skips unreadable markers and keeps climbing, so detection is no worse
///   than before.
#[cfg(fbcode_build)]
fn classify_eden_lookup(
    strict: Result<Option<PathBuf>, io::Error>,
    on_eden_mount: impl FnOnce() -> Option<bool>,
    lenient: impl FnOnce() -> bool,
) -> bool {
    match strict {
        Ok(root) => root.is_some(),
        Err(e) if is_unhealthy_mount_error(&e) => on_eden_mount().unwrap_or(true),
        Err(_) => lenient(),
    }
}

#[cfg(fbcode_build)]
fn is_unhealthy_mount_error(e: &io::Error) -> bool {
    if e.kind() == io::ErrorKind::NotConnected {
        return true;
    }
    #[cfg(unix)]
    if e.raw_os_error() == Some(libc::EIO) {
        return true;
    }
    false
}

/// The inner error type that IoError can hold.
#[derive(Debug)]
pub enum IoErrorSource {
    Io(io::Error),
    Internal(buck2_error::Error),
}

impl From<io::Error> for IoErrorSource {
    fn from(e: io::Error) -> Self {
        IoErrorSource::Io(e)
    }
}

impl IoError {
    /// Create an error from an io::Error or buck2_error::Error.
    pub fn new<T: Into<IoErrorSource>>(e: T) -> Self {
        Self {
            source: e.into(),
            context: Vec::new(),
            tags: Vec::new(),
            is_eden: false,
            is_input_path: None,
            not_found_tag: None,
        }
    }

    pub fn internal(e: buck2_error::Error) -> Self {
        Self::new(IoErrorSource::Internal(e))
    }

    pub fn from_join(e: JoinError) -> Self {
        Self::internal(buck2_error::Error::from(e))
    }

    /// Set operation and is_eden based on provided path.
    pub fn new_with_path<T: Into<IoErrorSource>, P: AsRef<AbsPath>>(
        op: &str,
        path: P,
        e: T,
    ) -> Self {
        let path = path.as_ref();
        IoError::new(e)
            .context(format!("{}({})", op, path.display()))
            .check_eden(path)
    }

    pub fn context(mut self, op: impl Into<String>) -> Self {
        self.context.push(op.into());
        self
    }

    pub fn tag(mut self, tag: ErrorTag) -> Self {
        self.tags.push(tag);
        self
    }

    /// Set the is_eden flag if provided path is on an eden fs.
    ///
    /// A marker lookup that itself fails with `EIO`/`ENOTCONN` on an EdenFS mount counts as
    /// Eden, see [`classify_eden_lookup`].
    #[cfg(fbcode_build)]
    pub fn check_eden(mut self, path: &AbsPath) -> Self {
        self.is_eden |= path.parent().is_some_and(|dir| {
            classify_eden_lookup(
                detect_eden::find_eden_root_strict(dir.to_path_buf()),
                || is_on_eden_mount(dir),
                || detect_eden::is_eden(dir.to_path_buf()).unwrap_or(false),
            )
        });
        self
    }

    #[cfg(not(fbcode_build))]
    pub fn check_eden(self, _path: &AbsPath) -> Self {
        self
    }

    pub fn inner_error(self) -> IoErrorSource {
        self.source
    }

    pub fn io_error_kind(&self) -> Option<io::ErrorKind> {
        match &self.source {
            IoErrorSource::Io(e) => Some(e.kind()),
            IoErrorSource::Internal(_) => None,
        }
    }

    fn convert_to_buck2_error(self) -> buck2_error::Error {
        let mut tags = vec![ErrorTag::IoSystem];

        if let IoErrorSource::Io(e) = &self.source {
            if self.is_eden {
                tags.push(ErrorTag::IoEden);
                match e.kind() {
                    io::ErrorKind::NotFound => tags.push(ErrorTag::IoEdenFileNotFound),
                    io::ErrorKind::TimedOut => {
                        // Eden timeouts are most likely caused by network issues.
                        // TODO check network health to be sure.
                        tags.push(ErrorTag::Environment);
                    }
                    _ => {}
                }
            }
            if e.kind() == io::ErrorKind::NotFound {
                tags.extend(self.not_found_tag);
            }
            if e.kind() == io::ErrorKind::NotADirectory {
                if self.is_input_path == Some(true) {
                    tags.push(ErrorTag::InputPathNotADirectory);
                }
            }
        }

        tags.extend(self.tags);
        let context = self.context.clone();
        let source_error: buck2_error::Error = match self.source {
            IoErrorSource::Io(e) => e.into(),
            IoErrorSource::Internal(e) => e,
        };
        let mut result = source_error.tag(tags);
        if self.is_eden {
            if let Some(hint) = eden_io_error_hint(&result) {
                result = result.context(hint);
            }
        }
        for ctx in context.into_iter().rev() {
            result = result.context(ctx);
        }
        result
    }

    /// Convert to a buck2_error::Error, applying `tag` if the path was missing.
    ///
    /// The tag names a path that was not there, so it is only meaningful for `NotFound`. Other
    /// error kinds from the same call - permission denied and friends - keep the categorization
    /// they already have rather than being labelled missing.
    pub fn categorize_tagged(mut self, tag: ErrorTag) -> buck2_error::Error {
        self.not_found_tag = Some(tag);
        self.convert_to_buck2_error()
    }

    /// Convert to a buck2_error::Error and tag as an input error if a path is missing.
    /// This should be used if the path was user provided,
    /// from a source file, configuration or CLI argument.
    pub fn categorize_input(mut self) -> buck2_error::Error {
        self.is_input_path = Some(true);
        self.categorize_tagged(ErrorTag::MissingInputPath)
    }

    /// Convert to a buck2_error::Error and tag as a tier0 error if a path is missing.
    /// This should be used if the path was not user provided,
    /// generated by buck2 or internal infra.
    /// This is also used by functions in fs_util that should never produce an error
    /// with kind==ErrorKind::NotFound, such as *_if_exists functions.
    pub fn categorize_internal(mut self) -> buck2_error::Error {
        self.is_input_path = Some(false);
        self.categorize_tagged(ErrorTag::MissingInternalPath)
    }

    /// Convert to a buck2_error::Error with no tags, new code should only use this in tests
    pub fn uncategorized(self) -> buck2_error::Error {
        self.convert_to_buck2_error()
    }
}

#[derive(Debug)]
pub struct IoError {
    pub(crate) source: IoErrorSource,
    pub(crate) context: Vec<String>,
    pub(crate) tags: Vec<ErrorTag>,
    pub(crate) is_eden: bool,
    pub(crate) is_input_path: Option<bool>,
    pub(crate) not_found_tag: Option<ErrorTag>,
}

pub trait IoResultExt<T> {
    fn categorize_input(self) -> buck2_error::Result<T>;
    fn categorize_internal(self) -> buck2_error::Result<T>;
    /// See [`IoError::categorize_tagged`].
    fn categorize_tagged(self, tag: ErrorTag) -> buck2_error::Result<T>;
    fn uncategorized(self) -> buck2_error::Result<T>;
}

impl<T> IoResultExt<T> for Result<T, IoError> {
    fn categorize_tagged(self, tag: ErrorTag) -> buck2_error::Result<T> {
        self.map_err(|e| e.categorize_tagged(tag))
    }
    fn categorize_input(self) -> buck2_error::Result<T> {
        self.map_err(|e| e.categorize_input())
    }
    fn categorize_internal(self) -> buck2_error::Result<T> {
        self.map_err(|e| e.categorize_internal())
    }
    fn uncategorized(self) -> buck2_error::Result<T> {
        self.map_err(|e| e.uncategorized())
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::*;

    #[cfg(unix)]
    fn io_error(code: i32) -> IoError {
        IoError {
            source: IoErrorSource::Io(io::Error::from_raw_os_error(code)),
            context: vec!["read_dir(/repo/dir)".to_owned()],
            tags: Vec::new(),
            is_eden: false,
            is_input_path: None,
            not_found_tag: None,
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_eden_eio_is_tagged_and_hinted() {
        let mut err = io_error(libc::EIO);
        err.is_eden = true;
        let err = err.categorize_input();
        assert!(err.has_tag(ErrorTag::IoEden));
        assert!(err.has_tag(ErrorTag::IoInputOutputError));
        assert!(format!("{err:#}").contains("eden doctor"), "{err:#}");
        // The hint sits next to the OS error, under the operation context.
        assert!(
            format!("{err:#}").starts_with("read_dir(/repo/dir)"),
            "{err:#}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_eden_enotconn_is_tagged_and_hinted() {
        let mut err = io_error(libc::ENOTCONN);
        err.is_eden = true;
        let err = err.categorize_internal();
        assert!(err.has_tag(ErrorTag::IoEden));
        assert!(err.has_tag(ErrorTag::IoNotConnected));
        assert!(
            format!("{err:#}").contains(EDEN_NOT_CONNECTED_HINT),
            "{err:#}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_non_eden_eio_is_not_hinted() {
        let err = io_error(libc::EIO).categorize_input();
        assert!(!err.has_tag(ErrorTag::IoEden));
        assert!(err.has_tag(ErrorTag::IoInputOutputError));
        assert!(!format!("{err:#}").contains("eden doctor"), "{err:#}");
    }

    #[cfg(unix)]
    #[test]
    fn test_eden_not_found_has_no_hint() {
        let mut err = io_error(libc::ENOENT);
        err.is_eden = true;
        let err = err.categorize_input();
        assert!(err.has_tag(ErrorTag::IoEden));
        assert!(!format!("{err:#}").contains("eden doctor"), "{err:#}");
    }

    #[cfg(unix)]
    #[test]
    fn test_tag_eden_io_error() {
        let eio = || -> buck2_error::Error { io::Error::from_raw_os_error(libc::EIO).into() };

        // Mount table confirms Eden, or there is no mount table: tagged and hinted.
        for on_eden_mount in [Some(true), None] {
            let tagged = tag_eden_io_error_impl(on_eden_mount, eio());
            assert!(tagged.has_tag(ErrorTag::IoEden));
            assert!(format!("{tagged:#}").contains(EDEN_EIO_HINT), "{tagged:#}");

            // Already classified by `IoError`: not hinted a second time.
            let twice = tag_eden_io_error_impl(on_eden_mount, tagged);
            assert_eq!(format!("{twice:#}").matches("eden doctor").count(), 1);
        }

        // Mount table says the path is on another filesystem (e.g. a `buck-out` redirection onto
        // local disk): left alone even though the caller thinks it is in an Eden checkout.
        let untouched = tag_eden_io_error_impl(Some(false), eio());
        assert!(!untouched.has_tag(ErrorTag::IoEden));
        assert!(!format!("{untouched:#}").contains("eden doctor"));

        // Other errors are left alone, even on Eden.
        let enoent: buck2_error::Error = io::Error::from_raw_os_error(libc::ENOENT).into();
        let untouched = tag_eden_io_error_impl(Some(true), enoent);
        assert!(!untouched.has_tag(ErrorTag::IoEden));
        assert!(!format!("{untouched:#}").contains("eden doctor"));
    }

    // Exercises the real filesystem and mount table, which Miri's isolation has none of.
    #[cfg(all(unix, not(miri)))]
    #[test]
    fn test_tag_eden_io_error_real_path() {
        // The real entry point on a path that is not on Eden (a temp dir) does not tag.
        let dir = tempfile::tempdir().unwrap();
        let path = AbsPath::new(dir.path()).unwrap();
        let eio: buck2_error::Error = io::Error::from_raw_os_error(libc::EIO).into();
        let untouched = tag_eden_io_error(path, eio);
        assert_eq!(
            untouched.has_tag(ErrorTag::IoEden),
            // Only the mount table can rule it out; without one the caller is trusted.
            is_on_eden_mount(path).is_none()
        );
    }

    #[cfg(all(fbcode_build, unix))]
    #[test]
    fn test_classify_eden_lookup() {
        fn no_mount_table() -> Option<bool> {
            None
        }
        fn not_consulted() -> bool {
            panic!("lenient walk should not run")
        }

        assert!(classify_eden_lookup(
            Ok(Some(PathBuf::from("/repo"))),
            no_mount_table,
            not_consulted
        ));
        assert!(!classify_eden_lookup(
            Ok(None),
            no_mount_table,
            not_consulted
        ));

        // Probe failed like a dead mount: the mount table decides, or Eden is assumed without one.
        for code in [libc::EIO, libc::ENOTCONN] {
            let err = || Err(io::Error::from_raw_os_error(code));
            assert!(classify_eden_lookup(err(), || Some(true), not_consulted));
            assert!(!classify_eden_lookup(err(), || Some(false), not_consulted));
            assert!(classify_eden_lookup(err(), no_mount_table, not_consulted));
        }

        // Any other probe failure (an ACL'd parent, a vanished marker) defers to the lenient walk.
        for code in [libc::EACCES, libc::ENOENT] {
            let err = || Err(io::Error::from_raw_os_error(code));
            assert!(classify_eden_lookup(err(), no_mount_table, || true));
            assert!(!classify_eden_lookup(err(), no_mount_table, || false));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_mount_is_eden() {
        // Real lines from a devserver: an EdenFS FUSE mount, a `buck-out` bind redirection onto
        // btrfs inside it, the root filesystem, and a mount point with an escaped space.
        let mountinfo = "\
87 1 0:35 / / rw,relatime shared:1 - btrfs /dev/vda3 rw,compress-force=zstd:3\n\
1953 87 0:104 / /data/users/riggspc/fbsource rw,nosuid,relatime shared:1292 - fuse edenfs: rw,user_id=131963,group_id=100,default_permissions,allow_other\n\
2105 1953 0:35 /data/users/riggspc/scratch/buck-out /data/users/riggspc/fbsource/buck-out rw,relatime shared:1 - btrfs /dev/vda3 rw,compress-force=zstd:3\n\
2201 87 0:110 / /data/users/riggspc/with\\040space rw,relatime - fuse edenfs: rw\n\
2300 87 0:111 / /mnt/nfs-eden rw,relatime - nfs edenfs: rw\n\
2400 87 0:112 / /data/users/riggspc/caf\\303\\251\\040repo rw,relatime - fuse edenfs: rw\n";

        assert!(mount_is_eden(
            mountinfo,
            Path::new("/data/users/riggspc/fbsource/fbcode/BUCK")
        ));
        assert!(mount_is_eden(
            mountinfo,
            Path::new("/data/users/riggspc/fbsource")
        ));
        assert!(!mount_is_eden(
            mountinfo,
            Path::new("/data/users/riggspc/fbsource/buck-out/v2/log")
        ));
        // A sibling directory with the mount point as a string prefix is not inside it.
        assert!(!mount_is_eden(
            mountinfo,
            Path::new("/data/users/riggspc/fbsource-other/x")
        ));
        assert!(!mount_is_eden(mountinfo, Path::new("/tmp/x")));
        assert!(mount_is_eden(
            mountinfo,
            Path::new("/data/users/riggspc/with space/file")
        ));
        assert!(mount_is_eden(mountinfo, Path::new("/mnt/nfs-eden/file")));
        // Non-ASCII bytes are escaped one byte at a time and must come back as UTF-8.
        assert!(mount_is_eden(
            mountinfo,
            Path::new("/data/users/riggspc/caf\u{e9} repo/BUCK")
        ));
        assert_eq!(
            unescape_mountinfo("caf\\303\\251\\040repo\\134x\\0"),
            std::ffi::OsString::from("caf\u{e9} repo\\x\\0")
        );
        assert!(!mount_is_eden("", Path::new("/anything")));
    }
}
