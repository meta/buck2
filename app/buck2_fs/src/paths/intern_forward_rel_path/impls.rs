/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Trait implementations and convenience methods for `IForwardRelativePath`.
//! Everything here is built on top of the base public API (`as_str`,
//! `as_forward_relative_path`, `is_empty`, `empty`, `intern`, `concat`,
//! `from_sub_path`).

use std::fmt;
use std::hash::Hash;
use std::hash::Hasher;
use std::ops::Deref;

use allocative::Allocative;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;
use strong_hash::StrongHash;

use super::IForwardRelativePath;
use crate::paths::file_name::FileName;
use crate::paths::forward_rel_path::ForwardRelativePath;
use crate::paths::forward_rel_path::ForwardRelativePathBuf;
use crate::paths::forward_rel_path::ForwardRelativePathIter;

// ---------------------------------------------------------------------------
// Sub-slicing / path methods
// ---------------------------------------------------------------------------

impl IForwardRelativePath {
    pub fn parent(self) -> Option<IForwardRelativePath> {
        self.as_forward_relative_path()
            .parent()
            .map(Self::from_sub_path)
    }

    pub fn file_name(self) -> Option<&'static FileName> {
        self.as_forward_relative_path().file_name()
    }

    pub fn split_first(self) -> Option<(&'static FileName, IForwardRelativePath)> {
        self.as_forward_relative_path()
            .split_first()
            .map(|(f, rest)| (f, Self::from_sub_path(rest)))
    }

    pub fn split_last(self) -> Option<(IForwardRelativePath, &'static FileName)> {
        self.as_forward_relative_path()
            .split_last()
            .map(|(rest, f)| (Self::from_sub_path(rest), f))
    }

    pub fn strip_prefix<P: AsRef<ForwardRelativePath>>(
        self,
        prefix: P,
    ) -> buck2_error::Result<IForwardRelativePath> {
        self.as_forward_relative_path()
            .strip_prefix(prefix)
            .map(Self::from_sub_path)
    }

    pub fn strip_prefix_opt<P: AsRef<ForwardRelativePath>>(
        self,
        prefix: P,
    ) -> Option<IForwardRelativePath> {
        self.as_forward_relative_path()
            .strip_prefix_opt(prefix)
            .map(Self::from_sub_path)
    }

    pub fn strip_suffix<P: AsRef<ForwardRelativePath>>(
        self,
        suffix: P,
    ) -> buck2_error::Result<IForwardRelativePath> {
        self.as_forward_relative_path()
            .strip_suffix(suffix)
            .map(Self::from_sub_path)
    }

    pub fn strip_suffix_opt<P: AsRef<ForwardRelativePath>>(
        self,
        suffix: P,
    ) -> Option<IForwardRelativePath> {
        self.as_forward_relative_path()
            .strip_suffix_opt(suffix)
            .map(Self::from_sub_path)
    }

    pub fn starts_with<P: AsRef<ForwardRelativePath>>(self, base: P) -> bool {
        self.as_forward_relative_path().starts_with(base)
    }

    pub fn ends_with<P: AsRef<ForwardRelativePath>>(self, child: P) -> bool {
        self.as_forward_relative_path().ends_with(child)
    }

    pub fn iter(self) -> ForwardRelativePathIter<'static> {
        self.as_forward_relative_path().iter()
    }

    pub fn extension(self) -> Option<&'static str> {
        self.as_forward_relative_path().extension()
    }

    pub fn file_stem(self) -> Option<&'static str> {
        self.as_forward_relative_path().file_stem()
    }

    pub fn strip_prefix_components(self, components: usize) -> Option<IForwardRelativePath> {
        self.as_forward_relative_path()
            .strip_prefix_components(components)
            .map(Self::from_sub_path)
    }
}

// ---------------------------------------------------------------------------
// Trait implementations
// ---------------------------------------------------------------------------

impl Deref for IForwardRelativePath {
    type Target = ForwardRelativePath;

    #[inline]
    fn deref(&self) -> &ForwardRelativePath {
        self.as_forward_relative_path()
    }
}

impl AsRef<ForwardRelativePath> for IForwardRelativePath {
    #[inline]
    fn as_ref(&self) -> &ForwardRelativePath {
        self.as_forward_relative_path()
    }
}

impl Hash for IForwardRelativePath {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_forward_relative_path().hash(state)
    }
}

impl StrongHash for IForwardRelativePath {
    #[inline]
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        self.as_forward_relative_path().strong_hash(state)
    }
}

impl PartialEq for IForwardRelativePath {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for IForwardRelativePath {}

impl PartialOrd for IForwardRelativePath {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for IForwardRelativePath {
    #[inline]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl fmt::Display for IForwardRelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for IForwardRelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "IForwardRelativePath({:?})", self.as_str())
    }
}

impl Serialize for IForwardRelativePath {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.as_str().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for IForwardRelativePath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        let path = ForwardRelativePath::new(&s).map_err(serde::de::Error::custom)?;
        Ok(IForwardRelativePath::intern_directly(path))
    }
}

impl Allocative for IForwardRelativePath {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        // We should figure out how to report these allocations to allocative - here is not the
        // right place though.
        visitor.enter_self_sized::<Self>().exit();
    }
}

impl pagable::PagableSerialize for IForwardRelativePath {
    fn pagable_serialize(
        &self,
        serializer: &mut dyn pagable::PagableSerializer,
    ) -> pagable::Result<()> {
        use serde::Serialize;
        self.as_str().serialize(serializer.serde())?;
        Ok(())
    }
}

