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
use std::ops::Deref;
use std::path::Path;
use std::path::PathBuf;

use allocative::Allocative;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::fmt::quoted_display;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathIter;
use buck2_fs::paths::relative_path::RelativePath;
use buck2_fs::paths::relative_path::RelativePathBuf;
use buck2_util::arc_str::StringInside;
use pagable::Pagable;
use pagable::PagableBoxDeserialize;
use pagable::PagableDeserialize;
use pagable::PagableDeserializer;
use pagable::PagableSerialize;
use ref_cast::RefCast;
use serde::Deserialize;
use serde::Serialize;

/// A 'PackageRelativePath' is a normalized, platform-agnostic path relative to
/// the base directory of the 'Package'.
#[derive(
    derive_more::Display,
    derivative::Derivative,
    Hash,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    RefCast,
    Allocative,
    strong_hash::StrongHash,
    PagableSerialize
)]
#[derivative(Debug)]
#[repr(transparent)]
pub struct PackageRelativePath(
    // Note we cast between `PackageRelativePath` and `ForwardRelativePath`.
    #[derivative(Debug(format_with = "quoted_display"))] ForwardRelativePath,
);

impl<'de> PagableBoxDeserialize<'de> for PackageRelativePath {
    fn deserialize_box<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Box<Self>> {
        let owned =
            <PackageRelativePathBuf as PagableDeserialize>::pagable_deserialize(deserializer)?;
        Ok(owned.into_box())
    }
}

/// The owned version of 'PackageRelativePath'
#[derive(
    Clone,
    derive_more::Display,
    derivative::Derivative,
    Hash,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Allocative,
    Serialize,
    Deserialize,
    Pagable
)]
#[derivative(Debug)]
pub struct PackageRelativePathBuf(
    #[derivative(Debug(format_with = "quoted_display"))] ForwardRelativePathBuf,
);

impl StringInside for PackageRelativePath {
    #[inline]
    fn as_str(wrapper: &PackageRelativePath) -> &str {
        wrapper.as_str()
    }

    #[inline]
    fn from_str(s: &str) -> &PackageRelativePath {
        PackageRelativePath::unchecked_new(s)
    }
}

impl AsRef<ForwardRelativePath> for PackageRelativePath {
    #[inline]
    fn as_ref(&self) -> &ForwardRelativePath {
        &self.0
    }
}

impl AsRef<RelativePath> for PackageRelativePath {
    #[inline]
    fn as_ref(&self) -> &RelativePath {
        self.0.as_ref()
    }
}

impl AsRef<ForwardRelativePath> for PackageRelativePathBuf {
    #[inline]
    fn as_ref(&self) -> &ForwardRelativePath {
        &self.0
    }
}

impl AsRef<RelativePath> for PackageRelativePathBuf {
    #[inline]
    fn as_ref(&self) -> &RelativePath {
        self.0.as_ref()
    }
}

impl AsRef<ForwardRelativePathBuf> for PackageRelativePathBuf {
    #[inline]
    fn as_ref(&self) -> &ForwardRelativePathBuf {
        &self.0
    }
}

impl AsRef<str> for PackageRelativePath {
    #[inline]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for PackageRelativePathBuf {
    #[inline]
    fn as_ref(&self) -> &str {
        self.0.as_str()
    }
}

impl Clone for Box<PackageRelativePath> {
    #[inline]
    fn clone(&self) -> Self {
        Self::from(&**self)
    }
}

impl PackageRelativePath {
    #[inline]
    pub fn unchecked_new<S: ?Sized + AsRef<str>>(s: &S) -> &Self {
        PackageRelativePath::ref_cast(ForwardRelativePath::unchecked_new(s))
    }

    #[inline]
    pub fn new_box(p: Box<ForwardRelativePath>) -> Box<PackageRelativePath> {
        // SAFETY: `PackageRelativePath` is a transparent wrapper around `ForwardRelativePath`.
        unsafe { Box::from_raw(Box::into_raw(p) as *mut PackageRelativePath) }
    }

