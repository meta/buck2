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

use buck2_core::configuration::transition::id::TransitionId;
use buck2_core::execution_types::execution_group::ExecutionGroup;
use buck2_core::plugins::PluginKindSet;
use buck2_error::BuckErrorContext;
use buck2_interpreter::coerce::COERCE_PROVIDERS_LABEL_FOR_BZL;
use buck2_interpreter::execution_group::ExecutionGroupArg;
use buck2_interpreter::types::provider::callable::ValueAsProviderCallableLike;
use buck2_interpreter::types::transition::ValueAsTransition;
use buck2_node::attrs::attr::Attribute;
use buck2_node::attrs::attr_type::AttrType;
use buck2_node::attrs::attr_type::configuration_dep::ConfigurationDepKind;
use buck2_node::attrs::coercion_context::AttrCoercionContext;
use buck2_node::attrs::configurable::AttrIsConfigurable;
use buck2_node::attrs::display::AttrDisplayWithContextExt;
use buck2_node::provider_id_set::ProviderIdSet;
use dupe::Dupe;
use dupe::OptionDupedExt;
use either::Either;
use starlark::environment::GlobalsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::StringValue;
use starlark::values::Value;
use starlark::values::ValueError;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::list_or_tuple::ListOrTupleType;
use starlark::values::none::NoneOr;
use starlark::values::tuple::TupleType;

use crate::attrs::coerce::attr_type::AttrTypeExt;
use crate::attrs::coerce::ctx::BuildAttrCoercionContext;
use crate::attrs::starlark_attribute::StarlarkAttribute;
use crate::attrs::starlark_attribute::ToolMarks;
use crate::attrs::starlark_attribute::register_attr_type;
use crate::interpreter::build_context::BuildContext;
use crate::interpreter::selector::StarlarkSelector;
use crate::plugins::AllPlugins;
use crate::plugins::PluginKindArg;

const OPTION_NONE_EXPLANATION: &str = "`None` as an attribute value always picks the default. For `attrs.option`, if the default isn't `None`, there is no way to express `None`.";

#[derive(buck2_error::Error, Debug)]
#[buck2(input)]
enum AttrError {
    #[error(
        "`attrs.option` `default` parameter must be `None` or absent, got `{0}`.\n{}",
        OPTION_NONE_EXPLANATION
    )]
    OptionDefaultNone(String),
    #[error("`attrs.default_only` argument must have a default")]
    DefaultOnlyMustHaveDefault,
}

pub(crate) trait AttributeExt {
    /// Helper to create an attribute from attrs.foo functions
    fn attr<'v>(
        eval: &mut Evaluator<'v, '_, '_>,
        default: Option<Value<'v>>,
        doc: &str,
        coercer: AttrType,
        tool_marks: ToolMarks,
    ) -> buck2_error::Result<StarlarkAttribute>;
}

impl AttributeExt for Attribute {
    /// Helper to create an attribute from attrs.foo functions
    fn attr<'v>(
        eval: &mut Evaluator<'v, '_, '_>,
        default: Option<Value<'v>>,
        doc: &str,
        coercer: AttrType,
        tool_marks: ToolMarks,
    ) -> buck2_error::Result<StarlarkAttribute> {
        let default = match default {
            None => None,
            Some(x) => {
                let ctx = attr_coercion_context_for_bzl(eval)?;
                let coerced = coercer
                    .coerce(AttrIsConfigurable::Yes, &ctx, x)
                    .buck_error_context("Error coercing attribute default")?;
                Some((ctx.take_attr_arena_builder(), coerced))
            }
        };
        Ok(StarlarkAttribute::new(
            Attribute::new_from_builder(default, doc, coercer)?,
            tool_marks,
        ))
    }
}

/// Coercion context for evaluating bzl files (attr default, transition rules).
///
/// Coercion through this context is unmemoized: a fresh context per call, no
/// `MemoizedCoercionCtx`. TODO(jtbraun): wrapping it in a `MemoizedCoercionCtx`
/// over the module's `InterpreterExtraValue` table would share coercions of
/// repeated default values across a whole bzl evaluation.
pub(crate) fn attr_coercion_context_for_bzl<'v>(
    eval: &Evaluator<'v, '_, '_>,
) -> buck2_error::Result<BuildAttrCoercionContext> {
    let build_context = BuildContext::from_context(eval)?;
    Ok(BuildAttrCoercionContext::new_no_package(
        build_context.cell_info().cell_resolver().dupe(),
        build_context.cell_info().name().name(),
        build_context.cell_info().cell_alias_resolver().dupe(),
        build_context.infer_target_names,
    ))
}

