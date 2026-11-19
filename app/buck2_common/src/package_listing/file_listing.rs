/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use buck2_core::package::package_relative_path::PackageRelativePath;
use buck2_util::arc_str::PackedStr;
use buck2_util::arc_str::PackedStringSlice;
use dupe::Dupe;
use pagable::Pagable;

#[derive(Clone, Dupe, Eq, PartialEq, Debug, Allocative, Pagable)]
pub struct PackageFileListing {
    /// Sorted by path content for efficient prefix matching.
    pub(crate) files: PackedStringSlice<PackageRelativePath>,
}

impl PackageFileListing {
    pub fn files(&self) -> impl ExactSizeIterator<Item = &PackageRelativePath> {
        self.files.iter()
    }

    /// Files at or under `prefix`. Yields long-lived handles that share the
    /// listing's bytes buffer.
    pub(crate) fn files_within<'a>(
        &'a self,
        prefix: &PackageRelativePath,
    ) -> impl Iterator<Item = PackedStr<PackageRelativePath>> + 'a {
        let prefix_len = prefix.as_str().len();
        let range = self.files.prefix_range(prefix.as_str());
        range.filter_map(move |i| {
            let s = self.files.get(i);
            // Same logic as PackageRelativePath::starts_with, but skipping the
            // prefix recheck since `prefix_range` already confirmed it.
            if prefix_len == 0
                || prefix_len == s.as_str().len()
                || s.as_str().as_bytes()[prefix_len] == b'/'
            {
                Some(self.files.get_handle(i))
            } else {
                None
            }
        })
    }

    pub fn files_with_prefix<'a>(
        &'a self,
        prefix: &str,
    ) -> impl Iterator<Item = &'a PackageRelativePath> + 'a {
        self.files
            .prefix_range(prefix)
            .map(move |i| self.files.get(i))
    }

    pub fn get_file(&self, file: &PackageRelativePath) -> Option<PackedStr<PackageRelativePath>> {
        if let Ok(i) = self.files.binary_search(file) {
            return Some(self.files.get_handle(i));
        }
        // We don't have the file directly, but we might have a symlink file that covers this file
        // so check for that. Would be much nicer if we didn't have random symlinks.
        // These random symlinks are NOT checked for changes etc, so are pretty dangerous.
        // The config `project.read_only_paths` is the allow-list of where such symlinks can exist in v1.
        let mut current = file;
        while let Some(parent) = current.parent() {
            current = parent;
            if self.files.contains(current) {
                return Some(PackedStr::new(file));
            }
        }
        None
    }
}

pub mod testing {
    use buck2_core::package::package_relative_path::PackageRelativePath;
    use buck2_util::arc_str::PackedStringSliceBuilder;

    use crate::package_listing::file_listing::PackageFileListing;

    impl PackageFileListing {
        pub fn testing_new(files: &[&str]) -> PackageFileListing {
            let mut builder: PackedStringSliceBuilder<PackageRelativePath> =
                PackedStringSliceBuilder::with_capacity(files.len());
            for f in files {
                builder.push(PackageRelativePath::new(*f).unwrap(), ());
            }
            PackageFileListing {
                files: builder.sort_and_build(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_listing_with_prefix() {
        let listing = PackageFileListing::testing_new(&["a/1", "a/2", "b/1", "b/2", "c/1", "c/2"]);
        assert_eq!(
            listing
                .files_with_prefix("a")
                .map(|p| p.as_str())
                .collect::<Vec<_>>(),
            vec!["a/1", "a/2"]
        );
        assert_eq!(
            listing
                .files_with_prefix("b")
                .map(|p| p.as_str())
                .collect::<Vec<_>>(),
            vec!["b/1", "b/2"]
        );
        assert_eq!(
            listing
                .files_with_prefix("c")
                .map(|p| p.as_str())
                .collect::<Vec<_>>(),
            vec!["c/1", "c/2"]
        );

        let listing = PackageFileListing::testing_new(&["a/1", "a/2", "a/3"]);
        assert_eq!(
            listing
                .files_with_prefix("a")
                .map(|p| p.as_str())
                .collect::<Vec<_>>(),
            vec!["a/1", "a/2", "a/3"]
        );

        assert_eq!(
            listing
                .files_with_prefix("a/1")
                .map(|p| p.as_str())
                .collect::<Vec<_>>(),
            vec!["a/1"],
        );

        assert_eq!(0, listing.files_with_prefix("d").count());
    }

    #[test]
    fn test_listing_within() {
        let listing = PackageFileListing::testing_new(&["a/1", "a/1/2", "aa/2", "b/1"]);
        let within = |s: &str| {
            listing
                .files_within(PackageRelativePath::new(s).unwrap())
                .map(|p| p.as_str().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(vec!["aa/2"], within("aa"));
        assert_eq!(vec!["a/1", "a/1/2"], within("a"));
        assert_eq!(vec!["a/1", "a/1/2"], within("a/1"));
        assert_eq!(vec!["b/1"], within("b/1"));
        assert_eq!(vec!["a/1", "a/1/2", "aa/2", "b/1"], within(""));
    }
}
