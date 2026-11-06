/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::iter::empty;
use std::time::Duration;

use allocative::Allocative;
use buck2_build_api_derive::internal_provider;
use buck2_core::execution_types::execution_group::ExecutionGroup;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::buck2_error;
use buck2_error::internal_error;
use buck2_hash::BuckMutInsertionMap;
use buck2_interpreter::execution_group::ExecutionGroupArg;
use buck2_interpreter::types::configured_providers_label::StarlarkConfiguredProvidersLabel;
use buck2_test_api::data::TestListingEntry;
use buck2_test_api::data::TestListingPresetResult;
use buck2_test_api::data::TestResultEntry;
use buck2_test_api::data::TestStatus;
use either::Either;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Module;
use starlark::eval::Evaluator;
use starlark::values::Freeze;
use starlark::values::FreezeError;
use starlark::values::OwnedFrozen;
use starlark::values::StarlarkPagable;
use starlark::values::Trace;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueOfUnchecked;
use starlark::values::ValueTyped;
use starlark::values::dict::DictRef;
use starlark::values::dict::DictType;
use starlark::values::float::StarlarkFloat;
use starlark::values::list::ListRef;
use starlark::values::list::ListType;
use starlark::values::none::NoneOr;
use starlark::values::none::NoneType;
use starlark::values::typing::StarlarkCallable;

use crate as buck2_build_api;
use crate::interpreter::rule_defs::cmd_args::CommandLineArgLike;
use crate::interpreter::rule_defs::cmd_args::CommandLineArtifactVisitor;
use crate::interpreter::rule_defs::command_executor_config::StarlarkCommandExecutorConfig;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::TestCommandMember;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::check_all;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::default_executor_with_execution_group;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::iter_executor_overrides;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::iter_local_resources;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::iter_opt_str_list;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::iter_test_command;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::iter_test_env;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::iter_value;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::unwrap_all;
use crate::interpreter::rule_defs::provider::builtin::worker_info::WorkerInfo;
use crate::interpreter::rule_defs::required_test_local_resource::StarlarkRequiredTestLocalResource;

/// Provider that signals that a rule can be tested using Buck2's internal test
/// runner, bypassing the external TPX runner. This provider has the same API as
/// `ExternalRunnerTestInfo` but uses a different execution strategy.
#[internal_provider(internal_runner_test_info_creator)]
#[derive(
    Clone,
    Debug,
    Trace,
    Freeze,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[freeze(validator = validate_internal_runner_test_info)]
