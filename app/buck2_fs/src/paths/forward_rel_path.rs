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
use std::borrow::Cow;
use std::hash::Hash;
use std::ops::Deref;
use std::path::Path;
use std::path::PathBuf;

use allocative::Allocative;
use buck2_util::arc_str::StringInside;
use derive_more::Display;
use pagable::Pagable;
use pagable::PagableBoxDeserialize;
use pagable::PagableDeserialize;
use pagable::PagableDeserializer;
use pagable::PagableSerialize;
use ref_cast::RefCastCustom;
use ref_cast::ref_cast_custom;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use smallvec::SmallVec;
use strong_hash::StrongHash;

use crate::paths::file_name::FileName;
use crate::paths::path_util::path_remove_prefix;
use crate::paths::relative_path::RelativePath;
use crate::paths::relative_path::RelativePathBuf;
use crate::paths::relative_path::verify_relative_path;

/// A forward pointing, fully normalized relative path and owned pathbuf.
/// This means that there is no '.' or '..' in this path, and does not begin
/// with '/'.
///
/// This path is platform agnostic, so path separators are always '/'.
#[derive(
    Display,
    Debug,
    Serialize,
    RefCastCustom,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Allocative,
    StrongHash,
    PagableSerialize
)]
#[repr(transparent)]
pub struct ForwardRelativePath(
    // Note we cast between `ForwardRelativePath` and `str`.
    str,
);

/// The owned version of 'ForwardRelativePath', like how 'PathBuf' relates to
/// 'Path'
#[derive(
    Default, Clone, Display, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash, Allocative,
    StrongHash, Pagable
)]
#[repr(transparent)]
pub struct ForwardRelativePathBuf(#[pagable(flatten_serde)] String);

impl<'de> Deserialize<'de> for ForwardRelativePathBuf {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        ForwardRelativePathBuf::try_from(s).map_err(serde::de::Error::custom)
    }
}

impl StringInside for ForwardRelativePath {
    #[inline]
    fn as_str(wrapper: &Self) -> &str {
        wrapper.as_str()
    }

    #[inline]
    fn from_str(s: &str) -> &Self {
        ForwardRelativePath::unchecked_new(s)
    }
}

impl AsRef<RelativePath> for ForwardRelativePath {
    #[inline]
    fn as_ref(&self) -> &RelativePath {
        RelativePath::unchecked_new(&self.0)
    }
}

impl AsRef<RelativePath> for ForwardRelativePathBuf {
    #[inline]
    fn as_ref(&self) -> &RelativePath {
        RelativePath::unchecked_new(&self.0)
    }
}

impl AsRef<str> for ForwardRelativePath {
    #[inline]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for ForwardRelativePathBuf {
    #[inline]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

pub struct ForwardRelativePathIter<'a>(&'a ForwardRelativePath);

impl<'a> ForwardRelativePathIter<'a> {
    /// Remaining path in the iterator.
    pub fn as_path(&self) -> &'a ForwardRelativePath {
        self.0
    }
}

impl<'a> Iterator for ForwardRelativePathIter<'a> {
    type Item = &'a FileName;

    #[inline]
    fn next(&mut self) -> Option<&'a FileName> {
        let (first, rem) = self.0.split_first()?;
        self.0 = rem;
        Some(first)
    }
}

impl<'a> DoubleEndedIterator for ForwardRelativePathIter<'a> {
    #[inline]
    fn next_back(&mut self) -> Option<&'a FileName> {
        let (rem, last) = self.0.split_last()?;
        self.0 = rem;
        Some(last)
    }
}

impl Clone for ForwardRelativePathIter<'_> {
    fn clone(&self) -> Self {
        ForwardRelativePathIter(ForwardRelativePath::unchecked_new(self.0.as_str()))
    }
}

impl ForwardRelativePath {
    #[ref_cast_custom]
    const fn ref_cast(s: &str) -> &ForwardRelativePath;

    #[inline]
    pub fn unchecked_new<S: ?Sized + AsRef<str>>(s: &S) -> &Self {
        ForwardRelativePath::ref_cast(s.as_ref())
    }

    #[inline]
    pub fn unchecked_new_box(s: Box<str>) -> Box<ForwardRelativePath> {
        // SAFETY: `ForwardRelativePath` is a transparent wrapper around `str`.
        unsafe { Box::from_raw(Box::into_raw(s) as *mut ForwardRelativePath) }
    }