    #[inline]
    pub fn empty() -> &'static PackageRelativePath {
        PackageRelativePath::unchecked_new("")
    }

    /// Creates an 'PackageRelativePath' if the given path represents a forward,
    /// normalized relative path, otherwise error.
    ///
    /// ```
    /// use std::path::Path;
    ///
    /// use buck2_core::package::package_relative_path::PackageRelativePath;
    ///
    /// assert!(PackageRelativePath::new("foo/bar").is_ok());
    /// assert!(PackageRelativePath::new("").is_ok());
    /// assert!(PackageRelativePath::new("/abs/bar").is_err());
    /// assert!(PackageRelativePath::new("normalize/./bar").is_err());
    /// assert!(PackageRelativePath::new("normalize/../bar").is_err());
    ///
    /// assert!(PackageRelativePath::new(Path::new("foo/bar")).is_ok());
    /// assert!(PackageRelativePath::new(Path::new("")).is_ok());
    /// assert!(PackageRelativePath::new(Path::new("/abs/bar")).is_err());
    /// assert!(PackageRelativePath::new(Path::new("normalize/./bar")).is_err());
    /// assert!(PackageRelativePath::new(Path::new("normalize/../bar")).is_err());
    /// ```
    #[inline]
    pub fn new<P: ?Sized + AsRef<Path>>(p: &P) -> buck2_error::Result<&PackageRelativePath> {
        Ok(PackageRelativePath::ref_cast(ForwardRelativePath::new(
            p.as_ref(),
        )?))
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    #[inline]
    pub fn as_forward_rel_path(&self) -> &ForwardRelativePath {
        &self.0
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Creates an owned 'PackageRelativePathBuf' with path adjoined to self.
    ///
    /// ```
    /// use std::path::Path;
    ///
    /// use buck2_core::package::package_relative_path::PackageRelativePath;
    /// use buck2_core::package::package_relative_path::PackageRelativePathBuf;
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    ///
    /// let path = PackageRelativePath::new("foo/bar")?;
    /// let other = ForwardRelativePath::new("baz")?;
    /// assert_eq!(
    ///     PackageRelativePathBuf::unchecked_new("foo/bar/baz".to_owned()),
    ///     path.join(other)
    /// );
    ///
    /// # buck2_error::Ok(())
    /// ```
    #[inline]
    pub fn join<P: AsRef<ForwardRelativePath>>(&self, path: P) -> PackageRelativePathBuf {
        PackageRelativePathBuf(self.0.join(path.as_ref()))
    }

    /// Returns a relative path of the parent directory
    ///
    /// ```
    /// use buck2_core::package::package_relative_path::PackageRelativePath;
    ///
    /// assert_eq!(
    ///     Some(PackageRelativePath::new("foo")?),
    ///     PackageRelativePath::new("foo/bar")?.parent()
    /// );
    ///
    /// # buck2_error::Ok(())
    /// ```
    #[inline]
    pub fn parent(&self) -> Option<&PackageRelativePath> {
        self.0.parent().map(PackageRelativePath::ref_cast)
    }

    /// Returns the final component of the `PackageRelativePath`, if there is
    /// one.
    ///
    /// If the path is a normal file, this is the file name. If it's the path of
    /// a directory, this is the directory name.
    ///
    /// ```
    /// use buck2_core::package::package_relative_path::PackageRelativePath;
    /// use buck2_fs::paths::file_name::FileName;
    ///
    /// assert_eq!(
    ///     Some(FileName::unchecked_new("bin")),
    ///     PackageRelativePath::new("usr/bin")?.file_name()
    /// );
    ///
    /// # buck2_error::Ok(())
    /// ```
    #[inline]
    pub fn file_name(&self) -> Option<&FileName> {
        self.0.file_name()
    }

    /// Returns a 'ForwardRelativePath' that, when joined onto `base`, yields
    /// `self`. Error if `base` is not a prefix of `self` or the returned
    /// path is not a 'ForwardRelativePath'
    ///
    /// ```
    /// use buck2_core::package::package_relative_path::PackageRelativePath;
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    ///
    /// let path = PackageRelativePath::new("test/haha/foo.txt")?;
    ///
    /// assert_eq!(
    ///     path.strip_prefix(PackageRelativePath::new("test")?)?,
    ///     ForwardRelativePath::new("haha/foo.txt")?
    /// );
    /// assert_eq!(
    ///     path.strip_prefix(PackageRelativePath::new("asdf")?)
    ///         .is_err(),
    ///     true
    /// );
    ///
    /// # buck2_error::Ok(())
    /// ```
    #[inline]
    pub fn strip_prefix<'a, P>(
        &'a self,
        base: &'a P,
    ) -> buck2_error::Result<&'a ForwardRelativePath>
    where
        P: ?Sized + AsRef<PackageRelativePath>,
    {
        self.0.strip_prefix(&base.as_ref().0)
    }

    /// Determines whether `base` is a prefix of `self`.
    ///
    /// ```
    /// use buck2_core::package::package_relative_path::PackageRelativePath;
    ///
    /// let path = PackageRelativePath::new("some/foo")?;
    ///
    /// assert!(path.starts_with(PackageRelativePath::new("some")?));
    ///
    /// # buck2_error::Ok(())
    /// ```
    #[inline]
    pub fn starts_with<P: AsRef<PackageRelativePath>>(&self, base: P) -> bool {
        self.0.starts_with(&base.as_ref().0)
    }

    /// Determines whether `child` is a suffix of `self`.
    /// Only considers whole path components to match.
    ///
    /// ```
    /// use std::path::Path;
    ///
    /// use buck2_core::package::package_relative_path::PackageRelativePath;
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    ///
    /// let path = PackageRelativePath::new("some/foo")?;
    ///
    /// assert!(path.ends_with(ForwardRelativePath::new("foo").unwrap()));
    ///
    /// # buck2_error::Ok(())
    /// ```
    #[inline]
    pub fn ends_with<P: AsRef<ForwardRelativePath>>(&self, child: P) -> bool {
        self.0.ends_with(child.as_ref())
    }

    /// Extracts the stem (non-extension) portion of `self.file_name`.
    ///
    /// The stem is:
    ///
    /// * [`None`], if there is no file name;
    /// * The entire file name if there is no embedded `.`;
    /// * The entire file name if the file name begins with `.` and has no other
    ///   `.`s within;
    /// * Otherwise, the portion of the file name before the final `.`
    ///
    /// ```
    /// use buck2_core::package::package_relative_path::PackageRelativePath;
    ///
    /// let path = PackageRelativePath::new("foo.rs")?;
    ///
    /// assert_eq!(Some("foo"), path.file_stem());
    ///
    /// # buck2_error::Ok(())
    /// ```
    #[inline]
    pub fn file_stem(&self) -> Option<&str> {
        self.0.file_stem()
    }

    /// Extracts the extension of `self.file_name`, if possible.
    ///
    /// ```
    /// use buck2_core::package::package_relative_path::PackageRelativePath;
    ///
    /// assert_eq!(
    ///     Some("rs"),
    ///     PackageRelativePath::new("hi/foo.rs")?.extension()
    /// );
    ///
    /// # buck2_error::Ok(())
    /// ```
    #[inline]
    pub fn extension(&self) -> Option<&str> {
        self.0.extension()
    }

    /// Iterator over the components of this path
    ///
    /// ```
    /// use buck2_core::package::package_relative_path::PackageRelativePath;
    /// use buck2_fs::paths::file_name::FileName;
    ///
    /// let p = PackageRelativePath::new("foo/bar/baz")?;
    /// let mut it = p.iter();
    ///
    /// assert_eq!(it.next(), Some(FileName::unchecked_new("foo")));
    /// assert_eq!(it.next(), Some(FileName::unchecked_new("bar")));
    /// assert_eq!(it.next(), Some(FileName::unchecked_new("baz")));
    /// assert_eq!(it.next(), None);
    ///
    /// # buck2_error::Ok(())
    /// ```
    #[inline]
    pub fn iter(&self) -> ForwardRelativePathIter<'_> {
        self.0.iter()
    }

    #[inline]
    pub fn to_buf(&self) -> PackageRelativePathBuf {
        self.to_owned()
    }

    #[inline]
    pub fn to_box(&self) -> Box<PackageRelativePath> {
        self.to_buf().into_box()
    }
}

