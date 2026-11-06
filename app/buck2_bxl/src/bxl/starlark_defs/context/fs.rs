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
use buck2_artifact::artifact::source_artifact::SourceArtifact;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_artifact::StarlarkArtifact;
use buck2_common::file_ops::dice::DiceFileComputations;
use buck2_common::file_ops::metadata::RawPathMetadata;
use buck2_common::package_listing::dice::DicePackageListingResolver;
use buck2_common::package_listing::resolver::PackageListingResolver;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::instance::CellInstance;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_core::package::PackageLabel;
use buck2_core::package::package_relative_path::PackageRelativePath;
use buck2_core::package::source_path::SourcePath;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_node::nodes::unconfigured::TargetNode;
use buck2_util::arc_str::PackedStr;
use derivative::Derivative;
use derive_more::Display;
use dice::DiceComputations;
use futures::FutureExt;
use starlark::any::ProvidesStaticType;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::AllocValue;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkValue;
use starlark::values::StringValue;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::none::NoneOr;
use starlark::values::starlark_value;

use super::BxlContext;
use crate::bxl::starlark_defs::file_expr::FileExpr;
use crate::bxl::starlark_defs::file_set::StarlarkReadDirSet;
use crate::bxl::starlark_defs::target_list_expr::TargetListExpr;
use crate::bxl::starlark_defs::target_list_expr::TargetListExprArg;

#[derive(
    ProvidesStaticType,
    Derivative,
    Display,
    Trace,
    NoSerialize,
    Allocative
)]
#[derivative(Debug)]
#[display("{:?}", self)]
#[allocative(skip)]
pub(crate) struct BxlFilesystem<'v> {
    #[derivative(Debug = "ignore")]
    ctx: ValueTyped<'v, BxlContext<'v>>,
}

impl<'v> BxlFilesystem<'v> {
    pub(crate) fn new(ctx: ValueTyped<'v, BxlContext<'v>>) -> Self {
        Self { ctx }
    }

    fn artifact_fs(&self) -> &ArtifactFs {
        self.ctx.artifact_fs()
    }

    fn project_fs(&self) -> &ProjectRoot {
        self.ctx.project_fs()
    }

    fn cell(&self) -> buck2_error::Result<&CellInstance> {
        self.ctx.cell_resolver().get(self.ctx.cell_name())
    }
}

starlark::methods_static!(BXL_FILESYSTEM_METHODS = fs_operations);

#[starlark_value(type = "bxl.Filesystem", StarlarkTypeRepr, UnpackValue)]
impl<'v> StarlarkValue<'v> for BxlFilesystem<'v> {
    fn get_methods() -> Option<&'static Methods> {
        Some(BXL_FILESYSTEM_METHODS.methods())
    }
}

impl<'v> AllocValue<'v> for BxlFilesystem<'v> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_complex_no_freeze(self)
    }
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
pub(crate) enum BxlFilesystemError {
    #[error("Inferred package path `{0}` is not a valid package within the given file path `{1}`")]
    PackageMismatch(PackageLabel, CellPath),
    #[error("Expected a single target hint, not an iterable: `{0}`")]
    MultipleTargetHintsNotSupported(String),
}

