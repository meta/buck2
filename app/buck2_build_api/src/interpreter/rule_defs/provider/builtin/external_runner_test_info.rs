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
use buck2_build_api_derive::internal_provider;
use buck2_core::execution_types::execution_group::ExecutionGroup;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_error::buck2_error;
use buck2_hash::BuckMutInsertionMap;
use buck2_interpreter::execution_group::ExecutionGroupArg;
use buck2_interpreter::types::configured_providers_label::StarlarkConfiguredProvidersLabel;
use dupe::Dupe;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::eval::Evaluator;
use starlark::values::Freeze;
use starlark::values::FreezeError;
use starlark::values::OwnedFrozen;
use starlark::values::StarlarkPagable;
use starlark::values::StringValue;
use starlark::values::Trace;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::dict::DictType;
use starlark::values::list_or_tuple::ListOrTupleType;
use starlark::values::none::NoneOr;

use crate as buck2_build_api;
use crate::analysis::extra_v::AnalysisExtraValue;
use crate::interpreter::rule_defs::cmd_args::CommandLineArgLike;
use crate::interpreter::rule_defs::cmd_args::CommandLineArtifactVisitor;
use crate::interpreter::rule_defs::cmd_args::CommandLineBuilder;
use crate::interpreter::rule_defs::cmd_args::value_as::ValueAsCommandLineLike;
use crate::interpreter::rule_defs::command_executor_config::StarlarkCommandExecutorConfig;
use crate::interpreter::rule_defs::provider::builtin::worker_info::WorkerInfo;
use crate::interpreter::rule_defs::required_test_local_resource::StarlarkRequiredTestLocalResource;
use crate::interpreter::rule_defs::resolved_macro::ResolvedStringWithMacros;

/// Provider that signals that a rule can be tested using an external runner. This is the
/// Buck1-compatible API for tests.
#[internal_provider(external_runner_test_info_creator)]
#[derive(
    Clone,
    Debug,
    Trace,
    Freeze,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[freeze(validator = validate_external_runner_test_info)]
#[repr(C)]
pub struct ExternalRunnerTestInfo<'v> {
    /// The type of this test.
    test_type: StringValue<'v>,

    /// The command for this test. The external test runner is what gives meaning to this
    /// command.
    command: Option<ValueOf<'v, ListOrTupleType<ValueAsCommandLineLike<'v>>>>,

    /// The environment for this test. Here again, the external test runner is what gives this
    /// meaning.
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

    /// Executors that Tpx can use to override the default executor.
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

    /// Whether test execution results can be read from the remote action cache.
    supports_test_execution_caching: Option<ValueOf<'v, bool>>,
}

/// An `ExternalRunnerTestInfo` kept alive by its owning frozen heap; usable across threads and
/// awaits.
pub type OwnedExternalRunnerTestInfo =
    OwnedFrozen<ValueTyped<'static, ExternalRunnerTestInfo<'static>>>;

impl<'v> ExternalRunnerTestInfo<'v> {
    pub fn test_type(&self) -> &'v str {
        self.test_type.as_str()
    }

    pub fn command(&self) -> impl Iterator<Item = buck2_error::Result<TestCommandMember<'v>>> + 'v {
        command_members(self.command)
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

    pub fn supports_test_execution_caching(&self) -> bool {
        self.supports_test_execution_caching
            .is_some_and(|v| v.unpack())
    }

    pub fn visit_artifacts(
        &self,
        visitor: &mut dyn CommandLineArtifactVisitor<'v>,
    ) -> buck2_error::Result<()> {
        visit_command_artifacts(self.command(), visitor)?;
        visit_env_artifacts(self.env(), visitor)?;
        // Ignoring local resources as those are built on-demand.
        Ok(())
    }
}

pub enum TestCommandMember<'v> {
    Literal(&'v str),
    Arglike(&'v dyn CommandLineArgLike<'v>),
}

impl<'v> TestCommandMember<'v> {
    /// A literal is a `str`, or a resolved macro that amounts to one; the test runner sees those
    /// verbatim and everything else through a handle.
    fn new(arg: ValueOf<'v, ValueAsCommandLineLike<'v>>) -> Self {
        let value = arg.to_value();
        if let Some(s) = value.unpack_str() {
            return Self::Literal(s);
        }
        if let Some(s) = value
            .downcast_ref::<ResolvedStringWithMacros>()
            .and_then(|s| s.downcast_str())
        {
            return Self::Literal(s);
        }
        Self::Arglike(arg.unpack().0)
    }

    pub fn add_to_command_line(
        &self,
        fmt: &mut CommandLineBuilder<'v, '_>,
    ) -> buck2_error::Result<()> {
        match self {
            Self::Literal(literal) => literal.add_to_command_line(fmt),
            Self::Arglike(arglike) => arglike.add_to_command_line(fmt),
        }
    }
}

// The field types `ExternalRunnerTestInfo` and `InternalRunnerTestInfo` share, and the readers
// over them. (The provider macro reads field types syntactically, so the structs spell them out.)

