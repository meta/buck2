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
use async_trait::async_trait;
use buck2_common::dice::cells::HasCellResolver;
use buck2_common::file_ops::dice::DiceFileComputations;
use buck2_common::file_ops::error::FileReadErrorContext;
use buck2_common::legacy_configs::dice::HasLegacyConfigs;
use buck2_common::legacy_configs::dice::OpaqueLegacyBuckConfigOnDice;
use buck2_common::package_listing::dice::DicePackageListingResolver;
use buck2_common::package_listing::listing::PackageListing;
use buck2_core::build_file_path::BuildFilePath;
use buck2_core::cells::build_file_cell::BuildFileCell;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::package::PackageLabel;
use buck2_core::package::package_relative_path::PackageRelativePath;
use buck2_error::BuckErrorContext;
use buck2_error::internal_error;
use buck2_events::dispatch::span;
use buck2_events::dispatch::span_async_simple;
use buck2_interpreter::allow_relative_paths::HasAllowRelativePaths;
use buck2_interpreter::dice::starlark_provider::StarlarkEvalKind;
use buck2_interpreter::factory::StarlarkEvaluatorProvider;
use buck2_interpreter::file_loader::LoadedModule;
use buck2_interpreter::file_loader::ModuleDeps;
use buck2_interpreter::from_freeze::from_freeze_error;
use buck2_interpreter::import_paths::HasImportPaths;
use buck2_interpreter::load_module::InterpreterCalculation;
use buck2_interpreter::paths::module::OwnedStarlarkModulePath;
use buck2_interpreter::paths::module::StarlarkModulePath;
use buck2_interpreter::paths::package::PackageFilePath;
use buck2_interpreter::paths::path::OwnedStarlarkPath;
use buck2_interpreter::paths::path::StarlarkPath;
use buck2_node::nodes::eval_result::EvaluationResult;
use buck2_node::package_visibility::HasPackageVisibilityDefaultIntersection;
use buck2_node::super_package::SuperPackage;
use buck2_util::time_span::TimeSpan;
use derive_more::Display;
use dice::DiceComputations;
use dice::EqualityBehavior;
use dice::Key;
use dice::OkPagableValueSerialize;
use dice::ValueSerialize;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use dupe::ResultDupedErrExt;
use pagable::Pagable;
use pagable::pagable_typetag;
use starlark::codemap::FileSpan;
use starlark::environment::Module;
use starlark::syntax::AstModule;
use starlark::values::FrozenHeapName;

use crate::interpreter::buckconfig::ConfigsOnDiceViewForStarlark;
use crate::interpreter::cell_info::InterpreterCellInfo;
use crate::interpreter::check_starlark_stack_size::check_starlark_stack_size;
use crate::interpreter::extra_value::FrozenInterpreterExtraValue;
use crate::interpreter::functions::load_visibility::LoadVisibilityError;
use crate::interpreter::global_interpreter_state::HasGlobalInterpreterState;
use crate::interpreter::interpreter_for_dir::InterpreterForDir;
use crate::interpreter::interpreter_for_dir::ParseData;
use crate::interpreter::interpreter_for_dir::ParseResult;
use crate::super_package::package_value::SuperPackageValuesImpl;

fn toml_value_to_json(value: toml::Value) -> serde_json::Value {
    match value {
        toml::Value::String(s) => serde_json::Value::String(s),
        toml::Value::Integer(i) => serde_json::Value::Number(i.into()),
        toml::Value::Float(f) => match serde_json::Number::from_f64(f) {
            Some(n) => serde_json::Value::Number(n),
            None => serde_json::Value::Null,
        },
        toml::Value::Boolean(b) => serde_json::Value::Bool(b),
        toml::Value::Datetime(dt) => serde_json::Value::String(dt.to_string()),
        toml::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(toml_value_to_json).collect())
        }
        toml::Value::Table(table) => serde_json::Value::Object(
            table
                .into_iter()
                .map(|(k, v)| (k, toml_value_to_json(v)))
                .collect(),
        ),
    }
}

#[async_trait]
pub trait HasCalculationDelegate<'c, 'd> {
    /// Get calculator for a file evaluation.
    ///
    /// This function only accepts cell names, but it is created
    /// per evaluated file (build file or `.bzl`).
    async fn get_interpreter_calculator(
        &'c mut self,
        path: OwnedStarlarkPath,
    ) -> buck2_error::Result<DiceCalculationDelegate<'c, 'd>>;

