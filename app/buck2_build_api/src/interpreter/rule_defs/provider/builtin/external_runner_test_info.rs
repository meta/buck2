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
use std::iter::once;

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
use dupe::Dupe;
use either::Either;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
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
use starlark::values::list::ListRef;
use starlark::values::none::NoneOr;
use starlark::values::none::NoneType;
use starlark::values::tuple::TupleRef;

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
    /// A Starlark value representing the type of this test.
    test_type: ValueOfUnchecked<'v, String>,

    /// A Starlark value representing the command for this test. The external test runner is what
    /// gives meaning to this command.
    command: ValueOfUnchecked<'v, Vec<Either<String, Value<'static>>>>,

    /// A Starlark value representing the environment for this test. Here again, the external test
    /// runner is what will this meaning.
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

    /// Executors that Tpx can use to override the default executor.
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

    /// Whether test execution results can be read from the remote action cache.
    supports_test_execution_caching: ValueOfUnchecked<'v, bool>,
}

/// An `ExternalRunnerTestInfo` kept alive by its owning frozen heap; usable across threads and
/// awaits.
pub type OwnedExternalRunnerTestInfo =
    OwnedFrozen<ValueTyped<'static, ExternalRunnerTestInfo<'static>>>;

// NOTE: All the methods here unwrap because we validate at freeze time.
impl<'v> ExternalRunnerTestInfo<'v> {
    pub fn test_type(&self) -> &'v str {
        self.test_type.get().unpack_str().unwrap()
    }

    pub fn command(&self) -> impl Iterator<Item = TestCommandMember<'v>> {
        unwrap_all(iter_test_command(self.command.get()))
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
        let executor_overrides = DictRef::from_value(self.executor_overrides.get()).unwrap();
        executor_overrides
            .get_str(key)
            .map(|v| StarlarkCommandExecutorConfig::from_value(v).unwrap())
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

    pub fn supports_test_execution_caching(&self) -> bool {
        NoneOr::<bool>::unpack_value(self.supports_test_execution_caching.get())
            .unwrap()
            .unwrap()
            .into_option()
            .unwrap_or(false)
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

        for (_, arglike) in self.env() {
            arglike.visit_artifacts(visitor)?;
        }

        // Ignoring local resources as those are built on-demand.

        Ok(())
    }
}

pub enum TestCommandMember<'v> {
    Literal(&'v str),
    Arglike(&'v dyn CommandLineArgLike<'v>),
}

impl<'v> TestCommandMember<'v> {
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

pub(super) fn iter_value<'v>(
    value: Value<'v>,
) -> buck2_error::Result<impl Iterator<Item = Value<'v>> + 'v> {
    match Either::<&ListRef, &TupleRef>::unpack_value_err(value)? {
        Either::Left(list) => Ok(list.iter()),
        Either::Right(tuple) => Ok(tuple.iter()),
    }
}

pub(super) fn iter_test_command<'v>(
    command: Value<'v>,
) -> impl Iterator<Item = buck2_error::Result<TestCommandMember<'v>>> {
    if command.is_none() {
        return Either::Left(Either::Left(empty()));
    }

    let iterable = match iter_value(command) {
        Ok(v) => v,
        Err(e) => {
            return Either::Left(Either::Right(once(Err(e.context("Invalid `command`")))));
        }
    };

    Either::Right(iterable.map(|item| {
        if let Some(s) = item.unpack_str() {
            return Ok(TestCommandMember::Literal(s));
        }

        if let Some(s) = item.downcast_ref::<ResolvedStringWithMacros>() {
            if let Some(s) = s.downcast_str() {
                return Ok(TestCommandMember::Literal(s));
            }
        }

        let arglike = ValueAsCommandLineLike::unpack_value_err(item)
            .with_buck_error_context(|| format!("Invalid item in `command`: {item}"))?
            .0;

        Ok(TestCommandMember::Arglike(arglike))
    }))
}