pub(crate) fn init_coerce_providers_label_for_bzl() {
    COERCE_PROVIDERS_LABEL_FOR_BZL
        .init(|eval, value| attr_coercion_context_for_bzl(eval)?.coerce_providers_label(value))
}

/// Common code to handle `providers` argument of dep-like attrs.
fn dep_like_attr_handle_providers_arg<'v>(
    providers: ValueOf<'v, ListOrTupleType<Value<'v>>>,
) -> buck2_error::Result<ProviderIdSet> {
    Ok(ProviderIdSet::from(
        providers
            .try_items()
            .map(|v| match v?.as_provider_callable() {
                Some(callable) => buck2_error::Ok(callable.id()?.dupe()),
                None => Err(
                    starlark::Error::from(ValueError::IncorrectParameterTypeNamed(
                        "providers".to_owned(),
                    ))
                    .into(),
                ),
            })
            .collect::<Result<Vec<_>, _>>()?,
    ))
}

/// This type is available as a global `attrs` symbol, to allow the definition of attributes to the `rule` function.
///
/// As an example:
///
/// ```python
/// rule(impl = _impl, attrs = {"foo": attrs.string(), "bar": attrs.int(default = 42)})
/// ```
///
/// Most attributes take at least two optional parameters:
///
/// * A `doc` parameter, which specifies documentation for the attribute.
///
/// * A `default` parameter, which if present specifies the default value for the attribute if omitted.
///   If there is no default, the user of the rule must supply that parameter.
///
/// Each attribute defines what values it accepts from the user, and which values it gives to the rule.
/// For simple types like `attrs.string` these are the same, for more complex types like `attrs.dep` these
/// are different (string from the user, dependency to the rule).
#[starlark_module]
fn attr_module(registry: &mut GlobalsBuilder) {
    /// Takes a string from the user, supplies a string to the rule.
    fn string<'v>(
        #[starlark(require = named)] default: Option<Value<'v>>,
        #[starlark(require = named)] validate: Option<Value<'v>>,
        #[starlark(require = named, default = "")] doc: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAttribute> {
        let _unused = validate;
        Ok(Attribute::attr(
            eval,
            default,
            doc,
            AttrType::string(),
            ToolMarks::NONE,
        )?)
    }

    /// Takes a list from the user, supplies a list to the rule.
    fn list<'v>(
        #[starlark(require = pos)] inner: &StarlarkAttribute,
        #[starlark(require = named)] default: Option<Value<'v>>,
        #[starlark(require = named, default = "")] doc: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAttribute> {
        let coercer = AttrType::list(inner.coercer_for_inner()?);
        Ok(Attribute::attr(
            eval,
            default,
            doc,
            coercer,
            inner.tool_marks(),
        )?)
    }

    /// Takes a target from the user, as a string, and supplies a dependency to the rule.
    /// The dependency will transition to the execution platform. Use `exec_dep` if you
    /// plan to execute things from this dependency as part of the compilation.
    ///
    /// With `exec_group`, the dependency is configured for the execution platform of that
    /// group instead, and constrains only that group's execution platform resolution. The rule
    /// must declare the group in `rule(exec_groups = ...)`. A rule that declares named execution
    /// groups must pass `exec_group`, with `DefaultExecGroup` for the default group.
    fn exec_dep<'v>(
        #[starlark(require = named, default = ValueOf::empty_list())] providers: ValueOf<
            'v,
            ListOrTupleType<Value<'v>>,
        >,
        #[starlark(require = named)] default: Option<Value<'v>>,
        #[starlark(require = named, default = "")] doc: &str,
        #[starlark(require = named, default = NoneOr::None)] exec_group: NoneOr<ExecutionGroupArg>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAttribute> {
        let required_providers = dep_like_attr_handle_providers_arg(providers)?;
        let (coercer, marks) = match exec_group.into_option() {
            None => (
                AttrType::exec_dep(required_providers),
                ToolMarks::UNMARKED_EXEC_DEP,
            ),
            Some(ExecutionGroupArg { group }) => (
                AttrType::exec_dep_in_group(required_providers, group),
                ToolMarks::NONE,
            ),
        };
        Ok(Attribute::attr(eval, default, doc, coercer, marks)?)
    }

    /// Takes a list of constraints from the user that the execution platform of `exec_group`
    /// must satisfy, in addition to the target's `exec_compatible_with`, which applies to every
    /// group. With `DefaultExecGroup`, the constraints apply to the default group only. A rule
    /// may have at most one such attribute per group, and must declare the group in
    /// `rule(exec_groups = ...)` (`DefaultExecGroup` is declared implicitly by rules
    /// without named groups). Supplies the constraint labels as strings to the rule.
    fn exec_compatible_with<'v>(
        #[starlark(require = named)] exec_group: ExecutionGroupArg,
        #[starlark(require = named)] default: Option<Value<'v>>,
        #[starlark(require = named, default = "")] doc: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAttribute> {
        let coercer = AttrType::list(AttrType::configuration_dep(
            ConfigurationDepKind::CompatibilityAttribute,
        ));
        let attr = Attribute::attr(eval, default, doc, coercer, ToolMarks::NONE)?;
        Ok(StarlarkAttribute::new_exec_compatible_with(
            attr.clone_attribute(),
            exec_group.group,
        ))
    }

    /// Takes a target from the user, as a string, and supplies a dependency to the rule.
    /// The dependency will be a toolchain dependency, meaning that its execution platform
    /// dependencies will be used to select the execution platform for this rule.
    ///
    /// With `exec_group`, the toolchain's execution platform dependencies select the
    /// execution platform of that group instead, and the toolchain is configured with that
    /// group's execution platform. The rule must declare the group in
    /// `rule(exec_groups = ...)`. A rule that declares named execution groups must pass
    /// `exec_group`, with `DefaultExecGroup` for the default group.
    fn toolchain_dep<'v>(
        #[starlark(require = named, default = ValueOf::empty_list())] providers: ValueOf<
            'v,
            ListOrTupleType<Value<'v>>,
        >,
        #[starlark(require = named)] default: Option<Value<'v>>,
        #[starlark(require = named, default = "")] doc: &str,
        #[starlark(require = named, default = NoneOr::None)] exec_group: NoneOr<ExecutionGroupArg>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAttribute> {
        let required_providers = dep_like_attr_handle_providers_arg(providers)?;
        let (coercer, marks) = match exec_group.into_option() {
            None => (
                AttrType::toolchain_dep(required_providers),
                ToolMarks::UNMARKED_TOOLCHAIN_DEP,
            ),
            Some(ExecutionGroupArg { group }) => (
                AttrType::toolchain_dep_in_group(required_providers, group),
                ToolMarks::NONE,
            ),
        };
        Ok(Attribute::attr(eval, default, doc, coercer, marks)?)
    }

    fn transition_dep<'v>(
        #[starlark(require = named, default = ValueOf::empty_list())] providers: ValueOf<
            'v,
            ListOrTupleType<Value<'v>>,
        >,
        #[starlark(require = named)] cfg: Option<Either<StringValue<'v>, ValueAsTransition<'v>>>,
        #[starlark(require = named)] default: Option<Value<'v>>,
        #[starlark(require = named, default = "")] doc: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAttribute> {
        let required_providers = dep_like_attr_handle_providers_arg(providers)?;
        let label_coercion_ctx = attr_coercion_context_for_bzl(eval)?;

        let transition_id = match cfg {
            Some(Either::Left(target)) => Some(Arc::new(TransitionId::Target(
                label_coercion_ctx.coerce_providers_label(&target)?,
            ))),
            Some(Either::Right(transition)) => Some(transition.transition_id()?),
            None => None,
        };

        let coercer = AttrType::transition_dep(required_providers, transition_id);
        let coerced_default = match default {
            None => None,
            Some(default) => {
                let coerced =
                    coercer.coerce(AttrIsConfigurable::Yes, &label_coercion_ctx, default)?;
                Some((label_coercion_ctx.take_attr_arena_builder(), coerced))
            }
        };

        Ok(StarlarkAttribute::new(
            Attribute::new_from_builder(coerced_default, doc, coercer)?,
            ToolMarks::NONE,
        ))
    }

    fn configured_dep<'v>(
        #[starlark(require = named, default = ValueOf::empty_list())] providers: ValueOf<
            'v,
            ListOrTupleType<Value<'v>>,
        >,
        #[starlark(require = named)] default: Option<Value<'v>>,
        #[starlark(require = named, default = "")] doc: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAttribute> {
        let required_providers = dep_like_attr_handle_providers_arg(providers)?;
        let coercer = AttrType::configured_dep(required_providers);
        Ok(Attribute::attr(
            eval,
            default,
            doc,
            coercer,
            ToolMarks::NONE,
        )?)
    }

    fn split_transition_dep<'v>(
        #[starlark(require = named, default = ValueOf::empty_list())] providers: ValueOf<
            'v,
            ListOrTupleType<Value<'v>>,
        >,
        #[starlark(require = named)] cfg: ValueAsTransition<'v>,
        #[starlark(require = named)] default: Option<Value<'v>>,
        #[starlark(require = named, default = "")] doc: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAttribute> {
        let required_providers = dep_like_attr_handle_providers_arg(providers)?;
        let coercer = AttrType::split_transition_dep(required_providers, cfg.transition_id()?);

        let coerced_default = match default {
            None => None,
            Some(default) => {
                let ctx = attr_coercion_context_for_bzl(eval)?;
                let coerced = coercer.coerce(AttrIsConfigurable::Yes, &ctx, default)?;
                Some((ctx.take_attr_arena_builder(), coerced))
            }
        };

        Ok(StarlarkAttribute::new(
            Attribute::new_from_builder(coerced_default, doc, coercer)?,
            ToolMarks::NONE,
        ))
    }

    /// Takes a target label from the user and registers it as a plugin dependency.
    ///
    /// Plugin dependencies are propagated as unconfigured target labels up the build graph,
    /// then configured as exec deps when used by a rule with `uses_plugins`. This is useful
    /// for dependencies like Rust proc macros that need to be accessible to transitive dependents.
    ///
    /// See the [`plugins`](../plugins) namespace documentation for a full explanation and examples.
    fn plugin_dep<'v>(
        #[starlark(require = named)] kind: PluginKindArg,
        #[starlark(require = named)] default: Option<Value<'v>>,
        #[starlark(require = named, default = "")] doc: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAttribute> {
        Ok(Attribute::attr(
            eval,
            default,
            doc,
            AttrType::plugin_dep(kind.plugin_kind),
            ToolMarks::NONE,
        )?)
    }

    /// Takes a target from the user, as a string, and supplies a dependency to the rule.
    /// A target can be specified as an absolute dependency `foo//bar:baz`, omitting the
    /// cell (`//bar:baz`) or omitting the package name (`:baz`).
    ///
    /// If supplied the `providers` argument ensures that specific providers will be present
    /// on the dependency.
    ///
    /// The `pulls_plugins` and `pulls_and_pushes_plugins` parameters control plugin propagation.
    /// See the [`plugins`](../plugins) namespace documentation for a full explanation.
    fn dep<'v>(
        #[starlark(require = named, default = ValueOf::empty_list())] providers: ValueOf<
            'v,
            ListOrTupleType<Value<'v>>,
        >,
        #[starlark(require = named, default = ValueOf::empty_list())] pulls_plugins: ValueOf<
            'v,
            ListOrTupleType<PluginKindArg>,
        >,
        #[starlark(require = named, default = Either::Left(ValueOf::empty_list()))]
        pulls_and_pushes_plugins: Either<
            ValueOf<'v, ListOrTupleType<PluginKindArg>>,
            &'v AllPlugins,
        >,
        #[starlark(require = named)] default: Option<Value<'v>>,
        #[starlark(require = named, default = "")] doc: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAttribute> {
        let required_providers = dep_like_attr_handle_providers_arg(providers)?;
        let plugin_kinds = match pulls_and_pushes_plugins {
            Either::Right(_) => PluginKindSet::ALL,
            Either::Left(pulls_and_pushes_plugins) => {
                let pulls_and_pushes_plugins: Vec<_> = pulls_and_pushes_plugins
                    .try_items()
                    .map(|arg| arg.map(|PluginKindArg { plugin_kind }| plugin_kind))
                    .collect::<starlark::Result<_>>()?;
                let pulls_plugins: Vec<_> = pulls_plugins
                    .try_items()
                    .map(|arg| arg.map(|PluginKindArg { plugin_kind }| plugin_kind))
                    .collect::<starlark::Result<_>>()?;
                PluginKindSet::new(pulls_plugins, pulls_and_pushes_plugins)?
            }
        };

        let coercer = AttrType::dep(required_providers, plugin_kinds);
        Ok(Attribute::attr(
            eval,
            default,
            doc,
            coercer,
            ToolMarks::NONE,
        )?)
    }

    /// Takes most builtin literals and passes them to the rule as a string.
    /// Discouraged, as it provides little type safety and destroys the structure.
    fn any<'v>(
        #[starlark(require = named, default = "")] doc: &str,
        #[starlark(require = named)] default: Option<Value<'v>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAttribute> {
        Ok(Attribute::attr(
            eval,
            default,
            doc,
            AttrType::any(),
            ToolMarks::NONE,
        )?)
    }

    /// Takes a boolean and passes it to the rule as a boolean.
    fn bool<'v>(
        #[starlark(require = named)] default: Option<Value<'v>>,
        #[starlark(require = named, default = "")] doc: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAttribute> {
        Ok(Attribute::attr(
            eval,
            default,
            doc,
            AttrType::bool(),
            ToolMarks::NONE,
        )?)
    }

    /// Takes a value that may be `None` or some inner type, and passes either `None` or the
    /// value corresponding to the inner to the rule. Often used to make a rule optional:
    ///
    /// ```python
    /// attrs.option(attr.string(), default = None)
    /// ```
    fn option<'v>(
        #[starlark(require = pos)] inner: &StarlarkAttribute,
        #[starlark(require = named)] default: Option<Value<'v>>,
        #[starlark(require = named, default = "")] doc: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAttribute> {
        let coercer = AttrType::option(inner.coercer_for_inner()?);
        let attr = Attribute::attr(eval, default, doc, coercer, inner.tool_marks())?;

        match attr.attribute().default_as_ref() {
            Some(default) if !default.may_return_none() => Err(buck2_error::Error::from(
                AttrError::OptionDefaultNone(default.as_display_no_ctx().to_string()),
            )
            .into()),
            _ => Ok(attr),
        }
    }

    /// Rejects all values and uses the default for the inner argument.
    /// Often used to resolve dependencies, which otherwise can't be resolved inside a rule.
    ///
    /// ```python
    /// attrs.default_only(attrs.dep(default = "foo//my_package:my_target"))
    /// ```
    fn default_only<'v>(
        #[starlark(require = pos)] inner: &StarlarkAttribute,
        #[starlark(require = named, default = "")] doc: &str,
    ) -> starlark::Result<StarlarkAttribute> {
        let Some(default) = inner.attribute().default_as_owned().duped() else {
            return Err(buck2_error::Error::from(AttrError::DefaultOnlyMustHaveDefault).into());
        };
        Ok(StarlarkAttribute::new(
            Attribute::new_default_only(default, doc, inner.coercer_for_default_only()?),
            inner.tool_marks(),
        ))
    }

    /// Takes a target (as per `deps`) and passes a `label` to the rule.
    /// Validates that the target exists, but does not introduce a dependency on it.
    fn label<'v>(
        #[starlark(require = named)] default: Option<Value<'v>>,
        #[starlark(require = named, default = "")] doc: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAttribute> {
        Ok(Attribute::attr(
            eval,
            default,
            doc,
            AttrType::label(),
            ToolMarks::NONE,
        )?)
    }

    /// Takes a dict from the user, supplies a dict to the rule.
    fn dict<'v>(
        // TODO(nga): require positional only for key and value.
        key: &StarlarkAttribute,
        value: &StarlarkAttribute,
        #[starlark(require = named, default = false)] sorted: bool,
        #[starlark(require = named)] default: Option<Value<'v>>,
        #[starlark(require = named, default = "")] doc: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAttribute> {
        let coercer = AttrType::dict(key.coercer_for_inner()?, value.coercer_for_inner()?, sorted);
        Ok(Attribute::attr(
            eval,
            default,
            doc,
            coercer,
            key.tool_marks().union(value.tool_marks()),
        )?)
    }

    /// Takes a command line argument from the user and supplies a `cmd_args` compatible value to the rule.
    /// The argument may contain special macros such as `$(location :my_target)` or `$(exe :my_target)` which
    /// will be replaced with references to those values in the rule. Takes in an optional `anon_target_compatible`
    /// flag, which indicates whether the args can be passed into anon targets. Note that there is a slight memory
    /// hit when using this flag.
    ///
    /// With `exec_group`, the dependencies of `$(exe)`, `$(location_exec)` and
    /// `$(location_toolchain)` macros are configured for the execution platform of that group
    /// instead, and constrain only that group's execution platform resolution. The rule must
    /// declare the group in `rule(exec_groups = ...)`. In a rule that declares named execution
    /// groups, an arg without `exec_group` may hold flags but not tools: a target whose value
    /// refers to a tool through these macros is an error. Pass `exec_group`, with
    /// `DefaultExecGroup` for the default group, for args that refer to tools.
    fn arg<'v>(
        #[starlark(require = named, default = false)] json: bool,
        #[starlark(require = named)] default: Option<Value<'v>>,
        #[starlark(require = named, default = "")] doc: &str,
        #[starlark(require = named, default = false)] anon_target_compatible: bool,
        #[starlark(require = named, default = NoneOr::None)] exec_group: NoneOr<ExecutionGroupArg>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAttribute> {
        let _unused = json;
        let (exec_group, marks) = match exec_group.into_option() {
            Some(ExecutionGroupArg {
                group: ExecutionGroup::Named(group),
            }) => (Some(group), ToolMarks::NONE),
            Some(ExecutionGroupArg {
                group: ExecutionGroup::Default,
            }) => (None, ToolMarks::DEFAULT_GROUP_ARG),
            None => (None, ToolMarks::UNMARKED_ARG),
        };
        Ok(Attribute::attr(
            eval,
            default,
            doc,
            AttrType::arg_in_group(
                anon_target_compatible,
                exec_group,
                marks == ToolMarks::UNMARKED_ARG,
            ),
            marks,
        )?)
    }

    /// Takes a string from one of the variants given, and gives that string to the rule.
    /// Strings are matched case-insensitively, and always passed to the rule lowercase.
    fn r#enum<'v>(
        #[starlark(require = pos)] variants: ValueOf<'v, ListOrTupleType<String>>,
        #[starlark(require = named)] default: Option<
            ValueOf<'v, Either<StringValue<'v>, ValueTyped<'v, StarlarkSelector<'v>>>>,
        >,
        #[starlark(require = named, default = "")] doc: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAttribute> {
        // Value seems to usually be a `[String]`, listing the possible values of the
        // enumeration. Unfortunately, for things like `exported_lang_preprocessor_flags`
        // it ends up being `Type` which doesn't match the data we see.
        Ok(Attribute::attr(
            eval,
            default.map(|v| v.to_value()),
            doc,
            AttrType::enumeration(variants.try_to_vec()?)?,
            ToolMarks::NONE,
        )?)
    }

    fn configuration_label<'v>(
        #[starlark(require = named, default = "")] doc: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAttribute> {
        // TODO(nga): explain how this is different from `dep`.
        //   This probably meant to be similar to `label`, but not configurable.
        Ok(Attribute::attr(
            eval,
            None,
            doc,
            AttrType::dep(ProviderIdSet::EMPTY, PluginKindSet::EMPTY),
            ToolMarks::NONE,
        )?)
    }

    /// Currently an alias for `attrs.string`.
    fn regex<'v>(
        #[starlark(require = named)] default: Option<Value<'v>>,
        #[starlark(require = named, default = "")] doc: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAttribute> {
        Ok(Attribute::attr(
            eval,
            default,
            doc,
            AttrType::string(),
            ToolMarks::NONE,
        )?)
    }

    fn set<'v>(
        #[starlark(require = pos)] value_type: &StarlarkAttribute,
        #[starlark(require = named, default = false)] sorted: bool,
        #[starlark(require = named)] default: Option<Value<'v>>,
        #[starlark(require = named, default = "")] doc: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAttribute> {
        let _unused = sorted;
        let coercer = AttrType::list(value_type.coercer_for_inner()?);
        Ok(Attribute::attr(
            eval,
            default,
            doc,
            coercer,
            value_type.tool_marks(),
        )?)
    }

    fn named_set<'v>(
        #[starlark(require = pos)] value_type: &StarlarkAttribute,
        #[starlark(require = named, default = false)] sorted: bool,
        #[starlark(require = named)] default: Option<Value<'v>>,
        #[starlark(require = named, default = "")] doc: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAttribute> {
        let value_coercer = value_type.coercer_for_inner()?;
        let coercer = AttrType::one_of(vec![
            AttrType::dict(AttrType::string(), value_coercer.dupe(), sorted),
            AttrType::list(value_coercer),
        ]);
        Ok(Attribute::attr(
            eval,
            default,
            doc,
            coercer,
            value_type.tool_marks(),
        )?)
    }

    /// Given a list of alternative attributes, selects the first that matches and gives that to the rule.
    fn one_of<'v>(
        #[starlark(args)] args: ValueOf<'v, TupleType<&'v StarlarkAttribute>>,
        #[starlark(require = named)] default: Option<Value<'v>>,
        #[starlark(require = named, default = "")] doc: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAttribute> {
        let marks = args
            .items()
            .fold(ToolMarks::NONE, |marks, arg| marks.union(arg.tool_marks()));
        let coercer = AttrType::one_of(
            args.items()
                .map(|arg| arg.coercer_for_inner())
                .collect::<Result<Vec<_>, _>>()?,
        );
        Ok(Attribute::attr(eval, default, doc, coercer, marks)?)
    }

    /// Takes a tuple of values and gives a tuple to the rule.
    fn tuple<'v>(
        #[starlark(args)] args: ValueOf<'v, TupleType<&'v StarlarkAttribute>>,
        #[starlark(require = named)] default: Option<Value<'v>>,
        #[starlark(require = named, default = "")] doc: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAttribute> {
        let marks = args
            .items()
            .fold(ToolMarks::NONE, |marks, arg| marks.union(arg.tool_marks()));
        let coercer = AttrType::tuple(
            args.items()
                .map(|arg| arg.coercer_for_inner())
                .collect::<Result<Vec<_>, _>>()?,
        );
        Ok(Attribute::attr(eval, default, doc, coercer, marks)?)
    }

    /// Takes an int from the user, supplies an int to the rule.
    fn int<'v>(
        #[starlark(require = named)] default: Option<Value<'v>>,
        #[starlark(require = named, default = "")] doc: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAttribute> {
        Ok(Attribute::attr(
            eval,
            default,
            doc,
            AttrType::int(),
            ToolMarks::NONE,
        )?)
    }

    fn query<'v>(
        #[starlark(require = named, default = "")] doc: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAttribute> {
        Ok(Attribute::attr(
            eval,
            None,
            doc,
            AttrType::query(),
            ToolMarks::NONE,
        )?)
    }

    fn versioned<'v>(
        value_type: &StarlarkAttribute,
        #[starlark(require = named, default = "")] doc: &str,
    ) -> starlark::Result<StarlarkAttribute> {
        // A versioned field looks like:
        // [ ({"key":"value1"}, arg), ({"key":"value2"}, arg) ]
        let element_type = AttrType::tuple(vec![
            AttrType::dict(AttrType::string(), AttrType::string(), false),
            value_type.coercer_for_inner()?,
        ]);
        let coercer = AttrType::list(element_type.dupe());

        Ok(StarlarkAttribute::new(
            Attribute::new(Some(|b| b.encode_list(&[])), doc, coercer)?,
            value_type.tool_marks(),
        ))
    }

    /// Takes a source file from the user, supplies an artifact to the rule.
    /// The source file may be specified as a literal string
    /// (representing the path within this package), or a target (which must have a
    /// `DefaultInfo` with a `default_outputs` value).
    fn source<'v>(
        #[starlark(require = named, default = false)] allow_directory: bool,
        #[starlark(require = named)] default: Option<Value<'v>>,
        #[starlark(require = named, default = "")] doc: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAttribute> {
        Ok(Attribute::attr(
            eval,
            default,
            doc,
            AttrType::source(allow_directory),
            ToolMarks::NONE,
        )?)
    }
}

pub(crate) fn register_attrs(globals: &mut GlobalsBuilder) {
    globals.namespace("attrs", attr_module);
    register_attr_type(globals);
}