impl<'de> pagable::PagableDeserialize<'de> for IForwardRelativePath {
    fn pagable_deserialize<D: pagable::PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        use serde::Deserialize;
        let s = String::deserialize(deserializer.serde())?;
        let path = ForwardRelativePathBuf::try_from(s).map_err(pagable::Error::msg)?;
        Ok(IForwardRelativePath::intern_directly(&path))
    }
}

#[cfg(test)]
mod tests {
    use crate::paths::forward_rel_path::ForwardRelativePath;
    use crate::paths::intern_forward_rel_path::IForwardRelativePath;

    #[test]
    fn test_parent() {
        let p =
            IForwardRelativePath::intern_directly(ForwardRelativePath::new("foo/bar/baz").unwrap());
        let parent = p.parent().unwrap();
        assert_eq!("foo/bar", parent.as_str());
        let grandparent = parent.parent().unwrap();
        assert_eq!("foo", grandparent.as_str());
        let root = grandparent.parent().unwrap();
        assert!(root.is_empty());
        assert_eq!(None, root.parent());
    }

    #[test]
    fn test_empty_parent_and_file_name() {
        let p = IForwardRelativePath::empty();
        assert_eq!(None, p.parent());
        assert_eq!(None, p.file_name());
    }

    #[test]
    fn test_split_first_last() {
        let p =
            IForwardRelativePath::intern_directly(ForwardRelativePath::new("foo/bar/baz").unwrap());

        let (first, rest) = p.split_first().unwrap();
        assert_eq!("foo", first.as_str());
        assert_eq!("bar/baz", rest.as_str());

        let (rest, last) = p.split_last().unwrap();
        assert_eq!("foo/bar", rest.as_str());
        assert_eq!("baz", last.as_str());
    }

    #[test]
    fn test_strip_prefix() {
        let p =
            IForwardRelativePath::intern_directly(ForwardRelativePath::new("foo/bar/baz").unwrap());
        // Accept IForwardRelativePath.
        let prefix =
            IForwardRelativePath::intern_directly(ForwardRelativePath::new("foo").unwrap());
        let rest = p.strip_prefix(prefix).unwrap();
        assert_eq!("bar/baz", rest.as_str());
        // Also accept &ForwardRelativePath directly.
        let rest2 = p
            .strip_prefix(ForwardRelativePath::new("foo").unwrap())
            .unwrap();
        assert_eq!("bar/baz", rest2.as_str());
    }

    #[test]
    fn test_join() {
        let a = IForwardRelativePath::intern_directly(ForwardRelativePath::new("foo").unwrap());
        let b = IForwardRelativePath::intern_directly(ForwardRelativePath::new("bar/baz").unwrap());
        let joined = a.join(b);
        assert_eq!("foo/bar/baz", joined.as_str());
    }

    #[test]
    fn test_join_empty() {
        let a = IForwardRelativePath::intern_directly(ForwardRelativePath::new("foo").unwrap());
        let e = IForwardRelativePath::empty();
        assert_eq!("foo", a.join(e).as_str());
        assert_eq!("foo", e.join(a).as_str());
        assert!(e.join(e).is_empty());
    }

    #[test]
    fn test_hash_matches_forward_relative_path() {
        use std::hash::DefaultHasher;
        use std::hash::Hash;
        use std::hash::Hasher;

        let path_str = "foo/bar/baz";
        let frp = ForwardRelativePath::new(path_str).unwrap();
        let ifrp = IForwardRelativePath::intern_directly(frp);

        let mut h1 = DefaultHasher::new();
        frp.hash(&mut h1);

        let mut h2 = DefaultHasher::new();
        ifrp.hash(&mut h2);

        assert_eq!(h1.finish(), h2.finish());
    }

    #[test]
    fn test_iter() {
        let p =
            IForwardRelativePath::intern_directly(ForwardRelativePath::new("foo/bar/baz").unwrap());
        let names: Vec<&str> = p.iter().map(|f| f.as_str()).collect();
        assert_eq!(vec!["foo", "bar", "baz"], names);
    }

    #[test]
    fn test_extension() {
        let p =
            IForwardRelativePath::intern_directly(ForwardRelativePath::new("foo/bar.rs").unwrap());
        assert_eq!(Some("rs"), p.extension());
    }

    #[test]
    fn test_file_stem() {
        let p =
            IForwardRelativePath::intern_directly(ForwardRelativePath::new("foo/bar.rs").unwrap());
        assert_eq!(Some("bar"), p.file_stem());
    }

    #[test]
    fn test_serde_roundtrip() {
        let p = IForwardRelativePath::intern_directly(ForwardRelativePath::new("foo/bar").unwrap());
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!("\"foo/bar\"", json);
        let deserialized: IForwardRelativePath = serde_json::from_str(&json).unwrap();
        assert_eq!(p, deserialized);
    }

    #[test]
    fn test_display_debug() {
        let p = IForwardRelativePath::intern_directly(ForwardRelativePath::new("foo/bar").unwrap());
        assert_eq!("foo/bar", format!("{p}"));
        assert_eq!("IForwardRelativePath(\"foo/bar\")", format!("{p:?}"));
    }

    #[test]
    fn test_deref_to_forward_relative_path() {
        let p = IForwardRelativePath::intern_directly(ForwardRelativePath::new("foo/bar").unwrap());
        let frp: &ForwardRelativePath = &p;
        assert_eq!("foo/bar", frp.as_str());
    }
}