pub(super) fn iter_test_env<'v>(
    env: Value<'v>,
) -> impl Iterator<Item = buck2_error::Result<(&'v str, &'v dyn CommandLineArgLike<'v>)>> {
    if env.is_none() {
        return Either::Left(Either::Left(empty()));
    }

    let env = match DictRef::from_value(env) {
        Some(env) => env,
        None => {
            return Either::Left(Either::Right(once(Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "Invalid `env`: Expected a dict, got: `{}`",
                env
            )))));
        }
    };

    let env = env.iter().collect::<Vec<_>>();

    Either::Right(env.into_iter().map(|(key, value)| {
        let key = key.unpack_str().with_internal_error(|| {
            format!("Invalid key in `env`: Expected a str, got: `{key}`")
        })?;

        let arglike = ValueAsCommandLineLike::unpack_value_err(value)
            .with_buck_error_context(|| format!("Invalid value in `env` for key `{key}`"))?
            .0;

        Ok((key, arglike))
    }))
}

pub(super) fn iter_opt_str_list<'v>(
    list: Value<'v>,
    name: &'static str,
) -> impl Iterator<Item = buck2_error::Result<&'v str>> {
    if list.is_none() {
        return Either::Left(Either::Left(empty()));
    }

    let iterable = match iter_value(list) {
        Ok(v) => v,
        Err(e) => {
            return Either::Left(Either::Right(once(Err(
                e.context(format!("Invalid `{name}`"))
            ))));
        }
    };

    Either::Right(iterable.map(move |item| {
        let item = item
            .unpack_str()
            .with_internal_error(|| format!("Invalid item in `{name}`: {item}"))?;

        Ok(item)
    }))
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

pub(super) fn iter_executor_overrides<'v>(
    executor_overrides: Value<'v>,
) -> impl Iterator<Item = buck2_error::Result<(&'v str, &'v StarlarkCommandExecutorConfig)>> {
    if executor_overrides.is_none() {
        return Either::Left(Either::Left(empty()));
    }

    let executor_overrides = match DictRef::from_value(executor_overrides) {
        Some(executor_overrides) => executor_overrides,
        None => {
            return Either::Left(Either::Right(once(Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "Invalid `executor_overrides`: Expected a dict, got: `{}`",
                executor_overrides
            )))));
        }
    };

    let executor_overrides = executor_overrides.iter().collect::<Vec<_>>();

    Either::Right(executor_overrides.into_iter().map(|(key, value)| {
        let key = key.unpack_str().ok_or_else(|| {
            internal_error!("Invalid key in `executor_overrides`: Expected a str, got: `{key}`")
        })?;

        let config = StarlarkCommandExecutorConfig::from_value(value).ok_or_else(|| {
            internal_error!("Invalid value in `executor_overrides` for key `{key}`")
        })?;

        Ok((key, config))
    }))
}

pub(super) fn iter_local_resources<'v>(
    local_resources: Value<'v>,
) -> impl Iterator<Item = buck2_error::Result<(&'v str, Option<&'v ConfiguredProvidersLabel>)>> {
    if local_resources.is_none() {
        return Either::Left(Either::Left(empty()));
    }

    let local_resources = match DictRef::from_value(local_resources) {
        Some(local_resources) => local_resources,
        None => {
            return Either::Left(Either::Right(once(Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "Invalid `local_resources`: Expected a dict, got: `{}`",
                local_resources
            )))));
        }
    };

    let local_resources = local_resources.iter().collect::<Vec<_>>();

    Either::Right(local_resources.into_iter().map(|(key, value)| {
        let key = key.unpack_str().ok_or_else(|| {
            internal_error!("Invalid key in `local_resources`: Expected a str, got: `{key}`")
        })?;

        let resource = if value.is_none() {
            None
        } else {
            Some(
                StarlarkConfiguredProvidersLabel::from_value(value)
                    .ok_or_else(|| {
                        buck2_error!(
                            buck2_error::ErrorTag::Input,
                            "{}",
                            format!("Invalid value in `local_resources` for key `{}`", key)
                        )
                    })?
                    .label(),
            )
        };

        Ok((key, resource))
    }))
}