#[repr(C)]
pub struct InternalRunnerTestInfo<'v> {
    /// A Starlark value representing the type of this test.
    test_type: ValueOfUnchecked<'v, String>,

    /// A Starlark value representing the command for this test. The test runner is what
    /// gives meaning to this command.
    command: ValueOfUnchecked<'v, Vec<Either<String, Value<'static>>>>,

    /// A Starlark value representing the command used for test discovery (listing).
    /// This is the command that runs the test binary with framework-specific listing
    /// flags (e.g., `["binary", "--gtest_list_tests"]` for GTest). The internal runner
    /// uses this for the listing step, while `command` is used for execution.
    listing_command: ValueOfUnchecked<'v, Vec<Either<String, Value<'static>>>>,

    /// A Starlark value representing the environment for this test.
    /// This is of type `dict[str, ArgLike]`.
    env: ValueOfUnchecked<'v, DictType<String, Value<'static>>>,

    /// A starlark value representing the labels for this test.
    labels: ValueOfUnchecked<'v, Vec<String>>,

    /// A starlark value representing the contacts for this test. This is largely expected to be an
    /// oncall, though it's not validated in any way.
    contacts: ValueOfUnchecked<'v, Vec<String>>,

    /// Whether this test should use relative paths
    use_project_relative_paths: ValueOfUnchecked<'v, bool>,

    /// Whether this test should run from the project root, as opposed to the cell root
    ///
    /// Defaults to `True`.
    run_from_project_root: ValueOfUnchecked<'v, bool>,

    /// Default executor to use to run tests. If none is
    /// passed we will default to the execution platform.
    default_executor: Option<ValueTyped<'v, StarlarkCommandExecutorConfig>>,

    /// The execution group whose execution platform runs the tests when the test runner does not
    /// request an executor. When the analysis of a rule that declares the group creates the test
    /// info, the group's executor is recorded in `default_executor`, so it still applies when
    /// another rule returns the test info. Nothing is recorded when the group has no compatible
    /// execution platform, when the rule does not declare the group, or outside rule analysis; the
    /// test then resolves the group against the tested target when it runs. A passed
    /// `default_executor` wins and the group is not resolved, so copying a test info field by
    /// field works in any rule. A rule that declares named groups but not `DefaultExecGroup` must
    /// set `exec_group` or `default_executor`; `executor_overrides` does not replace them, as the
    /// tests run in the default group whenever no override is picked.
    exec_group: ValueOfUnchecked<'v, NoneOr<ExecutionGroupArg>>,

    /// Executors that can be used to override the default executor.
    executor_overrides: ValueOfUnchecked<'v, DictType<String, StarlarkCommandExecutorConfig>>,

    /// Mapping from a local resource type to a target with a corresponding provider.
    /// Required types are passed from test runner.
    /// If the value for a corresponding type is omitted it means local resource
    /// should be ignored when executing tests even if those are passed as required from test runner.
    local_resources:
        ValueOfUnchecked<'v, DictType<String, Option<StarlarkConfiguredProvidersLabel>>>,

    /// List of local resource types which should be set up additionally to those which are
    /// passed from test runner. Allows specifying local resources on a per-rule basis.
    required_local_resources: ValueOfUnchecked<'v, Vec<StarlarkRequiredTestLocalResource>>,

    /// Configuration needed to spawn a new worker. This worker will be used to run every single
    /// command related to test execution, including listing.
    worker: Option<ValueTyped<'v, WorkerInfo<'v>>>,

    /// A Starlark callable that parses test listing output into structured test
    /// entries. The callback signature is:
    ///
    /// ```python
    /// def parse_test_listing(listing_content: str) -> list[dict[str, ...]]:
    ///     """Returns list of dicts with keys:
    ///         "name": str              — human-readable display name
    ///         "filter": str            — argument to select this test for execution
    ///         "status": str | None     — "SKIP" to report the test as skipped without running it
    ///         "message": str | None    — reason reported with `status`; requires `status`
    ///     """
    /// ```
    parse_test_listing: ValueOfUnchecked<
        'v,
        StarlarkCallable<'static, (String,), ListType<DictType<String, Value<'static>>>>,
    >,

    /// A Starlark callable that parses test execution output into structured
    /// per-test results. The callback signature is:
    ///
    /// ```python
    /// def parse_test_result(stdout: str, stderr: str, exit_code: int) -> list[dict[str, ...]]:
    ///     """Returns list of dicts with keys:
    ///         "name": str                — test name (should match listing name)
    ///         "status": str              — "PASS", "FAIL", "SKIP", "TIMEOUT", "OMITTED", etc.
    ///         "message": str | None      — failure/skip reason
    ///         "duration": float | None      — duration of this test case
    ///         "details": str | None      — full diagnostic output
    ///     """
    /// ```
    parse_test_result: ValueOfUnchecked<
        'v,
        StarlarkCallable<
            'static,
            (String, String, i32),
            ListType<DictType<String, Value<'static>>>,
        >,
    >,
}

/// An `InternalRunnerTestInfo` kept alive by its owning frozen heap; usable across threads and
/// awaits.
pub type OwnedInternalRunnerTestInfo =
    OwnedFrozen<ValueTyped<'static, InternalRunnerTestInfo<'static>>>;