impl<'a> From<&'a ForwardRelativePath> for &'a PackageRelativePath {
    ///
    /// ```
    /// use std::convert::From;
    ///
    /// use buck2_core::package::package_relative_path::PackageRelativePath;
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    ///
    /// let f = ForwardRelativePath::new("foo")?;
    ///
    /// assert_eq!(
    ///     <&PackageRelativePath>::from(f),
    ///     PackageRelativePath::new("foo")?
    /// );
    ///
    /// # buck2_error::Ok(())
    /// ```
    #[inline]
    fn from(p: &'a ForwardRelativePath) -> &'a PackageRelativePath {
        PackageRelativePath::ref_cast(p)
    }
}

impl PackageRelativePathBuf {
    #[inline]
    pub fn unchecked_new(s: String) -> Self {
        Self(ForwardRelativePathBuf::unchecked_new(s))
    }

    #[inline]
    pub fn as_path(&self) -> &PackageRelativePath {
        self
    }

    /// Creates a new 'PackageRelativePathBuf' with a given capacity used to create the internal
    /// 'String'. See 'with_capacity' defined on 'ForwardRelativePathBuf'
    #[inline]
    pub fn with_capacity(cap: usize) -> Self {
        Self(ForwardRelativePathBuf::with_capacity(cap))
    }

