/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::RefCell;
use std::fmt;
use std::fmt::Display;
use std::sync::Arc;

use allocative::Allocative;
use buck2_core::cells::CellResolver;
use buck2_core::execution_types::execution_group::ExecutionGroup;
use buck2_core::execution_types::execution_group::NamedExecutionGroup;
use buck2_core::plugins::PluginKind;
use buck2_error::BuckErrorOptionContext;
use buck2_hash::BuckMutInsertionMap;
use buck2_interpreter::execution_group::ExecutionGroupArg;
use buck2_interpreter::late_binding_ty::AnalysisContextReprLate;
use buck2_interpreter::late_binding_ty::ProviderReprLate;
use buck2_interpreter::starlark_promise::StarlarkPromise;
use buck2_interpreter::types::rule::FROZEN_PROMISE_ARTIFACT_MAPPINGS_GET_IMPL;
use buck2_interpreter::types::rule::FROZEN_RULE_GET_IMPL;
use buck2_interpreter::types::transition::ValueAsTransition;
use buck2_node::attrs::attr::Attribute;
use buck2_node::attrs::spec::AttributeId;
use buck2_node::attrs::spec::AttributeSpec;
use buck2_node::bzl_or_bxl_path::BzlOrBxlPath;
use buck2_node::nodes::unconfigured::RuleKind;
use buck2_node::nodes::unconfigured::UnsealedTargetNode;
use buck2_node::rule::Rule;
use buck2_node::rule::RuleExecutionGroup;
use buck2_node::rule::RuleIncomingTransition;
use buck2_node::rule_type::RuleType;
use buck2_node::rule_type::StarlarkRuleType;
use derive_more::Display;
use dupe::Dupe;
use either::Either;
use itertools::Itertools;
use starlark::any::ProvidesStaticType;
use starlark::docs::DocFunction;
use starlark::docs::DocItem;
use starlark::docs::DocMember;
use starlark::docs::DocStringKind;
use starlark::environment::GlobalsBuilder;
use starlark::eval::Arguments;
use starlark::eval::Evaluator;
use starlark::eval::ParametersSpec;
use starlark::starlark_module;
use starlark::typing::ParamSpec;
use starlark::typing::Ty;
use starlark::values::AllocValue;
use starlark::values::Freeze;
use starlark::values::FreezeError;
use starlark::values::FreezeResult;
use starlark::values::Freezer;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkRepresentations;
use starlark::values::StarlarkType;
use starlark::values::StarlarkValue;
use starlark::values::StringValue;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::dict::DictType;
use starlark::values::list::ListType;
use starlark::values::list_or_tuple::ListOrTupleType;
use starlark::values::starlark_value;
use starlark::values::typing::StarlarkCallable;
use starlark::values::typing::StarlarkCallableChecked;

use crate::attrs::coerce::ctx::MemoizedCoercionCtx;
use crate::attrs::starlark_attribute::StarlarkAttribute;
use crate::attrs::starlark_attribute::ToolMarks;
use crate::interpreter::build_context::BuildContext;
use crate::interpreter::build_context::PerFileTypeContext;
use crate::interpreter::extra_value::InterpreterExtraValue;
use crate::interpreter::module_internals::ModuleInternals;
use crate::nodes::attr_spec::AttributeSpecExt;
use crate::nodes::unconfigured::UnsealedTargetNodeExt;
use crate::plugins::PluginKindArg;

pub static NAME_ATTRIBUTE_FIELD: &str = "name";

#[derive(Debug, ProvidesStaticType, Trace, NoSerialize, Allocative, Clone, Copy)]
enum RuleImpl<'v> {
    BuildRule(StarlarkCallable<'v, (Value<'static>,), ListType<Value<'static>>>),
    BxlAnon(StarlarkCallable<'v, (Value<'static>, Value<'static>), ListType<Value<'static>>>),
}

/// The callable that's returned from a `rule()` call. Once frozen, and called, it adds targets'
/// parameters to the context
#[derive(Debug, ProvidesStaticType, Trace, NoSerialize, Allocative)]
pub struct StarlarkRuleCallable<'v> {
    /// The import path that contains the rule() call; stored here so we can retrieve extra
    /// information during `export_as()`
    rule_path: BzlOrBxlPath,
    /// Once exported, the `import_path` and `name` of the callable. Used in DICE to retrieve rule
    /// implementations
    id: RefCell<Option<StarlarkRuleType>>,
    /// The implementation function for this rule.
    /// If is a build rule or anon rule in bzl must take a ctx,
    /// If is a bxl anon rule must take a bxl context and attrs.
    implementation: RuleImpl<'v>,
    // Field Name -> Attribute
    attributes: AttributeSpec,
    /// Type for the typechecker.
    ty: Ty,
    /// When specified, this transition will be applied to the target before configuring it.
    cfg: RuleIncomingTransition,
    /// The plugins that are used by these targets
    uses_plugins: Vec<PluginKind>,
    /// The named execution groups these targets use
    execution_groups: Vec<RuleExecutionGroup>,
    /// Whether these targets may use the default execution group
    default_execution_group: bool,
    /// The attribute constraining the execution platform of the default execution group only
    default_exec_compatible_with_attr: Option<AttributeId>,
    /// In a rule with named execution groups, the attributes with an `attrs.arg()` without
    /// `exec_group`; their values must not refer to tools.
    ungrouped_arg_attrs: Vec<AttributeId>,
    /// This kind of the rule, e.g. whether it can be used in configuration context.
    rule_kind: RuleKind,
    /// The raw docstring for this rule
    docs: Option<String>,
    /// When evaluating rule function, take only the `name` argument, ignore the others.
    ignore_attrs_for_profiling: bool,
    /// Optional map of the promise artifact name to starlark function.
    /// `None` for normal rules, `Some` for anon targets.
    artifact_promise_mappings: Option<ArtifactPromiseMappings<'v>>,
}