// NOTE: All the methods here unwrap because we validate at freeze time.
impl<'v> InternalRunnerTestInfo<'v> {
    pub fn test_type(&self) -> &'v str {
        self.test_type.get().unpack_str().unwrap()
    }

    pub fn command(&self) -> impl Iterator<Item = TestCommandMember<'v>> {
        unwrap_all(iter_test_command(self.command.get()))
    }

    pub fn listing_command(&self) -> impl Iterator<Item = TestCommandMember<'v>> {
        unwrap_all(iter_test_command(self.listing_command.get()))
    }

    pub fn env(&self) -> impl Iterator<Item = (&'v str, &'v dyn CommandLineArgLike<'v>)> {
        unwrap_all(iter_test_env(self.env.get()))
    }

    pub fn labels(&self) -> impl Iterator<Item = &'v str> {
        unwrap_all(iter_opt_str_list(self.labels.get(), "labels"))
    }

    pub fn contacts(&self) -> impl Iterator<Item = &'v str> {
        unwrap_all(iter_opt_str_list(self.contacts.get(), "contacts"))
    }

    pub fn use_project_relative_paths(&self) -> bool {
        NoneOr::<bool>::unpack_value(self.use_project_relative_paths.get())
            .unwrap()
            .unwrap()
            .into_option()
            .unwrap_or_else(buck2_core::is_open_source)
    }

    pub fn run_from_project_root(&self) -> bool {
        NoneOr::<bool>::unpack_value(self.run_from_project_root.get())
            .unwrap()
            .unwrap()
            .into_option()
            .unwrap_or(true)
    }

    pub fn default_executor(&self) -> Option<&'v StarlarkCommandExecutorConfig> {
        self.default_executor.map(|v| v.as_ref())
    }

    pub fn execution_group(&self) -> buck2_error::Result<Option<ExecutionGroup>> {
        Ok(self
            .exec_group
            .unpack()?
            .into_option()
            .map(|ExecutionGroupArg { group }| group))
    }

    pub fn has_executor_overrides(&self) -> bool {
        !self.executor_overrides.get().is_none()
    }

    /// Access a specific executor override.
    pub fn executor_override(&self, key: &str) -> Option<&'v StarlarkCommandExecutorConfig> {
        let executor_overrides = DictRef::from_value(self.executor_overrides.get())?;
        executor_overrides
            .get_str(key)
            .map(|v| StarlarkCommandExecutorConfig::from_value(v.to_value()).unwrap())
    }

    pub fn local_resources(
        &self,
    ) -> BuckMutInsertionMap<&'v str, Option<&'v ConfiguredProvidersLabel>> {
        unwrap_all(iter_local_resources(self.local_resources.get())).collect()
    }

    pub fn required_local_resources(
        &self,
    ) -> impl Iterator<Item = &'v StarlarkRequiredTestLocalResource> {
        let val = self.required_local_resources.get();
        if val.is_none() {
            return Either::Left(empty());
        }
        Either::Right(
            iter_value(val)
                .expect("checked during construction")
                .map(|v| {
                    StarlarkRequiredTestLocalResource::from_value(v)
                        .expect("checked during construction")
                }),
        )
    }

    pub fn worker(&self) -> Option<&'v WorkerInfo<'v>> {
        self.worker.map(|v| v.as_ref())
    }

    /// Invoke the `parse_test_listing` Starlark callback with raw listing
    /// output and return structured `TestListingEntry` values.
    ///
    /// The callback runs on a scratch module, which needs to reference the provider's heap; hence
    /// the owner rather than a view.
    pub fn parse_test_listing_output(
        provider: &OwnedInternalRunnerTestInfo,
        listing_content: &str,
    ) -> buck2_error::Result<Vec<TestListingEntry>> {
        // calling frozen callback
        // ast-grep-ignore: rust/buck2-no-starlark-module
        Module::with_temp_heap(|env| {
            let heap = env.heap();
            let mut eval = Evaluator::new(&env);
            let callback = provider
                .as_ref()
                .add_to_heap(heap)
                .as_ref()
                .parse_test_listing
                .get();

            let listing_arg = heap.alloc(listing_content);
            let result = eval
                .eval_function(callback, &[listing_arg], &[])
                .map_err(|e| buck2_error!(buck2_error::ErrorTag::Input, "{}", e))?;

            let list = ListRef::from_value(result).ok_or_else(|| {
                buck2_error!(
                    buck2_error::ErrorTag::Input,
                    "parse_test_listing must return a list, got: `{}`",
                    result
                )
            })?;

            list.iter()
                .map(|item| {
                    let dict = DictRef::from_value(item).ok_or_else(|| {
                        buck2_error!(
                            buck2_error::ErrorTag::Input,
                            "parse_test_listing items must be dicts, got: `{}`",
                            item
                        )
                    })?;

                    let get_required_str = |key: &str| -> buck2_error::Result<String> {
                        match dict.get_str(key) {
                            Some(v) => {
                                let s = v.unpack_str().ok_or_else(|| {
                                    buck2_error!(
                                        buck2_error::ErrorTag::Input,
                                        "`{}` must be a string, got: `{}`",
                                        key,
                                        v
                                    )
                                })?;
                                Ok(s.to_owned())
                            }
                            None => Err(buck2_error!(
                                buck2_error::ErrorTag::Input,
                                "parse_test_listing dict missing required key `{}`",
                                key
                            )),
                        }
                    };

                    let name = get_required_str("name")?;
                    let preset_result = parse_listing_preset_result(&dict)
                        .with_buck_error_context(|| format!("In listing entry `{}`", name))?;

                    Ok(TestListingEntry {
                        name,
                        filter: get_required_str("filter")?,
                        preset_result,
                    })
                })
                .collect()
        })
    }

    pub fn visit_artifacts(
        &self,
        visitor: &mut dyn CommandLineArtifactVisitor<'v>,
    ) -> buck2_error::Result<()> {
        for member in self.command() {
            match member {
                TestCommandMember::Literal(..) => {}
                TestCommandMember::Arglike(arglike) => {
                    arglike.visit_artifacts(visitor)?;
                }
            }
        }

        for member in self.listing_command() {
            match member {
                TestCommandMember::Literal(..) => {}
                TestCommandMember::Arglike(arglike) => {
                    arglike.visit_artifacts(visitor)?;
                }
            }
        }

        for (_, arglike) in self.env() {
            arglike.visit_artifacts(visitor)?;
        }

        Ok(())
    }

    /// Invoke the `parse_test_result` Starlark callback with raw execution
    /// output and return structured `TestResultEntry` values.
    ///
    /// The callback runs on a scratch module, which needs to reference the provider's heap; hence
    /// the owner rather than a view.
    pub fn parse_test_result_output(
        provider: &OwnedInternalRunnerTestInfo,
        stdout: &str,
        stderr: &str,
        exit_code: i32,
    ) -> buck2_error::Result<Vec<TestResultEntry>> {
        // calling frozen callback
        // ast-grep-ignore: rust/buck2-no-starlark-module
        Module::with_temp_heap(|env| {
            let heap = env.heap();
            let mut eval = Evaluator::new(&env);
            let callback = provider
                .as_ref()
                .add_to_heap(heap)
                .as_ref()
                .parse_test_result
                .get();

            let stdout_arg = heap.alloc(stdout);
            let stderr_arg = heap.alloc(stderr);
            let exit_code_arg = heap.alloc(exit_code);
            let result = eval
                .eval_function(callback, &[stdout_arg, stderr_arg, exit_code_arg], &[])
                .map_err(|e| buck2_error!(buck2_error::ErrorTag::Input, "{}", e))?;

            let list = ListRef::from_value(result).ok_or_else(|| {
                buck2_error!(
                    buck2_error::ErrorTag::Input,
                    "parse_test_result must return a list, got: `{}`",
                    result
                )
            })?;

            list.iter()
                .map(|item| {
                    let dict = DictRef::from_value(item).ok_or_else(|| {
                        buck2_error!(
                            buck2_error::ErrorTag::Input,
                            "parse_test_result items must be dicts, got: `{}`",
                            item
                        )
                    })?;

                    let get_required_str = |key: &str| -> buck2_error::Result<String> {
                        get_opt_str(&dict, key)?.ok_or_else(|| {
                            buck2_error!(
                                buck2_error::ErrorTag::Input,
                                "parse_test_result dict missing required key `{}`",
                                key
                            )
                        })
                    };

                    let name = get_required_str("name")?;
                    let status_str = get_required_str("status")?;
                    let status = TestStatus::parse(&status_str)?;
                    let message = get_opt_str(&dict, "message")?;
                    let details = get_opt_str(&dict, "details")?;

                    let duration = match dict.get_str("duration") {
                        Some(v) => {
                            if v.is_none() {
                                None
                            } else if let Some(f) = v.downcast_ref::<StarlarkFloat>() {
                                if !f.0.is_finite() || f.0 < 0.0 {
                                    return Err(buck2_error!(
                                        buck2_error::ErrorTag::Input,
                                        "`duration` must be a finite non-negative number, got: `{}`",
                                        f.0
                                    ));
                                }
                                Some(Duration::from_secs_f64(f.0))
                            } else if let Some(i) = v.unpack_i32() {
                                if i < 0 {
                                    return Err(buck2_error!(
                                        buck2_error::ErrorTag::Input,
                                        "`duration` must be non-negative, got: `{}`",
                                        i
                                    ));
                                }
                                Some(Duration::from_secs_f64(i as f64))
                            } else {
                                return Err(buck2_error!(
                                    buck2_error::ErrorTag::Input,
                                    "`duration` must be a number, got: `{}`",
                                    v
                                ));
                            }
                        }
                        None => None,
                    };

                    Ok(TestResultEntry {
                        name,
                        status,
                        message,
                        duration,
                        details,
                    })
                })
                .collect()
        })
    }
}

