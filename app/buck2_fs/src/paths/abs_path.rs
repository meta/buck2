/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Borrow;
use std::ffi::OsString;
use std::fmt;
use std::ops::Deref;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;

use allocative::Allocative;
use derive_more::Display;
use pagable::Pagable;
use ref_cast::RefCast;

use crate::cwd;

#[derive(buck2_error::Error, Debug)]
#[buck2(input)]
enum AbsPathError {
    #[error("expected an absolute path but got a relative path instead: `{}`", _0.display())]
    PathNotAbsolute(PathBuf),
    #[error("Cannot convert path to UTF-8, `{0:?}`")]
    PathCannotBeConvertedToUtf8(OsString),
}

#[derive(Hash, Eq, PartialEq, PartialOrd, Ord, RefCast)]
#[repr(transparent)]
pub struct AbsPath(Path);

#[derive(
    Hash,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Clone,
    Allocative,
    Display,
    serde::Serialize,
    serde::Deserialize,
    Pagable
)]
#[display("{}", _0.display())]
pub struct AbsPathBuf(PathBuf);

impl fmt::Debug for AbsPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl fmt::Debug for AbsPathBuf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl AsRef<Path> for AbsPath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<AbsPath> for AbsPath {
    fn as_ref(&self) -> &AbsPath {
        self
    }
}

impl AsRef<Path> for AbsPathBuf {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<AbsPath> for AbsPathBuf {
    fn as_ref(&self) -> &AbsPath {
        self
    }
}

impl Deref for AbsPath {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Deref for AbsPathBuf {
    type Target = AbsPath;

    fn deref(&self) -> &Self::Target {
        unsafe { &*(self.0.as_path() as *const Path as *const AbsPath) }
    }
}

impl Borrow<AbsPath> for AbsPathBuf {
    fn borrow(&self) -> &AbsPath {
        self
    }
}

impl ToOwned for AbsPath {
    type Owned = AbsPathBuf;

    fn to_owned(&self) -> Self::Owned {
        AbsPathBuf(self.0.to_owned())
    }
}

impl PartialEq<AbsPath> for AbsPathBuf {
    fn eq(&self, other: &AbsPath) -> bool {
        self.0 == other.0
    }
}

impl PartialEq<&'_ AbsPath> for AbsPathBuf {
    fn eq(&self, other: &&AbsPath) -> bool {
        self.0 == other.0
    }
}

impl PartialEq<AbsPathBuf> for AbsPath {
    fn eq(&self, other: &AbsPathBuf) -> bool {
        self.0 == other.0
    }
}

impl PartialEq<AbsPathBuf> for &'_ AbsPath {
    fn eq(&self, other: &AbsPathBuf) -> bool {
        self.0 == other.0
    }
}

impl AbsPath {
    pub fn new<P: AsRef<Path> + ?Sized>(path: &P) -> buck2_error::Result<&AbsPath> {
        // Wrapper function to make sure the lifetimes are right
        fn inner(path: &Path) -> buck2_error::Result<&AbsPath> {
            if path.is_absolute() {
                // SAFETY: repr transparent.
                Ok(unsafe { &*(path as *const Path as *const AbsPath) })
            } else {
                Err(AbsPathError::PathNotAbsolute(path.to_path_buf()).into())
            }
        }
        inner(path.as_ref())
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    pub fn to_str(&self) -> buck2_error::Result<&str> {
        match self.0.to_str() {
            Some(s) => Ok(s),
            None => Err(AbsPathError::PathCannotBeConvertedToUtf8(self.0.to_owned().into()).into()),
        }
    }

    pub fn join<P: AsRef<Path>>(&self, other: P) -> AbsPathBuf {
        let path = self.0.join(other);
        AbsPathBuf::new(path).unwrap()
    }

    pub fn parent(&self) -> Option<&AbsPath> {
        self.0.parent().map(|p| AbsPath::new(p).unwrap())
    }

    /// ```
    /// use buck2_fs::paths::abs_path::AbsPath;
    ///
    /// if cfg!(not(windows)) {
    ///     assert_eq!(
    ///         AbsPath::new("/foo").unwrap().with_extension("rs"),
    ///         AbsPath::new("/foo.rs").unwrap()
    ///     );
    ///     assert_eq!(
    ///         AbsPath::new("/foo.tar.gz").unwrap().with_extension("xz"),
    ///         AbsPath::new("/foo.tar.xz").unwrap()
    ///     );
    ///     assert_eq!(
    ///         AbsPath::new("/foo.tar.gz")
    ///             .unwrap()
    ///             .with_extension("")
    ///             .with_extension("txt"),
    ///         AbsPath::new("/foo.txt").unwrap()
    ///     );
    ///     assert_eq!(
    ///         AbsPath::new("/foo").unwrap().with_extension("rs"),
    ///         AbsPath::new("/foo.rs").unwrap()
    ///     );
    /// }
    /// ```
    pub fn with_extension<P: AsRef<str>>(&self, extension: P) -> AbsPathBuf {
        let path = self.0.with_extension(extension.as_ref());
        AbsPathBuf::new(path).unwrap()
    }

    /// ```
    /// use buck2_fs::paths::abs_path::AbsPath;
    ///
    /// if cfg!(not(windows)) {
    ///     let path = AbsPath::new("/foo.rs").unwrap();
    ///     assert_eq!(
    ///         path.with_added_extension("txt"),
    ///         AbsPath::new("/foo.rs.txt").unwrap()
    ///     );
    ///
    ///     let path = AbsPath::new("/foo.tar.gz").unwrap();
    ///     assert_eq!(
    ///         path.with_added_extension(""),
    ///         AbsPath::new("/foo.tar.gz").unwrap()
    ///     );
    ///     assert_eq!(
    ///         path.with_added_extension("xz"),
    ///         AbsPath::new("/foo.tar.gz.xz").unwrap()
    ///     );
    ///     assert_eq!(
    ///         path.with_added_extension("").with_added_extension("txt"),
    ///         AbsPath::new("/foo.tar.gz.txt").unwrap()
    ///     );
    /// }
    /// ```
    pub fn with_added_extension<P: AsRef<str>>(&self, extension: P) -> AbsPathBuf {
        let path = self.0.with_added_extension(extension.as_ref());
        AbsPathBuf::new(path).unwrap()
    }

    pub fn strip_prefix<P: AsRef<AbsPath>>(&self, prefix: P) -> buck2_error::Result<&Path> {
        Ok(self.0.strip_prefix(prefix.as_ref())?)
    }

    pub fn ancestors(&self) -> impl Iterator<Item = &'_ AbsPath> {
        // Taking the ancestors of an AbsPath gives you more AbsPath.
        self.0.ancestors().map(AbsPath::ref_cast)
    }