    /// Returns the capacity of the underlying 'ForwardRelativePathBuf'
    #[inline]
    pub fn capacity(&self) -> usize {
        self.0.capacity()
    }

    /// Invokes 'reserve' on the underlying 'ForwardRelativePathBuf'
    #[inline]
    pub fn reserve(&mut self, additional: usize) {
        self.0.reserve(additional)
    }

    /// Invokes 'shrink_to_fit' on the underlying 'ForwardRelativePathBuf'
    #[inline]
    pub fn shrink_to_fit(&mut self) {
        self.0.shrink_to_fit()
    }

    /// Invokes 'shrink_to' on the underlying 'String'
    #[inline]
    pub fn shrink_to(&mut self, min_capacity: usize) {
        self.0.shrink_to(min_capacity)
    }

    /// Pushes a `ForwardRelativePath` to the existing buffer
    #[inline]
    pub fn push<P: AsRef<ForwardRelativePath>>(&mut self, path: P) {
        self.0.push(path)
    }

    #[inline]
    pub fn into_box(self) -> Box<PackageRelativePath> {
        let s: Box<str> = self.0.into_string().into_boxed_str();
        PackageRelativePath::new_box(ForwardRelativePath::unchecked_new_box(s))
    }
}

impl<'a> From<&'a PackageRelativePath> for Box<PackageRelativePath> {
    #[inline]
    fn from(p: &'a PackageRelativePath) -> Box<PackageRelativePath> {
        let path: Box<str> = Box::from(p.as_str());
        PackageRelativePath::new_box(ForwardRelativePath::unchecked_new_box(path))
    }
}

impl From<ForwardRelativePathBuf> for PackageRelativePathBuf {
    #[inline]
    fn from(p: ForwardRelativePathBuf) -> Self {
        Self(p)
    }
}

impl From<PackageRelativePathBuf> for ForwardRelativePathBuf {
    #[inline]
    fn from(p: PackageRelativePathBuf) -> Self {
        p.0
    }
}