fn get_opt_str(dict: &DictRef<'_>, key: &str) -> buck2_error::Result<Option<String>> {
    let Some(value) = dict.get_str(key) else {
        return Ok(None);
    };
    Ok(NoneOr::<&str>::unpack_value(value)?
        .ok_or_else(|| {
            buck2_error!(
                buck2_error::ErrorTag::Input,
                "`{}` must be a string, got: `{}`",
                key,
                value
            )
        })?
        .into_option()
        .map(str::to_owned))
}

fn parse_listing_preset_result(
    dict: &DictRef<'_>,
) -> buck2_error::Result<Option<TestListingPresetResult>> {
    match (get_opt_str(dict, "status")?, get_opt_str(dict, "message")?) {
        (None, None) => Ok(None),
        (None, Some(_)) => Err(buck2_error!(
            buck2_error::ErrorTag::Input,
            "parse_test_listing dict has `message` but no `status`"
        )),
        (Some(status), message) if status == "SKIP" => Ok(Some(TestListingPresetResult {
            status: TestStatus::SKIP,
            message,
        })),
        (Some(status), _) => Err(buck2_error!(
            buck2_error::ErrorTag::Input,
            "parse_test_listing may only set `status` to `SKIP`, got: `{}`",
            status
        )),
    }
}