pub(super) type TestCommand<'v> = ValueOf<'v, ListOrTupleType<ValueAsCommandLineLike<'v>>>;
pub(super) type TestEnv<'v> = ValueOf<'v, DictType<StringValue<'v>, ValueAsCommandLineLike<'v>>>;
pub(super) type StrSeq<'v> = ValueOf<'v, ListOrTupleType<StringValue<'v>>>;
pub(super) type ExecutorOverrides<'v> =
    ValueOf<'v, DictType<StringValue<'v>, ValueTyped<'v, StarlarkCommandExecutorConfig>>>;
pub(super) type LocalResources<'v> = ValueOf<
    'v,
    DictType<StringValue<'v>, NoneOr<ValueTyped<'v, StarlarkConfiguredProvidersLabel>>>,
>;
pub(super) type RequiredLocalResources<'v> =
    ValueOf<'v, ListOrTupleType<ValueTyped<'v, StarlarkRequiredTestLocalResource>>>;

pub(super) fn command_members<'v>(
    command: Option<TestCommand<'v>>,
) -> impl Iterator<Item = buck2_error::Result<TestCommandMember<'v>>> + 'v {
    command
        .into_iter()
        .flat_map(|command| command.iter())
        .map(|arg| Ok(TestCommandMember::new(arg?)))
}

pub(super) fn env_entries<'v>(
    env: Option<TestEnv<'v>>,
) -> impl Iterator<Item = buck2_error::Result<(&'v str, &'v dyn CommandLineArgLike<'v>)>> + 'v {
    env.into_iter().flat_map(|env| env.iter()).map(|entry| {
        let (key, value) = entry?;
        Ok((key.unpack().as_str(), value.unpack().0))
    })
}

pub(super) fn str_items<'v>(
    list: Option<StrSeq<'v>>,
) -> impl Iterator<Item = buck2_error::Result<&'v str>> + 'v {
    list.into_iter()
        .flat_map(|list| list.iter())
        .map(|item| Ok(item?.unpack().as_str()))
}

pub(super) fn lookup_executor_override<'v>(
    overrides: Option<ExecutorOverrides<'v>>,
    key: &str,
) -> buck2_error::Result<Option<&'v StarlarkCommandExecutorConfig>> {
    let Some(overrides) = overrides else {
        return Ok(None);
    };
    Ok(overrides
        .get_str(key)?
        .map(|config| config.unpack().as_ref()))
}

/// The executor for the test info's `default_executor` field. A passed `default_executor` wins,
/// and `exec_group` is then not resolved. Otherwise a test info that names an execution group
/// records that group's executor from the analysis that creates it, so its tests run there even
/// when another rule returns the test info. Records nothing when the group has no compatible
/// execution platform, when the rule does not declare the group, or outside rule analysis; the
/// test then resolves the group against the tested target when it runs. Any other resolution
/// error fails the creation of the test info.
///
/// In rule analysis, a test info with neither `default_executor` nor `exec_group` runs in the
/// default execution group whenever no executor override is picked, so its creation fails when the
/// rule does not declare that group, whether or not it has `executor_overrides`.
pub(super) fn default_executor_with_execution_group<'v>(
    exec_group: Option<&ExecutionGroup>,
    default_executor: Option<ValueTyped<'v, StarlarkCommandExecutorConfig>>,
    eval: &mut Evaluator<'v, '_, '_>,
) -> buck2_error::Result<Option<ValueTyped<'v, StarlarkCommandExecutorConfig>>> {
    if default_executor.is_some() {
        return Ok(default_executor);
    }
    let Some(resolution) = AnalysisExtraValue::execution_platform_resolution(eval.module()) else {
        return Ok(None);
    };
    let Some(group) = exec_group else {
        if !resolution.default_execution_group_declared() {
            return Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "The test info uses the default execution group, which the rule does not declare. Set `exec_group` or `default_executor`, or add `DefaultExecGroup` to `exec_groups`"
            ));
        }
        return Ok(None);
    };
    if let ExecutionGroup::Named(named) = group {
        if !resolution.is_unspecified()
            && !resolution
                .execution_group_platforms()
                .any(|(declared, _, _)| declared == named)
        {
            return Ok(None);
        }
    }
    if *group == ExecutionGroup::Default && !resolution.default_execution_group_declared() {
        return Err(buck2_error!(
            buck2_error::ErrorTag::Input,
            "The test info uses the default execution group, which the rule does not declare. Set `exec_group` to a declared group, or add `DefaultExecGroup` to `exec_groups`"
        ));
    }
    // Fails when resolution was not performed.
    Ok(resolution.platform_for_group(group)?.map(|platform| {
        eval.heap().alloc_typed(StarlarkCommandExecutorConfig(
            platform.executor_config().dupe(),
        ))
    }))
}

