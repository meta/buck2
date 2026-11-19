/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use allocative::Allocative;
use buck2_core::package::package_relative_path::PackageRelativePath;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::file_name::FileNameBuf;
use buck2_util::arc_str::PackedStr;
use buck2_util::arc_str::PackedStringSlice;
use dupe::Dupe;
use pagable::Pagable;

use crate::package_listing::file_listing::PackageFileListing;

#[derive(Clone, Dupe, Eq, PartialEq, Debug, Allocative, Pagable)]
pub struct PackageListing {
    listing: Arc<PackageListingData>,
}

#[derive(Eq, PartialEq, Debug, Allocative, Pagable)]
struct PackageListingData {
    files: PackageFileListing,
    /// Sorted by path content. All directories at or under this package, except
    /// for subpackages and their contents.
    directories: PackedStringSlice<PackageRelativePath>,
    /// Sorted by path content. Subpackages of this package.
    subpackages: PackedStringSlice<PackageRelativePath>,
    buildfile: FileNameBuf,
}

impl PackageListing {
    pub(crate) fn new(
        files: PackedStringSlice<PackageRelativePath>,
        directories: PackedStringSlice<PackageRelativePath>,
        subpackages: PackedStringSlice<PackageRelativePath>,
        buildfile: FileNameBuf,
    ) -> Self {
        Self {
            listing: Arc::new(PackageListingData {
                files: PackageFileListing { files },
                directories,
                subpackages,
                buildfile,
            }),
        }
    }

    pub fn empty(buildfile: FileNameBuf) -> Self {
        Self::new(
            PackedStringSlice::empty(),
            PackedStringSlice::empty(),
            PackedStringSlice::empty(),
            buildfile,
        )
    }

    pub fn files(&self) -> &PackageFileListing {
        &self.listing.files
    }

    pub fn get_file(&self, file: &PackageRelativePath) -> Option<PackedStr<PackageRelativePath>> {
        self.listing.files.get_file(file)
    }

    pub fn get_dir(&self, dir: &PackageRelativePath) -> Option<PackedStr<PackageRelativePath>> {
        // Empty paths must refer to a directory, since the whole thing is rooted
        // at a directory. But empty paths are not explicitly added to the `directories` variable,
        // so handle them specially.
        if dir.is_empty() {
            Some(PackedStr::new(PackageRelativePath::empty()))
        } else {
            match self.listing.directories.binary_search(dir) {
                Ok(i) => Some(self.listing.directories.get_handle(i)),
                Err(_) => None,
            }
        }
    }

    pub fn files_within<'a>(
        &'a self,
        dir: &PackageRelativePath,
    ) -> impl Iterator<Item = PackedStr<PackageRelativePath>> + 'a {
        self.listing.files.files_within(dir)
    }

    pub fn subpackages_within<'a>(
        &'a self,
        dir: &'a PackageRelativePath,
    ) -> impl Iterator<Item = &'a PackageRelativePath> + 'a {
        self.listing
            .subpackages
            .iter()
            .filter(move |x: &&PackageRelativePath| x.starts_with(dir))
    }

    pub fn buildfile(&self) -> &FileName {
        &self.listing.buildfile
    }
}

pub mod testing {
    use buck2_core::package::package_relative_path::PackageRelativePath;
    use buck2_fs::paths::file_name::FileNameBuf;
    use buck2_util::arc_str::PackedStringSlice;
    use buck2_util::arc_str::PackedStringSliceBuilder;

    use crate::package_listing::listing::PackageListing;

    pub trait PackageListingExt {
        fn testing_empty() -> Self;
        fn testing_files(files: &[&str]) -> Self;
        fn testing_new(files: &[&str], buildfile: &str) -> Self;
    }

    impl PackageListingExt for PackageListing {
        fn testing_empty() -> Self {
            Self::testing_files(&[])
        }

        fn testing_files(files: &[&str]) -> Self {
            Self::testing_new(files, "BUCK")
        }

        fn testing_new(files: &[&str], buildfile: &str) -> Self {
            let mut builder: PackedStringSliceBuilder<PackageRelativePath> =
                PackedStringSliceBuilder::with_capacity(files.len());
            for f in files {
                builder.push(PackageRelativePath::new(*f).unwrap(), ());
            }
            PackageListing::new(
                builder.sort_and_build(),
                PackedStringSlice::empty(),
                PackedStringSlice::empty(),
                FileNameBuf::unchecked_new(buildfile),
            )
        }
    }
}