    /// Get calculator for evaluating a `package`'s files: its `BUCK` file and
    /// its `PACKAGE` file, whichever name the latter uses.
    async fn get_interpreter_calculator_for_package(
        &'c mut self,
        package: PackageLabel,
    ) -> buck2_error::Result<DiceCalculationDelegate<'c, 'd>>;
}

#[async_trait]
impl<'c, 'd> HasCalculationDelegate<'c, 'd> for DiceComputations<'d> {
    async fn get_interpreter_calculator(
        &'c mut self,
        path: OwnedStarlarkPath,
    ) -> buck2_error::Result<DiceCalculationDelegate<'c, 'd>> {
        let build_file_cell = path.borrow().build_file_cell();
        let dir = path
            .borrow()
            .path()
            .parent()
            .expect("starlark path to have parent")
            .to_owned();
        calculator_for_dir(self, dir, build_file_cell).await
    }

    async fn get_interpreter_calculator_for_package(
        &'c mut self,
        package: PackageLabel,
    ) -> buck2_error::Result<DiceCalculationDelegate<'c, 'd>> {
        calculator_for_dir(
            self,
            package.as_cell_path().to_owned(),
            BuildFileCell::new(package.cell_name()),
        )
        .await
    }
}

/// The interpreter configuration is per directory and build-file cell; see
/// `InterpreterConfigForDirKey`.
async fn calculator_for_dir<'c, 'd>(
    ctx: &'c mut DiceComputations<'d>,
    dir: CellPath,
    build_file_cell: BuildFileCell,
) -> buck2_error::Result<DiceCalculationDelegate<'c, 'd>> {
    {
        #[derive(Clone, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
        #[display("{}@{}", _0, _1)]
        #[pagable_typetag(dice::DiceKeyDyn)]
        struct InterpreterConfigForDirKey(CellPath, BuildFileCell);

        #[async_trait]
        impl Key for InterpreterConfigForDirKey {
            type Value = buck2_error::Result<Arc<InterpreterForDir>>;
            async fn compute(
                &self,
                ctx: &mut DiceComputations,
                _cancellation: &CancellationContext,
            ) -> Self::Value {
                let global_state = ctx.get_global_interpreter_state().await?;

                let cell_alias_resolver = ctx.get_cell_alias_resolver(self.0.cell()).await?;

                let implicit_import_paths = ctx.import_paths_for_cell(self.1).await?;

                let dirs_allowing_relative_paths = ctx.dirs_allowing_relative_paths(self.0).await?;

                let cell_info = InterpreterCellInfo::new(
                    self.1,
                    ctx.get_cell_resolver().await?.dupe(),
                    cell_alias_resolver.dupe(),
                )?;

                Ok(Arc::new(InterpreterForDir::new(
                    cell_info,
                    global_state.dupe(),
                    implicit_import_paths.dupe(),
                    dirs_allowing_relative_paths.dupe(),
                )?))
            }

            fn equality_behavior() -> EqualityBehavior<Self::Value> {
                EqualityBehavior::AlwaysUnequal
            }

            fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
                OkPagableValueSerialize::<Self::Value>::new()
            }
        }

        let configs = ctx
            .compute(&InterpreterConfigForDirKey(dir, build_file_cell))
            .await?
            .as_ref()
            .duped_err()?;

        Ok(DiceCalculationDelegate {
            build_file_cell,
            ctx,
            configs,
        })
    }
}

pub struct DiceCalculationDelegate<'c, 'd> {
    build_file_cell: BuildFileCell,
    ctx: &'c mut DiceComputations<'d>,
    configs: &'d Arc<InterpreterForDir>,
}

#[derive(Clone, Copy)]
pub(crate) enum BuildFileEvaluationMode {
    Tracked,
    Untracked,
}