impl<'a> TryFrom<&'a str> for &'a PackageRelativePath {
    type Error = buck2_error::Error;

    /// no allocation conversion
    ///
    /// ```
    /// use std::convert::TryFrom;
    ///
    /// use buck2_core::package::package_relative_path::PackageRelativePath;
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    ///
    /// assert!(<&PackageRelativePath>::try_from("foo/bar").is_ok());
    /// assert!(<&PackageRelativePath>::try_from("").is_ok());
    /// assert!(<&PackageRelativePath>::try_from("/abs/bar").is_err());
    /// assert!(<&PackageRelativePath>::try_from("normalize/./bar").is_err());
    /// assert!(<&PackageRelativePath>::try_from("normalize/../bar").is_err());
    /// ```
    #[inline]
    fn try_from(s: &'a str) -> buck2_error::Result<&'a PackageRelativePath> {
        Ok(PackageRelativePath::ref_cast(ForwardRelativePath::new(s)?))
    }
}

impl<'a> TryFrom<&'a RelativePath> for &'a PackageRelativePath {
    type Error = buck2_error::Error;

    /// no allocation conversion
    ///
    /// ```
    /// use std::convert::TryFrom;
    ///
    /// use buck2_core::package::package_relative_path::PackageRelativePath;
    /// use buck2_fs::paths::RelativePath;
    ///
    /// assert!(<&PackageRelativePath>::try_from(RelativePath::unchecked_new("foo/bar")).is_ok());
    /// assert!(<&PackageRelativePath>::try_from(RelativePath::unchecked_new("")).is_ok());
    /// assert!(
    ///     <&PackageRelativePath>::try_from(RelativePath::unchecked_new("normalize/./bar")).is_err()
    /// );
    /// assert!(
    ///     <&PackageRelativePath>::try_from(RelativePath::unchecked_new("normalize/../bar")).is_err()
    /// );
    /// ```
    #[inline]
    fn try_from(s: &'a RelativePath) -> buck2_error::Result<&'a PackageRelativePath> {
        Ok(PackageRelativePath::ref_cast(ForwardRelativePath::new(
            s.as_str(),
        )?))
    }
}

impl TryFrom<String> for PackageRelativePathBuf {
    type Error = buck2_error::Error;

    /// no allocation conversion
    ///
    /// ```
    /// use std::convert::TryFrom;
    ///
    /// use buck2_core::package::package_relative_path::PackageRelativePath;
    /// use buck2_core::package::package_relative_path::PackageRelativePathBuf;
    ///
    /// assert!(PackageRelativePathBuf::try_from("foo/bar".to_owned()).is_ok());
    /// assert!(PackageRelativePathBuf::try_from("".to_owned()).is_ok());
    /// assert!(PackageRelativePathBuf::try_from("/abs/bar".to_owned()).is_err());
    /// assert!(PackageRelativePathBuf::try_from("normalize/./bar".to_owned()).is_err());
    /// assert!(PackageRelativePathBuf::try_from("normalize/../bar".to_owned()).is_err());
    /// ```
    #[inline]
    fn try_from(s: String) -> buck2_error::Result<PackageRelativePathBuf> {
        Ok(PackageRelativePathBuf::from(
            ForwardRelativePathBuf::try_from(s)?,
        ))
    }
}

impl TryFrom<RelativePathBuf> for PackageRelativePathBuf {
    type Error = buck2_error::Error;

    /// no allocation conversion (TODO make ForwardRelativePath a no allocation
    /// conversion)
    ///
    /// ```
    /// use std::convert::TryFrom;
    ///
    /// use buck2_core::package::package_relative_path::PackageRelativePath;
    /// use buck2_core::package::package_relative_path::PackageRelativePathBuf;
    /// use buck2_fs::paths::RelativePathBuf;
    ///
    /// assert!(PackageRelativePathBuf::try_from(RelativePathBuf::from("foo/bar")).is_ok());
    /// assert!(PackageRelativePathBuf::try_from(RelativePathBuf::from("")).is_ok());
    /// assert!(PackageRelativePathBuf::try_from(RelativePathBuf::from("normalize/./bar")).is_err());
    /// assert!(PackageRelativePathBuf::try_from(RelativePathBuf::from("normalize/../bar")).is_err());
    /// ```
    #[inline]
    fn try_from(p: RelativePathBuf) -> buck2_error::Result<PackageRelativePathBuf> {
        Ok(PackageRelativePathBuf::from(
            ForwardRelativePathBuf::try_from(p)?,
        ))
    }
}

