/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;
use std::cell::RefCell;
use std::convert::Infallible;
use std::fmt;
use std::fmt::Display;
use std::path::Path;

use allocative::Allocative;
use buck2_analysis::analysis::calculation::get_dep_analysis;
use buck2_analysis::analysis::calculation::resolve_queries;
use buck2_analysis::analysis::env::RuleAnalysisAttrResolutionContext;
use buck2_analysis::analysis::env::get_deps_from_analysis_results;
use buck2_analysis::attrs::resolve::configured_attr::ConfiguredAttrExt;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_artifact::artifact::source_artifact::SourceArtifact;
use buck2_build_api::actions::query::PackageLabelOption;
use buck2_build_api::analysis::AnalysisResult;
use buck2_build_api::bxl::unconfigured_attribute::StarlarkCoercedAttr;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_artifact::StarlarkArtifact;
use buck2_common::dice::cells::HasCellResolver;
use buck2_configuration_providers::configuration_info::ConfigurationInfo;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::package::PackageLabel;
use buck2_core::package::source_path::SourcePathRef;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_error::BuckErrorContext;
use buck2_fs::paths::abs_path::AbsPath;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutMap;
use buck2_hash::BuckMutSet;
use buck2_interpreter::types::target_label::StarlarkConfiguredTargetLabel;
use buck2_node::attrs::arena::AttrToken;
use buck2_node::attrs::arena::CoercedAttrArenaBuilder;
use buck2_node::attrs::arena::OwnedCoercedAttr;
use buck2_node::attrs::attr_type::arg::StringWithMacros;
use buck2_node::attrs::configured_attr::ConfiguredAttr;
use buck2_node::attrs::configured_traversal::ConfiguredAttrTraversal;
use buck2_node::attrs::display::AttrDisplayWithContext;
use buck2_node::attrs::fmt_context::AttrFmtContext;
use buck2_node::attrs::inspect_options::AttrInspectOptions;
use buck2_node::attrs::serialize::AttrSerializeWithContext;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use derivative::Derivative;
use derive_more::Display;
use dupe::Dupe;
use futures::FutureExt;
use pagable::Pagable;
use serde::Serialize;
use serde::Serializer;
use starlark::any::ProvidesStaticType;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::starlark_simple_value;
use starlark::values::AllocValue;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkValue;
use starlark::values::StringValue;
use starlark::values::Trace;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueTyped;
use starlark::values::list::AllocList;
use starlark::values::none::NoneOr;
use starlark::values::starlark_value;
use starlark::values::structs::AllocStruct;

use super::node_attrs::NodeAttributeGetter;
use crate::bxl::starlark_defs::context::BxlContext;
use crate::bxl::starlark_defs::file_set::StarlarkFileNode;
use crate::bxl::starlark_defs::nodes::configured::attr_resolution_ctx::LazyAttrResolutionCache;
use crate::bxl::starlark_defs::nodes::configured::attr_resolution_ctx::LazyAttrResolutionContext;

mod attr_resolution_ctx;

/// Strip configurations off `attr`, encoding the result into its own
/// single-value storage.
fn attr_with_stripped_cfg(attr: &ConfiguredAttr) -> buck2_error::Result<OwnedCoercedAttr> {
    let mut b = CoercedAttrArenaBuilder::default();
    let root = strip_cfg_into(attr, &mut b)?;
    OwnedCoercedAttr::seal_from_builder(b, root)
}