/// Mappings of promise artifact name to the starlark function that will produce it, for anon targets.
#[derive(Debug, ProvidesStaticType, Trace, NoSerialize, Allocative)]
struct ArtifactPromiseMappings<'v> {
    mappings: BuckMutInsertionMap<StringValue<'v>, Value<'v>>,
}

/// Mappings of frozen promise artifact name to the frozen starlark function that will produce it, for anon targets.
#[derive(Debug, ProvidesStaticType, Allocative, StarlarkPagable)]
pub struct FrozenArtifactPromiseMappings<'v> {
    pub mappings: BuckMutInsertionMap<StringValue<'v>, Value<'v>>,
}

impl<'v> Display for StarlarkRuleCallable<'v> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &*self.id.borrow() {
            Some(id) => write!(f, "{}()", id.name),
            None => write!(f, "<unbound rule>"),
        }
    }
}

/// Errors around rule declaration, instantiation, validation, etc
#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum RuleError {
    #[error("The output of rule() may only be called after the module is loaded")]
    RuleCalledBeforeFreezing,
    #[error("Rule used before the module defining it finished loading")]
    UsedBeforeLoaded,
    #[error("`{0}` is not a valid attribute name")]
    InvalidParameterName(String),
    #[error("Rule defined in `{0}` must be assigned to a variable, e.g. `my_rule = rule(...)`")]
    RuleNotAssigned(BzlOrBxlPath),
    #[error(
        "Rule defined with both `is_configuration_rule` and `is_toolchain_rule`, these options are mutually exclusive"
    )]
    IsConfigurationAndToolchain,
    #[error("`rule` can only be declared in bzl files")]
    RuleNotInBzl,
    #[error("Cannot specify `cfg` and `supports_incoming_transition` at the same time")]
    CfgAndSupportsIncomingTransition,
    #[error("{0} rules do not support incoming transitions")]
    RuleDoesNotSupportIncomingTransition(&'static str),
    #[error(
        "Attribute `{0}` uses the default execution group, which the rule does not declare. Add `DefaultExecGroup` to `exec_groups`, or put the dependency in a named execution group"
    )]
    DefaultExecutionGroupNotDeclared(String),
    #[error(
        "Attribute `{attr}` contains an `{function}` without `exec_group`. A rule that declares named execution groups must give every exec dep and toolchain dep an execution group: add `exec_group = DefaultExecGroup` (and list `DefaultExecGroup` in `exec_groups`) or a named execution group"
    )]
    ToolAttributeWithoutExecutionGroup {
        attr: String,
        function: &'static str,
    },
    #[error(
        "Attribute `{0}` constrains the default execution group, which the rule does not declare. Add `DefaultExecGroup` to `exec_groups`, or constrain a named execution group"
    )]
    DefaultExecCompatibleWithNotDeclared(String),
    #[error(
        "The default of attribute `{attr}` contains `$({macro_})`, a tool, in an `attrs.arg()` without `exec_group`. In a rule that declares named execution groups, an `attrs.arg()` that refers to tools must name its execution group: give it `exec_group = DefaultExecGroup` (and list `DefaultExecGroup` in `exec_groups`) or a named execution group"
    )]
    ToolInUngroupedArgDefault { attr: String, macro_: String },
    #[error(
        "Target `{target}`: attribute `{attr}` contains `$({macro_})`, a tool, in an `attrs.arg()` without `exec_group`. The rule declares named execution groups, so an `attrs.arg()` that refers to tools must name its execution group; this is fixed in the rule, by giving that `attrs.arg()` an `exec_group`, not in the BUCK file"
    )]
    ToolInUngroupedArg {
        target: String,
        attr: String,
        macro_: String,
    },
    #[error(
        "The rule uses plugins, which are dependencies in the default execution group, but does not declare it. Add `DefaultExecGroup` to `exec_groups`"
    )]
    PluginsWithoutDefaultExecutionGroup,
    #[error(
        "Execution groups of a rule must have distinct names; `{name}` is listed more than once{defined_in}"
    )]
    DuplicateExecutionGroup { name: String, defined_in: String },
    #[error(
        "Toolchain rules cannot use execution groups; their exec deps belong to the execution group of the rule depending on the toolchain"
    )]
    ExecutionGroupsOnToolchainRule,
    #[error(
        "Configuration rules cannot use execution groups; their targets are configured without an execution platform"
    )]
    ExecutionGroupsOnConfigurationRule,
    #[error(
        "Attribute `{attr}` uses execution group `{group}`, which the rule does not declare. Declared execution groups: [{declared}]{message}"
    )]
    UndeclaredExecutionGroup {
        attr: String,
        group: String,
        declared: String,
        message: String,
    },
    #[error(
        "Execution group `{group}` has more than one `attrs.exec_compatible_with()` attribute: `{first}` and `{second}`"
    )]
    MultipleExecCompatibleWith {
        group: String,
        first: String,
        second: String,
    },
}

