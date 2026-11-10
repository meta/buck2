/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Implements the core skylark interpreter. This encodes the primitive
//! operations of converting file content to ASTs and evaluating import and
//! build files.

use std::cell::OnceCell;
use std::cell::RefCell;
use std::sync::Arc;

use allocative::Allocative;
use buck2_common::legacy_configs::configs::LegacyBuckConfig;
use buck2_common::legacy_configs::key::BuckconfigKeyRef;
use buck2_common::package_listing::listing::PackageListing;
use buck2_core::build_file_path::BuildFilePath;
use buck2_core::bxl::BxlFilePath;
use buck2_core::bzl::ImportPath;
use buck2_core::cells::build_file_cell::BuildFileCell;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::cell_path_with_allowed_relative_dir::CellPathWithAllowedRelativeDir;
use buck2_error::BuckErrorContext;
use buck2_error::conversion::from_any_with_tag;
use buck2_error::internal_error;
use buck2_event_observer::humanized::HumanizedBytes;
use buck2_events::dispatch::get_dispatcher;
use buck2_interpreter::factory::BuckStarlarkModule;
use buck2_interpreter::factory::FinishedStarlarkEvaluation;
use buck2_interpreter::factory::StarlarkEvaluatorProvider;
use buck2_interpreter::file_loader::InterpreterFileLoader;
use buck2_interpreter::file_loader::LoadResolver;
use buck2_interpreter::file_loader::LoadedModules;
use buck2_interpreter::file_type::StarlarkFileType;
use buck2_interpreter::import_paths::ImplicitImportPaths;
use buck2_interpreter::package_imports::ImplicitImport;
use buck2_interpreter::parse_import::RelativeImports;
use buck2_interpreter::parse_import::parse_import;
use buck2_interpreter::paths::module::OwnedStarlarkModulePath;
use buck2_interpreter::paths::module::StarlarkModulePath;
use buck2_interpreter::paths::package::PackageFilePath;
use buck2_interpreter::paths::path::OwnedStarlarkPath;
use buck2_interpreter::paths::path::StarlarkPath;
use buck2_interpreter::prelude_path::PreludePath;
use buck2_interpreter::print_handler::EventDispatcherPrintHandler;
use buck2_interpreter::soft_error::Buck2StarlarkSoftErrorHandler;
use buck2_interpreter::starlark_profiler::data::StarlarkProfileDataAndStats;
use buck2_node::modifiers::ModifierLabelParser;
use buck2_node::nodes::eval_result::EvaluationResult;
use buck2_node::nodes::eval_result::EvaluationResultWithStats;
use buck2_node::super_package::SuperPackage;
use buck2_settings::PackageVisibilityDefaultIntersection;
use buck2_util::per_thread_instruction_counter::PerThreadInstructionCounter;
use dice::CancellationContext;
use dupe::Dupe;
use pagable::Pagable;
use starlark::codemap::FileSpan;
use starlark::environment::FrozenModule;
use starlark::syntax::AstModule;
use starlark::values::any_complex::StarlarkAnyComplex;

use crate::interpreter::buckconfig::BuckConfigsViewForStarlark;
use crate::interpreter::build_context::BuildContext;
use crate::interpreter::build_context::PerFileTypeContext;
use crate::interpreter::bzl_eval_ctx::BzlEvalCtx;
use crate::interpreter::cell_info::InterpreterCellInfo;
use crate::interpreter::extra_value::InterpreterExtraValue;
use crate::interpreter::global_interpreter_state::GlobalInterpreterState;
use crate::interpreter::module_internals::ModuleInternals;
use crate::interpreter::package_file_extra::FrozenPackageFileExtra;
use crate::super_package::eval_ctx::PackageFileEvalCtx;

const DEFAULT_STARLARK_MEMORY_USAGE_LIMIT: u64 = 2 * (1 << 30);

#[derive(Debug, buck2_error::Error)]
#[error("Tabs are not allowed in Buck files: `{0}`")]
#[buck2(input)]
struct StarlarkTabsError(OwnedStarlarkPath);