fn strip_cfg_into(
    attr: &ConfiguredAttr,
    b: &mut CoercedAttrArenaBuilder,
) -> buck2_error::Result<AttrToken> {
    Ok(match attr {
        ConfiguredAttr::Bool(v) => b.encode_bool(v.0)?,
        ConfiguredAttr::Int(v) => b.encode_int(*v)?,
        ConfiguredAttr::String(v) => b.encode_string(v.0.as_str())?,
        ConfiguredAttr::EnumVariant(v) => b.encode_enum_variant(v.0.as_str())?,
        ConfiguredAttr::List(list) => {
            let items = list
                .iter()
                .map(|v| strip_cfg_into(v, b))
                .collect::<buck2_error::Result<Vec<_>>>()?;
            b.encode_list(&items)?
        }
        ConfiguredAttr::Tuple(tuple) => {
            let items = tuple
                .iter()
                .map(|v| strip_cfg_into(v, b))
                .collect::<buck2_error::Result<Vec<_>>>()?;
            b.encode_tuple(&items)?
        }
        ConfiguredAttr::Dict(dict) => {
            let pairs = dict
                .iter()
                .map(|(k, v)| Ok((strip_cfg_into(k, b)?, strip_cfg_into(v, b)?)))
                .collect::<buck2_error::Result<Vec<_>>>()?;
            b.encode_dict(&pairs)?
        }
        ConfiguredAttr::None => b.encode_none()?,
        ConfiguredAttr::OneOf(attr, i) => {
            let inner = strip_cfg_into(attr, b)?;
            b.encode_one_of(*i, inner)?
        }
        ConfiguredAttr::Visibility(v) => b.encode_visibility(v)?,
        ConfiguredAttr::WithinView(v) => b.encode_within_view(v)?,
        ConfiguredAttr::ExplicitConfiguredDep(dep) => {
            // Platform information is lost during this conversion
            // This is fine for most use cases since dep and configured dep display the same
            b.encode_dep(&dep.label.unconfigured())?
        }
        ConfiguredAttr::SplitTransitionDep(dep) => {
            let deps: BuckMutSet<_> = dep.deps.values().map(|l| l.unconfigured()).collect();
            if deps.len() != 1 {
                return Err(buck2_error::internal_error!(
                    "ConfiguredSplitTransitionDep should have exactly one dep, but found {}",
                    deps.len()
                ));
            }
            // This loses the split transition information. This is fine for most use cases since
            // dep and transition dep display the same
            b.encode_dep(deps.iter().next().ok_or_else(|| {
                buck2_error::internal_error!("deps is empty after checking length")
            })?)?
        }
        ConfiguredAttr::TransitionDep(dep) => {
            // This loses the transition information. This is fine for most use cases since
            // dep and transition dep display the same
            b.encode_transition_dep(&dep.dep.unconfigured(), None)?
        }
        ConfiguredAttr::ConfigurationDep(dep) => b.encode_configuration_dep(dep)?,
        ConfiguredAttr::PluginDep(dep, _) => b.encode_plugin_dep(dep)?,
        ConfiguredAttr::Dep(dep) => b.encode_dep(&dep.label.unconfigured())?,
        ConfiguredAttr::SourceLabel(dep) => b.encode_source_label(&dep.unconfigured())?,
        ConfiguredAttr::Label(label) => b.encode_label(&label.unconfigured())?,
        ConfiguredAttr::Arg(arg) => {
            let unconfigured_parts = match &arg.string_with_macros {
                StringWithMacros::StringPart(s) => StringWithMacros::StringPart(s.clone()),
                StringWithMacros::ManyParts(parts) => StringWithMacros::ManyParts(
                    parts
                        .iter()
                        .map(|part| match part {
                            buck2_node::attrs::attr_type::arg::StringWithMacrosPart::String(s) => {
                                buck2_node::attrs::attr_type::arg::StringWithMacrosPart::String(
                                    s.clone(),
                                )
                            }
                            buck2_node::attrs::attr_type::arg::StringWithMacrosPart::Macro(
                                write_to_file,
                                macr,
                            ) => buck2_node::attrs::attr_type::arg::StringWithMacrosPart::Macro(
                                *write_to_file,
                                unconfigure_macro(macr),
                            ),
                        })
                        .collect::<Vec<_>>()
                        .into_boxed_slice(),
                ),
            };
            b.encode_arg(&unconfigured_parts)?
        }
        ConfiguredAttr::Query(query) => {
            b.encode_query(&buck2_node::attrs::attr_type::query::QueryAttrBase {
                query: query.query.query.clone(),
                resolved_literals: buck2_node::attrs::attr_type::query::ResolvedQueryLiterals(
                    query
                        .query
                        .resolved_literals
                        .0
                        .iter()
                        .map(|(k, v)| (*k, v.unconfigured()))
                        .collect(),
                ),
            })?
        }
        ConfiguredAttr::SourceFile(path) => b.encode_source_file(path)?,
        ConfiguredAttr::Metadata(m) => b.encode_metadata(m)?,
        ConfiguredAttr::TargetModifiers(m) => b.encode_target_modifiers(m)?,
    })
}

#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    Allocative,
    Clone,
    Dupe,
    pagable::Pagable,
    starlark::StarlarkPagableViaPagable
)]
#[derive(NoSerialize)] // TODO probably should be serializable the same as how queries serialize
#[display("configured_target_node(name = {}, ...)", self.0.label())]
pub(crate) struct StarlarkConfiguredTargetNode(pub(crate) ConfiguredTargetNode);

starlark_simple_value!(StarlarkConfiguredTargetNode);

starlark::methods_static!(
    CONFIGURED_TARGET_NODE_VALUE_METHODS = configured_target_node_value_methods
);

#[starlark_value(type = "bxl.ConfiguredTargetNode")]
impl<'v> StarlarkValue<'v> for StarlarkConfiguredTargetNode {
    fn get_methods() -> Option<&'static Methods> {
        Some(CONFIGURED_TARGET_NODE_VALUE_METHODS.methods())
    }
}

impl<'a> UnpackValue<'a> for StarlarkConfiguredTargetNode {
    type Error = Infallible;

    fn unpack_value_impl(value: Value<'a>) -> Result<Option<Self>, Self::Error> {
        Ok(value
            .downcast_ref::<Self>()
            .map(|value| Self(value.0.dupe())))
    }
}