impl<'v> BxlFilesystem<'v> {
    /// Returns the absolute path for a FileExpr.
    fn resolve(
        &'v self,
        expr: FileExpr<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> buck2_error::Result<AbsNormPathBuf> {
        let project_rel_path = self.project_relative_path(expr, eval)?;
        Ok(self.project_fs().resolve(&project_rel_path))
    }

    /// Returns the project relative path for a cellpath.
    fn project_relative_path(
        &'v self,
        expr: FileExpr<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> buck2_error::Result<ProjectRelativePathBuf> {
        let cell_path = self.ctx.via_dice(eval, |ctx| {
            ctx.via(|dice| async { expr.get(dice, self.cell()?).await }.boxed_local())
        })?;
        self.artifact_fs().resolve_cell_path(cell_path)
    }
}

/// Whether something is at the path. A symlink is something; what is behind one is not looked
/// at, so `exists("link/x")` is false whatever `link` points to.
async fn try_exists(ctx: &mut DiceComputations<'_>, path: CellPath) -> buck2_error::Result<bool> {
    Ok(
        match DiceFileComputations::read_path_metadata_if_exists(ctx, path).await? {
            None => false,
            Some(RawPathMetadata::Symlink { at, .. }) => at == path,
            Some(_) => true,
        },
    )
}

/// Provides some basic tracked filesystem access for bxl functions so that they can meaningfully
/// detect simple properties of artifacts, and source directories.
#[starlark_module]
fn fs_operations(builder: &mut MethodsBuilder) {
    /// Check if a path exists on disk, taking advantage of Buck's cached filesystem.
    /// Takes in a literal, a source artifact (via `artifact`), or a `file_node`.
    /// A symlink exists; what it points to is not consulted, so a path through a
    /// symlink does not.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_exists(ctx):
    ///     ctx.output.print(ctx.fs.exists("bin"))
    /// ```
    fn exists<'v>(
        this: &'v BxlFilesystem<'v>,
        expr: FileExpr<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<bool> {
        Ok(this.ctx.via_dice(eval, |ctx| {
            ctx.via(|dice| {
                async {
                    let path = expr.get(dice, this.cell()?).await;

                    match path {
                        Ok(p) => try_exists(dice, p).await,
                        Err(e) => Err(e),
                    }
                }
                .boxed_local()
            })
        })?)
    }

    /// Returns all the contents of the given input that points to a directory.
    /// Errors if the given path is a file. Takes an optional boolean `dirs_only` to only return directories, defaults to false.
    ///
    /// The input is a either a literal, a source artifact (via `artifact`), or a `file_node`.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_list(ctx):
    ///     list_results = ctx.fs.list("bin")
    ///     for result in list_results:
    ///         ctx.output.print(result)
    /// ```
    fn list<'v>(
        this: &'v BxlFilesystem<'v>,
        expr: FileExpr<'v>,
        #[starlark(require = named, default = false)] dirs_only: bool,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkReadDirSet> {
        Ok(this.ctx.via_dice(eval, |ctx| {
            ctx.via(|dice| {
                async {
                    let path = expr.get(dice, this.cell()?).await;

                    match path {
                        Ok(path) => {
                            let read_dir_output =
                                DiceFileComputations::read_dir(dice, path).await?;
                            Ok(StarlarkReadDirSet {
                                cell_path: path,
                                included: read_dir_output.included,
                                dirs_only,
                            })
                        }
                        Err(e) => Err(e),
                    }
                }
                .boxed_local()
            })
        })?)
    }

    /// Returns whether the provided path is a dir. Returns false is the dir does not exist.
    /// The input is a either a literal, a source artifact (via `artifact`), or a `file_node`.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_is_dir(ctx):
    ///     ctx.output.print(ctx.fs.is_dir("bin"))
    /// ```
    fn is_dir<'v>(
        this: &'v BxlFilesystem<'v>,
        expr: FileExpr<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<bool> {
        Ok(std::path::Path::is_dir(this.resolve(expr, eval)?.as_ref()))
    }

    /// Returns whether the provided path is a file. Returns false is the file does not exist.
    /// The input is a either a literal, a source artifact (via `artifact`), or a `file_node`.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_is_file(ctx):
    ///     ctx.output.print(ctx.fs.is_dir("bin"))
    /// ```
    fn is_file<'v>(
        this: &'v BxlFilesystem<'v>,
        expr: FileExpr<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<bool> {
        Ok(std::path::Path::is_file(this.resolve(expr, eval)?.as_ref()))
    }

    /// Returns the relative path to the project root, given the file expression.
    ///
    /// Sample usage:
    /// ```python
    /// def project_rel_path(ctx):
    ///     ctx.output.print(ctx.fs.project_rel_path("bin"))
    /// ```
    fn project_rel_path<'v>(
        this: &'v BxlFilesystem<'v>,
        expr: FileExpr<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StringValue<'v>> {
        Ok(eval
            .heap()
            .alloc_str(this.project_relative_path(expr, eval)?.as_str()))
    }

    /// Returns the absolute path, given the file expression. Use at your own risk, as the current working directory
    /// may have been changed when this function is called. In addition, passing the absolute path into actions that
    /// are run remotely will most likely result in failures since the absolute path most likely differs locally vs remotely.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_abs_path_unsafe(ctx):
    ///     ctx.output.print(ctx.fs.abs_path_unsafe("bin"))
    /// ```
    fn abs_path_unsafe<'v>(
        this: &'v BxlFilesystem<'v>,
        expr: FileExpr<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StringValue<'v>> {
        let abs_norm_path = this.resolve(expr, eval)?;
        Ok(eval.heap().alloc_str(abs_norm_path.as_abs_path().to_str()?))
    }

    /// Returns the source artifact for a path and an optional target hint (unconfigured target label or node)
    /// which points to the owning package. If no target hint is given, the nearest package will be used to
    /// guess the desired artifact. The path should be either an absolute path, or a project relative path.
    fn source<'v>(
        this: &'v BxlFilesystem<'v>,
        expr: FileExpr<'v>,
        #[starlark(default = NoneOr::None)] target_hint: NoneOr<ValueOf<'v, TargetListExprArg<'v>>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<ValueTyped<'v, StarlarkArtifact>> {
        let buck_path = this.ctx.via_dice(eval, |dice| {
            dice.via(|dice| {
                async {
                    let file_path_as_cell_path = expr.get(dice, this.cell()?).await?;
                    let file_path = this
                        .artifact_fs()
                        .resolve_cell_path(file_path_as_cell_path)?;

                    let package_label = match target_hint {
                        NoneOr::None => {
                            DicePackageListingResolver(dice)
                                .get_enclosing_package(file_path_as_cell_path)
                                .await
                        }
                        NoneOr::Other(target_hint) => {
                            let target_expr = TargetListExpr::<'v, TargetNode>::unpack(
                                target_hint.try_unpack()?,
                                &this.ctx,
                                dice,
                            )
                            .await?;
                            if let Some(node) = target_expr.get_one(dice).await? {
                                Ok(node.label().pkg())
                            } else {
                                Err(BxlFilesystemError::MultipleTargetHintsNotSupported(
                                    target_hint.to_value().to_repr(),
                                )
                                .into())
                            }
                        }
                    }?;

                    let package_path = this
                        .artifact_fs()
                        .resolve_cell_path(package_label.as_cell_path())?;

                    let forward_relative_path = match file_path.strip_prefix(&package_path) {
                        Ok(path) => path,
                        Err(_) => {
                            return Err(BxlFilesystemError::PackageMismatch(
                                package_label,
                                file_path_as_cell_path,
                            )
                            .into());
                        }
                    };

                    let package_relative_path =
                        PackageRelativePath::new(forward_relative_path.as_path())?;
                    Ok(SourcePath::new(
                        package_label,
                        PackedStr::new(package_relative_path),
                    ))
                }
                .boxed_local()
            })
        })?;

        Ok(eval
            .heap()
            .alloc_typed(StarlarkArtifact::new(SourceArtifact::new(buck_path).into())))
    }
}