pub(super) fn collect_local_resources<'v>(
    local_resources: Option<LocalResources<'v>>,
) -> buck2_error::Result<BuckMutInsertionMap<&'v str, Option<&'v ConfiguredProvidersLabel>>> {
    local_resources
        .into_iter()
        .flat_map(|resources| resources.iter())
        .map(|entry| {
            let (key, label) = entry?;
            Ok((
                key.unpack().as_str(),
                label
                    .unpack()
                    .into_option()
                    .map(|label| label.as_ref().label()),
            ))
        })
        .collect()
}

pub(super) fn required_local_resource_items<'v>(
    required: Option<RequiredLocalResources<'v>>,
) -> impl Iterator<Item = buck2_error::Result<&'v StarlarkRequiredTestLocalResource>> + 'v {
    required
        .into_iter()
        .flat_map(|required| required.iter())
        .map(|resource| Ok(resource?.unpack().as_ref()))
}

/// Every type in `required_local_resources` must have an entry in `local_resources`.
pub(super) fn check_required_local_resources<'v>(
    local_resources: Option<LocalResources<'v>>,
    required: Option<RequiredLocalResources<'v>>,
) -> buck2_error::Result<()> {
    let provided = collect_local_resources(local_resources)?;
    for resource in required_local_resource_items(required) {
        let resource = resource?;
        if !provided.contains_key(&resource.name as &str) {
            return Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "`required_local_resources` contains `{}` which is not present in `local_resources`",
                resource.name
            ));
        }
    }
    Ok(())
}

pub(super) fn visit_command_artifacts<'v>(
    command: impl Iterator<Item = buck2_error::Result<TestCommandMember<'v>>>,
    visitor: &mut dyn CommandLineArtifactVisitor<'v>,
) -> buck2_error::Result<()> {
    for member in command {
        if let TestCommandMember::Arglike(arglike) = member? {
            arglike.visit_artifacts(visitor)?;
        }
    }
    Ok(())
}

pub(super) fn visit_env_artifacts<'v>(
    env: impl Iterator<Item = buck2_error::Result<(&'v str, &'v dyn CommandLineArgLike<'v>)>>,
    visitor: &mut dyn CommandLineArtifactVisitor<'v>,
) -> buck2_error::Result<()> {
    for entry in env {
        let (_, arglike) = entry?;
        arglike.visit_artifacts(visitor)?;
    }
    Ok(())
}

fn validate_external_runner_test_info<'v>(
    info: &ExternalRunnerTestInfo<'v>,
) -> buck2_error::Result<()> {
    check_required_local_resources(info.local_resources, info.required_local_resources)
}

#[starlark_module]
fn external_runner_test_info_creator(globals: &mut GlobalsBuilder) {
    #[starlark(as_type = ExternalRunnerTestInfo<'static>)]
    fn ExternalRunnerTestInfo<'v>(
        #[starlark(require = named)] r#type: StringValue<'v>,
        #[starlark(require = named, default = NoneOr::None)] command: NoneOr<TestCommand<'v>>,
        #[starlark(require = named, default = NoneOr::None)] env: NoneOr<TestEnv<'v>>,
        #[starlark(require = named, default = NoneOr::None)] labels: NoneOr<StrSeq<'v>>,
        #[starlark(require = named, default = NoneOr::None)] contacts: NoneOr<StrSeq<'v>>,
        #[starlark(require = named, default = NoneOr::None)] use_project_relative_paths: NoneOr<
            ValueOf<'v, bool>,
        >,
        #[starlark(require = named, default = NoneOr::None)] run_from_project_root: NoneOr<
            ValueOf<'v, bool>,
        >,
        #[starlark(require = named, default = NoneOr::None)] default_executor: NoneOr<
            ValueTyped<'v, StarlarkCommandExecutorConfig>,
        >,
        #[starlark(require = named, default = NoneOr::None)] exec_group: NoneOr<
            ValueOf<'v, ExecutionGroupArg>,
        >,
        #[starlark(require = named, default = NoneOr::None)] executor_overrides: NoneOr<
            ExecutorOverrides<'v>,
        >,
        #[starlark(require = named, default = NoneOr::None)] local_resources: NoneOr<
            LocalResources<'v>,
        >,
        #[starlark(require = named, default = NoneOr::None)] required_local_resources: NoneOr<
            RequiredLocalResources<'v>,
        >,
        #[starlark(require = named, default = NoneOr::None)] worker: NoneOr<
            ValueTyped<'v, WorkerInfo<'v>>,
        >,
        #[starlark(require = named, default = NoneOr::None)]
        supports_test_execution_caching: NoneOr<ValueOf<'v, bool>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<ExternalRunnerTestInfo<'v>> {
        let exec_group = exec_group.into_option();
        let group = exec_group.map(|g| g.try_unpack()).transpose()?;
        let default_executor = default_executor_with_execution_group(
            group.as_ref().map(|g| &g.group),
            default_executor.into_option(),
            eval,
        )?;
        let res = ExternalRunnerTestInfo {
            test_type: r#type,
            command: command.into_option(),
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
            supports_test_execution_caching: supports_test_execution_caching.into_option(),
        };
        validate_external_runner_test_info(&res)?;
        Ok(res)
    }
}
