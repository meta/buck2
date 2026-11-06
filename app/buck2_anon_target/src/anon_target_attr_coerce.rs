/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cmp::Ordering;
use std::fmt::Debug;
use std::iter;

use buck2_build_api::artifact_groups::promise::PromiseArtifactAttr;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_artifact_like::StarlarkArtifactView;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_promise_artifact::StarlarkPromiseArtifact;
use buck2_build_api::interpreter::rule_defs::provider::dependency::Dependency;
use buck2_build_api::interpreter::rule_defs::resolved_macro::ResolvedStringWithMacros;
use buck2_core::deprecation;
use buck2_core::execution_types::execution_group::ExecutionGroup;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::provider::label::ProvidersLabel;
use buck2_interpreter::types::configured_providers_label::StarlarkProvidersLabel;
use buck2_interpreter::types::target_label::StarlarkTargetLabel;
use buck2_node::attrs::attr_type::AttrType;
use buck2_node::attrs::attr_type::AttrTypeInner;
use buck2_node::attrs::attr_type::arg::ConfiguredStringWithMacros;
use buck2_node::attrs::attr_type::arg::MacroBase;
use buck2_node::attrs::attr_type::arg::MacroDepKind;
use buck2_node::attrs::attr_type::arg::StringWithMacros;
use buck2_node::attrs::attr_type::arg::StringWithMacrosPart;
use buck2_node::attrs::attr_type::bool::BoolLiteral;
use buck2_node::attrs::attr_type::dep::DepAttr;
use buck2_node::attrs::attr_type::dep::DepAttrTransition;
use buck2_node::attrs::attr_type::dict::DictAttrType;
use buck2_node::attrs::attr_type::dict::DictLiteral;
use buck2_node::attrs::attr_type::list::ListAttrType;
use buck2_node::attrs::attr_type::list::ListLiteral;
use buck2_node::attrs::attr_type::one_of::OneOfAttrType;
use buck2_node::attrs::attr_type::string::StringLiteral;
use buck2_node::attrs::attr_type::tuple::TupleAttrType;
use buck2_node::attrs::attr_type::tuple::TupleLiteral;
use dupe::Dupe;
use dupe::IterDupedExt;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::dict::Dict;
use starlark::values::dict::DictRef;
use starlark::values::list::ListRef;
use starlark::values::string::STRING_TYPE;
use starlark::values::tuple::TupleRef;

use crate::anon_target_attr::AnonTargetAttr;
use crate::anon_targets::AnonAttrCtx;

pub trait AnonTargetAttrTypeCoerce {
    fn coerce_item(&self, ctx: &AnonAttrCtx, value: Value) -> buck2_error::Result<AnonTargetAttr>;
}