#[derive(Debug, buck2_error::Error)]
enum StarlarkPeakMemoryError {
    #[error(
        "Starlark peak memory usage for {0} is {1} which exceeds the limit {2}! Please reduce memory usage to prevent OOMs. See {3} for debugging tips."
    )]
    #[buck2(input)]
    ExceedsThreshold(BuildFilePath, HumanizedBytes, HumanizedBytes, String),
}

/// A ParseData includes the parsed AST and a list of the imported files.
///
/// The imports are under a separate Arc so that that can be shared with
/// the evaluation result (which needs the imports but no longer needs the AST).
pub struct ParseData(
    pub AstModule,
    pub Arc<Vec<(Option<FileSpan>, OwnedStarlarkModulePath)>>,
);

pub type ParseResult = Result<ParseData, buck2_error::Error>;

impl ParseData {
    fn new(
        ast: AstModule,
        implicit_imports: Vec<OwnedStarlarkModulePath>,
        resolver: &dyn LoadResolver,
    ) -> buck2_error::Result<Self> {
        let mut loads = implicit_imports
            .into_iter()
            .map(|x| (None, x))
            .collect::<Vec<_>>();
        for x in ast.loads() {
            let path = resolver
                .resolve_load(x.module_id, Some(&x.span))
                .with_buck_error_context(|| {
                    format!(
                        "Error loading `load` of `{}` from `{}`",
                        x.module_id, x.span
                    )
                })?;
            loads.push((Some(x.span), path));
        }
        Ok(Self(ast, Arc::new(loads)))
    }

    pub fn ast(&self) -> &AstModule {
        &self.0
    }

    pub fn imports(&self) -> &Arc<Vec<(Option<FileSpan>, OwnedStarlarkModulePath)>> {
        &self.1
    }
}

pub fn get_starlark_warning_link() -> &'static str {
    if buck2_core::is_open_source() {
        "https://buck2.build/docs/users/faq/starlark_peak_mem"
    } else {
        "https://fburl.com/starlark_peak_mem_warning"
    }
}
/// Interpreter for build files.
///
/// The Interpreter is responsible for parsing files to an AST and then
/// evaluating that AST. The Interpreter doesn't maintain state or cache results
/// of parsing or loading imports.
#[derive(Allocative, Pagable)]
pub(crate) struct InterpreterForDir {
    /// Non-cell-specific information.
    global_state: Arc<GlobalInterpreterState>,
    /// Cell-specific alias resolver.
    cell_info: InterpreterCellInfo,
    /// Log GC.
    verbose_gc: bool,
    /// When true, rule function creates a node with no attributes.
    /// (Which won't work correctly, but useful for profiling of starlark).
    ignore_attrs_for_profiling: bool,
    /// Implicit imports. These are only used for build files (e.g. `BUCK`),
    /// not for `bzl` or other files, because we only have implicit imports for build files.
    implicit_import_paths: Arc<ImplicitImportPaths>,
    /// Enable relative imports for the current dir
    current_dir_with_allowed_relative_dirs: Arc<CellPathWithAllowedRelativeDir>,
}

struct InterpreterLoadResolver {
    config: Arc<InterpreterForDir>,
    loader_file_type: StarlarkFileType,
    build_file_cell: BuildFileCell,
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum LoadResolutionError {
    #[error(
        "Cannot load `{0}`. Bxl loads are not allowed from within this context. bxl files can only be loaded from other bxl files."
    )]
    BxlLoadNotAllowed(CellPath),
    #[error("The `load` at {location} of `{got}` should use the canonical name `{wanted}`")]
    WrongCell {
        got: CellPath,
        wanted: CellPath,
        location: String,
    },
}