#[starlark_module]
fn configured_target_node_value_methods(builder: &mut MethodsBuilder) {
    /// Gets the configured target label of this target node.
    ///
    /// Note that you cannot get a non-configured label from a configured target node because the
    /// configured target node is not uniquely identified a non-configured label, only by the configured target label.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_label(ctx):
    ///     node = ctx.configured_targets("my_cell//bin:the_binary")
    ///     ctx.output.print(node.label)
    /// ```
    #[starlark(attribute)]
    fn label(
        this: &StarlarkConfiguredTargetNode,
    ) -> starlark::Result<StarlarkConfiguredTargetLabel> {
        Ok(StarlarkConfiguredTargetLabel::new(this.0.label().dupe()))
    }

    /// Gets the buildfile path from the configured target node.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_label(ctx):
    ///     target_node = ctx.cquery().eval("owner('path/to/file')")[0]
    ///     ctx.output.print(target_node.buildfile_path)
    /// ```
    #[starlark(attribute)]
    fn buildfile_path(this: &StarlarkConfiguredTargetNode) -> starlark::Result<StarlarkFileNode> {
        Ok(StarlarkFileNode(this.0.buildfile_path().path()))
    }

    /// Gets the attribute from the configured target node.
    /// If the attribute is unset, returns the default value.
    /// If the attribute is not defined by the rule, returns `None`.
    /// It will not return special attribute (attribute that start with 'buck.' in `buck2 cquery -A` command).
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_attributes(ctx):
    ///     target_node = ctx.uquery().eval("//foo:bar")[0]
    ///     ctx.output.print(target_node.get_attr('my_attr'))
    /// ```
    fn get_attr<'v>(
        this: &StarlarkConfiguredTargetNode,
        #[starlark(require=pos)] key: &str,
        heap: Heap<'v>,
    ) -> starlark::Result<NoneOr<Value<'v>>> {
        Ok(NodeAttributeGetter::get_attr(this, key, heap)?)
    }

    /// Gets all the attributes (excluding special attributes) from the configured target node.
    /// For attributes that are not explicitly set, the default value is returned.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_attributes(ctx):
    ///     target_node = ctx.uquery().eval("//foo:bar")[0]
    ///     ctx.output.print(target_node.get_attrs())
    /// ```
    fn get_attrs<'v>(
        this: &StarlarkConfiguredTargetNode,
        heap: Heap<'v>,
    ) -> starlark::Result<BuckMutInsertionMap<StringValue<'v>, Value<'v>>> {
        Ok(NodeAttributeGetter::get_attrs(this, heap)?)
    }

    /// Check if rule has the attribute.
    ///
    /// Known attribute is always set explicitly or to default value
    /// (otherwise target would not be created)
    /// For special attributes, it will return `False`
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_has_attr(ctx):
    ///     target_node = ctx.uquery().eval("//foo:bar")[0]
    ///     ctx.output.print(target_node.has_attr('my_attr'))
    /// ```
    fn has_attr<'v>(
        this: &StarlarkConfiguredTargetNode,
        #[starlark(require=pos)] key: &str,
    ) -> starlark::Result<bool> {
        Ok(NodeAttributeGetter::has_attr(this, key))
    }

    /// Returns a struct of all the attributes of this target node. The structs fields are the
    /// attributes names, and the values are [`StarlarkConfiguredAttr`].
    ///
    /// If you need to access many or all attrs on the same node, then this is the preferred way. Otherwise,
    /// using `attrs_lazy()` would be a better option for only accessing only a few attrs, although this really
    /// depends on what kind of attrs are on the node. Benchmarking performance will give you the best
    /// indication on which method to use.
    ///
    /// You should store the result of this function call for further usage in the code rather than calling
    /// `attrs_eager()` each time you need to access the attrs.
    ///
    /// The result includes the special attributes (those that start with `buck.`, as in
    /// `buck2 cquery -A`). `buck.exec_group_platforms` is present only for targets whose rule
    /// declares named execution groups.
    ///
    /// Right now, it is not recommended to use this method. Instead, use `get_attr` and `get_attrs` methods.
    /// We will deprecate this method in the future.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_attrs_eager(ctx):
    ///     node = ctx.cquery().owner("cell//path/to/TARGETS")[0]
    ///     attrs = node.attrs_eager() # cache once
    ///     ctx.output.print(attrs)
    ///     # do more stuff with attrs
    /// ```
    fn attrs_eager<'v>(
        this: &StarlarkConfiguredTargetNode,
        heap: Heap<'v>,
    ) -> starlark::Result<Value<'v>> {
        let attrs_iter = this.0.attrs(AttrInspectOptions::All);
        let special_attrs_iter = this.0.special_attrs();

        let attrs = attrs_iter
            .map(|a| {
                (
                    a.name,
                    StarlarkConfiguredAttr(a.value, this.0.label().pkg().dupe()),
                )
            })
            .chain(special_attrs_iter.map(|(name, attr)| {
                (
                    name,
                    StarlarkConfiguredAttr(attr, this.0.label().pkg().dupe()),
                )
            }));

        Ok(heap.alloc(AllocStruct(attrs)))
    }

    /// Returns a `lazy_attrs` object that you can call `get()` on that gets an attr one at a time.
    ///
    /// If you need to access only few attrs on the same node, then this is the preferred way. Otherwise,
    /// using `attrs_eager()` would be a better option for accessing many or all attrs, although this really
    /// depends on what kind of attrs are on the node. Benchmarking performance will give you the best
    /// indication on which method to use.
    ///
    /// You should store the result of this function call for further usage in the code rather than calling
    /// `attrs_lazy()` each time to get the `lazy_attrs` object. Note that if the `get()` is `None`,
    /// then any methods called on `None` will result in an error.
    ///
    /// `get()` also returns the special attributes (those that start with `buck.`, as in
    /// `buck2 cquery -A`). `buck.exec_group_platforms` is present only for targets whose rule
    /// declares named execution groups.
    ///
    /// Right now, it is not recommended to use this method. Instead, use `get_attr` and `get_attrs` methods.
    /// We will deprecate this method in the future.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_attrs_lazy(ctx):
    ///     node = ctx.cquery().owner("cell//path/to/TARGETS")[0]
    ///     attrs = node.attrs_lazy() # cache once
    ///     ctx.output.print(attrs.get("some_attributes").value())
    ///     ctx.output.print(attrs.get("some_attribute").label)
    /// ```
    fn attrs_lazy<'v>(
        this: &'v StarlarkConfiguredTargetNode,
    ) -> starlark::Result<StarlarkLazyAttrs<'v>> {
        Ok(StarlarkLazyAttrs::new(this))
    }

    /// Returns a `lazy_resolved_attrs` object that you can call `get()` on that gets a resolved attr one at a time.
    ///
    /// If you need to access only few resolved attrs on the same node, then this is the preferred way. Otherwise,
    /// using `resolved_attrs_eager()` would be a better option for accessing many or all resolved attrs, although this really
    /// depends on what kind of resolved attrs are on the node. Benchmarking performance will give you the best
    /// indication on which method to use.
    ///
    /// You should store the result of this function call for further usage in the code rather than calling
    /// `resolved_attrs_lazy()` each time to get the `lazy_resolved_attrs` object. Note that if the `get()` is `None`,
    /// then any methods called on `None` will result in an error.
    ///
    /// Right now, it is not recommended to use this method. Instead, use `get_attr` and `get_attrs` methods.
    /// We will deprecate this method in the future.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_resolved_attrs_lazy(ctx):
    ///     node = ctx.cquery().owner("cell//path/to/TARGETS")[0]
    ///     attrs = node.resolved_attrs_lazy(ctx) # cache once
    ///     ctx.output.print(attrs.get("some_attributes").value())
    ///     ctx.output.print(attrs.get("some_attribute").label)
    /// ```
    fn resolved_attrs_lazy<'v>(
        this: ValueTyped<'v, StarlarkConfiguredTargetNode>,
        ctx: ValueTyped<'v, BxlContext<'v>>,
    ) -> starlark::Result<StarlarkLazyResolvedAttrs<'v>> {
        Ok(StarlarkLazyResolvedAttrs::new(this, ctx))
    }

    /// Returns a struct of all the resolved attributes of this target node. The structs fields are the
    /// attributes names, and the values are the underlying Starlark values of the attributes.
    ///
    /// If you need to access many or all resolved attrs on the same node, then this is the preferred way. Otherwise,
    /// using `resolved_attrs_lazy()` would be a better option for accessing only a few resolved attrs, although this really
    /// depends on what kind of resolved attrs are on the node. Benchmarking performance will give you the best
    /// indication on which method to use.
    ///
    /// You should store the result of this function call for further usage in the code rather than calling
    /// `resolved_attrs_eager()` each time you need all the resolved attrs.
    ///
    /// Right now, it is not recommended to use this method. Instead, use `get_attr` and `get_attrs` methods.
    /// We will deprecate this method in the future.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_resolved_attrs_eager(ctx):
    ///     node = ctx.cquery().owner("cell//path/to/TARGETS")[0]
    ///     attrs = node.resolved_attrs_eager(ctx) # cache once
    ///     ctx.output.print(attrs)
    ///     # do more stuff with attrs
    /// ```
    fn resolved_attrs_eager<'v>(
        this: &'v StarlarkConfiguredTargetNode,
        ctx: &'v BxlContext<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<Value<'v>> {
        let configured_node = this.0.as_ref();

        let dep_analysis: buck2_error::Result<Vec<(&ConfiguredTargetLabel, AnalysisResult)>> = ctx
            .via_dice(eval, |ctx| {
                ctx.via(|dice_ctx| get_dep_analysis(configured_node, dice_ctx).boxed_local())
            });

        let query_results = ctx.via_dice(eval, |ctx| {
            ctx.via(|dice_ctx| resolve_queries(dice_ctx, configured_node).boxed_local())
        })?;

        let resolution_ctx = RuleAnalysisAttrResolutionContext {
            module: eval.module(),
            dep_analysis_results: get_deps_from_analysis_results(dep_analysis?)?,
            query_results,
            execution_platform_resolution: configured_node.execution_platform_resolution().clone(),
        };

        let attrs_iter = this.0.attrs(AttrInspectOptions::All);
        let mut resolved_attrs = Vec::with_capacity(attrs_iter.len());

        for a in attrs_iter {
            resolved_attrs.push((
                a.name,
                a.value
                    .resolve_single(this.0.label().pkg(), &mut &resolution_ctx)?,
            ));
        }

        Ok(eval.heap().alloc(AllocStruct(resolved_attrs)))
    }

    /// Skip incoming transition forward node.
    /// If a target is a forward node, which is created by applying incoming configuration transition,
    /// return the transition target, otherwise return itself.
    /// This is is particularly useful when you don't care about 'forward' node.
    ///
    /// Example usage:
    /// ```python
    /// def _impl_unwrap_forward(ctx):
    ///     node = ctx.configured_targets("my_cell//bin:the_binary")
    ///     actual_node = node.unwrap_forward()
    /// ```
    fn unwrap_forward<'v>(
        this: ValueTyped<'v, StarlarkConfiguredTargetNode>,
        heap: Heap<'v>,
    ) -> starlark::Result<ValueTyped<'v, StarlarkConfiguredTargetNode>> {
        match this.0.forward_target() {
            Some(n) => Ok(heap.alloc_typed(StarlarkConfiguredTargetNode(n.dupe()))),
            None => Ok(this),
        }
    }

    /// Gets the targets' corresponding rule's name. This is the fully qualified rule name including
    /// the import path.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_rule_type(ctx):
    ///     node = ctx.configured_targets("my_cell//bin:the_binary")
    ///     ctx.output.print(node.rule_type)
    /// ```
    #[starlark(attribute)]
    fn rule_type<'v>(
        this: &'v StarlarkConfiguredTargetNode,
        heap: Heap<'v>,
    ) -> starlark::Result<StringValue<'v>> {
        Ok(heap.alloc_str_intern(this.0.rule_type().to_string().as_str()))
    }

    /// Gets the targets' corresponding rule's kind which is one of
    ///  - normal (with no special properties)
    ///  - configured (usable in a configuration context)
    ///  - toolchain (only usable as a toolchain dep)
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_rule_kind(ctx):
    ///     node = ctx.configured_targets("my_cell//bin:the_binary")
    ///     ctx.output.print(node.rule_kind)
    /// ```
    #[starlark(attribute)]
    fn rule_kind<'v>(
        this: &'v StarlarkConfiguredTargetNode,
        heap: Heap<'v>,
    ) -> starlark::Result<StringValue<'v>> {
        Ok(heap.alloc_str_intern(this.0.rule_kind().as_str()))
    }

    /// Returns all source `Artifact`s exist in this target's attributes.
    /// This method will traverse all the attributes to find and collect all the source `Artifact`s.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_sources(ctx):
    ///     node = ctx.configured_targets("my_cell//bin:the_binary")
    ///     ctx.output.print(node.sources())
    /// ```
    fn sources(this: &StarlarkConfiguredTargetNode) -> starlark::Result<Vec<StarlarkArtifact>> {
        struct InputsCollector {
            inputs: Vec<StarlarkArtifact>,
        }
        impl ConfiguredAttrTraversal for InputsCollector {
            fn dep(&mut self, _dep: &ConfiguredProvidersLabel) -> buck2_error::Result<()> {
                Ok(())
            }

            fn input(&mut self, path: SourcePathRef) -> buck2_error::Result<()> {
                self.inputs
                    .push(StarlarkArtifact::new(Artifact::from(SourceArtifact::new(
                        path.to_owned(),
                    ))));
                Ok(())
            }
        }
        let mut traversal = InputsCollector { inputs: Vec::new() };
        for a in this.0.attrs(AttrInspectOptions::All) {
            a.traverse(this.0.label().pkg(), &mut traversal)?;
        }
        Ok(traversal.inputs)
    }

    /// Gets the source `Artifact` that corresponds to the given `path` given a context. The path should be the
    /// project relative path to the file, or an absolute path.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_get_source(ctx):
    ///     owner = ctx.cquery().owner("project/relative/path/to/file")[0]
    ///     artifact = owner.sources()[0]
    ///     ctx.output.print(artifact)
    /// ```
    fn get_source<'v>(
        this: &StarlarkConfiguredTargetNode,
        path: &str,
        ctx: &BxlContext<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<NoneOr<StarlarkArtifact>> {
        let path = Path::new(path);
        let fs = ctx.project_root();
        let path = if path.is_absolute() {
            Cow::Owned(
                fs.relativize_any(AbsPath::new(path)?)
                    .buck_error_context("Given path does not belong to the project root")?,
            )
        } else {
            Cow::Borrowed(ProjectRelativePath::new(path).buck_error_context(
                "Given path should either be absolute or a forward pointing project relative path",
            )?)
        };

        let cell_path = ctx.via_dice(eval, |ctx| {
            ctx.via(|ctx| {
                async move { Ok(ctx.get_cell_resolver().await?.get_cell_path(&path)) }.boxed_local()
            })
        })?;

        struct SourceFinder {
            found: Option<StarlarkArtifact>,
            target: CellPath,
        }
        impl ConfiguredAttrTraversal for SourceFinder {
            fn dep(&mut self, _dep: &ConfiguredProvidersLabel) -> buck2_error::Result<()> {
                Ok(())
            }

            fn input(&mut self, path: SourcePathRef) -> buck2_error::Result<()> {
                if path.to_cell_path() == self.target {
                    self.found = Some(StarlarkArtifact::new(Artifact::from(SourceArtifact::new(
                        path.to_owned(),
                    ))));
                }
                Ok(())
            }
        }
        let mut traversal = SourceFinder {
            found: None,
            target: cell_path,
        };
        for a in this.0.attrs(AttrInspectOptions::All) {
            a.traverse(this.0.label().pkg(), &mut traversal)?;

            if let Some(found) = traversal.found {
                return Ok(NoneOr::Other(found));
            }
        }
        Ok(NoneOr::None)
    }

    /// Gets the target's special attr `oncall`
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_get_oncall(ctx):
    ///     target_node = ctx.cquery().eval("//foo:bar")[0]
    ///     ctx.output.print(target_node.oncall)
    /// ```
    #[starlark(attribute)]
    fn oncall<'v>(
        this: &'v StarlarkConfiguredTargetNode,
        heap: Heap<'v>,
    ) -> starlark::Result<NoneOr<StringValue<'v>>> {
        match this.0.oncall() {
            Some(oncall) => Ok(NoneOr::Other(heap.alloc_str_intern(oncall))),
            None => Ok(NoneOr::None),
        }
    }

    /// Gets all deps for this target.
    /// The result is a list of `ConfiguredTargetNode`.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_get_deps(ctx):
    ///     target_node = ctx.uquery().eval("//foo:bar")[0]
    ///     ctx.output.print(target_node.deps())
    /// ```
    fn deps<'v>(
        this: &'v StarlarkConfiguredTargetNode,
        // ) -> buck2_error::Result<Vec<StarlarkConfiguredTargetNode>> {
    ) -> starlark::Result<AllocList<impl IntoIterator<Item = StarlarkConfiguredTargetNode> + 'v>>
    {
        Ok(AllocList(
            this.0
                .deps()
                .iter()
                .map(|node| StarlarkConfiguredTargetNode(node.dupe())),
        ))
    }

    /// Gets the target deps of this target: the deps in the target's own configuration (those of
    /// `attrs.dep()` and the other dependency attributes, transitioned deps included), without
    /// the exec deps, the toolchain deps and the configuration deps. The same set `target_deps()`
    /// selects in a `deps()` query. Together with `exec_deps()`, `toolchain_deps()` and
    /// `configuration_deps()` this partitions `deps()`.
    /// The result is a list of `ConfiguredTargetNode`.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_get_target_deps(ctx):
    ///     target_node = ctx.configured_targets("//foo:bar")
    ///     ctx.output.print(target_node.target_deps())
    /// ```
    fn target_deps<'v>(
        this: &'v StarlarkConfiguredTargetNode,
    ) -> starlark::Result<AllocList<impl IntoIterator<Item = StarlarkConfiguredTargetNode> + 'v>>
    {
        Ok(AllocList(
            this.0
                .target_deps()
                .map(|node| StarlarkConfiguredTargetNode(node.dupe())),
        ))
    }

    /// Gets the exec deps of this target: the deps of its `attrs.exec_dep()` attributes,
    /// configured for the target's execution platform. The same set `exec_deps()` selects in a
    /// `deps()` query.
    /// The result is a list of `ConfiguredTargetNode`.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_get_exec_deps(ctx):
    ///     target_node = ctx.configured_targets("//foo:bar")
    ///     ctx.output.print(target_node.exec_deps())
    /// ```
    fn exec_deps<'v>(
        this: &'v StarlarkConfiguredTargetNode,
    ) -> starlark::Result<AllocList<impl IntoIterator<Item = StarlarkConfiguredTargetNode> + 'v>>
    {
        Ok(AllocList(
            this.0
                .exec_deps()
                .iter()
                .map(|node| StarlarkConfiguredTargetNode(node.dupe())),
        ))
    }

    /// Gets the toolchain deps of this target: the deps of its `attrs.toolchain_dep()`
    /// attributes. The same set `toolchain_deps()` selects in a `deps()` query.
    /// The result is a list of `ConfiguredTargetNode`.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_get_toolchain_deps(ctx):
    ///     target_node = ctx.configured_targets("//foo:bar")
    ///     ctx.output.print(target_node.toolchain_deps())
    /// ```
    fn toolchain_deps<'v>(
        this: &'v StarlarkConfiguredTargetNode,
    ) -> starlark::Result<AllocList<impl IntoIterator<Item = StarlarkConfiguredTargetNode> + 'v>>
    {
        Ok(AllocList(
            this.0
                .toolchain_deps()
                .map(|node| StarlarkConfiguredTargetNode(node.dupe())),
        ))
    }

    /// Gets the configuration deps of this target: the configuration rules among its deps, such as
    /// the `constraint_value`s a `platform` names. The keys of a `select()` and the compatibility
    /// attributes are resolved while configuring the target and are not deps of the configured
    /// node. The same set `configuration_deps()` selects in a `deps()` query.
    /// The result is a list of `ConfiguredTargetNode`.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_get_configuration_deps(ctx):
    ///     target_node = ctx.configured_targets("//foo:bar")
    ///     ctx.output.print(target_node.configuration_deps())
    /// ```
    fn configuration_deps<'v>(
        this: &'v StarlarkConfiguredTargetNode,
    ) -> starlark::Result<AllocList<impl IntoIterator<Item = StarlarkConfiguredTargetNode> + 'v>>
    {
        Ok(AllocList(
            this.0
                .configuration_deps()
                .map(|node| StarlarkConfiguredTargetNode(node.dupe())),
        ))
    }

    /// Returns a `ConfigurationInfo` representing this node's configuration,
    /// or `None` if the configuration is not bound.
    ///
    /// The returned `ConfigurationInfo` has a `constraints` dict mapping
    /// `TargetLabel` (constraint settings) to `ConstraintValueInfo` (constraint values),
    /// and an empty `values` dict.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_configuration_info(ctx):
    ///     node = ctx.configured_targets("my_cell//bin:the_binary")
    ///     cfg_info = node.configuration_info()
    ///     if cfg_info != None:
    ///         for setting, value in cfg_info.constraints.items():
    ///             ctx.output.print(str(setting) + "=" + str(value))
    /// ```
    fn configuration_info<'v>(
        this: &StarlarkConfiguredTargetNode,
        heap: Heap<'v>,
    ) -> starlark::Result<NoneOr<Value<'v>>> {
        let cfg = this.0.label().cfg();
        if !cfg.is_bound() {
            return Ok(NoneOr::None);
        }
        let data = cfg.data()?;
        let info = ConfigurationInfo::from_configuration_data(data, heap);
        Ok(NoneOr::Other(heap.alloc(info)))
    }
}