    #[inline]
    pub const fn empty() -> &'static Self {
        ForwardRelativePath::ref_cast("")
    }

    /// Creates an 'ForwardRelativePath' if the given path represents a forward,
    /// normalized relative path, otherwise error.
    ///
    /// ```
    /// use std::path::Path;
    ///
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    ///
    /// assert!(ForwardRelativePath::new("foo/bar").is_ok());
    /// assert!(ForwardRelativePath::new("").is_ok());
    /// assert!(ForwardRelativePath::new("./bar").is_err());
    /// assert!(ForwardRelativePath::new("normalize/./bar").is_err());
    /// assert!(ForwardRelativePath::new("/abs/bar").is_err());
    /// assert!(ForwardRelativePath::new("foo//bar").is_err());
    /// assert!(ForwardRelativePath::new("normalize/../bar").is_err());
    ///
    /// assert!(ForwardRelativePath::new(Path::new("foo/bar")).is_ok());
    /// assert!(ForwardRelativePath::new(Path::new("")).is_ok());
    /// assert!(ForwardRelativePath::new(Path::new("./bar")).is_err());
    /// assert!(ForwardRelativePath::new(Path::new("normalize/./bar")).is_err());
    /// assert!(ForwardRelativePath::new(Path::new("/abs/bar")).is_err());
    /// assert!(ForwardRelativePath::new(Path::new("normalize/../bar")).is_err());
    /// assert!(ForwardRelativePath::new(Path::new("normalize\\bar")).is_err());
    /// assert!(ForwardRelativePath::new(Path::new("normalize/bar/")).is_err());
    ///
    /// # buck2_error::Ok(())
    /// ```
    // TODO(nga): this accepts `Path`, but should accept `str`
    //   because paths can have backslashes.
    //   Conversion of `Path` to `ForwardRelativePath` should be done via
    //   `ForwardRelativePathBuf` which should normalize slashes.
    #[inline]
    pub fn new<S: ?Sized + AsRef<Path>>(s: &S) -> buck2_error::Result<&ForwardRelativePath> {
        <&ForwardRelativePath>::try_from(s.as_ref())
    }

    /// `ForwardRelativePath` requires no trailing slashes. This function constructs a path
    /// ignoring trailing slashes.
    ///
    /// ```
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    ///
    /// assert!(ForwardRelativePath::new_trim_trailing_slashes("foo/bar").is_ok());
    /// assert!(ForwardRelativePath::new_trim_trailing_slashes("foo/bar/").is_ok());
    /// assert!(ForwardRelativePath::new_trim_trailing_slashes("foo/bar//").is_ok());
    /// assert!(ForwardRelativePath::new_trim_trailing_slashes("foo//bar").is_err());
    /// ```
    pub fn new_trim_trailing_slashes<S: ?Sized + AsRef<Path>>(
        path: &S,
    ) -> buck2_error::Result<&ForwardRelativePath> {
        let path = path.as_ref();
        let path = path
            .to_str()
            .ok_or_else(|| ForwardRelativePathError::PathNotUtf8(path.display().to_string()))?;
        let path = path.trim_end_matches('/');
        ForwardRelativePath::new(path)
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[inline]
    pub fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Creates an owned 'ForwardRelativePathBuf' with path adjoined to self.
    ///
    /// ```
    /// use std::path::Path;
    ///
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
    ///
    /// let path = ForwardRelativePath::new("foo/bar")?;
    /// let other = ForwardRelativePath::new("baz")?;
    /// assert_eq!(
    ///     ForwardRelativePathBuf::unchecked_new("foo/bar/baz".to_owned()),
    ///     path.join(other)
    /// );
    ///
    /// # buck2_error::Ok(())
    /// ```
    #[inline]
    pub fn join<P: AsRef<ForwardRelativePath>>(&self, path: P) -> ForwardRelativePathBuf {
        let path = path.as_ref();
        if self.0.is_empty() {
            path.to_buf()
        } else if path.0.is_empty() {
            self.to_buf()
        } else {
            let mut buf = String::with_capacity(self.0.len() + 1 + path.0.len());
            buf.push_str(&self.0);
            buf.push('/');
            buf.push_str(&path.0);
            ForwardRelativePathBuf::unchecked_new(buf)
        }
    }

    pub fn join_cow<'a>(&'a self, path: &'a ForwardRelativePath) -> Cow<'a, ForwardRelativePath> {
        if self.is_empty() {
            Cow::Borrowed(path)
        } else if path.is_empty() {
            Cow::Borrowed(self)
        } else {
            Cow::Owned(self.join(path))
        }
    }

    /// Returns a relative path of the parent directory
    ///
    /// ```
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    ///
    /// assert_eq!(
    ///     Some(ForwardRelativePath::new("foo")?),
    ///     ForwardRelativePath::new("foo/bar")?.parent()
    /// );
    /// assert_eq!(
    ///     Some(ForwardRelativePath::new("")?),
    ///     ForwardRelativePath::new("foo")?.parent()
    /// );
    /// assert_eq!(None, ForwardRelativePath::new("")?.parent());
    ///
    /// # buck2_error::Ok(())
    /// ```
    pub fn parent(&self) -> Option<&ForwardRelativePath> {
        let s = &self.0;
        for i in (0..s.len()).rev() {
            if s.as_bytes()[i] == b'/' {
                return Some(ForwardRelativePath::unchecked_new(&s[..i]));
            }
        }
        if s.is_empty() {
            None
        } else {
            Some(ForwardRelativePath::empty())
        }
    }

    /// Returns the final component of the `ForwardRelativePath`, if there is
    /// one.
    ///
    /// If the path is a normal file, this is the file name. If it's the path of
    /// a directory, this is the directory name.
    ///
    /// ```
    /// use buck2_fs::paths::file_name::FileName;
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    ///
    /// assert_eq!(
    ///     Some(FileName::unchecked_new("ls")),
    ///     ForwardRelativePath::new("usr/bin/ls")?.file_name()
    /// );
    /// assert_eq!(
    ///     Some(FileName::unchecked_new("bin")),
    ///     ForwardRelativePath::new("usr/bin")?.file_name()
    /// );
    /// assert_eq!(
    ///     Some(FileName::unchecked_new("usr")),
    ///     ForwardRelativePath::new("usr")?.file_name()
    /// );
    /// assert_eq!(None, ForwardRelativePath::new("")?.file_name());
    ///
    /// # buck2_error::Ok(())
    /// ```
    pub fn file_name(&self) -> Option<&FileName> {
        let s = &self.0;
        for (i, b) in s.bytes().enumerate().rev() {
            if b == b'/' {
                return Some(FileName::unchecked_new(&s[i + 1..]));
            }
        }
        if s.is_empty() {
            None
        } else {
            Some(FileName::unchecked_new(s))
        }
    }

    /// Get the first component of the path and the remaining path,
    /// of `None` if the path is empty.
    pub fn split_first(&self) -> Option<(&FileName, &ForwardRelativePath)> {
        let s = &self.0;
        for (i, b) in s.bytes().enumerate() {
            if b == b'/' {
                return Some((
                    FileName::unchecked_new(&s[..i]),
                    ForwardRelativePath::unchecked_new(&s[i + 1..]),
                ));
            }
        }
        if s.is_empty() {
            None
        } else {
            Some((FileName::unchecked_new(s), ForwardRelativePath::empty()))
        }
    }

    /// Split off the last component of the path.
    pub fn split_last(&self) -> Option<(&ForwardRelativePath, &FileName)> {
        let s = &self.0;
        for (i, b) in s.bytes().enumerate().rev() {
            if b == b'/' {
                return Some((
                    ForwardRelativePath::unchecked_new(&s[..i]),
                    FileName::unchecked_new(&s[i + 1..]),
                ));
            }
        }
        if s.is_empty() {
            None
        } else {
            Some((ForwardRelativePath::empty(), FileName::unchecked_new(s)))
        }
    }

    /// Returns a 'ForwardRelativePath' that, when joined onto `base`, yields
    /// `self`.
    ///
    /// Error if `base` is not a prefix of `self` or the returned
    /// path is not a 'ForwardRelativePath'
    ///
    /// ```
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    ///
    /// let path = ForwardRelativePath::new("test/haha/foo.txt")?;
    ///
    /// assert_eq!(
    ///     path.strip_prefix(ForwardRelativePath::new("test/haha/foo.txt")?)?,
    ///     ForwardRelativePath::new("")?
    /// );
    /// assert_eq!(
    ///     path.strip_prefix(ForwardRelativePath::new("test/haha")?)?,
    ///     ForwardRelativePath::new("foo.txt")?
    /// );
    /// assert_eq!(
    ///     path.strip_prefix(ForwardRelativePath::new("test")?)?,
    ///     ForwardRelativePath::new("haha/foo.txt")?
    /// );
    /// assert_eq!(
    ///     path.strip_prefix(ForwardRelativePath::new("")?)?,
    ///     ForwardRelativePath::new("test/haha/foo.txt")?
    /// );
    /// assert_eq!(
    ///     path.strip_prefix(ForwardRelativePath::new("asdf")?)
    ///         .is_err(),
    ///     true
    /// );
    ///
    /// # buck2_error::Ok(())
    /// ```
    pub fn strip_prefix<P: AsRef<ForwardRelativePath>>(
        &self,
        prefix: P,
    ) -> buck2_error::Result<&ForwardRelativePath> {
        let prefix = prefix.as_ref();
        self.strip_prefix_opt(prefix).ok_or_else(|| {
            ForwardRelativePathError::StripPrefix(
                self.as_str().to_owned(),
                prefix.as_str().to_owned(),
            )
            .into()
        })
    }

    pub fn strip_prefix_opt<P: AsRef<ForwardRelativePath>>(
        &self,
        prefix: P,
    ) -> Option<&ForwardRelativePath> {
        let prefix = prefix.as_ref();
        if prefix.0.is_empty() {
            Some(self)
        } else if self.starts_with(prefix) {
            if self.0.len() == prefix.0.len() {
                Some(ForwardRelativePath::empty())
            } else {
                Some(ForwardRelativePath::unchecked_new(
                    &self.0[prefix.0.len() + 1..],
                ))
            }
        } else {
            None
        }
    }

    pub fn strip_suffix<P: AsRef<ForwardRelativePath>>(
        &self,
        suffix: P,
    ) -> buck2_error::Result<&ForwardRelativePath> {
        let suffix = suffix.as_ref();
        self.strip_suffix_opt(suffix).ok_or_else(|| {
            ForwardRelativePathError::StripSuffix(
                self.as_str().to_owned(),
                suffix.as_str().to_owned(),
            )
            .into()
        })
    }

    pub fn strip_suffix_opt<P: AsRef<ForwardRelativePath>>(
        &self,
        suffix: P,
    ) -> Option<&ForwardRelativePath> {
        let suffix = suffix.as_ref();
        if suffix.0.is_empty() {
            Some(self)
        } else if self.ends_with(suffix) {
            if self.0.len() == suffix.0.len() {
                Some(ForwardRelativePath::empty())
            } else {
                Some(ForwardRelativePath::unchecked_new(
                    &self.0[..self.0.len() - suffix.0.len() - 1],
                ))
            }
        } else {
            None
        }
    }

    /// Determines whether `base` is a prefix of `self`.
    ///
    /// ```
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    ///
    /// let path = ForwardRelativePath::new("some/foo")?;
    ///
    /// assert!(path.starts_with(ForwardRelativePath::new("some/foo")?));
    /// assert!(path.starts_with(ForwardRelativePath::new("some")?));
    /// assert!(!path.starts_with(ForwardRelativePath::new("som")?));
    /// assert!(path.starts_with(ForwardRelativePath::new("")?));
    ///
    /// # buck2_error::Ok(())
    /// ```
    pub fn starts_with<P: AsRef<ForwardRelativePath>>(&self, base: P) -> bool {
        let path = self.as_str();
        let prefix = base.as_ref().as_str();
        path_remove_prefix(path, prefix).is_some()
    }

    /// Determines whether `child` is a suffix of `self`.
    /// Only considers whole path components to match.
    ///
    /// ```
    /// use std::path::Path;
    ///
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    ///
    /// let path = ForwardRelativePath::new("some/foo")?;
    ///
    /// assert!(path.ends_with(ForwardRelativePath::new("some/foo")?));
    /// assert!(path.ends_with(ForwardRelativePath::new("foo")?));
    /// assert!(!path.ends_with(ForwardRelativePath::new("oo")?));
    /// assert!(path.ends_with(ForwardRelativePath::new("")?));
    ///
    /// # buck2_error::Ok(())
    /// ```
    pub fn ends_with<P: AsRef<ForwardRelativePath>>(&self, child: P) -> bool {
        let child = child.as_ref();
        child.0.is_empty()
            || (self.0.ends_with(&child.0)
                && (self.0.len() == child.0.len()
                    || self.0.as_bytes()[self.0.len() - child.0.len() - 1] == b'/'))
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
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    ///
    /// let path = ForwardRelativePath::new("foo.rs")?;
    ///
    /// assert_eq!(Some("foo"), path.file_stem());
    /// assert_eq!(
    ///     Some("foo.bar"),
    ///     ForwardRelativePath::new("hi/foo.bar.rs")?.file_stem()
    /// );
    ///
    /// # buck2_error::Ok(())
    /// ```
    pub fn file_stem(&self) -> Option<&str> {
        let file = self.file_name();
        file.map(|f| {
            let f = f.as_str();
            for (i, b) in f.bytes().enumerate().rev() {
                if b == b'.' && i > 0 {
                    return &f[0..i];
                }
            }

            f
        })
    }

    /// Extracts the extension of `self.file_name`, if possible.
    ///
    /// ```
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    ///
    /// assert_eq!(
    ///     Some("rs"),
    ///     ForwardRelativePath::new("hi/foo.rs")?.extension()
    /// );
    /// assert_eq!(
    ///     Some("rs"),
    ///     ForwardRelativePath::new("hi/foo.bar.rs")?.extension()
    /// );
    /// assert_eq!(None, ForwardRelativePath::new(".git")?.extension());
    /// assert_eq!(None, ForwardRelativePath::new("foo/.git")?.extension());
    /// assert_eq!(None, ForwardRelativePath::new("")?.extension());
    ///
    /// # buck2_error::Ok(())
    /// ```
    pub fn extension(&self) -> Option<&str> {
        let s = &self.0;
        let bytes = s.as_bytes();
        let mut i = s.len().checked_sub(1)?;
        while i > 0 {
            let b = bytes[i];
            if b == b'/' {
                return None;
            }
            if b == b'.' {
                if bytes[i - 1] == b'/' {
                    return None;
                }
                return Some(&s[i + 1..]);
            }

            i -= 1;
        }
        None
    }

    /// Build an owned `ForwardRelativePathBuf`, joined with the given path and
    /// normalized.
    ///
    /// ```
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
    ///
    /// assert_eq!(
    ///     ForwardRelativePathBuf::unchecked_new("foo/baz.txt".into()),
    ///     ForwardRelativePath::new("foo/bar")?.join_normalized("../baz.txt")?,
    /// );
    ///
    /// assert_eq!(
    ///     ForwardRelativePath::new("foo")?
    ///         .join_normalized("../../baz.txt")
    ///         .is_err(),
    ///     true
    /// );
    ///
    /// # buck2_error::Ok(())
    /// ```
    pub fn join_normalized<P: AsRef<RelativePath>>(
        &self,
        path: P,
    ) -> buck2_error::Result<ForwardRelativePathBuf> {
        let self_rel_path: &RelativePath = self.as_ref();
        let inner = self_rel_path.join_normalized(path.as_ref());
        ForwardRelativePathBuf::try_from(inner)
    }

    /// Append a relative system path, obtained from e.g. `read_link`.
    ///
    /// The path will be converted to an internal path (i.e. forward slashes) before joining.
    pub fn join_system_normalized(
        &self,
        path: &Path,
    ) -> buck2_error::Result<ForwardRelativePathBuf> {
        let path = RelativePathBuf::from_system_path(path)?;
        self.join_normalized(path)
    }

    /// Iterator over the components of this path
    ///
    /// ```
    /// use buck2_fs::paths::file_name::FileName;
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    ///
    /// let p = ForwardRelativePath::new("foo/bar/baz")?;
    /// let mut it = p.iter();
    ///
    /// assert_eq!(it.next(), Some(FileName::unchecked_new("foo")));
    /// assert_eq!(it.next(), Some(FileName::unchecked_new("bar")));
    /// assert_eq!(it.next(), Some(FileName::unchecked_new("baz")));
    /// assert_eq!(it.next(), None);
    /// assert_eq!(it.next(), None);
    ///
    /// # buck2_error::Ok(())
    /// ```
    #[inline]
    pub fn iter(&self) -> ForwardRelativePathIter<'_> {
        ForwardRelativePathIter(self)
    }

    /// Strip a given number of components from the prefix of a path,
    /// returning the remaining path or `None` if there were none left.
    ///
    /// ```
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    ///
    /// let p = ForwardRelativePath::new("foo/bar/baz")?;
    /// assert_eq!(
    ///     p.strip_prefix_components(0),
    ///     Some(ForwardRelativePath::new("foo/bar/baz")?),
    /// );
    /// assert_eq!(
    ///     p.strip_prefix_components(1),
    ///     Some(ForwardRelativePath::new("bar/baz")?),
    /// );
    /// assert_eq!(
    ///     p.strip_prefix_components(2),
    ///     Some(ForwardRelativePath::new("baz")?),
    /// );
    /// assert_eq!(
    ///     p.strip_prefix_components(3),
    ///     Some(ForwardRelativePath::new("")?),
    /// );
    /// assert_eq!(p.strip_prefix_components(4), None,);
    /// # buck2_error::Ok(())
    /// ```
    pub fn strip_prefix_components(&self, components: usize) -> Option<&Self> {
        let mut rem = self;
        for _ in 0..components {
            rem = rem.split_first()?.1;
        }
        Some(rem)
    }

    #[inline]
    pub fn to_buf(&self) -> ForwardRelativePathBuf {
        self.to_owned()
    }

    pub fn to_box(&self) -> Box<ForwardRelativePath> {
        self.to_buf().into_box()
    }

    /// Return a RelativePath representation of this ForwardRelativePath.
    #[inline]
    pub fn as_relative_path(&self) -> &RelativePath {
        RelativePath::unchecked_new(&self.0)
    }
}