fn validate_internal_runner_test_info<'v>(
    info: &InternalRunnerTestInfo<'v>,
) -> buck2_error::Result<()> {
    check_all(iter_test_command(info.command.get()))?;
    check_all(iter_test_command(info.listing_command.get()))?;
    // listing_command must be non-empty — it's the command used for test discovery.
    let listing_cmd_val = info.listing_command.get();
    if listing_cmd_val.is_none() {
        return Err(buck2_error!(
            buck2_error::ErrorTag::Input,
            "`listing_command` is required for InternalRunnerTestInfo"
        ));
    }
    if let Some(list) = ListRef::from_value(listing_cmd_val) {
        if list.is_empty() {
            return Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "`listing_command` must be non-empty"
            ));
        }
    }
    check_all(iter_test_env(info.env.get()))?;
    check_all(iter_opt_str_list(info.labels.get(), "labels"))?;
    check_all(iter_opt_str_list(info.contacts.get(), "contacts"))?;
    check_all(iter_executor_overrides(info.executor_overrides.get()))?;

    let provided_local_resources = iter_local_resources(info.local_resources.get())
        .collect::<buck2_error::Result<
        BuckMutInsertionMap<&str, Option<&ConfiguredProvidersLabel>>,
    >>()?;

    let required_local_resources = info.required_local_resources.get();
    if !required_local_resources.is_none() {
        for resource_type in iter_value(required_local_resources).buck_error_context("`required_local_resources` should be a list or a tuple of `RequiredTestLocalResource` objects")? {
            let resource_type = StarlarkRequiredTestLocalResource::from_value(resource_type)
                .ok_or_else(|| buck2_error!(buck2_error::ErrorTag::Input, "`required_local_resources` should only contain `RequiredTestLocalResource` values, got {}", resource_type))?;
            if !provided_local_resources.contains_key(&resource_type.name as &str) {
                return Err(buck2_error!(
                    buck2_error::ErrorTag::Input,
                    "`required_local_resources` contains `{}` which is not present in `local_resources`",
                    resource_type.name
                ));
            }
        }
    }

    NoneOr::<bool>::unpack_value(info.use_project_relative_paths.get())?.ok_or_else(|| {
        internal_error!("`use_project_relative_paths` must be a bool if provided")
    })?;
    NoneOr::<bool>::unpack_value(info.run_from_project_root.get())?
        .internal_error("`run_from_project_root` must be a bool if provided")?;

    // Both parse_test_listing and parse_test_result are required callables.
    let ptl = info.parse_test_listing.get();
    if ptl.is_none() {
        return Err(buck2_error!(
            buck2_error::ErrorTag::Input,
            "`parse_test_listing` is required for InternalRunnerTestInfo"
        ));
    }
    NoneOr::<StarlarkCallable>::unpack_value(ptl)?
        .internal_error("`parse_test_listing` must be a callable")?;

    let ptr = info.parse_test_result.get();
    if ptr.is_none() {
        return Err(buck2_error!(
            buck2_error::ErrorTag::Input,
            "`parse_test_result` is required for InternalRunnerTestInfo"
        ));
    }
    NoneOr::<StarlarkCallable>::unpack_value(ptr)?
        .internal_error("`parse_test_result` must be a callable")?;

    info.test_type
        .get()
        .unpack_str()
        .internal_error("`type` must be a str")?;
    Ok(())
}