impl<'v> AllocValue<'v> for StarlarkRuleCallable<'v> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_complex(self)
    }
}

impl<'v> StarlarkRuleCallable<'v> {
    fn new(
        implementation: RuleImpl<'v>,
        attrs: ValueOf<'v, DictType<&'v str, &'v StarlarkAttribute>>,
        cfg: Option<ValueAsTransition>,
        supports_incoming_transition: Option<bool>,
        doc: &str,
        is_configuration_rule: bool,
        is_toolchain_rule: bool,
        uses_plugins: Vec<PluginKind>,
        execution_groups: Vec<ExecutionGroup>,
        artifact_promise_mappings: Option<ArtifactPromiseMappings<'v>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> buck2_error::Result<StarlarkRuleCallable<'v>> {
        let build_context = BuildContext::from_context(eval)?;

        let rule_path: BzlOrBxlPath = match (&build_context.additional, &implementation) {
            (PerFileTypeContext::Bzl(bzl_path), RuleImpl::BuildRule(_)) => {
                BzlOrBxlPath::Bzl(bzl_path.bzl_path.clone())
            }
            (PerFileTypeContext::Bxl(bxl_path), RuleImpl::BxlAnon(_)) => {
                BzlOrBxlPath::Bxl(bxl_path.clone())
            }
            (PerFileTypeContext::Bxl(_), RuleImpl::BuildRule(_)) => {
                return Err(RuleError::RuleNotInBzl.into());
            }
            // TODO(nero): add error for it
            (_, _) => unreachable!(
                "unreachable, since bxl.anon_rule is not registered for eval for bzl files"
            ),
        };

        let attrs = attrs.try_items().collect::<starlark::Result<Vec<_>>>()?;

        let tool_marks: Vec<(String, ToolMarks)> = attrs
            .iter()
            .map(|(name, attr)| ((*name).to_owned(), attr.tool_marks()))
            .collect();

        let exec_compatible_with_attrs: Vec<(String, ExecutionGroup)> = attrs
            .iter()
            .filter_map(|(name, attr)| {
                Some((
                    (*name).to_owned(),
                    attr.exec_compatible_with_group()?.dupe(),
                ))
            })
            .collect();

        let sorted_validated_attrs = attrs
            .into_iter()
            .sorted_by(|(k1, _), (k2, _)| Ord::cmp(k1, k2))
            .map(|(name, value)| {
                if name == NAME_ATTRIBUTE_FIELD {
                    Err(RuleError::InvalidParameterName(NAME_ATTRIBUTE_FIELD.to_owned()).into())
                } else {
                    Ok((name.to_owned(), value.clone_attribute()))
                }
            })
            .collect::<buck2_error::Result<Vec<(String, Attribute)>>>()?;

        let cfg = match (cfg, supports_incoming_transition) {
            (Some(_), Some(_)) => return Err(RuleError::CfgAndSupportsIncomingTransition.into()),
            (Some(cfg), None) => RuleIncomingTransition::Fixed(cfg.transition_id()?),
            (None, Some(true)) => RuleIncomingTransition::FromAttribute,
            (None, Some(false) | None) => RuleIncomingTransition::None,
        };

        let rule_kind = match (is_configuration_rule, is_toolchain_rule) {
            (false, false) => RuleKind::Normal,
            (true, false) => RuleKind::Configuration,
            (false, true) => RuleKind::Toolchain,
            (true, true) => return Err(RuleError::IsConfigurationAndToolchain.into()),
        };

        if cfg != RuleIncomingTransition::None {
            let unsupported_rule_kind_str = match rule_kind {
                RuleKind::Normal => None,
                RuleKind::Configuration => Some("Configuration"),
                RuleKind::Toolchain => Some("Toolchain"),
            };
            if let Some(unsupported_rule_kind_str) = unsupported_rule_kind_str {
                return Err(RuleError::RuleDoesNotSupportIncomingTransition(
                    unsupported_rule_kind_str,
                )
                .into());
            }
        }

        let attributes = AttributeSpec::from(
            sorted_validated_attrs,
            artifact_promise_mappings.is_some(),
            &cfg,
        )?;

        let (
            execution_groups,
            default_execution_group,
            default_exec_compatible_with_attr,
            ungrouped_arg_attrs,
        ) = validate_execution_groups(
            execution_groups,
            rule_kind,
            &attributes,
            exec_compatible_with_attrs,
            build_context.cell_resolver(),
            &tool_marks,
            !uses_plugins.is_empty(),
        )?;
        let ty = Ty::ty_function(attributes.ty_function());

        Ok(StarlarkRuleCallable {
            rule_path,
            id: RefCell::new(None),
            implementation,
            attributes,
            ty,
            cfg,
            rule_kind,
            uses_plugins,
            execution_groups,
            default_execution_group,
            default_exec_compatible_with_attr,
            ungrouped_arg_attrs,
            docs: Some(doc.to_owned()),
            ignore_attrs_for_profiling: build_context.ignore_attrs_for_profiling,
            artifact_promise_mappings,
        })
    }

    fn new_anon_impl(
        implementation: RuleImpl<'v>,
        attrs: ValueOf<'v, DictType<&'v str, &'v StarlarkAttribute>>,
        doc: &str,
        artifact_promise_mappings: ValueOf<
            'v,
            DictType<
                StringValue<'v>,
                StarlarkCallable<'v, (Value<'static>,), ListType<Value<'static>>>,
            >,
        >,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> buck2_error::Result<Self> {
        Self::new(
            implementation,
            attrs,
            None,
            None,
            doc,
            false,
            false,
            Vec::new(),
            Vec::new(),
            Some(ArtifactPromiseMappings {
                mappings: artifact_promise_mappings
                    .try_items()
                    .map(|entry| {
                        let (k, v) = entry?;
                        Ok::<_, starlark::Error>((k, v.0))
                    })
                    .collect::<starlark::Result<BuckMutInsertionMap<_, _>>>()?,
            }),
            eval,
        )
    }

    fn new_anon(
        implementation: StarlarkCallable<'v, (Value<'static>,), ListType<Value<'static>>>,
        attrs: ValueOf<'v, DictType<&'v str, &'v StarlarkAttribute>>,
        doc: &str,
        artifact_promise_mappings: ValueOf<
            'v,
            DictType<
                StringValue<'v>,
                StarlarkCallable<'v, (Value<'static>,), ListType<Value<'static>>>,
            >,
        >,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> buck2_error::Result<Self> {
        Self::new_anon_impl(
            RuleImpl::BuildRule(implementation),
            attrs,
            doc,
            artifact_promise_mappings,
            eval,
        )
    }