    pub fn as_maybe_relativized(&self) -> &Path {
        cwd::maybe_relativize(&self.0)
    }

    pub fn as_maybe_relativized_str(&self) -> buck2_error::Result<&str> {
        Ok(cwd::maybe_relativize_str(self.to_str()?))
    }
}

impl AbsPathBuf {
    pub fn new<P: AsRef<Path>>(path: P) -> buck2_error::Result<Self> {
        let p = AbsPath::new(path.as_ref())?;
        Ok(p.to_owned())
    }

    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }

    pub fn into_os_string(self) -> OsString {
        self.0.into_os_string()
    }

    /// Convert a path into a String. Fails if the path is not UTF8.
    pub fn into_string(self) -> buck2_error::Result<String> {
        self.into_os_string()
            .into_string()
            .map_err(|x| AbsPathError::PathCannotBeConvertedToUtf8(x).into())
    }

    pub fn capacity(&self) -> usize {
        self.0.capacity()
    }

    pub fn reserve(&mut self, additional: usize) {
        self.0.reserve(additional)
    }

    pub fn shrink_to_fit(&mut self) {
        self.0.shrink_to_fit()
    }

    pub fn shrink_to(&mut self, min_capacity: usize) {
        self.0.shrink_to(min_capacity)
    }

    pub fn push<P: AsRef<Path>>(&mut self, path: P) {
        self.0.push(path);
        assert!(self.0.is_absolute());
    }

    pub fn pop(&mut self) -> bool {
        let r = self.0.pop();
        assert!(self.0.is_absolute());
        r
    }

    /// ```
    /// use buck2_fs::paths::abs_path::AbsPath;
    /// use buck2_fs::paths::abs_path::AbsPathBuf;
    ///
    /// if cfg!(not(windows)) {
    ///     let mut path = AbsPathBuf::new("/foo").unwrap();
    ///     path.set_extension("rs");
    ///     assert_eq!(path, AbsPath::new("/foo.rs").unwrap());
    ///
    ///     let mut path = AbsPathBuf::new("/foo.tar.gz").unwrap();
    ///     path.set_extension("xz");
    ///     assert_eq!(path, AbsPath::new("/foo.tar.xz").unwrap());
    ///
    ///     let mut path = AbsPathBuf::new("/foo.tar.gz").unwrap();
    ///     path.set_extension("");
    ///     path.set_extension("txt");
    ///     assert_eq!(path, AbsPath::new("/foo.txt").unwrap());
    /// }
    /// ```
    pub fn set_extension<S: AsRef<str>>(&mut self, extension: S) {
        self.0.set_extension(extension.as_ref());
        assert!(self.0.is_absolute());
    }

    /// ```
    /// use buck2_fs::paths::abs_path::AbsPath;
    /// use buck2_fs::paths::abs_path::AbsPathBuf;
    ///
    /// if cfg!(not(windows)) {
    ///     let mut path = AbsPathBuf::new("/foo.rs").unwrap();
    ///     path.add_extension("");
    ///     assert_eq!(path, AbsPath::new("/foo.rs").unwrap());
    ///
    ///     let mut path = AbsPathBuf::new("/foo.rs").unwrap();
    ///     path.add_extension("txt");
    ///     assert_eq!(path, AbsPath::new("/foo.rs.txt").unwrap());
    /// }
    /// ```
    pub fn add_extension<S: AsRef<str>>(&mut self, extension: S) {
        self.0.add_extension(extension.as_ref());
        assert!(self.0.is_absolute());
    }
}

impl TryFrom<PathBuf> for AbsPathBuf {
    type Error = buck2_error::Error;

    fn try_from(path: PathBuf) -> Result<Self, Self::Error> {
        AbsPath::new(&path)?;
        Ok(AbsPathBuf(path))
    }
}

impl TryFrom<String> for AbsPathBuf {
    type Error = buck2_error::Error;

    fn try_from(path: String) -> Result<Self, Self::Error> {
        AbsPathBuf::try_from(PathBuf::from(path))
    }
}

impl FromStr for AbsPathBuf {
    type Err = buck2_error::Error;

    fn from_str(s: &str) -> buck2_error::Result<AbsPathBuf> {
        AbsPathBuf::try_from(s.to_owned())
    }
}