pub(super) fn check_all<I, T>(it: I) -> buck2_error::Result<()>
where
    I: IntoIterator<Item = buck2_error::Result<T>>,
{
    for e in it {
        e?;
    }
    Ok(())
}

pub(super) fn unwrap_all<I, T>(it: I) -> impl Iterator<Item = T>
where
    I: IntoIterator<Item = buck2_error::Result<T>>,
{
    it.into_iter().map(|e| e.unwrap())
}

fn validate_external_runner_test_info<'v>(
    info: &ExternalRunnerTestInfo<'v>,
) -> buck2_error::Result<()> {
    check_all(iter_test_command(info.command.get()))?;
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
    NoneOr::<bool>::unpack_value(info.supports_test_execution_caching.get())?.ok_or_else(|| {
        internal_error!("`supports_test_execution_caching` must be a bool if provided")
    })?;
    info.test_type
        .get()
        .unpack_str()
        .internal_error("`type` must be a str")?;
    Ok(())
}

#[starlark_module]
fn external_runner_test_info_creator(globals: &mut GlobalsBuilder) {
    #[starlark(as_type = ExternalRunnerTestInfo<'static>)]
    fn ExternalRunnerTestInfo<'v>(
        // TODO(nga): these need types.
        #[starlark(require = named)] r#type: Value<'v>,
        #[starlark(require = named, default = NoneType)] command: Value<'v>,
        #[starlark(require = named, default = NoneType)] env: Value<'v>,
        #[starlark(require = named, default = NoneType)] labels: Value<'v>,
        #[starlark(require = named, default = NoneType)] contacts: Value<'v>,
        #[starlark(require = named, default = NoneType)] use_project_relative_paths: Value<'v>,
        #[starlark(require = named, default = NoneType)] run_from_project_root: Value<'v>,
        #[starlark(require = named, default = NoneOr::None)] default_executor: NoneOr<
            ValueTyped<'v, StarlarkCommandExecutorConfig>,
        >,
        #[starlark(require = named, default = NoneOr::None)] exec_group: NoneOr<
            ValueOf<'v, ExecutionGroupArg>,
        >,
        #[starlark(require = named, default = NoneType)] executor_overrides: Value<'v>,
        #[starlark(require = named, default = NoneType)] local_resources: Value<'v>,
        #[starlark(require = named, default = NoneType)] required_local_resources: Value<'v>,
        #[starlark(require = named, default = NoneOr::None)] worker: NoneOr<
            ValueTyped<'v, WorkerInfo<'v>>,
        >,
        #[starlark(require = named, default = NoneType)] supports_test_execution_caching: Value<'v>,
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
            test_type: ValueOfUnchecked::new(r#type),
            command: ValueOfUnchecked::new(command),
            env: ValueOfUnchecked::new(env),
            labels: ValueOfUnchecked::new(labels),
            contacts: ValueOfUnchecked::new(contacts),
            use_project_relative_paths: ValueOfUnchecked::new(use_project_relative_paths),
            run_from_project_root: ValueOfUnchecked::new(run_from_project_root),
            default_executor,
            exec_group: ValueOfUnchecked::new(
                exec_group.map_or(Value::new_none(), |g| g.to_value()),
            ),
            executor_overrides: ValueOfUnchecked::new(executor_overrides),
            local_resources: ValueOfUnchecked::new(local_resources),
            required_local_resources: ValueOfUnchecked::new(required_local_resources),
            worker: worker.into_option(),
            supports_test_execution_caching: ValueOfUnchecked::new(supports_test_execution_caching),
        };
        validate_external_runner_test_info(&res)?;
        Ok(res)
    }
}