    pub fn new_bxl_anon(
        implementation: StarlarkCallable<
            'v,
            (Value<'static>, Value<'static>),
            ListType<Value<'static>>,
        >,
        attrs: ValueOf<'v, DictType<&'v str, &'v StarlarkAttribute>>,
        doc: &str,
        artifact_promise_mappings: ValueOf<
            'v,
            DictType<
                StringValue<'v>,
                StarlarkCallable<'v, (Value<'static>,), ListType<Value<'static>>>,
            >,
        >,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> buck2_error::Result<Self> {
        Self::new_anon_impl(
            RuleImpl::BxlAnon(implementation),
            attrs,
            doc,
            artifact_promise_mappings,
            eval,
        )
    }

    fn documentation_impl(&self) -> DocItem {
        let name = self
            .id
            .borrow()
            .as_ref()
            .map_or_else(|| "unbound_rule".to_owned(), |rt| rt.name.clone());
        let parameters_spec = self.attributes.signature_with_default_value(name);
        let parameter_types = self.attributes.starlark_types();
        let parameter_docs = self.attributes.docstrings();
        let params = parameters_spec.documentation_with_default_value_formatter(
            parameter_types,
            parameter_docs,
            String::clone,
        );

        let function_docs = DocFunction::from_docstring(
            DocStringKind::Starlark,
            params,
            Ty::none(),
            self.docs.as_deref(),
        );

        DocItem::Member(DocMember::Function(function_docs))
    }
}