impl ForwardRelativePathBuf {
    #[inline]
    pub fn new(s: String) -> buck2_error::Result<ForwardRelativePathBuf> {
        ForwardRelativePath::new(&s)?;
        Ok(ForwardRelativePathBuf(s))
    }

    #[inline]
    pub fn empty() -> Self {
        Self("".to_owned())
    }

    #[inline]
    pub fn unchecked_new(s: String) -> Self {
        Self(s)
    }

    /// Creates a new 'ForwardRelativePathBuf' with a given capacity used to create the internal
    /// 'String'. See 'with_capacity' defined on 'String'
    #[inline]
    pub fn with_capacity(cap: usize) -> Self {
        Self(String::with_capacity(cap))
    }

    pub fn with_capacity_for_concat(
        items: impl IntoIterator<Item = impl AsRef<ForwardRelativePath>>,
    ) -> Self {
        let mut cap = 0;
        for item in items {
            let item = item.as_ref();
            if !item.is_empty() {
                if cap != 0 {
                    // `/`.
                    cap += 1;
                }
                cap += item.0.len();
            }
        }
        ForwardRelativePathBuf::with_capacity(cap)
    }

    /// Returns the capacity of the underlying 'String'
    #[inline]
    pub fn capacity(&self) -> usize {
        self.0.capacity()
    }