impl AnonTargetAttrTypeCoerce for AttrType {
    fn coerce_item(&self, ctx: &AnonAttrCtx, value: Value) -> buck2_error::Result<AnonTargetAttr> {
        match &self.0.inner {
            AttrTypeInner::Any(_) => to_anon_target_any(value, ctx),
            AttrTypeInner::Bool(_) => match value.unpack_bool() {
                Some(s) => Ok(AnonTargetAttr::Bool(BoolLiteral(s))),
                None => Err(AnonTargetCoercionError::type_error("bool", value).into()),
            },
            AttrTypeInner::Int(_) => match i64::unpack_value(value)? {
                Some(x) => Ok(AnonTargetAttr::Int(x)),
                None => Err(AnonTargetCoercionError::type_error("int", value).into()),
            },
            AttrTypeInner::Dict(x) => to_anon_target_dict(x, ctx, value),
            AttrTypeInner::List(x) => to_anon_target_list(x, ctx, value),
            AttrTypeInner::Tuple(x) => to_anon_target_tuple(x, ctx, value),
            AttrTypeInner::OneOf(x) => to_anon_target_one_of(x, ctx, value),
            AttrTypeInner::Option(x) => {
                if value.is_none() {
                    Ok(AnonTargetAttr::None)
                } else {
                    Ok(x.inner.coerce_item(ctx, value)?)
                }
            }
            AttrTypeInner::String(_) => match value.unpack_str() {
                Some(s) => Ok(AnonTargetAttr::String(StringLiteral(ctx.intern_str(s)))),
                None => Err(AnonTargetCoercionError::type_error(STRING_TYPE, value).into()),
            },
            AttrTypeInner::Enum(x) => match value.unpack_str() {
                Some(s) => {
                    // Enum names in Buck can be specified upper or lower case,
                    // so we normalise them to lowercase to make rule implementations easier
                    let s = s.to_lowercase();
                    if let Some(s) = x.variants.get(s.as_str()) {
                        Ok(AnonTargetAttr::EnumVariant(StringLiteral(s.dupe())))
                    } else {
                        let wanted = x.variants.iter().map(|x| x.as_str().to_owned()).collect();
                        Err(AnonTargetCoercionError::InvalidEnumVariant(s, wanted).into())
                    }
                }
                None => Err(AnonTargetCoercionError::type_error(STRING_TYPE, value).into()),
            },
            AttrTypeInner::Dep(x) => {
                match Dependency::from_value(value) {
                    Some(dep) => {
                        let label = dep.label().inner().dupe();

                        let attr_type = match x.transition {
                            DepAttrTransition::Identity(..) => x.clone(),
                            DepAttrTransition::Exec => {
                                match dep.execution_platform() {
                                Some(exec_dep_resolution) => {
                                    // Compare only base platform, not the full resolution
                                    if exec_dep_resolution.platform()? != ctx.execution_platform_resolution.platform()? {
                                        return Err(AnonTargetCoercionError::ExecDepPlatformMismatch(exec_dep_resolution.platform()?.id(), ctx.execution_platform_resolution.platform()?.id()).into());
                                    }
                                },
                                None => return Err(AnonTargetCoercionError::ExecDepMissingExecPlatformResolution.into()),
                            }

                                x.clone()
                            }
                            _ => {
                                return Err(
                                    AnonTargetCoercionError::OnlyIdentityDepSupported.into()
                                );
                            }
                        };

                        Ok(AnonTargetAttr::Dep(Box::new(DepAttr { attr_type, label })))
                    }
                    _ => Err(AnonTargetCoercionError::type_error("dependency", value).into()),
                }
            }
            AttrTypeInner::Source(_) => {
                // Check if this is a StarlarkPromiseArtifact first before checking other artifact types to
                // allow anon targets to accept unresolved promise artifacts.
                if let Some(promise_artifact) = StarlarkPromiseArtifact::from_value(value) {
                    Ok(AnonTargetAttr::PromiseArtifact(Box::new(
                        PromiseArtifactAttr {
                            id: promise_artifact.artifact.id.clone(),
                            short_path: promise_artifact.short_path.clone(),
                            has_content_based_path: promise_artifact.has_content_based_path,
                        },
                    )))
                } else if let Some(artifact_like) = StarlarkArtifactView::unpack_value(value)? {
                    let artifact = artifact_like
                        .as_input_artifact_like()
                        .get_bound_artifact()?;
                    Ok(AnonTargetAttr::Artifact(artifact))
                } else {
                    // TODO(nga): `EnsuredArtifact` gets here with unhelpful error message like:
                    //    ```
                    //    Expected value of type `artifact`, got value with type `ensured_artifact`
                    //    (value was `<ensured <build artifact merged_cdb bound to ...
                    //    ```
                    Err(AnonTargetCoercionError::type_error("artifact", value).into())
                }
            }
            AttrTypeInner::Arg(_) => {
                if let Some(resolved_macro) = ResolvedStringWithMacros::from_value(value) {
                    match resolved_macro.configured_macros() {
                        Some(configured_macros) => {
                            check_tool_macro_platforms(ctx, configured_macros)?;
                            Ok(AnonTargetAttr::Arg(Box::new(configured_macros.clone())))
                        }
                        None => Err(AnonTargetCoercionError::ArgNotAnonTargetCompatible.into()),
                    }
                } else if let Some(s) = value.unpack_str() {
                    // It's fine to use a string for attrs.arg()
                    Ok(AnonTargetAttr::String(StringLiteral(ctx.intern_str(s))))
                } else {
                    Err(AnonTargetCoercionError::type_error("resolved_macro", value).into())
                }
            }
            AttrTypeInner::Label(_) => {
                if let Some(label) = StarlarkProvidersLabel::from_value(value) {
                    Ok(AnonTargetAttr::Label(label.label().dupe()))
                } else if let Some(label) = StarlarkTargetLabel::from_value(value) {
                    Ok(AnonTargetAttr::Label(ProvidersLabel::default_for(
                        label.label().dupe(),
                    )))
                } else {
                    Err(AnonTargetCoercionError::type_error(
                        "providers_label or target_label",
                        value,
                    )
                    .into())
                }
            }
            _ => Err(AnonTargetCoercionError::AttrTypeNotSupported(self.to_string()).into()),
        }
    }
}