/// Checks the `exec_groups` a rule declares, and that every execution group its attributes
/// use is among them. Returns the named groups, whether the rule may use the default execution
/// group (implicitly when it declares no named groups, otherwise only when listed), and the
/// `attrs.exec_compatible_with()` attribute of the default group. In a rule with named groups,
/// every exec dep and toolchain dep must name its execution group, `DefaultExecGroup`
/// included; args without one are recorded so their values can be checked for tools.
fn validate_execution_groups(
    execution_groups: Vec<ExecutionGroup>,
    rule_kind: RuleKind,
    attributes: &AttributeSpec,
    exec_compatible_with_attrs: Vec<(String, ExecutionGroup)>,
    cell_resolver: &CellResolver,
    tool_marks: &[(String, ToolMarks)],
    uses_plugins: bool,
) -> buck2_error::Result<(
    Vec<RuleExecutionGroup>,
    bool,
    Option<AttributeId>,
    Vec<AttributeId>,
)> {
    // The project-relative path of the file that defines `group`, or its cell path when that
    // does not resolve, so the error that names the file always surfaces.
    let defining_file = |group: &NamedExecutionGroup| {
        let file = group.defining_file();
        match cell_resolver.resolve_path(file) {
            Ok(path) => path.to_string(),
            Err(_) => file.to_string(),
        }
    };

    if !execution_groups.is_empty() {
        match rule_kind {
            RuleKind::Normal => {}
            RuleKind::Toolchain => return Err(RuleError::ExecutionGroupsOnToolchainRule.into()),
            RuleKind::Configuration => {
                return Err(RuleError::ExecutionGroupsOnConfigurationRule.into());
            }
        }
    }

    let mut declared: Vec<RuleExecutionGroup> = Vec::with_capacity(execution_groups.len());
    let mut default_listed = false;
    for group in execution_groups {
        let group = match group {
            ExecutionGroup::Default if default_listed => {
                return Err(RuleError::DuplicateExecutionGroup {
                    name: group.to_string(),
                    defined_in: String::new(),
                }
                .into());
            }
            ExecutionGroup::Default => {
                default_listed = true;
                continue;
            }
            ExecutionGroup::Named(group) => group,
        };
        // Names, not `(name, .bzl file)`, must be distinct: users see a target's groups by name.
        if let Some(first) = declared.iter().find(|g| g.group.name() == group.name()) {
            let defined_in = if first.group == group {
                String::new()
            } else {
                format!(
                    " (defined in `{}` and in `{}`)",
                    defining_file(&first.group),
                    defining_file(&group)
                )
            };
            return Err(RuleError::DuplicateExecutionGroup {
                name: group.name().to_owned(),
                defined_in,
            }
            .into());
        }
        declared.push(RuleExecutionGroup {
            group,
            exec_compatible_with_attr: None,
        });
    }

    let mut used: Vec<(&str, NamedExecutionGroup)> = Vec::new();
    for (name, _, attr) in attributes.attr_specs() {
        attr.coercer()
            .for_each_named_execution_group(&mut |group| used.push((name, group.dupe())));
    }
    used.extend(
        exec_compatible_with_attrs
            .iter()
            .filter_map(|(name, group)| match group {
                ExecutionGroup::Named(group) => Some((name.as_str(), group.dupe())),
                ExecutionGroup::Default => None,
            }),
    );
    if rule_kind == RuleKind::Toolchain
        && !(used.is_empty() && exec_compatible_with_attrs.is_empty())
    {
        return Err(RuleError::ExecutionGroupsOnToolchainRule.into());
    }
    if let Some((attr, group)) = used
        .into_iter()
        .find(|(_, g)| !declared.iter().any(|d| &d.group == g))
    {
        return Err(RuleError::UndeclaredExecutionGroup {
            attr: attr.to_owned(),
            group: group.name().to_owned(),
            declared: declared
                .iter()
                .map(|g| format!("`{}`", g.group))
                .collect::<Vec<_>>()
                .join(", "),
            message: match group.same_name_in(declared.iter().map(|d| &d.group)) {
                Some(d) => format!(
                    ". The declared `{}` is defined in `{}`, this one in `{}`",
                    group.name(),
                    defining_file(d),
                    defining_file(&group)
                ),
                None => String::new(),
            },
        }
        .into());
    }

    let mut default_exec_compatible_with_attr: Option<AttributeId> = None;
    for (name, group) in exec_compatible_with_attrs {
        let slot = match &group {
            ExecutionGroup::Default => &mut default_exec_compatible_with_attr,
            ExecutionGroup::Named(named) => {
                &mut declared
                    .iter_mut()
                    .find(|d| &d.group == named)
                    .expect("checked above that every used group is declared")
                    .exec_compatible_with_attr
            }
        };
        if let Some(first) = slot {
            return Err(RuleError::MultipleExecCompatibleWith {
                group: group.to_string(),
                first: attributes.attribute_name_by_id(*first).to_owned(),
                second: name,
            }
            .into());
        }
        *slot = Some(
            attributes
                .attribute_id_by_name(&name)
                .expect("every attribute of the rule is in its spec"),
        );
    }

    if !declared.is_empty() {
        if let Some((attr, function)) = tool_marks
            .iter()
            .find_map(|(attr, marks)| Some((attr, marks.first_unmarked_dep()?)))
        {
            return Err(RuleError::ToolAttributeWithoutExecutionGroup {
                attr: attr.clone(),
                function,
            }
            .into());
        }
    }

    // In a rule with named groups, only args without tools may omit `exec_group`; target values
    // are checked when targets are created, defaults here.
    let mut ungrouped_arg_attrs = Vec::new();
    if !declared.is_empty() {
        for (attr, marks) in tool_marks {
            if !marks.contains(ToolMarks::UNMARKED_ARG) {
                continue;
            }
            let tool = match attributes.attribute(attr) {
                Some(a) => match a.default_as_ref() {
                    Some(default) => default.first_ungrouped_tool_macro(a.coercer(), None)?,
                    None => None,
                },
                None => None,
            };
            if let Some(tool) = tool {
                return Err(RuleError::ToolInUngroupedArgDefault {
                    attr: attr.clone(),
                    macro_: tool,
                }
                .into());
            }
            ungrouped_arg_attrs.push(
                attributes
                    .attribute_id_by_name(attr)
                    .expect("every attribute of the rule is in its spec"),
            );
        }
    }

    let default_execution_group = declared.is_empty() || default_listed;
    if !default_execution_group {
        if let Some(attr) = attributes
            .attr_specs()
            .find(|(_, _, attr)| attr.coercer().uses_default_execution_group())
            .map(|(attr, _, _)| attr)
            .or_else(|| {
                tool_marks
                    .iter()
                    .find(|(_, marks)| marks.contains(ToolMarks::DEFAULT_GROUP_ARG))
                    .map(|(attr, _)| attr.as_str())
            })
        {
            return Err(RuleError::DefaultExecutionGroupNotDeclared(attr.to_owned()).into());
        }
        if let Some(attr) = default_exec_compatible_with_attr {
            return Err(RuleError::DefaultExecCompatibleWithNotDeclared(
                attributes.attribute_name_by_id(attr).to_owned(),
            )
            .into());
        }
        if uses_plugins {
            return Err(RuleError::PluginsWithoutDefaultExecutionGroup.into());
        }
    }
    Ok((
        declared,
        default_execution_group,
        default_exec_compatible_with_attr,
        ungrouped_arg_attrs,
    ))
}