    /// Invokes 'reserve' on the underlying 'String'
    #[inline]
    pub fn reserve(&mut self, additional: usize) {
        self.0.reserve(additional)
    }

    /// Invokes 'shrink_to_fit' on the underlying 'String'
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
    ///
    /// ```
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
    ///
    /// let mut path = ForwardRelativePathBuf::unchecked_new("foo".to_owned());
    /// path.push(ForwardRelativePath::unchecked_new("bar"));
    ///
    /// assert_eq!(
    ///     ForwardRelativePathBuf::unchecked_new("foo/bar".to_owned()),
    ///     path
    /// );
    ///
    /// path.push(ForwardRelativePath::unchecked_new("more/file.rs"));
    /// assert_eq!(
    ///     ForwardRelativePathBuf::unchecked_new("foo/bar/more/file.rs".to_owned()),
    ///     path
    /// );
    ///
    /// path.push(ForwardRelativePath::empty());
    /// assert_eq!(
    ///     ForwardRelativePathBuf::unchecked_new("foo/bar/more/file.rs".to_owned()),
    ///     path
    /// );
    ///
    /// let mut path = ForwardRelativePathBuf::unchecked_new("".to_owned());
    /// path.push(ForwardRelativePath::unchecked_new("foo"));
    /// assert_eq!(
    ///     ForwardRelativePathBuf::unchecked_new("foo".to_owned()),
    ///     path
    /// );
    ///
    /// # buck2_error::Ok(())
    /// ```
    pub fn push<P: AsRef<ForwardRelativePath>>(&mut self, path: P) {
        let path = path.as_ref();
        if path.is_empty() {
            return;
        }

        if self.is_empty() {
            self.0.push_str(path.as_str());
        } else {
            self.reserve(1 + path.0.len());
            self.0.push('/');
            self.0.push_str(path.as_str());
        }
    }