#[derive(
    Debug,
    Clone,
    ProvidesStaticType,
    Allocative,
    Pagable,
    starlark::StarlarkPagableViaPagable
)]
#[repr(C)]
pub(crate) struct StarlarkConfiguredAttr(ConfiguredAttr, PackageLabel);

impl Display for StarlarkConfiguredAttr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(
            &AttrFmtContext {
                package: Some(self.1.dupe()),
                options: Default::default(),
            },
            f,
        )
    }
}

impl Serialize for StarlarkConfiguredAttr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize_with_ctx(
            &AttrFmtContext {
                package: Some(self.1.dupe()),
                options: Default::default(),
            },
            serializer,
        )
    }
}

starlark_simple_value!(StarlarkConfiguredAttr);

starlark::methods_static!(CONFIGURED_ATTR_METHODS = configured_attr_methods);

#[starlark_value(type = "bxl.ConfiguredAttr")]
impl<'v> StarlarkValue<'v> for StarlarkConfiguredAttr {
    fn get_methods() -> Option<&'static Methods> {
        Some(CONFIGURED_ATTR_METHODS.methods())
    }
}

/// Methods on configured target node's attributes.
#[starlark_module]
fn configured_attr_methods(builder: &mut MethodsBuilder) {
    /// Returns the type name of the attribute
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_type(ctx):
    ///     node = ctx.cquery().owner("bin/TARGETS")[0]
    ///     attrs = node.attrs_eager()
    ///     ctx.output.print(attrs.name.type)
    /// ```
    // FIXME(JakobDegen): Strings as types are mostly dead, users should be getting the value and
    // using `isinstance` instead. Remove this.
    #[starlark(attribute)]
    fn r#type<'v>(this: &StarlarkConfiguredAttr, heap: Heap<'v>) -> starlark::Result<&'v str> {
        Ok(this
            .0
            .to_value(PackageLabelOption::PackageLabel(this.1.dupe()), heap)?
            .get_type())
    }

    /// Returns the value of this attribute. The value here is not fully resolved like in rules.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_value(ctx):
    ///     node = ctx.cquery().owner("bin/TARGETS")[0]
    ///     attrs = node.attrs_eager()
    ///     ctx.output.print(attrs.name.value())
    /// ```
    fn value<'v>(this: &StarlarkConfiguredAttr, heap: Heap<'v>) -> starlark::Result<Value<'v>> {
        Ok(this
            .0
            .to_value(PackageLabelOption::PackageLabel(this.1.dupe()), heap)?)
    }

    /// Returns this attribute with all configurations stripped.
    ///
    /// This is useful when you want to display or compare attributes after configuration
    /// without configuration-specific information in the output. For example, macros
    /// like `$(location)` may include configuration strings when displayed that can be
    /// removed with this method.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_strip_cfg(ctx):
    ///     node = ctx.configured_targets("my_cell//:my_genrule")
    ///     attrs = node.attrs_eager()
    ///
    ///     # With configuration: $(exe <dep> (<cfg>))
    ///     ctx.output.print(attrs.bash)
    ///
    ///     # Without configuration: $(exe <dep>)
    ///     ctx.output.print(attrs.bash.strip_cfg())
    /// ```
    fn strip_cfg<'v>(
        this: &StarlarkConfiguredAttr,
        _heap: Heap<'v>,
    ) -> starlark::Result<StarlarkCoercedAttr> {
        let stripped = attr_with_stripped_cfg(&this.0)
            .buck_error_context("Failed to strip configuration from attribute")?;
        Ok(StarlarkCoercedAttr(stripped, this.1.dupe()))
    }
}