/// Tools that args refer to (`$(exe)`, `$(location_exec)`, `$(location_toolchain)`) were
/// configured by the target that configured the arg, for the platform of their execution group in
/// that target's resolution; the anon target runs them on its own execution platform, so the two
/// must match. Only tools in execution groups are checked: those of grouped macros, and every tool
/// when the creator declares named groups or creates the anon target with `exec_group`.
fn check_tool_macro_platforms(
    ctx: &AnonAttrCtx,
    macros: &ConfiguredStringWithMacros,
) -> buck2_error::Result<()> {
    let (StringWithMacros::ManyParts(parts), Some(configured_by)) = (
        &macros.string_with_macros,
        macros.anon_target_tool_resolution(),
    ) else {
        return Ok(());
    };
    for part in parts.iter() {
        let StringWithMacrosPart::Macro(_, macro_) = part else {
            continue;
        };
        let tool_resolution = match macro_ {
            MacroBase::InGroup(box (_, group)) => {
                configured_by.resolution_for_group(&ExecutionGroup::Named(group.dupe()))?
            }
            MacroBase::Exe { exec_dep: true, .. }
            | MacroBase::Location {
                dep_kind: MacroDepKind::Exec | MacroDepKind::Toolchain,
                ..
            } if ctx.uses_execution_groups => configured_by.dupe(),
            _ => continue,
        };
        let tool_platform = tool_resolution.platform()?;
        let anon_platform = ctx.execution_platform_resolution.platform()?;
        if tool_platform != anon_platform {
            return Err(AnonTargetCoercionError::ArgToolPlatformMismatch(
                tool_macro_display(macro_),
                tool_platform.id(),
                anon_platform.id(),
            )
            .into());
        }
    }
    Ok(())
}

/// `$(exe //pkg:tool)` and the like, without the tool's configuration.
fn tool_macro_display(macro_: &MacroBase<ConfiguredProvidersLabel>) -> String {
    match macro_ {
        MacroBase::InGroup(box (inner, _)) => tool_macro_display(inner),
        MacroBase::Exe { label, .. } => format!("$(exe {})", label.unconfigured()),
        MacroBase::Location {
            label,
            dep_kind: MacroDepKind::Toolchain,
        } => format!("$(location_toolchain {})", label.unconfigured()),
        MacroBase::Location { label, .. } => format!("$(location_exec {})", label.unconfigured()),
        other => format!("$({other})"),
    }
}

