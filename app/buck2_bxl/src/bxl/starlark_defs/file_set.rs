/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;
use std::fmt;
use std::ops::Deref;

use allocative::Allocative;
use buck2_common::file_ops::metadata::FileType;
use buck2_core::cells::cell_path::CellPath;
use buck2_fs::paths::file_name::FileName;
use buck2_hash::BuckMutInsertionSet;
use buck2_query::query::syntax::simple::eval::file_set::FileNode;
use buck2_query::query::syntax::simple::eval::file_set::FileSet;
use buck2_util::arc_str::PackedStringSlice;
use derive_more::Display;
use display_container::fmt_container;
use starlark::any::ProvidesStaticType;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::starlark_module;
use starlark::starlark_simple_value;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueError;
use starlark::values::ValueOf;
use starlark::values::list_or_tuple::ListOrTupleType;
use starlark::values::starlark_value;
use starlark::values::type_repr::StarlarkTypeRepr;

use crate::bxl::starlark_defs::context::BxlContext;
use crate::bxl::starlark_defs::context::BxlContextCoreData;

/// FileSetExpr is just a simple type that can be used in starlark_module
/// functions for arguments that should be file sets. It will accept either a
/// literal (like `//path/to/some/file.txt`) or a FileSet Value (from one of the
/// bxl functions that return them).
#[derive(StarlarkTypeRepr, UnpackValue)]
pub(crate) enum FileSetExpr<'v> {
    Literal(&'v str),
    Literals(ValueOf<'v, ListOrTupleType<&'v str>>),
    FileSet(&'v StarlarkFileSet),
}

impl<'a> FileSetExpr<'a> {
    pub(crate) async fn get(self, bxl: &BxlContext<'_>) -> buck2_error::Result<Cow<'a, FileSet>> {
        let set = match self {
            FileSetExpr::Literal(val) => Cow::Owned(FileSet::from_iter([FileNode(
                bxl.parse_query_file_literal(val)?,
            )])),
            FileSetExpr::Literals(val) => {
                let mut file_set = FileSet::new(BuckMutInsertionSet::default());
                for arg in val.try_items() {
                    file_set.insert(FileNode(bxl.parse_query_file_literal(arg?)?));
                }
                Cow::Owned(file_set)
            }
            FileSetExpr::FileSet(val) => Cow::Borrowed(&val.0),
        };
        Ok(set)
    }
}

#[derive(Debug, Clone, Allocative)]
pub(crate) enum OwnedFileSetExpr {
    Literal(String),
    Literals(Vec<String>),
    FileSet(FileSet),
}

impl OwnedFileSetExpr {
    pub(crate) fn from_ref(expr: &FileSetExpr) -> buck2_error::Result<Self> {
        Ok(match expr {
            FileSetExpr::Literal(val) => Self::Literal(val.to_string()),
            FileSetExpr::Literals(val) => Self::Literals(
                val.try_items()
                    .map(|s| buck2_error::Ok(s?.to_owned()))
                    .collect::<buck2_error::Result<_>>()?,
            ),
            FileSetExpr::FileSet(val) => Self::FileSet(val.0.clone()),
        })
    }

    pub(crate) fn get<'a>(
        &'a self,
        core_data: &BxlContextCoreData,
    ) -> buck2_error::Result<Cow<'a, FileSet>> {
        let set = match self {
            OwnedFileSetExpr::Literal(val) => Cow::Owned(FileSet::from_iter([FileNode(
                core_data.parse_query_file_literal(val)?,
            )])),
            OwnedFileSetExpr::Literals(val) => {
                let mut file_set = FileSet::new(BuckMutInsertionSet::default());
                for arg in val {
                    file_set.insert(FileNode(core_data.parse_query_file_literal(arg)?));
                }
                Cow::Owned(file_set)
            }
            OwnedFileSetExpr::FileSet(val) => Cow::Borrowed(val),
        };
        Ok(set)
    }
}

#[derive(Debug, Display, ProvidesStaticType, Allocative, StarlarkPagable)]
#[derive(NoSerialize)] // TODO maybe this should be
pub(crate) struct StarlarkFileSet(
    /// Set of files or directories.
    #[starlark_pagable(pagable)]
    pub(crate) FileSet,
);

starlark_simple_value!(StarlarkFileSet);

#[starlark_value(type = "bxl.FileSet")]
impl<'v> StarlarkValue<'v> for StarlarkFileSet {
    fn iterate_collect(&self, heap: Heap<'v>) -> starlark::Result<Vec<Value<'v>>> {
        Ok(self
            .0
            .iter()
            .map(|cell_path| heap.alloc(StarlarkFileNode(*cell_path)))
            .collect())
    }

    fn length(&self) -> starlark::Result<i32> {
        i32::try_from(self.0.len()).map_err(starlark::Error::new_other)
    }

    fn sub(&self, other: Value<'v>, heap: Heap<'v>) -> starlark::Result<Value<'v>> {
        let Some(other) = other.downcast_ref::<Self>() else {
            return ValueError::unsupported_with(self, "-", other);
        };
        let difference = self.0.difference(&other.0)?;
        Ok(heap.alloc(Self(difference)))
    }