#[starlark_value]
impl<'v> StarlarkValue<'v> for StarlarkRuleCallable<'v> {
    type Type = StarlarkRuleCallableView<'v>;
}

#[derive(Debug, ProvidesStaticType, Allocative, Clone, Dupe, StarlarkPagable)]
enum FrozenRuleImpl<'v> {
    BuildRule(StarlarkCallable<'v, (Value<'static>,), ListType<Value<'static>>>),
    BxlAnon(StarlarkCallable<'v, (Value<'static>, Value<'static>), ListType<Value<'static>>>),
}

impl<'v> FrozenRuleImpl<'v> {
    fn into_value(self) -> Value<'v> {
        match self {
            FrozenRuleImpl::BuildRule(callable) => callable.0,
            FrozenRuleImpl::BxlAnon(callable) => callable.0,
        }
    }
}

impl<'v> Freeze<'v> for RuleImpl<'v> {
    type Frozen<'fv> = FrozenRuleImpl<'fv>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        match self {
            RuleImpl::BuildRule(impl_) => Ok(FrozenRuleImpl::BuildRule(impl_.freeze(freezer)?)),
            RuleImpl::BxlAnon(impl_) => Ok(FrozenRuleImpl::BxlAnon(impl_.freeze(freezer)?)),
        }
    }
}

impl<'v> Freeze<'v> for StarlarkRuleCallable<'v> {
    type Frozen<'fv> = FrozenStarlarkRuleCallable<'fv>;
    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        let frozen_impl = self.implementation.freeze(freezer)?;
        let rule_docs = self.documentation_impl();
        let id = match self.id.into_inner() {
            Some(x) => x,
            None => {
                return Err(FreezeError::new(
                    RuleError::RuleNotAssigned(self.rule_path).to_string(),
                ));
            }
        };
        let rule_type = Arc::new(id);
        let rule_name = rule_type.name.to_owned();

        let signature = self.attributes.signature(rule_name);

        let artifact_promise_mappings = match self.artifact_promise_mappings {
            Some(artifacts) => {
                let mut mappings = BuckMutInsertionMap::new();
                for (name, implementation) in artifacts.mappings {
                    mappings.insert(name.freeze(freezer)?, implementation.freeze(freezer)?);
                }
                Some(FrozenArtifactPromiseMappings { mappings })
            }
            None => None,
        };

        Ok(FrozenStarlarkRuleCallable {
            rule: Arc::new(Rule {
                attributes: self.attributes,
                rule_type: RuleType::Starlark(rule_type.dupe()),
                cfg: self.cfg,
                rule_kind: self.rule_kind,
                uses_plugins: self.uses_plugins,
                execution_groups: self.execution_groups,
                default_execution_group: self.default_execution_group,
                default_exec_compatible_with_attr: self.default_exec_compatible_with_attr,
            }),
            rule_type,
            implementation: frozen_impl,
            signature,
            rule_docs,
            ty: self.ty,
            ignore_attrs_for_profiling: self.ignore_attrs_for_profiling,
            artifact_promise_mappings,
            ungrouped_arg_attrs: self.ungrouped_arg_attrs,
        })
    }
}

#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("{}()", rule.rule_type.name())]
pub struct FrozenStarlarkRuleCallable<'v> {
    #[starlark_pagable(pagable)]
    rule: Arc<Rule>,
    /// Identical to `rule.rule_type` but more specific type.
    #[starlark_pagable(pagable)]
    rule_type: Arc<StarlarkRuleType>,
    implementation: FrozenRuleImpl<'v>,
    /// Defaults are read from `rule.attributes`, never from here.
    signature: ParametersSpec<Value<'v>>,
    #[starlark_pagable(pagable)]
    rule_docs: DocItem,
    #[starlark_pagable(pagable)]
    ty: Ty,
    ignore_attrs_for_profiling: bool,
    artifact_promise_mappings: Option<FrozenArtifactPromiseMappings<'v>>,
    /// See `StarlarkRuleCallable::ungrouped_arg_attrs`.
    #[starlark_pagable(pagable)]
    ungrouped_arg_attrs: Vec<AttributeId>,
}

fn downcast_frozen_rule<'v>(
    rule: Value<'v>,
) -> buck2_error::Result<ValueTyped<'v, FrozenStarlarkRuleCallable<'v>>> {
    ValueTyped::new(rule).internal_error("Expecting FrozenRuleCallable")
}