/// The context for getting attrs lazily on a `StarlarkConfiguredTargetNode`.
#[derive(
    ProvidesStaticType,
    Derivative,
    Display,
    Trace,
    NoSerialize,
    Allocative
)]
#[derivative(Debug)]
#[display("{:?}", self)]
pub(crate) struct StarlarkLazyAttrs<'v> {
    #[trace(unsafe_ignore)]
    #[derivative(Debug = "ignore")]
    #[allocative(skip)]
    configured_target_node: &'v StarlarkConfiguredTargetNode,
}

starlark::methods_static!(LAZY_ATTRS_METHODS = lazy_attrs_methods);

#[starlark_value(type = "bxl.LazyAttrs", StarlarkTypeRepr, UnpackValue)]
impl<'v> StarlarkValue<'v> for StarlarkLazyAttrs<'v> {
    fn get_methods() -> Option<&'static Methods> {
        Some(LAZY_ATTRS_METHODS.methods())
    }
}

impl<'v> AllocValue<'v> for StarlarkLazyAttrs<'v> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_complex_no_freeze(self)
    }
}

impl<'v> StarlarkLazyAttrs<'v> {
    pub(crate) fn new(
        configured_target_node: &'v StarlarkConfiguredTargetNode,
    ) -> StarlarkLazyAttrs<'v> {
        Self {
            configured_target_node,
        }
    }
}

