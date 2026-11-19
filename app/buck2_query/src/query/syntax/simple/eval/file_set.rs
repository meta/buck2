/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;

use allocative::Allocative;
use buck2_core::cells::cell_path::CellPath;
use buck2_hash::BuckMutInsertionSet;
use derive_more::Display;
use display_container::fmt_container;
use fancy_regex::Regex;
use pagable::Pagable;

use crate::query::environment::QueryEnvironment;
use crate::query::environment::QueryTarget;
use crate::query::syntax::simple::eval::error::QueryError;
use crate::query::syntax::simple::eval::set::TargetSet;

/// An entry in a FileSet.
#[derive(Hash, Eq, PartialEq, Debug, Clone, Display, Allocative, Pagable)]
pub struct FileNode(pub CellPath);

/// Iterates in insertion order; `==` is set equality, so the order the files arrived in does not
/// matter.
#[derive(Debug, Eq, Clone, Allocative, Pagable)]
pub struct FileSet {
    files: BuckMutInsertionSet<FileNode>,
}

impl PartialEq for FileSet {
    fn eq(&self, other: &Self) -> bool {
        self.files.eq_unordered(&other.files)
    }
}

impl fmt::Display for FileSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_container(f, "[", "]", self.files.iter())
    }
}

impl FileSet {
    pub fn new(files: BuckMutInsertionSet<FileNode>) -> Self {
        Self { files }
    }

    pub(crate) fn filter_name(&self, regex: &str) -> buck2_error::Result<Self> {
        let re = Regex::new(regex)?;
        self.filter(|node| Ok(re.is_match(&node.0.to_string())?))
    }

    fn filter<F: Fn(&FileNode) -> buck2_error::Result<bool>>(
        &self,
        filter: F,
    ) -> buck2_error::Result<Self> {
        let mut files = BuckMutInsertionSet::default();
        for file in self.files.iter() {
            if filter(file)? {
                files.insert(file.clone());
            }
        }
        Ok(Self { files })
    }

    pub fn insert(&mut self, file: FileNode) {
        self.files.insert(file);
    }

    pub fn insert_all(&mut self, other: &FileSet) {
        for v in other.files.iter() {
            self.files.insert(v.clone());
        }
    }

    pub fn union(&self, right: &FileSet) -> FileSet {
        let mut files = self.files.clone();
        for file in right.files.iter() {
            files.insert(file.clone());
        }
        Self { files }
    }

    pub fn owner<T: QueryTarget>(
        &self,
        _env: &impl QueryEnvironment<Target = T>,
    ) -> buck2_error::Result<TargetSet<T>> {
        Err(QueryError::FunctionUnimplemented("owner()").into())
    }

    pub fn iter(&self) -> impl Iterator<Item = &CellPath> + '_ {
        self.files.iter().map(|v| &v.0)
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn contains(&self, item: &FileNode) -> bool {
        self.files.contains(item)
    }

    pub fn intersect(&self, right: &FileSet) -> buck2_error::Result<FileSet> {
        self.filter(|file| Ok(right.contains(file)))
    }

    pub fn difference(&self, right: &FileSet) -> buck2_error::Result<FileSet> {
        self.filter(|file| Ok(!right.contains(file)))
    }
}

impl FromIterator<FileNode> for FileSet {
    fn from_iter<T: IntoIterator<Item = FileNode>>(iter: T) -> FileSet {
        FileSet {
            files: BuckMutInsertionSet::from_iter(iter),
        }
    }
}

#[cfg(test)]
mod tests {
    use buck2_core::cells::cell_path::CellPath;
    use buck2_core::cells::name::CellName;
    use buck2_core::cells::paths::CellRelativePath;

    use super::*;

    fn file(name: &str) -> FileNode {
        FileNode(CellPath::new(
            CellName::testing_new("root"),
            CellRelativePath::unchecked_new(name),
        ))
    }

    #[test]
    fn equality_ignores_insertion_order() {
        let ab: FileSet = [file("a"), file("b")].into_iter().collect();
        let ba: FileSet = [file("b"), file("a")].into_iter().collect();
        let abc: FileSet = [file("a"), file("b"), file("c")].into_iter().collect();
        assert_eq!(ab, ba);
        assert_ne!(ab, abc);
        assert!(ab.iter().map(|p| p.to_string()).eq(["root//a", "root//b"]));
    }
}