/// A rule in either of its representations; the binding for a native parameter of type `Rule`.
#[derive(Debug, Clone, Copy, StarlarkRepresentations)]
pub enum StarlarkRuleCallableView<'v> {
    Unfrozen(ValueTyped<'v, StarlarkRuleCallable<'v>>),
    Frozen(ValueTyped<'v, FrozenStarlarkRuleCallable<'v>>),
}

impl<'v> StarlarkRuleCallableView<'v> {
    /// The rule once its module is loaded, which is what instantiating it requires. A rule can
    /// only be defined while its module loads, and nothing that instantiates rules runs then, so
    /// the unfrozen representation never reaches such a function.
    pub fn frozen(self) -> buck2_error::Result<ValueTyped<'v, FrozenStarlarkRuleCallable<'v>>> {
        match self {
            StarlarkRuleCallableView::Frozen(rule) => Ok(rule),
            StarlarkRuleCallableView::Unfrozen(_) => Err(RuleError::UsedBeforeLoaded.into()),
        }
    }
}

#[starlark_value(type = "Rule")]
impl<'v> StarlarkType<'v> for StarlarkRuleCallableView<'v> {
    fn export_as(
        &self,
        variable_name: &str,
        _eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<()> {
        if let StarlarkRuleCallableView::Unfrozen(this) = self {
            *this.id.borrow_mut() = Some(StarlarkRuleType {
                path: this.rule_path.clone(),
                name: variable_name.to_owned(),
            });
        }
        Ok(())
    }

    fn invoke(
        &self,
        _me: Value<'v>,
        args: &Arguments<'v, '_>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<Value<'v>> {
        let this = match self {
            StarlarkRuleCallableView::Frozen(this) => this,
            StarlarkRuleCallableView::Unfrozen(_) => {
                return Err(buck2_error::Error::from(RuleError::RuleCalledBeforeFreezing).into());
            }
        };
        let record_target_call_stack =
            ModuleInternals::from_context(eval, this.rule.rule_type.name())?
                .record_target_call_stacks();
        let call_stack = if record_target_call_stack {
            Some(eval.call_stack())
        } else {
            None
        };
        let arg_count = args.len()?;
        this.signature.parser(args, eval, |param_parser, eval| {
            // The body of the callable returned by `rule()`.
            // Records the target in this package's `TargetMap`.
            let internals = ModuleInternals::from_context(eval, this.rule.rule_type.name())?;
            let coercion_ctx = MemoizedCoercionCtx {
                base: internals.attr_coercion_context(),
                memo: &InterpreterExtraValue::get(eval.module())?.coerce_memo,
            };
            let target_node = UnsealedTargetNode::from_params(
                this.rule.dupe(),
                internals.package(),
                internals,
                &coercion_ctx,
                param_parser,
                arg_count,
                this.ignore_attrs_for_profiling,
                call_stack,
            )?;
            this.check_ungrouped_args(&target_node, internals)?;
            internals.record(target_node)?;
            Ok(Value::new_none())
        })
    }

    fn documentation(&self) -> DocItem {
        match self {
            StarlarkRuleCallableView::Unfrozen(this) => this.documentation_impl(),
            StarlarkRuleCallableView::Frozen(this) => this.rule_docs.clone(),
        }
    }

    fn typechecker_ty(&self) -> Option<Ty> {
        match self {
            StarlarkRuleCallableView::Unfrozen(this) => Some(this.ty.clone()),
            StarlarkRuleCallableView::Frozen(this) => Some(this.ty.clone()),
        }
    }

    fn get_type_starlark_repr() -> Ty {
        Ty::function(ParamSpec::kwargs(Ty::any()), Ty::none())
    }
}

/// The `AttributeSpec` of a frozen `rule()` / `anon_rule()` / `bxl.anon_rule()` value.
pub fn frozen_rule_attribute_spec<'v>(rule: Value<'v>) -> buck2_error::Result<&'v AttributeSpec> {
    Ok(downcast_frozen_rule(rule)?.as_ref().attributes())
}

pub(crate) fn init_frozen_rule_get_impl() {
    FROZEN_RULE_GET_IMPL.init(|rule| {
        let rule = downcast_frozen_rule(rule)?;
        Ok(rule.as_ref().implementation.dupe().into_value())
    })
}

pub(crate) fn init_frozen_promise_artifact_mappings_get_impl() {
    FROZEN_PROMISE_ARTIFACT_MAPPINGS_GET_IMPL.init(|rule| {
        let rule = downcast_frozen_rule(rule)?;
        Ok(rule
            .as_ref()
            .artifact_promise_mappings
            .as_ref()
            .map_or_else(BuckMutInsertionMap::new, |m| m.mappings.clone()))
    })
}

impl<'v> FrozenStarlarkRuleCallable<'v> {
    pub fn rule_type(&self) -> &Arc<StarlarkRuleType> {
        &self.rule_type
    }

    pub fn attributes(&self) -> &AttributeSpec {
        &self.rule.attributes
    }

    pub fn declares_execution_groups(&self) -> bool {
        !self.rule.execution_groups.is_empty()
    }