#[derive(Debug, buck2_error::Error)]
pub(crate) enum AnonTargetCoercionError {
    #[error("Expected value of type `{0}`, got value with type `{1}` (value was `{2}`)")]
    #[buck2(tag = Input)]
    TypeError(String, String, String),
    #[error("Used one_of with an empty list.")]
    #[buck2(tag = Input)]
    OneOfEmpty,
    #[error("one_of fails, the errors against each alternative in turn were:\n{}", .0.iter().map(|x| format!("{x:#}")).collect::<Vec<_>>().join("\n"))]
    #[buck2(tag = Input)]
    OneOfMany(Vec<buck2_error::Error>),
    #[error("enum called with `{0}`, only allowed: {}", .1.iter().map(|x| format!("`{x}`")).collect::<Vec<_>>().join(", "))]
    #[buck2(tag = Input)]
    InvalidEnumVariant(String, Vec<String>),
    #[error("Cannot coerce value of type `{0}` to any: `{1}`")]
    #[buck2(tag = Input)]
    CannotCoerceToAny(&'static str, String),
    #[error("Attr value of type `{0}` not supported")]
    #[buck2(tag = Input)]
    AttrTypeNotSupported(String),
    #[error("Arg attribute must have `anon_target_compatible` set to `True`")]
    #[buck2(tag = Input)]
    ArgNotAnonTargetCompatible,
    #[error(
        "`{0}` refers to a tool built for execution platform `{1}`, but the anon target runs on `{2}`. Create the anon target with the `exec_group` of the attribute the tool comes from"
    )]
    #[buck2(tag = Input)]
    ArgToolPlatformMismatch(String, String, String),
    #[error("Internal error: exec dep is missing the execution platform resolution")]
    #[buck2(tag = Tier0)]
    ExecDepMissingExecPlatformResolution,
    #[error(
        "Exec deps and the current anon target must have the same execution platform resolution. Exec dep's execution platform: ({0}), anon target's execution platform: ({1})"
    )]
    #[buck2(tag = Input)]
    ExecDepPlatformMismatch(String, String),
    #[error(
        "`transition_dep`, and `toolchain_dep` are not supported. By design, anon targets do not support configurations/transitions."
    )]
    #[buck2(tag = Input)]
    OnlyIdentityDepSupported,
}

impl AnonTargetCoercionError {
    pub(crate) fn type_error(expected_type: &str, value: Value) -> AnonTargetCoercionError {
        AnonTargetCoercionError::TypeError(
            expected_type.to_owned(),
            value.get_type().to_owned(),
            value.to_repr(),
        )
    }

    pub fn one_of_many(mut errs: Vec<buck2_error::Error>) -> buck2_error::Error {
        if errs.is_empty() {
            AnonTargetCoercionError::OneOfEmpty.into()
        } else if errs.len() == 1 {
            errs.pop().unwrap()
        } else {
            AnonTargetCoercionError::OneOfMany(errs).into()
        }
    }
}

fn to_anon_target_any(value: Value, ctx: &AnonAttrCtx) -> buck2_error::Result<AnonTargetAttr> {
    if value.is_none() {
        Ok(AnonTargetAttr::None)
    } else if let Some(x) = value.unpack_bool() {
        Ok(AnonTargetAttr::Bool(BoolLiteral(x)))
    } else if let Some(x) = i64::unpack_value(value)? {
        Ok(AnonTargetAttr::Int(x))
    } else if let Some(x) = DictRef::from_value(value) {
        Ok(AnonTargetAttr::Dict(
            x.iter()
                .map(|(k, v)| Ok((to_anon_target_any(k, ctx)?, to_anon_target_any(v, ctx)?)))
                .collect::<buck2_error::Result<_>>()?,
        ))
    } else if let Some(x) = TupleRef::from_value(value) {
        // TODO(wendyy) intern attr
        Ok(AnonTargetAttr::Tuple(TupleLiteral(
            x.iter()
                .map(|v| to_anon_target_any(v, ctx))
                .collect::<buck2_error::Result<Vec<_>>>()?
                .into(),
        )))
    } else if let Some(x) = ListRef::from_value(value) {
        // TODO(wendyy) intern attr
        Ok(AnonTargetAttr::List(ListLiteral(
            x.iter()
                .map(|v| to_anon_target_any(v, ctx))
                .collect::<buck2_error::Result<Vec<_>>>()?
                .into(),
        )))
    } else if let Some(s) = value.unpack_str() {
        Ok(AnonTargetAttr::String(StringLiteral(ctx.intern_str(s))))
    } else {
        deprecation!(
            "coerce_to_any",
            AnonTargetCoercionError::CannotCoerceToAny(value.get_type(), value.to_repr()).into(),
            owner: "jtbraun", started: "2023-04-29"
        );
        Ok(AnonTargetAttr::String(StringLiteral(
            ctx.intern_str(&value.to_str()),
        )))
    }
}