impl TryFrom<PathBuf> for PackageRelativePathBuf {
    type Error = buck2_error::Error;

    /// no allocation conversion
    ///
    /// ```
    /// use std::convert::TryFrom;
    /// use std::path::PathBuf;
    ///
    /// use buck2_core::package::package_relative_path::PackageRelativePath;
    /// use buck2_core::package::package_relative_path::PackageRelativePathBuf;
    ///
    /// assert!(PackageRelativePathBuf::try_from(PathBuf::from("foo/bar")).is_ok());
    /// assert!(PackageRelativePathBuf::try_from(PathBuf::from("")).is_ok());
    /// assert!(PackageRelativePathBuf::try_from(PathBuf::from("/abs/bar")).is_err());
    /// assert!(PackageRelativePathBuf::try_from(PathBuf::from("normalize/./bar")).is_err());
    /// assert!(PackageRelativePathBuf::try_from(PathBuf::from("normalize/../bar")).is_err());
    /// ```
    #[inline]
    fn try_from(p: PathBuf) -> buck2_error::Result<PackageRelativePathBuf> {
        Ok(PackageRelativePathBuf::from(
            ForwardRelativePathBuf::try_from(p)?,
        ))
    }
}

impl ToOwned for PackageRelativePath {
    type Owned = PackageRelativePathBuf;

    #[inline]
    fn to_owned(&self) -> PackageRelativePathBuf {
        PackageRelativePathBuf(self.0.to_owned())
    }
}

impl AsRef<PackageRelativePath> for PackageRelativePath {
    #[inline]
    fn as_ref(&self) -> &PackageRelativePath {
        self
    }
}

impl AsRef<PackageRelativePath> for PackageRelativePathBuf {
    #[inline]
    fn as_ref(&self) -> &PackageRelativePath {
        PackageRelativePath::ref_cast(&self.0)
    }
}

impl Borrow<PackageRelativePath> for PackageRelativePathBuf {
    #[inline]
    fn borrow(&self) -> &PackageRelativePath {
        self.as_ref()
    }
}

impl Deref for PackageRelativePathBuf {
    type Target = PackageRelativePath;

    #[inline]
    fn deref(&self) -> &PackageRelativePath {
        PackageRelativePath::ref_cast(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use buck2_hash::BuckMutMap;

    use crate::package::package_relative_path::PackageRelativePath;
    use crate::package::package_relative_path::PackageRelativePathBuf;

    #[test]
    fn paths_work_in_maps() -> buck2_error::Result<()> {
        let mut map = BuckMutMap::default();

        let p1 = PackageRelativePath::new("foo")?;
        let p2 = PackageRelativePath::new("bar")?;

        map.insert(p1.to_buf(), p2.to_buf());

        assert_eq!(Some(p2), map.get(p1).map(|p| p.as_ref()));

        Ok(())
    }

    #[test]
    fn path_is_comparable() -> buck2_error::Result<()> {
        let path1_buf = PackageRelativePathBuf::unchecked_new("foo".into());
        let path2_buf = PackageRelativePathBuf::unchecked_new("foo".into());
        let path3_buf = PackageRelativePathBuf::unchecked_new("bar".into());

        let path1 = PackageRelativePath::new("foo")?;
        let path2 = PackageRelativePath::new("foo")?;
        let path3 = PackageRelativePath::new("bar")?;

        assert_eq!(path1_buf, path2_buf);
        assert_ne!(path1_buf, path3_buf);

        assert_eq!(path1, path2);
        assert_ne!(path1, path3);

        assert_eq!(path1_buf, path2);
        assert_ne!(path1, path3_buf);

        Ok(())
    }
}