impl LoadResolver for InterpreterLoadResolver {
    fn resolve_load(
        &self,
        path: &str,
        location: Option<&FileSpan>,
    ) -> buck2_error::Result<OwnedStarlarkModulePath> {
        let relative_import_option = RelativeImports::Allow {
            current_dir_with_allowed_relative: &self.config.current_dir_with_allowed_relative_dirs,
        };
        let path = parse_import(
            self.config.cell_info.cell_alias_resolver(),
            relative_import_option,
            path,
        )?;

        // check for bxl files first before checking for prelude.
        // All bxl imports are parsed the same regardless of prelude or not.
        if path.path().extension() == Some("bxl") {
            match self.loader_file_type {
                StarlarkFileType::Bzl
                | StarlarkFileType::Buck
                | StarlarkFileType::Package
                | StarlarkFileType::Json
                | StarlarkFileType::Toml => {
                    return Err(LoadResolutionError::BxlLoadNotAllowed(path).into());
                }
                StarlarkFileType::Bxl => {
                    return Ok(OwnedStarlarkModulePath::BxlFile(BxlFilePath::new(path)?));
                }
            }
        }

        // If you load the same .bzl file twice via different aliases (e.g. fbcode//buck2/prelude/foo.bzl and prelude.bzl)
        // then anything doing pointer equality (t-sets, provider identities) will go wrong.
        let project_path = self.config.global_state.cell_resolver.resolve_path(path)?;
        let reformed_path = self
            .config
            .global_state
            .cell_resolver
            .get_cell_path(&project_path);
        if reformed_path.cell() != path.cell() {
            // We actually call resolve_load twice for each loadable - once with all load's up front,
            // then again on each one when we are loading. The second time we don't have a location,
            // so just omit the soft_error that time. Once it is a real error, we should real error on either.
            if let Some(location) = location {
                return Err(LoadResolutionError::WrongCell {
                    got: path,
                    wanted: reformed_path,
                    location: location.to_string(),
                }
                .into());
            }
        }

        // If importing from the prelude, then do not let that inherit the configuration. This
        // ensures that if you define a UDR outside of the prelude's cell, it gets the same prelude
        // as using the exported rules from the prelude would. This matters notably for identity
        // checks in t-sets, which would fail if we had > 1 copy of the prelude.
        if let Some(prelude_import) = self.config.global_state.configuror.prelude_import() {
            if prelude_import.is_prelude_path(&path) {
                if path.path().extension() == Some("json") {
                    return Ok(OwnedStarlarkModulePath::JsonFile(
                        ImportPath::new_same_cell(path)?,
                    ));
                } else {
                    return Ok(OwnedStarlarkModulePath::LoadFile(
                        ImportPath::new_same_cell(path)?,
                    ));
                }
            }
        }
        let import_path = ImportPath::new_with_build_file_cells(path, self.build_file_cell)?;
        Ok(match import_path.path().path().extension() {
            Some("json") => OwnedStarlarkModulePath::JsonFile(import_path),
            Some("toml") => OwnedStarlarkModulePath::TomlFile(import_path),
            _ => OwnedStarlarkModulePath::LoadFile(import_path),
        })
    }
}

struct EvalResult {
    additional: PerFileTypeContext,
    starlark_peak_allocated_byte_limit: OnceCell<Option<u64>>,
    is_profiling_enabled: bool,
    cpu_instruction_count: Option<u64>,
    starlark_tick_count: u64,
}

impl InterpreterForDir {
    fn verbose_gc() -> buck2_error::Result<bool> {
        match std::env::var_os("BUCK2_STARLARK_VERBOSE_GC") {
            Some(val) => Ok(!val.is_empty()),
            None => Ok(false),
        }
    }

    fn is_ignore_attrs_for_profiling() -> buck2_error::Result<bool> {
        // If unsure, feel free to break this code or just delete it.
        // It is intended only for profiling of very specific use cases.
        let ignore_attrs_for_profiling = match std::env::var_os("BUCK2_IGNORE_ATTRS_FOR_PROFILING")
        {
            Some(val) => !val.is_empty(),
            None => false,
        };
        if ignore_attrs_for_profiling {
            // This messages is printed in each run once per cell.
            // Somewhat inconvenient, but it is safe.
            eprintln!("Ignoring rule attributes");
        }
        Ok(ignore_attrs_for_profiling)
    }