    /// Pop the last component of the path, if there is one.
    pub fn pop(&mut self) -> bool {
        let Some((me, _pop)) = self.split_last() else {
            return false;
        };
        self.0.truncate(me.0.len());
        true
    }

    pub fn concat<'a, I: IntoIterator<Item = &'a ForwardRelativePath> + Copy>(
        items: I,
    ) -> ForwardRelativePathBuf {
        let mut path = ForwardRelativePathBuf::with_capacity_for_concat(items);
        path.extend(items);
        path
    }

    /// Pushes a `RelativePath` to the existing buffer, normalizing it: the in-place form of
    /// `join_normalized`. Note that this does not visit the filesystem to resolve `..`s. Instead,
    /// it cancels out the components directly. On error, `self` is left unchanged.
    ///
    /// ```
    /// use buck2_fs::paths::RelativePath;
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
    ///
    /// let mut path = ForwardRelativePathBuf::unchecked_new("foo".to_owned());
    /// path.push_normalized(RelativePath::unchecked_new("bar"))?;
    ///
    /// assert_eq!(
    ///     ForwardRelativePathBuf::unchecked_new("foo/bar".to_owned()),
    ///     path
    /// );
    ///
    /// path.push_normalized(RelativePath::unchecked_new("more/file.rs"))?;
    /// assert_eq!(
    ///     ForwardRelativePathBuf::unchecked_new("foo/bar/more/file.rs".to_owned()),
    ///     path
    /// );
    ///
    /// path.push_normalized(RelativePath::unchecked_new("../other.rs"))?;
    /// assert_eq!(
    ///     ForwardRelativePathBuf::unchecked_new("foo/bar/more/other.rs".to_owned()),
    ///     path
    /// );
    ///
    /// path.push_normalized(RelativePath::unchecked_new(".."))?;
    /// assert_eq!(
    ///     ForwardRelativePathBuf::unchecked_new("foo/bar/more".to_owned()),
    ///     path
    /// );
    ///
    /// path.push_normalized(RelativePath::unchecked_new("../.."))?;
    /// assert_eq!(
    ///     ForwardRelativePathBuf::unchecked_new("foo".to_owned()),
    ///     path
    /// );
    ///
    /// path.push_normalized(RelativePath::unchecked_new(".."))?;
    /// assert_eq!(ForwardRelativePathBuf::unchecked_new("".to_owned()), path);
    ///
    /// assert!(
    ///     path.push_normalized(RelativePath::unchecked_new(".."))
    ///         .is_err()
    /// );
    ///
    /// # buck2_error::Ok(())
    /// ```
    pub fn push_normalized<P: AsRef<RelativePath>>(&mut self, path: P) -> buck2_error::Result<()> {
        *self = self.join_normalized(path)?;
        Ok(())
    }

    #[inline]
    pub fn into_string(self) -> String {
        self.0
    }

    pub fn into_box(self) -> Box<ForwardRelativePath> {
        let s: Box<str> = self.0.into_boxed_str();
        ForwardRelativePath::unchecked_new_box(s)
    }
}

impl Clone for Box<ForwardRelativePath> {
    #[inline]
    fn clone(&self) -> Self {
        self.to_buf().into_box()
    }
}

impl<'de> PagableBoxDeserialize<'de> for ForwardRelativePath {
    fn deserialize_box<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Box<Self>> {
        let owned =
            <ForwardRelativePathBuf as PagableDeserialize>::pagable_deserialize(deserializer)?;
        Ok(owned.into_box())
    }
}

impl<P: AsRef<ForwardRelativePath>> Extend<P> for ForwardRelativePathBuf {
    fn extend<T: IntoIterator<Item = P>>(&mut self, iter: T) {
        for p in iter {
            self.push(p);
        }
    }
}

/// Errors from ForwardRelativePath creation
///
/// TODO: `PathNotRelative` and `PathNotUtf8` duplicate `PathError`, which describes the same
/// failures, so the same bad path is reported differently depending on which constructor the
/// caller reached for. Fold them in - but note these are tagged input today and `PathError`
/// defaults to internal, so the call sites need attributing at the same time or ~158/day move
/// from USER to INFRA.
#[derive(buck2_error::Error, Debug)]
#[buck2(input)]
enum ForwardRelativePathError {
    #[error("expected a relative path but got an absolute path instead: `{0}`")]
    PathNotRelative(String),
    #[error("Path is not UTF-8: `{0}`")]
    PathNotUtf8(String),
    #[error("`{0}` does not start with `{1}`")]
    StripPrefix(String, String),
    #[error("`{0}` does not end with `{1}`")]
    StripSuffix(String, String),
}

impl<'a> IntoIterator for &'a ForwardRelativePath {
    type Item = &'a FileName;
    type IntoIter = ForwardRelativePathIter<'a>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a> TryFrom<&'a str> for &'a ForwardRelativePath {
    type Error = buck2_error::Error;

    /// no allocation conversion
    ///
    /// ```
    /// use std::convert::TryFrom;
    ///
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    ///
    /// assert!(<&ForwardRelativePath>::try_from("foo/bar").is_ok());
    /// assert!(<&ForwardRelativePath>::try_from("").is_ok());
    /// assert!(<&ForwardRelativePath>::try_from("./bar").is_err());
    /// assert!(<&ForwardRelativePath>::try_from("normalize/./bar").is_err());
    /// assert!(<&ForwardRelativePath>::try_from("/abs/bar").is_err());
    /// assert!(<&ForwardRelativePath>::try_from("normalize/../bar").is_err());
    ///
    /// # buck2_error::Ok(())
    /// ```
    #[inline]
    fn try_from(s: &'a str) -> buck2_error::Result<&'a ForwardRelativePath> {
        ForwardRelativePath::new(s)
    }
}

impl<'a> From<&'a FileName> for &'a ForwardRelativePath {
    #[inline]
    fn from(p: &'a FileName) -> Self {
        ForwardRelativePath::unchecked_new(p.as_str())
    }
}