#[starlark_module]
fn internal_runner_test_info_creator(globals: &mut GlobalsBuilder) {
    #[starlark(as_type = InternalRunnerTestInfo<'static>)]
    fn InternalRunnerTestInfo<'v>(
        r#type: Value<'v>,
        parse_test_listing: Value<'v>,
        parse_test_result: Value<'v>,
        listing_command: Value<'v>,
        #[starlark(default = NoneType)] command: Value<'v>,
        #[starlark(default = NoneType)] env: Value<'v>,
        #[starlark(default = NoneType)] labels: Value<'v>,
        #[starlark(default = NoneType)] contacts: Value<'v>,
        #[starlark(default = NoneType)] use_project_relative_paths: Value<'v>,
        #[starlark(default = NoneType)] run_from_project_root: Value<'v>,
        #[starlark(default = NoneOr::None)] default_executor: NoneOr<
            ValueTyped<'v, StarlarkCommandExecutorConfig>,
        >,
        #[starlark(default = NoneType)] executor_overrides: Value<'v>,
        #[starlark(default = NoneType)] local_resources: Value<'v>,
        #[starlark(default = NoneType)] required_local_resources: Value<'v>,
        #[starlark(default = NoneOr::None)] worker: NoneOr<ValueTyped<'v, WorkerInfo<'v>>>,
        #[starlark(require = named, default = NoneOr::None)] exec_group: NoneOr<
            ValueOf<'v, ExecutionGroupArg>,
        >,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<InternalRunnerTestInfo<'v>> {
        let exec_group = exec_group.into_option();
        let default_executor = default_executor_with_execution_group(
            exec_group.as_ref().map(|g| &g.typed.group),
            default_executor.into_option(),
            eval,
        )?;
        let res = InternalRunnerTestInfo {
            test_type: ValueOfUnchecked::new(r#type),
            command: ValueOfUnchecked::new(command),
            listing_command: ValueOfUnchecked::new(listing_command),
            env: ValueOfUnchecked::new(env),
            labels: ValueOfUnchecked::new(labels),
            contacts: ValueOfUnchecked::new(contacts),
            use_project_relative_paths: ValueOfUnchecked::new(use_project_relative_paths),
            run_from_project_root: ValueOfUnchecked::new(run_from_project_root),
            default_executor,
            exec_group: ValueOfUnchecked::new(exec_group.map_or(Value::new_none(), |g| g.value)),
            executor_overrides: ValueOfUnchecked::new(executor_overrides),
            local_resources: ValueOfUnchecked::new(local_resources),
            required_local_resources: ValueOfUnchecked::new(required_local_resources),
            worker: worker.into_option(),
            parse_test_listing: ValueOfUnchecked::new(parse_test_listing),
            parse_test_result: ValueOfUnchecked::new(parse_test_result),
        };
        validate_internal_runner_test_info(&res)?;
        Ok(res)
    }
}