    //, configuror: Arc<dyn InterpreterConfigurer>
    pub(crate) fn new(
        cell_info: InterpreterCellInfo,
        global_state: Arc<GlobalInterpreterState>,
        implicit_import_paths: Arc<ImplicitImportPaths>,
        current_dir_with_allowed_relative_dirs: Arc<CellPathWithAllowedRelativeDir>,
    ) -> buck2_error::Result<Self> {
        Ok(Self {
            global_state,
            cell_info,
            verbose_gc: Self::verbose_gc()?,
            ignore_attrs_for_profiling: Self::is_ignore_attrs_for_profiling()?,
            implicit_import_paths,
            current_dir_with_allowed_relative_dirs,
        })
    }

    fn create_env<'v>(
        &self,
        env: BuckStarlarkModule<'v>,
        starlark_path: StarlarkPath<'_>,
        loaded_modules: &LoadedModules,
    ) -> buck2_error::Result<BuckStarlarkModule<'v>> {
        if let Some(prelude_import) = self.prelude_import(starlark_path) {
            let prelude_env = loaded_modules
                .map
                .get(&StarlarkModulePath::LoadFile(prelude_import.import_path()))
                .ok_or_else(|| {
                    internal_error!(
                        "Should've had an env for the prelude import `{prelude_import}`"
                    )
                })?;
            env.import_public_symbols(prelude_env.env());
            if let StarlarkPath::BuildFile(_) = starlark_path {
                if let Some(native) = prelude_env.native_globals_for_buck_files()? {
                    for (name, value) in native.add_to_heap(env.heap()).iter() {
                        env.set(name.as_str(), value);
                    }
                }
            }
        }

        env.set_extra_value_no_overwrite(
            env.heap()
                .alloc_representation(StarlarkAnyComplex {
                    value: InterpreterExtraValue::default(),
                })
                .to_value(),
        )
        .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Interpreter))?;

        Ok(env)
    }

    // The environment for evaluating a build file contains additional information
    // to support the extra things available in that context. For example, rule
    // functions can be invoked when evaluating a build file, the package (cell
    // + path) is available. It also includes the implicit root include and
    // implicit package include.
    fn create_build_env<'v>(
        &self,
        env: BuckStarlarkModule<'v>,
        build_file: &BuildFilePath,
        package_listing: &PackageListing,
        super_package: SuperPackage,
        loaded_modules: &LoadedModules,
    ) -> buck2_error::Result<(BuckStarlarkModule<'v>, ModuleInternals)> {
        let internals = self.global_state.configuror.new_extra_context(
            &self.cell_info,
            build_file.clone(),
            package_listing.dupe(),
            super_package,
            loaded_modules,
            self.package_import(build_file),
            self.current_dir_with_allowed_relative_dirs
                .as_ref()
                .to_owned(),
        )?;
        let env = self.create_env(env, StarlarkPath::BuildFile(build_file), loaded_modules)?;

        if let Some(root_import) = self.root_import() {
            let root_env = loaded_modules
                .map
                .get(&StarlarkModulePath::LoadFile(&root_import))
                .ok_or_else(|| {
                    internal_error!("Should've had an env for the root import `{root_import}`")
                })?
                .env();
            env.import_public_symbols(root_env);
        }

        Ok((env, internals))
    }

    fn load_resolver(
        self: &Arc<Self>,
        current_file_path: StarlarkPath<'_>,
    ) -> InterpreterLoadResolver {
        InterpreterLoadResolver {
            config: self.dupe(),
            loader_file_type: current_file_path.file_type(),
            build_file_cell: current_file_path.build_file_cell(),
        }
    }

    fn package_import(&self, build_file_import: &BuildFilePath) -> Option<&Arc<ImplicitImport>> {
        self.implicit_import_paths
            .package_imports
            .get(build_file_import.package())
    }

    fn root_import(&self) -> Option<ImportPath> {
        self.implicit_import_paths.root_import.clone()
    }

    fn prelude_import(&self, import: StarlarkPath) -> Option<&PreludePath> {
        let prelude_import = self.global_state.configuror.prelude_import();
        if let Some(prelude_import) = prelude_import {
            let import_path = import.path();

            match import {
                StarlarkPath::BuildFile(_)
                | StarlarkPath::PackageFile(_)
                | StarlarkPath::BxlFile(_) => return Some(prelude_import),
                StarlarkPath::LoadFile(_) => {
                    if !prelude_import.is_prelude_path(&import_path) {
                        return Some(prelude_import);
                    }
                }
                StarlarkPath::JsonFile(_) | StarlarkPath::TomlFile(_) => return None,
            }
        }

        None
    }

    /// Parses skylark code to an AST.
    pub(crate) fn parse(
        self: &Arc<Self>,
        import: StarlarkPath,
        content: String,
    ) -> buck2_error::Result<ParseResult> {
        // Indentation with tabs is prohibited by starlark spec and configured starlark dialect.
        // This check also prohibits tabs even where spaces are not significant,
        // for example inside parentheses in function call arguments,
        // which restricts what the spec allows.
        if content.contains('\t') {
            return Err(StarlarkTabsError(OwnedStarlarkPath::new(import)).into());
        }

        let project_relative_path = self
            .global_state
            .cell_resolver
            .resolve_path(*import.path())?;

        let disable_starlark_types = self.global_state.disable_starlark_types;
        let ast = match AstModule::parse(
            project_relative_path.as_str(),
            content,
            &import.file_type().dialect(disable_starlark_types),
        ) {
            Ok(ast) => ast,
            Err(e) => {
                return Ok(Err(buck2_error::Error::from(e).context(format!(
                    "Error parsing: `{}`",
                    OwnedStarlarkPath::new(import)
                ))));
            }
        };
        let mut implicit_imports = Vec::new();
        if let Some(i) = self.prelude_import(import) {
            implicit_imports.push(OwnedStarlarkModulePath::LoadFile(i.import_path().clone()));
        }
        if let StarlarkPath::BuildFile(build_file) = import {
            if let Some(i) = self.package_import(build_file) {
                implicit_imports.push(OwnedStarlarkModulePath::LoadFile(i.import().clone()));
            }
            if let Some(i) = self.root_import() {
                implicit_imports.push(OwnedStarlarkModulePath::LoadFile(i));
            }
        }
        ParseData::new(ast, implicit_imports, &self.load_resolver(import)).map(Ok)
    }

    pub(crate) fn resolve_path(
        self: &Arc<Self>,
        import: StarlarkPath<'_>,
        import_string: &str,
    ) -> buck2_error::Result<OwnedStarlarkModulePath> {
        self.load_resolver(import).resolve_load(import_string, None)
    }

    fn eval(
        self: &Arc<Self>,
        env: &BuckStarlarkModule,
        ast: AstModule,
        buckconfigs: &mut dyn BuckConfigsViewForStarlark,
        loaded_modules: LoadedModules,
        extra_context: PerFileTypeContext,
        eval_provider: StarlarkEvaluatorProvider,
        unstable_typecheck: bool,
        cancellation: &CancellationContext,
    ) -> buck2_error::Result<(FinishedStarlarkEvaluation, EvalResult)> {
        let import = extra_context.starlark_path();
        let globals = self.global_state.globals();
        let file_loader =
            InterpreterFileLoader::new(loaded_modules, Arc::new(self.load_resolver(import)));
        let host_info = self.global_state.configuror.host_info();
        let extra = BuildContext::new(
            &self.cell_info,
            buckconfigs,
            host_info,
            extra_context,
            self.ignore_attrs_for_profiling,
            self.global_state.configuror.infer_target_names(),
        );

        let print = EventDispatcherPrintHandler(get_dispatcher());
        let (finished_eval, (cpu_instruction_count, starlark_tick_count, is_profiling_enabled)) =
            eval_provider.with_evaluator(
                env,
                cancellation.into(),
                |eval, is_profiling_enabled_by_provider| {
                    eval.enable_static_typechecking(unstable_typecheck);
                    eval.set_print_handler(&print);
                    eval.set_soft_error_handler(&Buck2StarlarkSoftErrorHandler);
                    eval.set_loader(&file_loader);
                    eval.extra = Some(&extra);
                    if self.verbose_gc {
                        eval.verbose_gc();
                    }

                    // Ignore error if failed to initialize instruction counter.
                    let instruction_counter: Option<PerThreadInstructionCounter> =
                        PerThreadInstructionCounter::init().ok().unwrap_or_default();

                    match eval.eval_module(ast, globals) {
                        Ok(_) => {
                            // Undersampled counts are mostly extrapolation
                            // (the PMU was contended); report nothing rather
                            // than a number that is largely a guess.
                            let cpu_instruction_count = instruction_counter
                                .and_then(|c| c.collect().ok())
                                .flatten()
                                .filter(|c| c.get_sampling_ratio() >= 0.5)
                                .map(|c| c.count);
                            let starlark_tick_count = eval.get_total_tick_count();
                            Ok((
                                cpu_instruction_count,
                                starlark_tick_count,
                                is_profiling_enabled_by_provider,
                            ))
                        }
                        Err(p) => Err(p.into()),
                    }
                },
            )?;
        Ok((
            finished_eval,
            EvalResult {
                additional: extra.additional,
                is_profiling_enabled,
                starlark_peak_allocated_byte_limit: extra.starlark_peak_allocated_byte_limit,
                cpu_instruction_count,
                starlark_tick_count,
            },
        ))
    }

    /// Evaluates the AST for a parsed module. Loaded modules must contain the loaded
    /// environment for all (transitive) required imports.
    /// Returns the FrozenModule for the module.
    pub(crate) fn eval_module(
        self: &Arc<Self>,
        starlark_path: StarlarkModulePath<'_>,
        buckconfigs: &mut dyn BuckConfigsViewForStarlark,
        ast: AstModule,
        loaded_modules: LoadedModules,
        eval_provider: StarlarkEvaluatorProvider,
        cancellation: &CancellationContext,
    ) -> buck2_error::Result<FrozenModule> {
        BuckStarlarkModule::with_profiling(|env| {
            let env = self.create_env(env, starlark_path.into(), &loaded_modules)?;
            let extra_context = match starlark_path {
                StarlarkModulePath::LoadFile(bzl) => PerFileTypeContext::Bzl(BzlEvalCtx {
                    bzl_path: bzl.clone(),
                }),
                StarlarkModulePath::BxlFile(bxl) => PerFileTypeContext::Bxl(bxl.clone()),
                StarlarkModulePath::JsonFile(j) => PerFileTypeContext::Json(j.clone()),
                StarlarkModulePath::TomlFile(t) => PerFileTypeContext::Toml(t.clone()),
            };
            let typecheck = self.global_state.unstable_typecheck
                || matches!(starlark_path, StarlarkModulePath::BxlFile(..))
                || match self.global_state.configuror.prelude_import() {
                    Some(prelude_import) => {
                        prelude_import.prelude_cell()
                            == self.cell_info.cell_alias_resolver().resolve_self()
                    }
                    None => false,
                };
            let (finished_eval, _) = self.eval(
                &env,
                ast,
                buckconfigs,
                loaded_modules,
                extra_context,
                eval_provider,
                typecheck,
                cancellation,
            )?;
            let (token, frozen, _) = finished_eval.freeze_and_finish(env)?;

            Ok((token, frozen))
        })
    }

    pub(crate) fn eval_package_file(
        self: &Arc<Self>,
        package_file_path: &PackageFilePath,
        ast: AstModule,
        parent: SuperPackage,
        buckconfigs: &mut dyn BuckConfigsViewForStarlark,
        loaded_modules: LoadedModules,
        eval_provider: StarlarkEvaluatorProvider,
        cancellation: &CancellationContext,
        package_visibility_default_intersection: PackageVisibilityDefaultIntersection,
    ) -> buck2_error::Result<SuperPackage> {
        BuckStarlarkModule::with_profiling(|env| {
            let env = self.create_env(
                env,
                StarlarkPath::PackageFile(package_file_path),
                &loaded_modules,
            )?;

            let extra_context = PerFileTypeContext::Package(PackageFileEvalCtx {
                path: package_file_path.clone(),
                parent,
                visibility: RefCell::new(None),
                test_config_unification_rollout: RefCell::new(None),
                enforces_visibility_intersection: RefCell::new(false),
                enforces_within_view_intersection: RefCell::new(false),
                package_visibility_default_intersection,
            });

            let (finished_eval, eval_result) = self.eval(
                &env,
                ast,
                buckconfigs,
                loaded_modules,
                extra_context,
                eval_provider,
                false,
                cancellation,
            )?;

            let per_file_context = eval_result.additional;

            let (token, extra) = if InterpreterExtraValue::get(&env)?
                .package_extra
                .get()
                .is_some()
            {
                // Only freeze if there's something to freeze, otherwise we will needlessly freeze
                // globals. TODO(nga): add API to only freeze extra.
                let (token, frozen, _) = finished_eval.freeze_and_finish(env)?;
                (token, FrozenPackageFileExtra::get(&frozen)?)
            } else {
                let (token, _) = finished_eval.finish()?;
                (token, None)
            };

            let package_file_eval_ctx = per_file_context.into_package_file()?;

            let modifier_labels = ModifierLabelParser::new(&self.global_state.cell_resolver);
            Ok((
                token,
                package_file_eval_ctx.build_super_package(extra, &modifier_labels)?,
            ))
        })
    }

    /// Evaluates the AST for a parsed build file. Loaded modules must contain the
    /// loaded environment for all (transitive) required imports.
    /// Returns the result of evaluation.
    pub(crate) fn eval_build_file(
        self: &Arc<Self>,
        build_file: &BuildFilePath,
        buckconfigs: &mut dyn BuckConfigsViewForStarlark,
        listing: PackageListing,
        super_package: SuperPackage,
        ast: AstModule,
        loaded_modules: LoadedModules,
        eval_provider: StarlarkEvaluatorProvider,
        unstable_typecheck: bool,
        cancellation: &CancellationContext,
    ) -> buck2_error::Result<(
        Option<Arc<StarlarkProfileDataAndStats>>,
        EvaluationResultWithStats,
    )> {
        BuckStarlarkModule::with_profiling(|env| {
            let (env, internals) =
                self.create_build_env(env, build_file, &listing, super_package, &loaded_modules)?;
            let buckconfig_key = BuckconfigKeyRef {
                section: "buck2",
                property: "check_starlark_peak_memory",
            };
            let starlark_peak_mem_config_enabled = LegacyBuckConfig::parse_value(
                buckconfig_key,
                buckconfigs
                    .read_root_cell_config(buckconfig_key)?
                    .as_deref(),
            )?
            .unwrap_or(false);

            let (finished_eval, eval_result) = self.eval(
                &env,
                ast,
                buckconfigs,
                loaded_modules,
                PerFileTypeContext::Build(internals),
                eval_provider,
                unstable_typecheck,
                cancellation,
            )?;

            let internals = eval_result.additional.into_build()?;
            let starlark_peak_allocated_bytes = env.heap().peak_allocated_bytes() as u64;
            let starlark_peak_mem_check_enabled =
                !eval_result.is_profiling_enabled && starlark_peak_mem_config_enabled;
            let starlark_mem_limit = eval_result
                .starlark_peak_allocated_byte_limit
                .get()
                .and_then(|limit| *limit)
                .unwrap_or(DEFAULT_STARLARK_MEMORY_USAGE_LIMIT);

            if starlark_peak_mem_check_enabled && starlark_peak_allocated_bytes > starlark_mem_limit
            {
                Err(StarlarkPeakMemoryError::ExceedsThreshold(
                    build_file.to_owned(),
                    HumanizedBytes::fixed_width(starlark_peak_allocated_bytes),
                    HumanizedBytes::fixed_width(starlark_mem_limit),
                    get_starlark_warning_link().to_owned(),
                )
                .into())
            } else {
                let (token, profile_data) = finished_eval.finish()?;

                let mut result = EvaluationResult::from(internals);
                result.starlark_peak_allocated_bytes = starlark_peak_allocated_bytes;

                Ok((
                    token,
                    (
                        profile_data,
                        EvaluationResultWithStats {
                            result,
                            starlark_peak_allocated_bytes,
                            cpu_instruction_count: eval_result.cpu_instruction_count,
                            starlark_tick_count: eval_result.starlark_tick_count,
                        },
                    ),
                ))
            }
        })
    }
}