impl<'a> TryFrom<&'a Path> for &'a ForwardRelativePath {
    type Error = buck2_error::Error;

    /// no allocation conversion
    ///
    /// ```
    /// use std::convert::TryFrom;
    /// use std::path::Path;
    ///
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    ///
    /// assert!(<&ForwardRelativePath>::try_from(Path::new("foo/bar")).is_ok());
    /// assert!(<&ForwardRelativePath>::try_from(Path::new("")).is_ok());
    /// assert!(<&ForwardRelativePath>::try_from(Path::new("./bar")).is_err());
    /// assert!(<&ForwardRelativePath>::try_from(Path::new("normalize/./bar")).is_err());
    /// assert!(<&ForwardRelativePath>::try_from(Path::new("/abs/bar")).is_err());
    /// assert!(<&ForwardRelativePath>::try_from(Path::new("normalize/../bar")).is_err());
    ///
    /// # buck2_error::Ok(())
    /// ```
    fn try_from(s: &'a Path) -> buck2_error::Result<&'a ForwardRelativePath> {
        let s = s
            .as_os_str()
            .to_str()
            .ok_or_else(|| ForwardRelativePathError::PathNotUtf8(s.display().to_string()))?;
        verify_relative_path(s, true)?;
        Ok(ForwardRelativePath::unchecked_new(s))
    }
}

impl<'a> TryFrom<&'a RelativePath> for &'a ForwardRelativePath {
    type Error = buck2_error::Error;

    /// no allocation conversion
    ///
    /// ```
    /// use std::convert::TryFrom;
    ///
    /// use buck2_fs::paths::RelativePath;
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    ///
    /// assert!(<&ForwardRelativePath>::try_from(RelativePath::unchecked_new("foo/bar")).is_ok());
    /// assert!(<&ForwardRelativePath>::try_from(RelativePath::unchecked_new("")).is_ok());
    /// assert!(<&ForwardRelativePath>::try_from(RelativePath::unchecked_new("./bar")).is_err());
    /// assert!(
    ///     <&ForwardRelativePath>::try_from(RelativePath::unchecked_new("normalize/./bar")).is_err()
    /// );
    /// assert!(
    ///     <&ForwardRelativePath>::try_from(RelativePath::unchecked_new("normalize/../bar")).is_err()
    /// );
    ///
    /// # buck2_error::Ok(())
    /// ```
    #[inline]
    fn try_from(p: &'a RelativePath) -> buck2_error::Result<&'a ForwardRelativePath> {
        ForwardRelativePath::new(p.as_str())
    }
}

impl From<ForwardRelativePathBuf> for RelativePathBuf {
    fn from(p: ForwardRelativePathBuf) -> Self {
        RelativePathBuf::from(p.0)
    }
}

impl TryFrom<String> for ForwardRelativePathBuf {
    type Error = buck2_error::Error;

    /// no allocation conversion
    ///
    /// ```
    /// use std::convert::TryFrom;
    ///
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
    ///
    /// assert!(ForwardRelativePathBuf::try_from("foo/bar".to_owned()).is_ok());
    /// assert!(ForwardRelativePathBuf::try_from("".to_owned()).is_ok());
    /// assert!(ForwardRelativePathBuf::try_from("./bar".to_owned()).is_err());
    /// assert!(ForwardRelativePathBuf::try_from("normalize/./bar".to_owned()).is_err());
    /// assert!(ForwardRelativePathBuf::try_from("/abs/bar".to_owned()).is_err());
    /// assert!(ForwardRelativePathBuf::try_from("normalize/../bar".to_owned()).is_err());
    ///
    /// # buck2_error::Ok(())
    /// ```
    #[inline]
    fn try_from(s: String) -> buck2_error::Result<ForwardRelativePathBuf> {
        ForwardRelativePathBuf::new(s)
    }
}

impl TryFrom<PathBuf> for ForwardRelativePathBuf {
    type Error = buck2_error::Error;

    /// no allocation conversion
    ///
    /// ```
    /// use std::convert::TryFrom;
    /// use std::path::PathBuf;
    ///
    /// use buck2_fs::paths::RelativePathBuf;
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
    ///
    /// assert!(ForwardRelativePathBuf::try_from(PathBuf::from("foo/bar")).is_ok());
    /// assert!(ForwardRelativePathBuf::try_from(PathBuf::from("")).is_ok());
    /// assert!(ForwardRelativePathBuf::try_from(PathBuf::from("./bar")).is_err());
    /// assert!(ForwardRelativePathBuf::try_from(PathBuf::from("normalize/./bar")).is_err());
    /// assert!(ForwardRelativePathBuf::try_from(PathBuf::from("/abs/bar")).is_err());
    /// assert!(ForwardRelativePathBuf::try_from(PathBuf::from("normalize/../bar")).is_err());
    ///
    /// # buck2_error::Ok(())
    /// ```
    fn try_from(p: PathBuf) -> buck2_error::Result<ForwardRelativePathBuf> {
        // `RelativePathBuf::from_system_path` would allocate a copy. Avoid it by going through
        // `OsString::into_string` and letting the string verifier reject any backslashes.
        ForwardRelativePathBuf::try_from(p.into_os_string().into_string().map_err(|os| {
            ForwardRelativePathError::PathNotUtf8(os.to_string_lossy().into_owned())
        })?)
    }
}

impl TryFrom<RelativePathBuf> for ForwardRelativePathBuf {
    type Error = buck2_error::Error;

