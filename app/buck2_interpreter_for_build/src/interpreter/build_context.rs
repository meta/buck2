/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::OnceCell;
use std::fmt::Debug;

use buck2_core::bxl::BxlFilePath;
use buck2_core::bzl::ImportPath;
use buck2_core::cells::CellResolver;
use buck2_core::cells::build_file_cell::BuildFileCell;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::package::PackageLabel;
use buck2_core::pattern::pattern::InferTargetNames;
use buck2_interpreter::build_context::STARLARK_PATH_FROM_BUILD_CONTEXT;
use buck2_interpreter::file_type::StarlarkFileType;
use buck2_interpreter::paths::path::StarlarkPath;
use starlark::any::ProvidesStaticType;
use starlark::eval::Evaluator;

use crate::interpreter::buckconfig::BuckConfigsViewForStarlark;
use crate::interpreter::buckconfig::LegacyBuckConfigsForStarlark;
use crate::interpreter::bzl_eval_ctx::BzlEvalCtx;
use crate::interpreter::cell_info::InterpreterCellInfo;
use crate::interpreter::functions::host_info::HostInfo;
use crate::interpreter::module_internals::ModuleInternals;
use crate::super_package::eval_ctx::PackageFileEvalCtx;

#[derive(buck2_error::Error, Debug)]
#[buck2(input)]
enum BuildContextError {
    #[error(
        "This function is unavailable during analysis (usual solution is to place the information on a toolchain)"
    )]
    UnavailableDuringAnalysis,
    #[error("`{0}()` can only be called from a build file context; current file context is {1:?}")]
    NotBuildFile(String, StarlarkFileType),
    #[error("Expecting build file context, but current file context is {0:?}")]
    NotBuildFileNoFunction(StarlarkFileType),
    #[error(
        "`{0}()` can only be called from a package file context; current file context is {1:?}"
    )]
    NotPackageFile(String, StarlarkFileType),
    #[error("Expecting package file context, but current file context is {0:?}")]
    NotPackageFileNoFunction(StarlarkFileType),
    #[error("Package can only be fetched from a build file")]
    PackageOnlyFromBuildFile,
    #[error(
        "Base path is only defined for build file or PACKAGE file; current file context is {0:?}"
    )]
    BasePathOnlyDefinedForPackageOrBuildFile(StarlarkFileType),
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum PerFileTypeContext {
    /// Context for evaluating `BUCK` files.
    Build(ModuleInternals),
    /// Context for evaluating `PACKAGE` files.
    Package(PackageFileEvalCtx),
    /// Context for evaluating `bzl` files.
    Bzl(BzlEvalCtx),
    Bxl(BxlFilePath),
    /// Context for evaluating `json` files.
    Json(ImportPath),
    Toml(ImportPath),
}

impl PerFileTypeContext {
    pub(crate) fn starlark_path(&self) -> StarlarkPath<'_> {
        match self {
            PerFileTypeContext::Build(module) => StarlarkPath::BuildFile(module.buildfile_path()),
            PerFileTypeContext::Package(package) => StarlarkPath::PackageFile(&package.path),
            PerFileTypeContext::Bzl(path) => StarlarkPath::LoadFile(&path.bzl_path),
            PerFileTypeContext::Bxl(path) => StarlarkPath::BxlFile(path),
            PerFileTypeContext::Json(path) => StarlarkPath::JsonFile(path),
            PerFileTypeContext::Toml(path) => StarlarkPath::TomlFile(path),
        }
    }

    /// Gets base path from buildfile or PACKAGE file context.
    /// For example, for `foo//bar/PACKAGE` this returns `foo//bar`.
    /// For `foo//bar/baz/BUCK` this returns `foo//bar/baz`.
    /// Throws an error if it's used in any other context.
    fn base_path(&self) -> buck2_error::Result<CellPath> {
        match self {
            PerFileTypeContext::Build(module) => {
                Ok(module.buildfile_path().package().to_cell_path())
            }
            PerFileTypeContext::Package(package) => Ok(package.path.dir().to_owned()),
            _ => Err(BuildContextError::BasePathOnlyDefinedForPackageOrBuildFile(
                self.starlark_path().file_type(),
            )
            .into()),
        }
    }

    pub(crate) fn file_type(&self) -> StarlarkFileType {
        self.starlark_path().file_type()
    }

    pub(crate) fn require_build(
        &self,
        function_name: &str,
    ) -> buck2_error::Result<&ModuleInternals> {
        match self {
            PerFileTypeContext::Build(internals) => Ok(internals),
            x => {
                Err(BuildContextError::NotBuildFile(function_name.to_owned(), x.file_type()).into())
            }
        }
    }

    pub(crate) fn into_build(self) -> buck2_error::Result<ModuleInternals> {
        match self {
            PerFileTypeContext::Build(internals) => Ok(internals),
            x => Err(BuildContextError::NotBuildFileNoFunction(x.file_type()).into()),
        }
    }

    pub(crate) fn require_package_file(
        &self,
        function_name: &str,
    ) -> buck2_error::Result<&PackageFileEvalCtx> {
        match self {
            PerFileTypeContext::Package(ctx) => Ok(ctx),
            x => Err(
                BuildContextError::NotPackageFile(function_name.to_owned(), x.file_type()).into(),
            ),
        }
    }

    pub(crate) fn into_package_file(self) -> buck2_error::Result<PackageFileEvalCtx> {
        match self {
            PerFileTypeContext::Package(ctx) => Ok(ctx),
            x => Err(BuildContextError::NotPackageFileNoFunction(x.file_type()).into()),
        }
    }
}