/// The context for getting attrs lazily on a `target_node`.
#[starlark_module]
fn lazy_attrs_methods(builder: &mut MethodsBuilder) {
    /// Gets a single attribute. Returns an optional `[configured_attr]`.
    ///
    /// ```python
    /// def _impl_attrs_lazy(ctx):
    ///     node = ctx.cquery().owner("cell//path/to/TARGETS")[0]
    ///     attrs = node.attrs_lazy() # cache once
    ///     ctx.output.print(attrs.get("some_attributes").value())
    ///     ctx.output.print(attrs.get("some_attribute").label)
    /// ```
    fn get<'v>(
        this: &StarlarkLazyAttrs<'v>,
        attr: &str,
    ) -> starlark::Result<NoneOr<StarlarkConfiguredAttr>> {
        Ok(
            match this
                .configured_target_node
                .0
                .get(attr, AttrInspectOptions::All)
            {
                Some(attr) => NoneOr::Other(StarlarkConfiguredAttr(
                    attr.value,
                    this.configured_target_node.0.label().pkg().dupe(),
                )),
                None => {
                    // Check special attrs
                    let special_attrs = this
                        .configured_target_node
                        .0
                        .special_attrs()
                        .collect::<BuckMutMap<_, _>>();
                    let attr = special_attrs.get(attr);
                    match attr {
                        None => NoneOr::None,
                        Some(attr) => NoneOr::Other(StarlarkConfiguredAttr(
                            attr.clone(),
                            this.configured_target_node.0.label().pkg().dupe(),
                        )),
                    }
                }
            },
        )
    }
}