    /// no allocation conversion
    ///
    /// ```
    /// use std::convert::TryFrom;
    ///
    /// use buck2_fs::paths::RelativePathBuf;
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
    ///
    /// assert!(ForwardRelativePathBuf::try_from(RelativePathBuf::from("foo/bar")).is_ok());
    /// assert!(ForwardRelativePathBuf::try_from(RelativePathBuf::from("")).is_ok());
    /// assert!(ForwardRelativePathBuf::try_from(RelativePathBuf::from("./bar")).is_err());
    /// assert!(ForwardRelativePathBuf::try_from(RelativePathBuf::from("normalize/./bar")).is_err());
    /// assert!(ForwardRelativePathBuf::try_from(RelativePathBuf::from("normalize/../bar")).is_err());
    ///
    /// # buck2_error::Ok(())
    /// ```
    #[inline]
    fn try_from(p: RelativePathBuf) -> buck2_error::Result<ForwardRelativePathBuf> {
        ForwardRelativePathBuf::try_from(p.into_string())
    }
}

impl ToOwned for ForwardRelativePath {
    type Owned = ForwardRelativePathBuf;

    #[inline]
    fn to_owned(&self) -> ForwardRelativePathBuf {
        ForwardRelativePathBuf::unchecked_new(self.0.to_owned())
    }
}

impl AsRef<ForwardRelativePath> for ForwardRelativePath {
    #[inline]
    fn as_ref(&self) -> &ForwardRelativePath {
        self
    }
}

impl AsRef<ForwardRelativePath> for ForwardRelativePathBuf {
    #[inline]
    fn as_ref(&self) -> &ForwardRelativePath {
        ForwardRelativePath::unchecked_new(&self.0)
    }
}

impl Borrow<ForwardRelativePath> for ForwardRelativePathBuf {
    #[inline]
    fn borrow(&self) -> &ForwardRelativePath {
        self.as_ref()
    }
}

impl Deref for ForwardRelativePathBuf {
    type Target = ForwardRelativePath;

    #[inline]
    fn deref(&self) -> &ForwardRelativePath {
        ForwardRelativePath::unchecked_new(&self.0)
    }
}

/// Normalize ForwardRelativePath path if needed.
pub struct ForwardRelativePathNormalizer {}

impl ForwardRelativePathNormalizer {
    pub fn normalize_path<P: AsRef<Path> + ?Sized>(
        rel_path: &P,
    ) -> buck2_error::Result<Cow<'_, ForwardRelativePath>> {
        let rel_path = rel_path.as_ref();
        if !rel_path.is_relative() {
            return Err(
                ForwardRelativePathError::PathNotRelative(rel_path.display().to_string()).into(),
            );
        }
        let path_str = rel_path
            .to_str()
            .ok_or_else(|| ForwardRelativePathError::PathNotUtf8(rel_path.display().to_string()))?;
        let bytes = path_str.as_bytes();
        if cfg!(windows) && memchr::memchr(b'\\', bytes).is_some() {
            let normalized_path = path_str.replace('\\', "/");
            Ok(Cow::Owned(ForwardRelativePathBuf::try_from(
                normalized_path,
            )?))
        } else {
            Ok(Cow::Borrowed(ForwardRelativePath::new(path_str)?))
        }
    }
}

impl<A: AsRef<ForwardRelativePath>> FromIterator<A> for ForwardRelativePathBuf {
    fn from_iter<T: IntoIterator<Item = A>>(iter: T) -> Self {
        // Collect up to 20 pointers to the stack.
        // This avoids a reallocation when joining paths of up to 20 components.
        let parts = iter.into_iter().collect::<SmallVec<[_; 20]>>();

        let mut result = ForwardRelativePathBuf::with_capacity_for_concat(&parts);
        result.extend(parts);
        result
    }
}

#[cfg(test)]
mod tests {
    use crate::paths::forward_rel_path::FileName;
    use crate::paths::forward_rel_path::ForwardRelativePath;
    use crate::paths::forward_rel_path::ForwardRelativePathBuf;
    use crate::paths::relative_path::RelativePath;

    #[test]
    fn forward_path_is_comparable() -> buck2_error::Result<()> {
        let path1_buf = ForwardRelativePathBuf::unchecked_new("foo".into());
        let path2_buf = ForwardRelativePathBuf::unchecked_new("foo".into());
        let path3_buf = ForwardRelativePathBuf::unchecked_new("bar".into());

        let path1 = ForwardRelativePath::new("foo")?;
        let path2 = ForwardRelativePath::new("foo")?;
        let path3 = ForwardRelativePath::new("bar")?;

        assert_eq!(path1_buf, path2_buf);
        assert_ne!(path1_buf, path3_buf);

        assert_eq!(path1, path2);
        assert_ne!(path1, path3);

        assert_eq!(path1_buf, path2);
        assert_ne!(path1, path3_buf);

        Ok(())
    }

    #[test]
    fn test_concat() {
        assert_eq!(
            ForwardRelativePath::new("").unwrap(),
            AsRef::<ForwardRelativePath>::as_ref(&ForwardRelativePathBuf::concat([]))
        );
        assert_eq!(
            ForwardRelativePath::new("foo/bar/baz").unwrap(),
            AsRef::<ForwardRelativePath>::as_ref(&ForwardRelativePathBuf::concat([
                ForwardRelativePath::new("foo").unwrap(),
                ForwardRelativePath::new("bar/baz").unwrap(),
            ]))
        );
        assert_eq!(
            ForwardRelativePath::new("foo/bar/baz").unwrap(),
            AsRef::<ForwardRelativePath>::as_ref(&ForwardRelativePathBuf::concat([
                ForwardRelativePath::new("").unwrap(),
                ForwardRelativePath::new("foo").unwrap(),
                ForwardRelativePath::new("bar/baz").unwrap(),
            ]))
        );
    }

    #[test]
    fn test_from_iter() {
        let parts = &["foo", "bar", "baz"]
            .into_iter()
            .map(FileName::unchecked_new)
            .collect::<Vec<_>>();

        assert_eq!(
            ForwardRelativePath::new("foo/bar/baz").unwrap().to_buf(),
            ForwardRelativePathBuf::from_iter(parts.iter().copied()),
        );
    }

    #[test]
    fn test_iter_as_path() {
        let path = ForwardRelativePath::new("foo/bar/baz").unwrap();
        let mut iter = path.iter();
        assert_eq!(
            ForwardRelativePath::new("foo/bar/baz").unwrap(),
            iter.as_path()
        );
        iter.next().unwrap();
        assert_eq!(ForwardRelativePath::new("bar/baz").unwrap(), iter.as_path());
        iter.next().unwrap();
        assert_eq!(ForwardRelativePath::new("baz").unwrap(), iter.as_path());
        iter.next().unwrap();
        assert_eq!(ForwardRelativePath::new("").unwrap(), iter.as_path());
        assert_eq!(None, iter.next());
    }