fn to_anon_target_dict(
    dict_attr_type: &DictAttrType,

    ctx: &AnonAttrCtx,
    value: Value,
) -> buck2_error::Result<AnonTargetAttr> {
    if let Some(dict) = DictRef::from_value(value) {
        let mut res = Vec::with_capacity(dict.len());
        if dict_attr_type.sorted {
            // First sort the values
            let mut items = dict.iter().collect::<Vec<_>>();
            // If two things are incompatible, just return Eq. The resulting order is undefined, but safely undefined.
            items.sort_by(|a, b| a.0.compare(b.0).unwrap_or(Ordering::Equal));

            for (k, v) in items {
                res.push((
                    dict_attr_type.key.coerce_item(ctx, k)?,
                    dict_attr_type.value.coerce_item(ctx, v)?,
                ));
            }
        } else {
            for (k, v) in dict.iter() {
                res.push((
                    dict_attr_type.key.coerce_item(ctx, k)?,
                    dict_attr_type.value.coerce_item(ctx, v)?,
                ));
            }
        }
        Ok(AnonTargetAttr::Dict(DictLiteral(res.into())))
    } else {
        Err(AnonTargetCoercionError::type_error(Dict::TYPE, value).into())
    }
}

fn to_anon_target_one_of(
    one_of_attr_type: &OneOfAttrType,

    ctx: &AnonAttrCtx,
    value: Value,
) -> buck2_error::Result<AnonTargetAttr> {
    let mut errs = Vec::new();
    // Bias towards the start of the list - try and use success/failure from first in preference
    for (i, x) in one_of_attr_type.xs.iter().enumerate() {
        match x.coerce_item(ctx, value) {
            Ok(v) => return Ok(AnonTargetAttr::OneOf(Box::new(v), i as u32)),
            Err(e) => {
                // TODO(nga): anyhow error creation is expensive.
                errs.push(e)
            }
        }
    }
    Err(AnonTargetCoercionError::one_of_many(errs))
}

fn to_anon_target_tuple(
    tuple_attr_type: &TupleAttrType,

    ctx: &AnonAttrCtx,
    value: Value,
) -> buck2_error::Result<AnonTargetAttr> {
    let coerce = |value, items: &[Value]| {
        // Use comparison rather than equality below. If the tuple is too short,
        // it is implicitly extended using None.
        if items.len() <= tuple_attr_type.xs.len() {
            let mut res = Vec::with_capacity(tuple_attr_type.xs.len());
            for (c, v) in tuple_attr_type
                .xs
                .iter()
                .zip(items.iter().duped().chain(iter::repeat(Value::new_none())))
            {
                res.push(c.coerce_item(ctx, v)?);
            }
            // TODO(wendyy) intern attr
            Ok(AnonTargetAttr::Tuple(TupleLiteral(res.into())))
        } else {
            Err(AnonTargetCoercionError::type_error(
                &format!("Tuple of at most length {}", tuple_attr_type.xs.len()),
                value,
            )
            .into())
        }
    };
    if let Some(list) = TupleRef::from_value(value) {
        coerce(value, list.content())
    } else if let Some(list) = ListRef::from_value(value) {
        coerce(value, list.content())
    } else {
        Err(AnonTargetCoercionError::type_error(TupleRef::TYPE, value).into())
    }
}

fn to_anon_target_list(
    list_attr_type: &ListAttrType,

    ctx: &AnonAttrCtx,
    value: Value,
) -> buck2_error::Result<AnonTargetAttr> {
    if let Some(list) = ListRef::from_value(value) {
        Ok(AnonTargetAttr::List(ListLiteral(
            // TODO(wendyy) intern attr
            list.content()
                .iter()
                .map(|v| list_attr_type.inner.coerce_item(ctx, *v))
                .collect::<Result<Vec<_>, _>>()?
                .into(),
        )))
    } else if let Some(list) = TupleRef::from_value(value) {
        Ok(AnonTargetAttr::List(ListLiteral(
            // TODO(wendyy) intern attr
            list.content()
                .iter()
                .map(|v| list_attr_type.inner.coerce_item(ctx, *v))
                .collect::<Result<Vec<_>, _>>()?
                .into(),
        )))
    } else {
        Err(AnonTargetCoercionError::type_error(ListRef::TYPE, value).into())
    }
}