#[derive(
    ProvidesStaticType,
    Derivative,
    Display,
    Trace,
    NoSerialize,
    Allocative
)]
#[derivative(Debug)]
#[display("{:?}", self)]
pub(crate) struct StarlarkLazyResolvedAttrs<'v> {
    #[derivative(Debug = "ignore")]
    configured_node: ValueTyped<'v, StarlarkConfiguredTargetNode>,
    #[derivative(Debug = "ignore")]
    bxl_context: ValueTyped<'v, BxlContext<'v>>,
    #[derivative(Debug = "ignore")]
    resolution_ctx_data: RefCell<LazyAttrResolutionCache>,
}

starlark::methods_static!(LAZY_RESOLVED_ATTRS_METHODS = lazy_resolved_attrs_methods);

#[starlark_value(type = "bxl.LazyResolvedAttrs", StarlarkTypeRepr, UnpackValue)]
impl<'v> StarlarkValue<'v> for StarlarkLazyResolvedAttrs<'v> {
    fn get_methods() -> Option<&'static Methods> {
        Some(LAZY_RESOLVED_ATTRS_METHODS.methods())
    }
}

impl<'v> AllocValue<'v> for StarlarkLazyResolvedAttrs<'v> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_complex_no_freeze(self)
    }
}

impl<'v> StarlarkLazyResolvedAttrs<'v> {
    pub(crate) fn new(
        configured_node: ValueTyped<'v, StarlarkConfiguredTargetNode>,
        bxl_context: ValueTyped<'v, BxlContext<'v>>,
    ) -> StarlarkLazyResolvedAttrs<'v> {
        let resolution_ctx = LazyAttrResolutionCache {
            dep_analysis_results: None,
            query_results: None,
        };