    fn equals(&self, other: Value<'v>) -> starlark::Result<bool> {
        match other.downcast_ref::<StarlarkFileSet>() {
            Some(other) => Ok(self.0 == other.0),
            None => Ok(false),
        }
    }

    fn bit_or(&self, other: Value<'v>, heap: Heap<'v>) -> starlark::Result<Value<'v>> {
        let other = other.downcast_ref_err::<Self>()?;
        let union = self.0.union(&other.0);
        Ok(heap.alloc(Self(union)))
    }

    fn bit_and(&self, other: Value<'v>, heap: Heap<'v>) -> starlark::Result<Value<'v>> {
        let Some(other) = other.downcast_ref::<Self>() else {
            return ValueError::unsupported_with(self, "&", other);
        };
        let intersect = self.0.intersect(&other.0)?;
        Ok(heap.alloc(Self(intersect)))
    }

    fn get_methods() -> Option<&'static Methods> {
        Some(FILE_SET_METHODS.methods())
    }
}

starlark::methods_static!(FILE_SET_METHODS = register_file_set);

/// A set of `file_node`s. Supports union (`|`), intersection (`&`), difference (`-`), length,
/// iteration and equality.
#[starlark_module]
pub(crate) fn register_file_set(globals: &mut MethodsBuilder) {}

impl From<FileSet> for StarlarkFileSet {
    fn from(v: FileSet) -> Self {
        Self(v)
    }
}

impl Deref for StarlarkFileSet {
    type Target = FileSet;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Display, ProvidesStaticType, Clone, Allocative, StarlarkPagable)]
#[derive(NoSerialize)]
pub(crate) struct StarlarkFileNode(#[starlark_pagable(pagable)] pub(crate) CellPath);

starlark_simple_value!(StarlarkFileNode);

starlark::methods_static!(FILE_NODE_METHODS = file_node_methods);

#[starlark_value(type = "bxl.FileNode")]
impl<'v> StarlarkValue<'v> for StarlarkFileNode {
    fn get_methods() -> Option<&'static Methods> {
        Some(FILE_NODE_METHODS.methods())
    }
}

/// Wrapper around the cell relative path to the file or directory.
#[starlark_module]
pub(crate) fn file_node_methods(methods: &mut MethodsBuilder) {
    /// The cell relative path as a string.
    #[starlark(attribute)]
    fn path<'v>(this: &'v StarlarkFileNode) -> starlark::Result<&'v str> {
        Ok(this.0.path().as_str())
    }

    /// The cell name for the file_node.
    #[starlark(attribute)]
    fn cell<'v>(this: &StarlarkFileNode) -> starlark::Result<&'v str> {
        Ok(this.0.cell().as_str())
    }
}

#[derive(
    Debug,
    ProvidesStaticType,
    Clone,
    Allocative,
    pagable::Pagable,
    starlark::StarlarkPagableViaPagable
)]
#[derive(NoSerialize)]
pub(crate) struct StarlarkReadDirSet {
    /// Cell path to the directory/files.
    pub(crate) cell_path: CellPath,
    /// Files that are not ignored within the buckconfig.
    /// Sorted.
    pub(crate) included: PackedStringSlice<FileName, FileType>,
    /// Only return directories when iterating or printing.
    pub(crate) dirs_only: bool,
}

starlark_simple_value!(StarlarkReadDirSet);

impl StarlarkReadDirSet {
    fn children(&self) -> buck2_error::Result<Vec<CellPath>> {
        let mut result: Vec<CellPath> = Vec::with_capacity(self.included.len());
        result.extend(
            self.included
                .iter_with_tag()
                .filter_map(|(name, file_type)| {
                    if !self.dirs_only || file_type.is_dir() {
                        Some(self.cell_path.join(name))
                    } else {
                        None
                    }
                }),
        );
        result.sort();
        Ok(result)
    }
}

impl fmt::Display for StarlarkReadDirSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.children() {
            Ok(children) => fmt_container(f, "[", "]", children),
            Err(e) => write!(f, "<Error: {e}>"),
        }
    }
}

#[starlark_value(type = "bxl.ReadDirSet")]
impl<'v> StarlarkValue<'v> for StarlarkReadDirSet {
    fn iterate_collect(&self, heap: Heap<'v>) -> starlark::Result<Vec<Value<'v>>> {
        Ok(self
            .children()?
            .into_iter()
            .map(|cell_path| heap.alloc(StarlarkFileNode(cell_path)))
            .collect::<Vec<_>>())
    }
}