/// Buck-specific information exposed to the starlark environment via the context's extra field.
/// This would include things like the current cell or package name or the package listing. It's
/// used for quite a few other things, including recording declared rules.
///
/// Many of the functions available in build files reach out to this as part of
/// their implementation.
#[derive(ProvidesStaticType, Debug)]
pub struct BuildContext<'a> {
    /// The CellInfo for the top-level module being interpreted. Note that this
    /// is not necessarily the same as the file currently being processed
    /// (i.e. if `cell1//BUCK` loads `cell2//defs.bzl` the cell info when
    /// loading `cell2//defs.bzl` will be based on `cell1`).
    ///
    /// This cell_info is used only for things that should be resolved in the
    /// context of the top-level module. The most important of these is the
    /// `read_config()` implementation.
    ///
    /// For example of where this isn't used, it is not used for resolving
    /// `load()` statements.
    pub cell_info: &'a InterpreterCellInfo,

    pub(crate) buckconfigs: LegacyBuckConfigsForStarlark<'a>,

    pub(crate) host_info: &'a HostInfo,

    /// Context specific to type type.
    pub additional: PerFileTypeContext,

    /// When true, rule function is no-op.
    pub(crate) ignore_attrs_for_profiling: bool,

    /// Whether to infer a target name when a label pattern does not provide one,
    /// making `//foo/bar` equivalent to `//foo/bar:bar`. Controlled by the
    /// `buck2.infer_target_names` buckconfig.
    pub(crate) infer_target_names: InferTargetNames,

    /// Peak allocated bytes limit for starlark.
    pub(crate) starlark_peak_allocated_byte_limit: OnceCell<Option<u64>>,
}

impl<'a> BuildContext<'a> {
    /// Create a build context for the given module.
    pub(crate) fn new(
        cell_info: &'a InterpreterCellInfo,
        buckconfigs: &'a mut dyn BuckConfigsViewForStarlark,
        host_info: &'a HostInfo,
        additional: PerFileTypeContext,
        ignore_attrs_for_profiling: bool,
        infer_target_names: InferTargetNames,
    ) -> BuildContext<'a> {
        let buckconfigs = LegacyBuckConfigsForStarlark::new(buckconfigs);
        BuildContext {
            cell_info,
            buckconfigs,
            host_info,
            additional,
            ignore_attrs_for_profiling,
            infer_target_names,
            starlark_peak_allocated_byte_limit: OnceCell::new(),
        }
    }

    pub fn from_context<'v, 'a1>(
        eval: &Evaluator<'v, 'a1, 'a>,
    ) -> buck2_error::Result<&'a1 BuildContext<'a>> {
        let f = || eval.extra?.downcast_ref::<BuildContext>();
        f().ok_or_else(|| BuildContextError::UnavailableDuringAnalysis.into())
    }

    pub(crate) fn cell_info(&self) -> &InterpreterCellInfo {
        self.cell_info
    }

    pub fn build_file_cell(&self) -> BuildFileCell {
        self.cell_info.name()
    }

    pub fn cell_resolver(&self) -> &CellResolver {
        self.cell_info.cell_resolver()
    }

    pub fn require_package(&self) -> buck2_error::Result<PackageLabel> {
        match &self.additional {
            PerFileTypeContext::Build(module) => Ok(module.buildfile_path().package()),
            _ => Err(BuildContextError::PackageOnlyFromBuildFile.into()),
        }
    }

    pub fn starlark_path(&self) -> StarlarkPath<'_> {
        self.additional.starlark_path()
    }

    pub(crate) fn base_path(&self) -> buck2_error::Result<CellPath> {
        self.additional.base_path()
    }
}

pub(crate) fn init_starlark_path_from_build_context() {
    STARLARK_PATH_FROM_BUILD_CONTEXT
        .init(|eval| Ok(BuildContext::from_context(eval)?.starlark_path()))
}

/// Arbitrary object made available to the execution context. Converted to
/// EvalResult at the end of interpreting
impl ModuleInternals {
    /// Try to get this inner context from the `ctx.extra` property.
    pub fn from_context<'a>(
        ctx: &'a Evaluator,
        function_name: &str,
    ) -> buck2_error::Result<&'a Self> {
        BuildContext::from_context(ctx)?
            .additional
            .require_build(function_name)
    }
}
