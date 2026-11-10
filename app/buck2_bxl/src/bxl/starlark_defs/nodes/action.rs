/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::convert::Infallible;
use std::sync::Arc;

use allocative::Allocative;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_build_api::actions::RegisteredAction;
use buck2_build_api::actions::query::ActionQueryNode;
use buck2_build_api::actions::query::OwnedActionAttr;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_artifact::StarlarkArtifact;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_artifact_like::StarlarkArtifactView;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_error::buck2_error;
use buck2_interpreter::types::target_label::StarlarkConfiguredTargetLabel;
use buck2_query::query::environment::QueryTarget;
use derive_more::Display;
use dupe::Dupe;
use serde::Serialize;
use starlark::any::ProvidesStaticType;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::starlark_module;
use starlark::starlark_simple_value;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkValue;
use starlark::values::StringValue;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::none::NoneOr;
use starlark::values::starlark_value;
use starlark::values::structs::AllocStruct;

use crate::bxl::starlark_defs::analysis_result::StarlarkAnalysisResult;

#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    Allocative,
    pagable::Pagable,
    starlark::StarlarkPagableViaPagable
)]
#[derive(NoSerialize)]
#[display("{}", self.0)]
pub(crate) struct StarlarkAction(pub(crate) Arc<RegisteredAction>);

starlark_simple_value!(StarlarkAction);

starlark::methods_static!(BXL_ACTION_METHODS = action_methods);

#[starlark_value(type = "bxl.Action")]
impl<'v> StarlarkValue<'v> for StarlarkAction {
    fn get_methods() -> Option<&'static Methods> {
        Some(BXL_ACTION_METHODS.methods())
    }
}

impl<'v> UnpackValue<'v> for StarlarkAction {
    type Type = Self;
    type Error = Infallible;

    fn unpack(_value: ValueOf<'v, Self>, get: ValueTyped<'v, Self>) -> Result<Self, Infallible> {
        Ok(Self(get.as_ref().0.dupe()))
    }
}

/// Methods for an action obtained from [`bxl.AuditContext.output()`](../AuditContext#output).
#[starlark_module]
fn action_methods(builder: &mut MethodsBuilder) {
    /// Gets the owning configured target label for an action.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_action(ctx):
    ///     action = ctx.audit().output("buck-out/path/to/__target__/artifact", "your_target_platform")
    ///     ctx.output.print(action.owner())
    /// ```
    fn owner<'v>(
        this: StarlarkAction,
        heap: Heap<'v>,
    ) -> starlark::Result<ValueOf<'v, StarlarkConfiguredTargetLabel>> {
        match this.0.owner() {
            BaseDeferredKey::TargetLabel(label) => Ok(ValueOf::alloc(
                heap,
                StarlarkConfiguredTargetLabel::new(label.dupe()),
            )),
            _ => Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "BXL and anon targets not supported."
            )
            .into()),
        }
    }

    /// Gets the artifacts built by this action.
    fn outputs<'v>(
        this: StarlarkAction,
        heap: Heap<'v>,
    ) -> starlark::Result<Vec<ValueOf<'v, StarlarkArtifactView<'v>>>> {
        Ok(this
            .0
            .action()
            .outputs()
            .iter()
            .map(|a| ValueOf::alloc(heap, StarlarkArtifact::new(Artifact::from(a.dupe()))))
            .collect())
    }
}

#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    Allocative,
    starlark::StarlarkPagableUnsupported
)]
#[derive(NoSerialize)]
#[display("{}", self.0.key())]
pub(crate) struct StarlarkActionQueryNode(pub(crate) ActionQueryNode);

starlark_simple_value!(StarlarkActionQueryNode);

starlark::methods_static!(ACTION_QUERY_NODE_VALUE_METHODS = action_query_node_value_methods);

#[starlark_value(type = "bxl.ActionQueryNode")]
impl<'v> StarlarkValue<'v> for StarlarkActionQueryNode {
    fn get_methods() -> Option<&'static Methods> {
        Some(ACTION_QUERY_NODE_VALUE_METHODS.methods())
    }
}

impl<'v> UnpackValue<'v> for StarlarkActionQueryNode {
    type Type = Self;
    type Error = Infallible;

    fn unpack(_value: ValueOf<'v, Self>, get: ValueTyped<'v, Self>) -> Result<Self, Infallible> {
        Ok(Self(get.as_ref().0.dupe()))
    }
}

/// Methods for action query node.
#[starlark_module]
fn action_query_node_value_methods(builder: &mut MethodsBuilder) {
    /// Gets the attributes from the action query node. Returns a struct.
    #[starlark(attribute)]
    fn attrs<'v>(this: StarlarkActionQueryNode, heap: Heap<'v>) -> starlark::Result<Value<'v>> {
        let mut result = Vec::new();
        this.0.attrs_for_each(|k, v| {
            result.push((
                k.to_owned(),
                heap.alloc_representation(StarlarkActionAttr(v.to_owned()))
                    .to_value(),
            ));
            buck2_error::Ok(())
        })?;

        Ok(heap.alloc(AllocStruct(result)))
    }

    /// Gets optional action from the action query target node.
    fn action<'v>(
        this: &StarlarkActionQueryNode,
        heap: Heap<'v>,
    ) -> starlark::Result<NoneOr<ValueOf<'v, StarlarkAction>>> {
        Ok(NoneOr::from_option(
            this.0
                .action()
                .map(|a| ValueOf::alloc(heap, StarlarkAction(a.dupe()))),
        ))
    }

    /// Gets optional analysis from the action query target node.
    fn analysis<'v>(
        this: &StarlarkActionQueryNode,
        heap: Heap<'v>,
    ) -> starlark::Result<NoneOr<ValueOf<'v, StarlarkAnalysisResult>>> {
        match this.0.analysis_opt() {
            Some(a) => Ok(NoneOr::Other(ValueOf::alloc(
                heap,
                StarlarkAnalysisResult::new(a.analysis_result().lookup(a.target())?),
            ))),
            None => Ok(NoneOr::None),
        }
    }

    /// Gets the kind of action query node, either analysis or action kind.
    #[starlark(attribute)]
    fn rule_type(this: &StarlarkActionQueryNode) -> starlark::Result<String> {
        Ok(this.0.rule_type().to_string())
    }
}

#[derive(
    Debug,
    ProvidesStaticType,
    Allocative,
    derive_more::Display,
    Serialize,
    pagable::Pagable,
    starlark::StarlarkPagableViaPagable
)]
#[repr(transparent)]
#[serde(transparent)]
pub(crate) struct StarlarkActionAttr(pub(crate) OwnedActionAttr);

starlark_simple_value!(StarlarkActionAttr);

starlark::methods_static!(ACTION_ATTR_METHODS = action_attr_methods);

/// Action attr from an action query node.
#[starlark_value(type = "bxl.ActionAttr")]
impl<'v> StarlarkValue<'v> for StarlarkActionAttr {
    fn get_methods() -> Option<&'static Methods> {
        Some(ACTION_ATTR_METHODS.methods())
    }
}

/// Methods on action query node's attributes.
#[starlark_module]
fn action_attr_methods(builder: &mut MethodsBuilder) {
    /// Returns the value of this attribute.
    fn value<'v>(this: &StarlarkActionAttr, heap: Heap<'v>) -> starlark::Result<StringValue<'v>> {
        Ok(heap.alloc_str(&this.0.0))
    }
}