    /// Rejects tools in the values of `attrs.arg()` attributes without `exec_group` in a rule
    /// that declares named execution groups.
    fn check_ungrouped_args(
        &self,
        target_node: &UnsealedTargetNode,
        internals: &ModuleInternals,
    ) -> buck2_error::Result<()> {
        if self.ungrouped_arg_attrs.is_empty() {
            return Ok(());
        }
        let pkg = target_node.label.pkg();
        internals
            .attr_coercion_context()
            .with_attr_arena_ref(|arena| {
                for &id in &self.ungrouped_arg_attrs {
                    let Some(value) = target_node.attributes.get(id) else {
                        continue;
                    };
                    let name = target_node.rule.attributes.attribute_name_by_id(id);
                    let attr = target_node
                        .rule
                        .attributes
                        .attribute(name)
                        .internal_error("attribute id is in the rule's spec")?;
                    if let Some(tool) = arena
                        .view(value)
                        .first_ungrouped_tool_macro(attr.coercer(), Some(pkg.dupe()))?
                    {
                        return Err(RuleError::ToolInUngroupedArg {
                            target: target_node.label.to_string(),
                            attr: name.to_owned(),
                            macro_: tool,
                        }
                        .into());
                    }
                }
                Ok(())
            })
    }

    pub fn artifact_promise_mappings(&self) -> &Option<FrozenArtifactPromiseMappings<'v>> {
        &self.artifact_promise_mappings
    }
}

#[starlark_value(frozen_vtable)]
impl<'v> StarlarkValue<'v> for FrozenStarlarkRuleCallable<'v> {
    type Type = StarlarkRuleCallableView<'v>;
}

#[starlark_module]
#[starlark_types(StarlarkRuleCallable<'_> as Rule)]
pub fn register_rule_function(builder: &mut GlobalsBuilder) {
    /// Define a rule. As a simple example:
    ///
    /// ```python
    /// def _my_rule(ctx: AnalysisContext) -> list[Provider]:
    ///     output = ctx.actions.write("hello.txt", ctx.attrs.contents, executable = ctx.attrs.exe)
    ///     return [DefaultInfo(outputs = [output])]
    ///
    /// MyRule = rule(impl = _my_rule, attrs = {
    ///     "contents": attrs.string(),
    ///     "exe": attrs.option(attrs.bool(), default = False),
    /// })
    /// ```
    fn rule<'v>(
        #[starlark(require = named)] r#impl: StarlarkCallableChecked<
            'v,
            (AnalysisContextReprLate,),
            Either<ListType<ProviderReprLate>, StarlarkPromise>,
        >,
        #[starlark(require = named)] attrs: ValueOf<'v, DictType<&'v str, &'v StarlarkAttribute>>,
        #[starlark(require = named)] cfg: Option<ValueAsTransition<'v>>,
        #[starlark(require = named)] supports_incoming_transition: Option<bool>,
        #[starlark(require = named, default = "")] doc: &str,
        #[starlark(require = named, default = false)] is_configuration_rule: bool,
        #[starlark(require = named, default = false)] is_toolchain_rule: bool,
        #[starlark(require = named, default = ValueOf::empty_list())] uses_plugins: ValueOf<
            'v,
            ListOrTupleType<PluginKindArg>,
        >,
        #[starlark(require = named, default = ValueOf::empty_list())] exec_groups: ValueOf<
            'v,
            ListOrTupleType<ExecutionGroupArg>,
        >,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkRuleCallable<'v>> {
        Ok(StarlarkRuleCallable::new(
            RuleImpl::BuildRule(StarlarkCallable::unchecked_new(r#impl.0)),
            attrs,
            cfg,
            supports_incoming_transition,
            doc,
            is_configuration_rule,
            is_toolchain_rule,
            uses_plugins
                .try_items()
                .map(|arg| arg.map(|PluginKindArg { plugin_kind }| plugin_kind))
                .collect::<starlark::Result<_>>()?,
            exec_groups
                .try_items()
                .map(|arg| arg.map(|ExecutionGroupArg { group }| group))
                .collect::<starlark::Result<_>>()?,
            None,
            eval,
        )?)
    }

    /// Define an anon rule, similar to how a normal rule is defined, except with an extra `artifact_promise_mappings` field. This
    /// is a dict where the keys are the string name of the artifact, and the values are the callable functions that produce
    /// the artifact. This is only intended to be used with anon targets.
    fn anon_rule<'v>(
        #[starlark(require = named)] r#impl: StarlarkCallable<
            'v,
            (Value<'static>,),
            ListType<Value<'static>>,
        >,
        #[starlark(require = named)] attrs: ValueOf<'v, DictType<&'v str, &'v StarlarkAttribute>>,
        #[starlark(require = named, default = "")] doc: &str,
        #[starlark(require = named)] artifact_promise_mappings: ValueOf<
            'v,
            DictType<
                StringValue<'v>,
                StarlarkCallable<'v, (Value<'static>,), ListType<Value<'static>>>,
            >,
        >,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkRuleCallable<'v>> {
        StarlarkRuleCallable::new_anon(r#impl, attrs, doc, artifact_promise_mappings, eval)
            .map_err(Into::into)
    }
}