impl<'c, 'd: 'c> DiceCalculationDelegate<'c, 'd> {
    async fn get_legacy_buck_config_for_starlark(
        &mut self,
    ) -> buck2_error::Result<OpaqueLegacyBuckConfigOnDice<'d>> {
        self.ctx
            .get_legacy_config_on_dice(self.build_file_cell.name())
            .await
    }

    async fn parse_file(
        &mut self,
        starlark_path: StarlarkPath<'_>,
    ) -> buck2_error::Result<ParseResult> {
        let result = DiceFileComputations::read_file(self.ctx, *starlark_path.path()).await;
        let content = match starlark_path {
            StarlarkPath::BuildFile(file) => {
                result.with_package_context_information(file.path().path().to_string())
            }
            // Should potentially add support for other file types as well
            _ => result.without_package_context_information(),
        }?;

        self.configs.parse(starlark_path, content)
    }

    async fn eval_deps(
        ctx: &mut DiceComputations<'_>,
        modules: &[(Option<FileSpan>, OwnedStarlarkModulePath)],
    ) -> buck2_error::Result<ModuleDeps> {
        Ok(ModuleDeps(
            ctx.try_compute_join(modules, async |ctx, (span, import)| {
                ctx.get_loaded_module(import.borrow())
                    .await
                    .with_buck_error_context(|| {
                        format!(
                            "From load at {}",
                            span.as_ref()
                                .map_or("implicit location".to_owned(), |file_span| file_span
                                    .resolve()
                                    .begin_file_line()
                                    .to_string())
                        )
                    })
                    .map(|m| m.dupe())
            })
            .await?,
        ))
    }

    /// Always reads with dependency tracking, and must stay that way: its callers are the
    /// bodies of memoized keys (`EvalImportKey`, `PackageFileKey`). Threading a
    /// `BuildFileEvaluationMode` through here would let a memoized value depend on a read
    /// DICE cannot see, which it would then never invalidate. Uncached build-file
    /// evaluation selects its untracked package-listing path in `eval_build_file_with_mode`
    /// before calling this tracked helper.
    pub async fn prepare_eval(
        &mut self,
        starlark_file: StarlarkPath<'_>,
    ) -> buck2_error::Result<(AstModule, ModuleDeps)> {
        let ParseData(ast, imports) = self.parse_file(starlark_file).await??;
        let deps = Self::eval_deps(self.ctx, &imports).await?;
        self.check_load_visibility(starlark_file, &imports, &deps)
            .await?;
        Ok((ast, deps))
    }

    /// Enforce load visibility declared via the `visibility()` builtin: a `.bzl`
    /// file with a non-public `visibility()` may only be `load()`ed from its own
    /// package or from packages that its visibility list matches. See the
    /// `visibility()` builtin for what a file's package is. Can be disabled with
    /// `buck2.check_load_visibility = false`.
    async fn check_load_visibility(
        &mut self,
        starlark_file: StarlarkPath<'_>,
        imports: &[(Option<FileSpan>, OwnedStarlarkModulePath)],
        deps: &ModuleDeps,
    ) -> buck2_error::Result<()> {
        if !self
            .ctx
            .get_global_interpreter_state()
            .await?
            .check_load_visibility
        {
            return Ok(());
        }

        // `imports` and `deps` are in the same order (`eval_deps` maps over
        // `imports`), so zipping pairs each load with its resolved module.
        let mut restricted = Vec::new();
        for ((span, _), module) in imports.iter().zip(deps.0.iter()) {
            // Only `.bzl` files declare load visibility, and one that never calls
            // `visibility()` is public.
            let StarlarkModulePath::LoadFile(imp) = module.path() else {
                continue;
            };
            if let Some(visibility) = FrozenInterpreterExtraValue::load_visibility(module.env())?
                && !visibility.is_public()
            {
                restricted.push((span, *imp.path(), visibility));
            }
        }
        if restricted.is_empty() {
            return Ok(());
        }

        let loader = *starlark_file.path();
        let loader_package = match starlark_file {
            StarlarkPath::BuildFile(path) => Some(path.package()),
            _ => {
                DicePackageListingResolver(self.ctx)
                    .find_enclosing_package(loader)
                    .await?
            }
        };

        for (span, loaded, visibility) in restricted {
            let error = match loader_package {
                Some(loader_package) => {
                    if visibility.matches_package(loader_package.to_cell_path())
                        || DicePackageListingResolver(self.ctx)
                            .find_enclosing_package(loaded)
                            .await?
                            == Some(loader_package)
                    {
                        continue;
                    }
                    LoadVisibilityError::NotVisible {
                        loader,
                        loaded,
                        visibility: visibility.to_string(),
                        pattern: format!("{loader_package}:"),
                    }
                }
                None => LoadVisibilityError::NotVisibleNoPackage {
                    loader,
                    loaded,
                    visibility: visibility.to_string(),
                },
            };
            return Err(buck2_error::Error::from(error)).with_buck_error_context(|| {
                format!(
                    "From load at {}",
                    span.as_ref().map_or("implicit location".to_owned(), |s| s
                        .resolve()
                        .begin_file_line()
                        .to_string())
                )
            });
        }
        Ok(())
    }

    pub fn prepare_eval_with_content(
        &self,
        starlark_file: StarlarkPath<'_>,
        content: String,
    ) -> buck2_error::Result<ParseResult> {
        self.configs.parse(starlark_file, content)
    }

    pub async fn resolve_load(
        &self,
        starlark_file: StarlarkPath<'_>,
        load_string: &str,
    ) -> buck2_error::Result<OwnedStarlarkModulePath> {
        self.configs.resolve_path(starlark_file, load_string)
    }

    pub async fn eval_module_uncached(
        &mut self,
        starlark_file: StarlarkModulePath<'_>,
        cancellation: &CancellationContext,
    ) -> buck2_error::Result<LoadedModule> {
        match starlark_file {
            StarlarkModulePath::JsonFile(_) => self.eval_json_module_uncached(starlark_file).await,
            StarlarkModulePath::TomlFile(_) => self.eval_toml_file_uncached(starlark_file).await,
            _ => {
                self.eval_starlark_module_uncached(starlark_file, cancellation)
                    .await
            }
        }
    }

    async fn eval_json_module_uncached(
        &mut self,
        starlark_file: StarlarkModulePath<'_>,
    ) -> buck2_error::Result<LoadedModule> {
        let path = starlark_file.path();
        let contents = DiceFileComputations::read_file(self.ctx, *path)
            .await
            .with_package_context_information(path.path().to_string())?;

        let value: serde_json::Value = serde_json::from_str(&contents)
            .with_buck_error_context(|| format!("Parsing {path}"))?;

        // We expect these to be small + simple
        // ast-grep-ignore: rust/buck2-no-starlark-module
        let frozen = Module::with_temp_heap(|module| {
            module.set("value", module.heap().alloc(value));
            module
                .freeze_named(FrozenHeapName::User(Box::new(StarlarkEvalKind::Load(
                    Arc::new(OwnedStarlarkModulePath::new(starlark_file)),
                ))))
                .map_err(from_freeze_error)
        })?;
        Ok(LoadedModule::new(
            OwnedStarlarkModulePath::new(starlark_file),
            Default::default(),
            frozen,
        ))
    }

    async fn eval_toml_file_uncached(
        &mut self,
        starlark_file: StarlarkModulePath<'_>,
    ) -> buck2_error::Result<LoadedModule> {
        let path = starlark_file.path();
        let contents = DiceFileComputations::read_file(self.ctx, *path)
            .await
            .with_package_context_information(path.path().to_string())?;

        let value: toml::Value =
            toml::from_str(&contents).with_buck_error_context(|| format!("Parsing {path}"))?;
        let json_value = toml_value_to_json(value);

        // We expect these to be small + simple
        // ast-grep-ignore: rust/buck2-no-starlark-module
        let frozen = Module::with_temp_heap(|module| {
            module.set("value", module.heap().alloc(json_value));
            module
                .freeze_named(FrozenHeapName::User(Box::new(StarlarkEvalKind::Load(
                    Arc::new(OwnedStarlarkModulePath::new(starlark_file)),
                ))))
                .map_err(from_freeze_error)
        })?;
        Ok(LoadedModule::new(
            OwnedStarlarkModulePath::new(starlark_file),
            Default::default(),
            frozen,
        ))
    }

    async fn eval_starlark_module_uncached(
        &mut self,
        starlark_file: StarlarkModulePath<'_>,
        cancellation: &CancellationContext,
    ) -> buck2_error::Result<LoadedModule> {
        let (ast, deps) = self.prepare_eval(starlark_file.into()).await?;
        let loaded_modules = deps.get_loaded_modules();
        let buckconfig = self.get_legacy_buck_config_for_starlark().await?;
        let root_buckconfig = self.ctx.get_legacy_root_config_on_dice().await?;

        let configs = self.configs;
        let ctx = &mut *self.ctx;

        let eval_kind = StarlarkEvalKind::Load(Arc::new(starlark_file.to_owned()));
        let provider = StarlarkEvaluatorProvider::new(ctx, eval_kind).await?;

        let mut buckconfigs = ConfigsOnDiceViewForStarlark::new(ctx, buckconfig, root_buckconfig);
        let evaluation = configs
            .eval_module(
                starlark_file,
                &mut buckconfigs,
                ast,
                loaded_modules.clone(),
                provider,
                cancellation,
            )
            .with_buck_error_context(|| format!("Error evaluating module: `{}`", starlark_file))?;

        Ok(LoadedModule::new(
            OwnedStarlarkModulePath::new(starlark_file),
            loaded_modules,
            evaluation,
        ))
    }

    /// Eval parent `PACKAGE` file for given package file.
    async fn eval_parent_package_file(
        &mut self,
        file: PackageLabel,
    ) -> buck2_error::Result<SuperPackage> {
        let cell_resolver = self.ctx.get_cell_resolver().await?;
        let proj_rel_path = cell_resolver.resolve_path(file.as_cell_path())?;
        match proj_rel_path.parent() {
            None => {
                // We are in the project root, there's no parent.
                Ok(SuperPackage::empty::<SuperPackageValuesImpl>()?)
            }
            Some(parent) => {
                let parent_cell = cell_resolver.get_cell_path(parent);
                self.eval_package_file(PackageLabel::from_cell_path(parent_cell)?)
                    .await
            }
        }
    }

    /// Return `None` if there's no `PACKAGE` file in the directory.
    pub async fn prepare_package_file_eval(
        &mut self,
        package: PackageLabel,
    ) -> buck2_error::Result<Option<(PackageFilePath, AstModule, ModuleDeps)>> {
        // Note:
        /// To avoid paying the cost of read_dir when computing if any specific file has changed (e.g. PACKAGE),
        /// we depend on directory_sublisting_matching_any_case_key to invalidate all files that match (regardless of case).
        /// We need to do this to make sure to work with case-sensitive file paths.
        //   * `read_path_metadata` would not tell us if the file name is `PACKAGE`
        //     and not `package` on case-insensitive filesystems.
        //     We do case-sensitive comparison for `BUCK` files, so we do the same here.
        //   * we fail here if `PACKAGE` (but not `package`) exists, and it is not a file.

        // package file results capture starlark values and so cannot be checked for equality. This means we
        // can't get early cutoff for the consumers, and so we need to be careful to ensure our deps are precise.
        // Otherwise noop package value recomputations can lead to large recompute costs.
        //
        // Here we put the package file check behind an additional dice key so that we don't recompute on irrelevant
        // changes to the directory contents.
        #[derive(Debug, Display, Clone, Allocative, Eq, PartialEq, Hash, Pagable)]
        #[pagable_typetag(dice::DiceKeyDyn)]
        struct PackageFileLookupKey(PackageLabel);

        #[async_trait]
        impl Key for PackageFileLookupKey {
            type Value = buck2_error::Result<Option<PackageFilePath>>;

            async fn compute(
                &self,
                ctx: &mut DiceComputations,
                _cancellation: &CancellationContext,
            ) -> Self::Value {
                for package_file_path in PackageFilePath::for_dir(self.0.as_cell_path()) {
                    if DiceFileComputations::exists(ctx, *package_file_path.path()).await? {
                        return Ok(Some(package_file_path));
                    }
                }
                Ok(None)
            }

            fn equality_behavior() -> EqualityBehavior<Self::Value> {
                EqualityBehavior::Compare(|x, y| match (x, y) {
                    (Ok(x), Ok(y)) => x == y,
                    _ => false,
                })
            }

            fn validity(x: &Self::Value) -> bool {
                x.is_ok()
            }

            fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
                OkPagableValueSerialize::<Self::Value>::new()
            }
        }

        match self
            .ctx
            .compute(&PackageFileLookupKey(package.dupe()))
            .await?
            .as_ref()
            .duped_err()?
        {
            Some(package_file_path) => {
                let (module, deps) = self
                    .prepare_eval(StarlarkPath::PackageFile(package_file_path))
                    .await?;
                Ok(Some((package_file_path.clone(), module, deps)))
            }
            None => Ok(None),
        }
    }

    async fn eval_package_file_uncached(
        &mut self,
        path: PackageLabel,
        cancellation: &CancellationContext,
    ) -> buck2_error::Result<SuperPackage> {
        let parent = self.eval_parent_package_file(path.dupe()).await?;
        let ast_deps = self.prepare_package_file_eval(path.dupe()).await?;

        let (package_file_path, ast, deps) = match ast_deps {
            Some(x) => x,
            None => {
                // If there's no `PACKAGE` file, return parent.
                return Ok(parent);
            }
        };

        let buckconfig = self.get_legacy_buck_config_for_starlark().await?;
        let root_buckconfig = self.ctx.get_legacy_root_config_on_dice().await?;

        let configs = self.configs;
        let ctx = &mut *self.ctx;

        let package_visibility_default_intersection = ctx
            .per_transaction_data()
            .get_package_visibility_default_intersection();

        let eval_kind = StarlarkEvalKind::LoadPackageFile(path.dupe());
        let provider = StarlarkEvaluatorProvider::new(ctx, eval_kind).await?;

        let mut buckconfigs = ConfigsOnDiceViewForStarlark::new(ctx, buckconfig, root_buckconfig);

        configs
            .eval_package_file(
                &package_file_path,
                ast,
                parent,
                &mut buckconfigs,
                deps.get_loaded_modules(),
                provider,
                cancellation,
                package_visibility_default_intersection,
            )
            .with_buck_error_context(|| format!("evaluating Starlark PACKAGE file `{path}`"))
    }

    pub(crate) async fn eval_package_file(
        &mut self,
        path: PackageLabel,
    ) -> buck2_error::Result<SuperPackage> {
        #[derive(Debug, Display, Clone, Allocative, Eq, PartialEq, Hash, Pagable)]
        #[pagable_typetag(dice::DiceKeyDyn)]
        struct PackageFileKey(PackageLabel);

        #[async_trait]
        impl Key for PackageFileKey {
            type Value = buck2_error::Result<SuperPackage>;

            async fn compute(
                &self,
                ctx: &mut DiceComputations,
                cancellation: &CancellationContext,
            ) -> Self::Value {
                let mut interpreter = ctx.get_interpreter_calculator_for_package(self.0).await?;
                interpreter
                    .eval_package_file_uncached(self.0.dupe(), cancellation)
                    .await
            }

            fn equality_behavior() -> EqualityBehavior<Self::Value> {
                EqualityBehavior::Compare(|x, y| match (x, y) {
                    (Ok(x), Ok(y)) => x == y,
                    _ => false,
                })
            }

            fn validity(x: &Self::Value) -> bool {
                x.is_ok()
            }

            fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
                OkPagableValueSerialize::<Self::Value>::new()
            }
        }

        self.ctx
            .compute(&PackageFileKey(path))
            .await?
            .as_ref()
            .duped_err()
            .map(|sp| sp.dupe())
    }

    /// Most directories do not contain a `PACKAGE` file, this function
    /// optimizes `eval_package_file` for this case by avoiding creation of DICE key.
    pub(crate) async fn eval_package_file_for_build_file(
        &mut self,
        package: PackageLabel,
        package_listing: &PackageListing,
    ) -> buck2_error::Result<SuperPackage> {
        for package_file_name in PackageFilePath::package_file_names() {
            if package_listing
                .get_file(PackageRelativePath::new(package_file_name)?)
                .is_some()
            {
                return self.eval_package_file(package).await;
            }
        }

        // Without this optimization, `cquery <that android target>` has 6% time regression.
        // With this optimization, check for `PACKAGE` files adds 2% to time.
        self.eval_parent_package_file(package).await
    }

    async fn resolve_package_listing(
        ctx: &mut DiceComputations<'_>,
        package: PackageLabel,
        mode: BuildFileEvaluationMode,
    ) -> buck2_error::Result<PackageListing> {
        span_async_simple(
            buck2_data::LoadPackageStart {
                path: package.as_cell_path().to_string(),
            },
            async move {
                match mode {
                    BuildFileEvaluationMode::Tracked => {
                        DicePackageListingResolver(ctx)
                            .resolve_package_listing(package.dupe())
                            .await
                    }
                    BuildFileEvaluationMode::Untracked => {
                        DicePackageListingResolver(ctx)
                            .resolve_package_listing_untracked(package.dupe())
                            .await
                    }
                }
            },
            buck2_data::LoadPackageEnd {
                path: package.as_cell_path().to_string(),
            },
        )
        .await
    }

    pub async fn eval_build_file(
        &mut self,
        package: PackageLabel,
        cancellation: &CancellationContext,
    ) -> (TimeSpan, buck2_error::Result<Arc<EvaluationResult>>) {
        self.eval_build_file_with_mode(package, BuildFileEvaluationMode::Tracked, cancellation)
            .await
    }

    pub(crate) async fn eval_build_file_with_mode(
        &mut self,
        package: PackageLabel,
        mode: BuildFileEvaluationMode,
        cancellation: &CancellationContext,
    ) -> (TimeSpan, buck2_error::Result<Arc<EvaluationResult>>) {
        let mut now = None;
        let eval_kind = StarlarkEvalKind::LoadBuildFile(package.dupe());
        let eval_result: buck2_error::Result<_> = try {
            let ((), listing) = self
                .ctx
                .try_compute2(
                    async |ctx| check_starlark_stack_size(ctx).await,
                    async |ctx| Self::resolve_package_listing(ctx, package.dupe(), mode).await,
                )
                .await?;

            let build_file_path =
                BuildFilePath::new(package.dupe(), listing.buildfile().to_owned());
            let (ast, deps) = self
                .prepare_eval(StarlarkPath::BuildFile(&build_file_path))
                .await?;
            let super_package = self
                .eval_package_file_for_build_file(package.dupe(), &listing)
                .await?;

            let buckconfig = self.get_legacy_buck_config_for_starlark().await?;
            let root_buckconfig = self.ctx.get_legacy_root_config_on_dice().await?;
            let module_id = build_file_path.to_string();
            let cell_str = build_file_path.cell().as_str().to_owned();
            let start_event = buck2_data::LoadBuildFileStart {
                cell: cell_str.clone(),
                module_id: module_id.clone(),
            };

            let configs = self.configs;
            let ctx = &mut *self.ctx;

            now = Some(TimeSpan::start_now());
            let provider = StarlarkEvaluatorProvider::new(ctx, eval_kind).await?;
            let mut buckconfigs =
                ConfigsOnDiceViewForStarlark::new(ctx, buckconfig, root_buckconfig);

            let (profile_data, eval_result) = span(start_event, move || {
                let result_with_stats = configs
                    .eval_build_file(
                        &build_file_path,
                        &mut buckconfigs,
                        listing,
                        super_package,
                        ast,
                        deps.get_loaded_modules(),
                        provider,
                        false,
                        cancellation,
                    )
                    .with_buck_error_context(|| {
                        format!("Error evaluating build file: `{}`", build_file_path)
                    });
                let error = result_with_stats.as_ref().err().map(|e| format!("{e:#}"));
                let (
                    starlark_peak_allocated_bytes,
                    cpu_instruction_count,
                    starlark_tick_count,
                    target_count,
                ) = match &result_with_stats {
                    Ok((_, rs)) => (
                        Some(rs.starlark_peak_allocated_bytes),
                        rs.cpu_instruction_count,
                        Some(rs.starlark_tick_count),
                        Some(rs.result.targets().len() as u64),
                    ),
                    Err(_) => (None, None, None, None),
                };

                (
                    result_with_stats,
                    buck2_data::LoadBuildFileEnd {
                        module_id,
                        cell: cell_str,
                        target_count,
                        starlark_peak_allocated_bytes,
                        cpu_instruction_count,
                        error,
                        starlark_tick_count,
                    },
                )
            })?;

            let mut eval_result = eval_result.result;

            if eval_result.starlark_profile.is_some() {
                return (
                    now.unwrap().end_now(),
                    Err(internal_error!(
                        "starlark_profile field must not be set yet"
                    )),
                );
            }
            eval_result.starlark_profile = profile_data.map(|d| d as _);
            eval_result
        };

        (
            now.map_or(TimeSpan::empty_now(), |v| v.end_now()),
            eval_result.map(Arc::new),
        )
    }
}

mod keys {
    use allocative::Allocative;
    use buck2_interpreter::paths::module::OwnedStarlarkModulePath;
    use derive_more::Display;
    use pagable::Pagable;
    use pagable::pagable_typetag;

    #[derive(Clone, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
    #[pagable_typetag(dice::DiceKeyDyn)]
    pub struct EvalImportKey(pub OwnedStarlarkModulePath);
}

pub mod testing {
    // re-exports for testing
    pub use super::keys::EvalImportKey;
}