    #[test]
    fn test_iter() {
        assert_eq!(
            vec!["foo", "bar", "baz"],
            ForwardRelativePath::new("foo/bar/baz")
                .unwrap()
                .iter()
                .map(|p| p.as_str())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            vec!["baz", "bar", "foo"],
            ForwardRelativePath::new("foo/bar/baz")
                .unwrap()
                .iter()
                .rev()
                .map(|p| p.as_str())
                .collect::<Vec<_>>()
        )
    }

    #[test]
    fn test_strip_suffix() {
        let path = ForwardRelativePath::new("foo/bar/baz").unwrap();
        assert_eq!(
            Some(ForwardRelativePath::new("foo/bar/baz").unwrap()),
            path.strip_suffix_opt(ForwardRelativePath::new("").unwrap()),
        );
        assert_eq!(
            Some(ForwardRelativePath::new("foo/bar").unwrap()),
            path.strip_suffix_opt(ForwardRelativePath::new("baz").unwrap()),
        );
        assert_eq!(
            Some(ForwardRelativePath::new("foo").unwrap()),
            path.strip_suffix_opt(ForwardRelativePath::new("bar/baz").unwrap()),
        );
        assert_eq!(
            Some(ForwardRelativePath::new("").unwrap()),
            path.strip_suffix_opt(ForwardRelativePath::new("foo/bar/baz").unwrap()),
        );
        assert_eq!(
            None,
            path.strip_suffix_opt(ForwardRelativePath::new("foo/bar/baz/qux").unwrap()),
        );
        assert_eq!(
            None,
            path.strip_suffix_opt(ForwardRelativePath::new("az").unwrap()),
        );

        assert_eq!(
            Some(ForwardRelativePath::new("").unwrap()),
            ForwardRelativePath::new("")
                .unwrap()
                .strip_suffix_opt(ForwardRelativePath::new("").unwrap())
        );
        assert_eq!(
            None,
            ForwardRelativePath::new("")
                .unwrap()
                .strip_suffix_opt(ForwardRelativePath::new("xx").unwrap())
        );
    }

    #[test]
    fn test_serde() {
        fn test_roundtrip(path: &str, json: &str) {
            let path = ForwardRelativePathBuf::try_from(path.to_owned()).unwrap();
            assert_eq!(json, serde_json::to_string(&path).unwrap());
            assert_eq!(
                path,
                serde_json::from_str::<ForwardRelativePathBuf>(json).unwrap()
            );
        }

        test_roundtrip("", r#""""#);
        test_roundtrip("foo", r#""foo""#);
        test_roundtrip("foo/bar", r#""foo/bar""#);

        let err = serde_json::from_str::<ForwardRelativePathBuf>(r#""a//b""#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("contains an empty path component"), "{}", err);
    }

    #[test]
    fn test_split_last() {
        assert_eq!(
            Some((
                ForwardRelativePath::new("foo/bar").unwrap(),
                FileName::new("baz").unwrap()
            )),
            ForwardRelativePath::new("foo/bar/baz")
                .unwrap()
                .split_last(),
        );
        assert_eq!(
            Some((
                ForwardRelativePath::new("foo").unwrap(),
                FileName::new("bar").unwrap()
            )),
            ForwardRelativePath::new("foo/bar").unwrap().split_last(),
        );
        assert_eq!(
            Some((
                ForwardRelativePath::new("").unwrap(),
                FileName::new("foo").unwrap()
            )),
            ForwardRelativePath::new("foo").unwrap().split_last(),
        );
        assert_eq!(None, ForwardRelativePath::new("").unwrap().split_last());
    }

    #[test]
    fn test_push_normalized_at_start() -> buck2_error::Result<()> {
        // A segment added at the start of the buffer, on an empty buffer or after `..` removed the
        // first component, must not get a leading `/`.
        let mut path = ForwardRelativePathBuf::unchecked_new("".to_owned());
        path.push_normalized(RelativePath::unchecked_new("a"))?;
        assert_eq!("a", path.as_str());

        let mut path = ForwardRelativePathBuf::unchecked_new("foo".to_owned());
        path.push_normalized(RelativePath::unchecked_new("../bar"))?;
        assert_eq!("bar", path.as_str());

        let mut path = ForwardRelativePathBuf::unchecked_new("a/b".to_owned());
        path.push_normalized(RelativePath::unchecked_new("../../a"))?;
        assert_eq!("a", path.as_str());

        // An error leaves the buffer unchanged.
        let mut path = ForwardRelativePathBuf::unchecked_new("a".to_owned());
        assert!(
            path.push_normalized(RelativePath::unchecked_new("b/../../.."))
                .is_err()
        );
        assert_eq!("a", path.as_str());

        Ok(())
    }

    #[test]
    fn test_pop() {
        let mut p = ForwardRelativePath::new("foo/bar/baz").unwrap().to_buf();
        assert!(p.pop());
        assert_eq!(ForwardRelativePath::new("foo/bar").unwrap(), p);
        assert!(p.pop());
        assert_eq!(ForwardRelativePath::new("foo").unwrap(), p);
        assert!(p.pop());
        assert_eq!(ForwardRelativePath::new("").unwrap(), p);
        assert!(!p.pop());
    }

    #[test]
    fn path_buf_is_clonable() -> buck2_error::Result<()> {
        let buf = ForwardRelativePathBuf::unchecked_new("foo".into());
        let buf_ref = &buf;

        let cloned: ForwardRelativePathBuf = buf_ref.clone();
        assert_eq!(buf, cloned);

        Ok(())
    }

    #[test]
    fn relative_path_display_is_readable() -> buck2_error::Result<()> {
        let buf = ForwardRelativePathBuf::unchecked_new("foo/bar".into());
        assert_eq!("foo/bar", format!("{buf}"));
        assert_eq!("ForwardRelativePathBuf(\"foo/bar\")", format!("{buf:?}"));
        let refpath: &ForwardRelativePath = &buf;
        assert_eq!("foo/bar", format!("{refpath}"));
        assert_eq!("ForwardRelativePath(\"foo/bar\")", format!("{refpath:?}"));

        Ok(())
    }
}
