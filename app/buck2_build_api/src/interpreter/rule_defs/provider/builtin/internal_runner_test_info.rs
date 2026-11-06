/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::time::Duration;

use allocative::Allocative;
use buck2_build_api_derive::internal_provider;
use buck2_core::execution_types::execution_group::ExecutionGroup;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_error::buck2_error;
use buck2_hash::BuckMutInsertionMap;
use buck2_interpreter::execution_group::ExecutionGroupArg;
use buck2_interpreter::types::configured_providers_label::StarlarkConfiguredProvidersLabel;
use buck2_test_api::data::TestListingEntry;
use buck2_test_api::data::TestResultEntry;
use buck2_test_api::data::TestStatus;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Module;
use starlark::eval::Evaluator;
use starlark::values::Freeze;
use starlark::values::FreezeError;
use starlark::values::OwnedFrozen;
use starlark::values::StarlarkPagable;
use starlark::values::StringValue;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::dict::DictRef;
use starlark::values::dict::DictType;
use starlark::values::float::StarlarkFloat;
use starlark::values::list::ListRef;
use starlark::values::list::ListType;
use starlark::values::list_or_tuple::ListOrTupleType;
use starlark::values::none::NoneOr;
use starlark::values::typing::StarlarkCallable;

use crate as buck2_build_api;
use crate::interpreter::rule_defs::cmd_args::CommandLineArgLike;
use crate::interpreter::rule_defs::cmd_args::CommandLineArtifactVisitor;
use crate::interpreter::rule_defs::cmd_args::value_as::ValueAsCommandLineLike;
use crate::interpreter::rule_defs::command_executor_config::StarlarkCommandExecutorConfig;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::ExecutorOverrides;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::LocalResources;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::RequiredLocalResources;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::StrSeq;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::TestCommand;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::TestCommandMember;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::TestEnv;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::check_required_local_resources;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::collect_local_resources;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::command_members;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::default_executor_with_execution_group;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::env_entries;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::lookup_executor_override;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::required_local_resource_items;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::str_items;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::visit_command_artifacts;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::visit_env_artifacts;
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
    /// The type of this test.
    test_type: StringValue<'v>,

    /// The command for this test. The test runner is what gives meaning to this command.
    command: Option<ValueOf<'v, ListOrTupleType<ValueAsCommandLineLike<'v>>>>,

    /// The command used for test discovery (listing). This is the command that runs the test
    /// binary with framework-specific listing flags (e.g., `["binary", "--gtest_list_tests"]`
    /// for GTest). The internal runner uses this for the listing step, while `command` is used
    /// for execution.
    listing_command: ValueOf<'v, ListOrTupleType<ValueAsCommandLineLike<'v>>>,

    /// The environment for this test.
    env: Option<ValueOf<'v, DictType<StringValue<'v>, ValueAsCommandLineLike<'v>>>>,

    /// The labels for this test.
    labels: Option<ValueOf<'v, ListOrTupleType<StringValue<'v>>>>,

    /// The contacts for this test. This is largely expected to be an oncall, though it's not
    /// validated in any way.
    contacts: Option<ValueOf<'v, ListOrTupleType<StringValue<'v>>>>,

    /// Whether this test should use relative paths
    use_project_relative_paths: Option<ValueOf<'v, bool>>,

    /// Whether this test should run from the project root, as opposed to the cell root
    ///
    /// Defaults to `True`.
    run_from_project_root: Option<ValueOf<'v, bool>>,

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
    exec_group: Option<ValueOf<'v, ExecutionGroupArg>>,

    /// Executors that can be used to override the default executor.
    executor_overrides: Option<
        ValueOf<'v, DictType<StringValue<'v>, ValueTyped<'v, StarlarkCommandExecutorConfig>>>,
    >,

    /// Mapping from a local resource type to a target with a corresponding provider.
    /// Required types are passed from test runner.
    /// If the value for a corresponding type is omitted it means local resource
    /// should be ignored when executing tests even if those are passed as required from test runner.
    local_resources: Option<
        ValueOf<
            'v,
            DictType<StringValue<'v>, NoneOr<ValueTyped<'v, StarlarkConfiguredProvidersLabel>>>,
        >,
    >,

    /// List of local resource types which should be set up additionally to those which are
    /// passed from test runner. Allows specifying local resources on a per-rule basis.
    required_local_resources:
        Option<ValueOf<'v, ListOrTupleType<ValueTyped<'v, StarlarkRequiredTestLocalResource>>>>,

    /// Configuration needed to spawn a new worker. This worker will be used to run every single
    /// command related to test execution, including listing.
    worker: Option<ValueTyped<'v, WorkerInfo<'v>>>,

    /// A Starlark callable that parses test listing output into structured test
    /// entries. The callback signature is:
    ///
    /// ```python
    /// def parse_test_listing(listing_content: str) -> list[dict[str, ...]]:
    ///     """Returns list of dicts with keys:
    ///         "name": str       — human-readable display name
    ///         "filter": str     — argument to select this test for execution
    ///     """
    /// ```
    parse_test_listing:
        ValueOf<'v, StarlarkCallable<'v, (String,), ListType<DictType<String, Value<'static>>>>>,

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
    parse_test_result: ValueOf<
        'v,
        StarlarkCallable<'v, (String, String, i32), ListType<DictType<String, Value<'static>>>>,
    >,
}