        Self {
            configured_node,
            bxl_context,
            resolution_ctx_data: RefCell::new(resolution_ctx),
        }
    }

    fn resolution_ctx<'a, 'e, 'c>(
        &'c self,
        eval: &'c mut Evaluator<'v, 'a, 'e>,
    ) -> LazyAttrResolutionContext<'v, 'a, 'e, 'c> {
        LazyAttrResolutionContext {
            eval,
            configured_node: &self.configured_node.as_ref().0,
            ctx: self.bxl_context.as_ref(),
            cache: self.resolution_ctx_data.borrow_mut(),
        }
    }
}

/// The context for getting resolved attrs lazily on a `target_node`.
#[starlark_module]
fn lazy_resolved_attrs_methods(builder: &mut MethodsBuilder) {
    /// Gets a single resolved attribute. Returns an optional configured attribute.
    ///
    /// Gets a single attribute.
    ///
    /// ```python
    /// def _impl_resolved_attrs_lazy(ctx):
    ///     node = ctx.cquery().owner("cell//path/to/TARGETS")[0]
    ///     attrs = node.resolved_attrs_lazy(ctx) # cache once
    ///     ctx.output.print(attrs.get("some_attribute").value())
    ///     ctx.output.print(attrs.get("some_attribute").label)
    /// ```
    fn get<'v>(
        this: &StarlarkLazyResolvedAttrs<'v>,
        attr: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<NoneOr<Value<'v>>> {
        Ok(
            match this.configured_node.0.get(attr, AttrInspectOptions::All) {
                Some(attr) => NoneOr::Other(attr.value.resolve_single(
                    this.configured_node.0.label().pkg(),
                    &mut this.resolution_ctx(eval),
                )?),
                None => {
                    // Check special attrs
                    let special_attrs = this
                        .configured_node
                        .0
                        .special_attrs()
                        .collect::<BuckMutMap<_, _>>();
                    let attr = special_attrs.get(attr);
                    match attr {
                        None => NoneOr::None,
                        Some(attr) => NoneOr::Other(attr.resolve_single(
                            this.configured_node.0.label().pkg(),
                            &mut this.resolution_ctx(eval),
                        )?),
                    }
                }
            },
        )
    }
}

fn unconfigure_macro(
    macr: &buck2_node::attrs::attr_type::arg::ConfiguredMacro,
) -> buck2_node::attrs::attr_type::arg::UnconfiguredMacro {
    match macr {
        buck2_node::attrs::attr_type::arg::MacroBase::Location { label, dep_kind } => {
            buck2_node::attrs::attr_type::arg::MacroBase::Location {
                label: label.unconfigured(),
                dep_kind: *dep_kind,
            }
        }
        buck2_node::attrs::attr_type::arg::MacroBase::Exe { label, exec_dep } => {
            buck2_node::attrs::attr_type::arg::MacroBase::Exe {
                label: label.unconfigured(),
                exec_dep: *exec_dep,
            }
        }
        buck2_node::attrs::attr_type::arg::MacroBase::UserUnkeyedPlaceholder(var_name) => {
            buck2_node::attrs::attr_type::arg::MacroBase::UserUnkeyedPlaceholder(var_name.clone())
        }
        buck2_node::attrs::attr_type::arg::MacroBase::UserKeyedPlaceholder(box_value) => {
            let (var_name, target, arg) = &**box_value;
            buck2_node::attrs::attr_type::arg::MacroBase::UserKeyedPlaceholder(Box::new((
                var_name.clone(),
                target.unconfigured(),
                arg.clone(),
            )))
        }
        buck2_node::attrs::attr_type::arg::MacroBase::Query(query_macro) => {
            buck2_node::attrs::attr_type::arg::MacroBase::Query(Box::new(
                buck2_node::attrs::attr_type::query::QueryMacroBase {
                    expansion_type: query_macro.expansion_type.clone(),
                    query: buck2_node::attrs::attr_type::query::QueryAttrBase {
                        query: query_macro.query.query.clone(),
                        resolved_literals:
                            buck2_node::attrs::attr_type::query::ResolvedQueryLiterals(
                                query_macro
                                    .query
                                    .resolved_literals
                                    .0
                                    .iter()
                                    .map(|(k, v)| (*k, v.unconfigured()))
                                    .collect(),
                            ),
                    },
                },
            ))
        }
        buck2_node::attrs::attr_type::arg::MacroBase::Source(path) => {
            buck2_node::attrs::attr_type::arg::MacroBase::Source(path.clone())
        }
        // The group comes from the attribute type at configuration, so the coerced macro is
        // the ungrouped one.
        buck2_node::attrs::attr_type::arg::MacroBase::InGroup(in_group) => {
            unconfigure_macro(&in_group.0)
        }
    }
}