/// An `InternalRunnerTestInfo` kept alive by its owning frozen heap; usable across threads and
/// awaits.
pub type OwnedInternalRunnerTestInfo =
    OwnedFrozen<ValueTyped<'static, InternalRunnerTestInfo<'static>>>;

impl<'v> InternalRunnerTestInfo<'v> {
    pub fn test_type(&self) -> &'v str {
        self.test_type.as_str()
    }

    pub fn command(&self) -> impl Iterator<Item = buck2_error::Result<TestCommandMember<'v>>> + 'v {
        command_members(self.command)
    }

    pub fn listing_command(
        &self,
    ) -> impl Iterator<Item = buck2_error::Result<TestCommandMember<'v>>> + 'v {
        command_members(Some(self.listing_command))
    }

    pub fn env(
        &self,
    ) -> impl Iterator<Item = buck2_error::Result<(&'v str, &'v dyn CommandLineArgLike<'v>)>> + 'v
    {
        env_entries(self.env)
    }

    pub fn labels(&self) -> impl Iterator<Item = buck2_error::Result<&'v str>> + 'v {
        str_items(self.labels)
    }

    pub fn contacts(&self) -> impl Iterator<Item = buck2_error::Result<&'v str>> + 'v {
        str_items(self.contacts)
    }

    pub fn use_project_relative_paths(&self) -> bool {
        self.use_project_relative_paths
            .map_or_else(buck2_core::is_open_source, |v| v.unpack())
    }

    pub fn run_from_project_root(&self) -> bool {
        self.run_from_project_root.is_none_or(|v| v.unpack())
    }

    pub fn default_executor(&self) -> Option<&'v StarlarkCommandExecutorConfig> {
        self.default_executor.map(|v| v.as_ref())
    }

    pub fn execution_group(&self) -> buck2_error::Result<Option<ExecutionGroup>> {
        match self.exec_group {
            None => Ok(None),
            Some(exec_group) => Ok(Some(exec_group.try_unpack()?.group)),
        }
    }

    pub fn has_executor_overrides(&self) -> bool {
        self.executor_overrides.is_some()
    }

    /// Access a specific executor override.
    pub fn executor_override(
        &self,
        key: &str,
    ) -> buck2_error::Result<Option<&'v StarlarkCommandExecutorConfig>> {
        lookup_executor_override(self.executor_overrides, key)
    }

    pub fn local_resources(
        &self,
    ) -> buck2_error::Result<BuckMutInsertionMap<&'v str, Option<&'v ConfiguredProvidersLabel>>>
    {
        collect_local_resources(self.local_resources)
    }

    pub fn required_local_resources(
        &self,
    ) -> impl Iterator<Item = buck2_error::Result<&'v StarlarkRequiredTestLocalResource>> + 'v {
        required_local_resource_items(self.required_local_resources)
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
                .to_value();

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

                    Ok(TestListingEntry {
                        name: get_required_str("name")?,
                        filter: get_required_str("filter")?,
                    })
                })
                .collect()
        })
    }

    pub fn visit_artifacts(
        &self,
        visitor: &mut dyn CommandLineArtifactVisitor<'v>,
    ) -> buck2_error::Result<()> {
        visit_command_artifacts(self.command(), visitor)?;
        visit_command_artifacts(self.listing_command(), visitor)?;
        visit_env_artifacts(self.env(), visitor)
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
                .to_value();

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

                    let get_opt_str = |key: &str| -> buck2_error::Result<Option<String>> {
                        match dict.get_str(key) {
                            Some(v) => {
                                if v.is_none() {
                                    Ok(None)
                                } else {
                                    let s = v.unpack_str().ok_or_else(|| {
                                        buck2_error!(
                                            buck2_error::ErrorTag::Input,
                                            "`{}` must be a string, got: `{}`",
                                            key,
                                            v
                                        )
                                    })?;
                                    Ok(Some(s.to_owned()))
                                }
                            }
                            None => Ok(None),
                        }
                    };

                    let get_required_str = |key: &str| -> buck2_error::Result<String> {
                        get_opt_str(key)?.ok_or_else(|| {
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
                    let message = get_opt_str("message")?;
                    let details = get_opt_str("details")?;

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

fn validate_internal_runner_test_info<'v>(
    info: &InternalRunnerTestInfo<'v>,
) -> buck2_error::Result<()> {
    if info.listing_command.iter().next().is_none() {
        return Err(buck2_error!(
            buck2_error::ErrorTag::Input,
            "`listing_command` must be non-empty"
        ));
    }
    check_required_local_resources(info.local_resources, info.required_local_resources)
}

#[starlark_module]
fn internal_runner_test_info_creator(globals: &mut GlobalsBuilder) {
    #[starlark(as_type = InternalRunnerTestInfo<'static>)]
    fn InternalRunnerTestInfo<'v>(
        r#type: StringValue<'v>,
        parse_test_listing: ValueOf<
            'v,
            StarlarkCallable<'v, (String,), ListType<DictType<String, Value<'static>>>>,
        >,
        parse_test_result: ValueOf<
            'v,
            StarlarkCallable<'v, (String, String, i32), ListType<DictType<String, Value<'static>>>>,
        >,
        listing_command: TestCommand<'v>,
        #[starlark(default = NoneOr::None)] command: NoneOr<TestCommand<'v>>,
        #[starlark(default = NoneOr::None)] env: NoneOr<TestEnv<'v>>,
        #[starlark(default = NoneOr::None)] labels: NoneOr<StrSeq<'v>>,
        #[starlark(default = NoneOr::None)] contacts: NoneOr<StrSeq<'v>>,
        #[starlark(default = NoneOr::None)] use_project_relative_paths: NoneOr<ValueOf<'v, bool>>,
        #[starlark(default = NoneOr::None)] run_from_project_root: NoneOr<ValueOf<'v, bool>>,
        #[starlark(default = NoneOr::None)] default_executor: NoneOr<
            ValueTyped<'v, StarlarkCommandExecutorConfig>,
        >,
        #[starlark(default = NoneOr::None)] executor_overrides: NoneOr<ExecutorOverrides<'v>>,
        #[starlark(default = NoneOr::None)] local_resources: NoneOr<LocalResources<'v>>,
        #[starlark(default = NoneOr::None)] required_local_resources: NoneOr<
            RequiredLocalResources<'v>,
        >,
        #[starlark(default = NoneOr::None)] worker: NoneOr<ValueTyped<'v, WorkerInfo<'v>>>,
        #[starlark(require = named, default = NoneOr::None)] exec_group: NoneOr<
            ValueOf<'v, ExecutionGroupArg>,
        >,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<InternalRunnerTestInfo<'v>> {
        let exec_group = exec_group.into_option();
        let group = exec_group.map(|g| g.try_unpack()).transpose()?;
        let default_executor = default_executor_with_execution_group(
            group.as_ref().map(|g| &g.group),
            default_executor.into_option(),
            eval,
        )?;
        let res = InternalRunnerTestInfo {
            test_type: r#type,
            command: command.into_option(),
            listing_command,
            env: env.into_option(),
            labels: labels.into_option(),
            contacts: contacts.into_option(),
            use_project_relative_paths: use_project_relative_paths.into_option(),
            run_from_project_root: run_from_project_root.into_option(),
            default_executor,
            exec_group,
            executor_overrides: executor_overrides.into_option(),
            local_resources: local_resources.into_option(),
            required_local_resources: required_local_resources.into_option(),
            worker: worker.into_option(),
            parse_test_listing,
            parse_test_result,
        };
        validate_internal_runner_test_info(&res)?;
        Ok(res)
    }
}
