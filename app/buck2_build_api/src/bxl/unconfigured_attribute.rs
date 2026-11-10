/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Display;
use std::fmt::Formatter;

use allocative::Allocative;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_artifact::artifact::source_artifact::SourceArtifact;
use buck2_core::package::PackageLabel;
use buck2_core::package::source_path::SourcePath;
use buck2_error::internal_error;
use buck2_error::starlark_error::from_starlark_with_options;
use buck2_hash::BuckMutInsertionMap;
use buck2_interpreter::types::configured_providers_label::StarlarkProvidersLabel;
use buck2_interpreter::types::select_fail::StarlarkSelectFail;
use buck2_interpreter::types::select_incompatible::StarlarkSelectIncompatible;
use buck2_interpreter::types::target_label::StarlarkTargetLabel;
use buck2_node::attrs::arena::CoercedAttrRef;
use buck2_node::attrs::arena::OwnedCoercedAttr;
use buck2_node::attrs::coerced::CoercedAttrRefKind;
use buck2_node::attrs::display::AttrDisplayWithContext;
use buck2_node::attrs::fmt_context::AttrFmtContext;
use buck2_node::attrs::serialize::AttrSerializeWithContext;
use buck2_node::visibility::VisibilityPattern;
use buck2_node::visibility::VisibilityPatternList;
use buck2_node::visibility::VisibilitySpecification;
use buck2_node::visibility::WithinViewSpecification;
use derive_more::From;
use dupe::Dupe;
use pagable::Pagable;
use serde::Serialize;
use starlark::__derive_refs::serde::Serializer;
use starlark::any::ProvidesStaticType;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::starlark_module;
use starlark::starlark_simple_value;
use starlark::values::Heap;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::Value;
use starlark::values::list::AllocList;
use starlark::values::starlark_value;
use starlark::values::tuple::AllocTuple;

use crate::bxl::select::StarlarkSelectConcat;
use crate::bxl::select::StarlarkSelectDict;
use crate::interpreter::rule_defs::artifact::starlark_artifact::StarlarkArtifact;

#[derive(Debug, ProvidesStaticType, From, Allocative, Pagable, StarlarkPagable)]
pub struct StarlarkCoercedAttr(
    #[starlark_pagable(pagable)] pub OwnedCoercedAttr,
    #[starlark_pagable(pagable)] pub PackageLabel,
);

starlark_simple_value!(StarlarkCoercedAttr);

impl Display for StarlarkCoercedAttr {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.as_ref().fmt(
            &AttrFmtContext {
                package: Some(self.1.dupe()),
                options: Default::default(),
            },
            f,
        )
    }
}

impl Serialize for StarlarkCoercedAttr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.as_ref().serialize_with_ctx(
            &AttrFmtContext {
                package: Some(self.1.dupe()),
                options: Default::default(),
            },
            serializer,
        )
    }
}

starlark::methods_static!(COERCED_ATTR_METHODS = coerced_attr_methods);

/// Coerced attr from an unconfigured target node.
#[starlark_value(type = "CoercedAttr")]
impl<'v> StarlarkValue<'v> for StarlarkCoercedAttr {
    fn get_methods() -> Option<&'static Methods> {
        Some(COERCED_ATTR_METHODS.methods())
    }
}

/// Methods on configured target node's attributes.
#[starlark_module]
fn coerced_attr_methods(builder: &mut MethodsBuilder) {
    /// Returns the type name of the attribute
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_type(ctx):
    ///     node = ctx.uquery().owner("bin/TARGETS")[0]
    ///     ctx.output.print(node.attrs.name.type)
    /// ```
    // FIXME(JakobDegen): Strings as types are mostly dead, users should be getting the value and
    // using `isinstance` instead. Remove this.
    #[starlark(attribute)]
    fn r#type<'v>(this: &StarlarkCoercedAttr, heap: Heap<'v>) -> starlark::Result<&'v str> {
        Ok(this.0.to_value(this.1.dupe(), heap)?.get_type())
    }

    /// Returns the value of this attribute. Limited support of selects, concats, and explicit configuration deps
    /// at this time.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_value(ctx):
    ///     node = ctx.uquery().owner("bin/TARGETS")[0]
    ///     ctx.output.print(node.attrs.name.value())
    /// ```
    fn value<'v>(this: &StarlarkCoercedAttr, heap: Heap<'v>) -> starlark::Result<Value<'v>> {
        Ok(this.0.to_value(this.1.dupe(), heap)?)
    }
}

pub trait OwnedCoercedAttrExt {
    fn to_value<'v>(&self, pkg: PackageLabel, heap: Heap<'v>) -> buck2_error::Result<Value<'v>>;
}

impl OwnedCoercedAttrExt for OwnedCoercedAttr {
    /// Converts the coerced attr to a starlark value
    fn to_value<'v>(&self, pkg: PackageLabel, heap: Heap<'v>) -> buck2_error::Result<Value<'v>> {
        ref_to_value(self.as_ref(), self, pkg, heap)
    }
}

/// Converts one value of `owner`'s storage to a starlark value. `attr` must
/// be a view into `owner`'s storage: select values allocated here hold
/// `owner.sub(..)` handles to render their entries lazily.
pub(crate) fn ref_to_value<'v>(
    attr: CoercedAttrRef<'_>,
    owner: &OwnedCoercedAttr,
    pkg: PackageLabel,
    heap: Heap<'v>,
) -> buck2_error::Result<Value<'v>> {
    Ok(match attr.kind() {
        CoercedAttrRefKind::None => Value::new_none(),
        CoercedAttrRefKind::Bool(b) => heap.alloc(b),
        CoercedAttrRefKind::Int(i) => heap.alloc(i),
        CoercedAttrRefKind::String(s) | CoercedAttrRefKind::EnumVariant(s) => heap.alloc(s),
        // Mirrors `Display for StringWithMacros`: a simple arg renders as its
        // text, a macro-bearing one as its parts with `$(...)` wrappers.
        CoercedAttrRefKind::ArgSimple(s) => heap.alloc(s),
        CoercedAttrRefKind::Arg(parts) => heap.alloc(parts.to_string()),
        CoercedAttrRefKind::List(seq) => heap.alloc(
            seq.iter()
                .map(|v| ref_to_value(v, owner, pkg.dupe(), heap))
                .collect::<buck2_error::Result<Vec<_>>>()?,
        ),
        CoercedAttrRefKind::Tuple(seq) => heap.alloc(AllocTuple(
            seq.iter()
                .map(|v| ref_to_value(v, owner, pkg.dupe(), heap))
                .collect::<buck2_error::Result<Vec<_>>>()?,
        )),
        CoercedAttrRefKind::Dict(pairs) => {
            let mut res = BuckMutInsertionMap::with_capacity(pairs.len());
            for (k, v) in pairs.iter() {
                res.insert_hashed(
                    ref_to_value(k, owner, pkg.dupe(), heap)?
                        .get_hashed()
                        .map_err(|e| {
                            from_starlark_with_options(
                                e,
                                buck2_error::starlark_error::NativeErrorHandling::Unknown,
                                false,
                            )
                        })?,
                    ref_to_value(v, owner, pkg.dupe(), heap)?,
                );
            }
            heap.alloc(res)
        }
        CoercedAttrRefKind::OneOf((_, l)) => ref_to_value(l, owner, pkg, heap)?,
        CoercedAttrRefKind::Selector(_) => heap
            .alloc_representation(StarlarkSelectDict::new(owner.sub(attr), pkg.dupe()))
            .to_value(),
        CoercedAttrRefKind::Concat(_) => heap
            .alloc_representation(StarlarkSelectConcat::new(owner.sub(attr), pkg.dupe()))
            .to_value(),
        CoercedAttrRefKind::Dep(d) => heap
            .alloc_representation(StarlarkProvidersLabel::new(d.dupe()))
            .to_value(),
        CoercedAttrRefKind::Label(l) => heap
            .alloc_representation(StarlarkProvidersLabel::new(l.dupe()))
            .to_value(),
        CoercedAttrRefKind::SourceLabel(s) => heap
            .alloc_representation(StarlarkProvidersLabel::new(s.dupe()))
            .to_value(),
        CoercedAttrRefKind::ConfigurationDep(c) => {
            // TODO(T198210718)
            heap.alloc_representation(StarlarkTargetLabel::new(c.target().dupe()))
                .to_value()
        }
        CoercedAttrRefKind::SplitTransitionDep(d) => heap
            .alloc_representation(StarlarkProvidersLabel::new(d.dupe()))
            .to_value(),
        CoercedAttrRefKind::PluginDep(d) => heap
            .alloc_representation(StarlarkTargetLabel::new(d.dupe()))
            .to_value(),
        CoercedAttrRefKind::SourceFile(f) => heap
            .alloc_representation(StarlarkArtifact::new(Artifact::from(SourceArtifact::new(
                SourcePath::new(pkg.to_owned(), f.path().dupe()),
            ))))
            .to_value(),
        CoercedAttrRefKind::SelectFail(message) => heap
            .alloc_representation(StarlarkSelectFail::new(heap.alloc_str(message)))
            .to_value(),
        CoercedAttrRefKind::SelectIncompatible(message) => heap
            .alloc_representation(StarlarkSelectIncompatible::new(heap.alloc_str(message)))
            .to_value(),
        // TODO(@wendyy) - this needs better support
        CoercedAttrRefKind::ExplicitConfiguredDep((label, _)) => heap
            .alloc_representation(StarlarkProvidersLabel::new(label.dupe()))
            .to_value(),
        CoercedAttrRefKind::TransitionDep((dep, transition)) => {
            let label = heap
                .alloc_representation(StarlarkProvidersLabel::new(dep.dupe()))
                .to_value();
            match transition {
                Some(t) => heap.alloc((
                    label,
                    heap.alloc_representation(StarlarkProvidersLabel::new(t.dupe()))
                        .to_value(),
                )),
                None => label,
            }
        }
        CoercedAttrRefKind::Query(q) => heap.alloc(q.query().0),
        CoercedAttrRefKind::Modifiers(m) => m.alloc_starlark(heap),
        CoercedAttrRefKind::Visibility(VisibilitySpecification(specs))
        | CoercedAttrRefKind::WithinView(WithinViewSpecification(specs)) => {
            pattern_list_to_value(specs, heap)?
        }
        CoercedAttrRefKind::Metadata(data) => heap.alloc(data.to_value()),
    })
}

fn pattern_list_to_value<'v>(
    specs: &VisibilityPatternList,
    heap: Heap<'v>,
) -> buck2_error::Result<Value<'v>> {
    Ok(match specs {
        VisibilityPatternList::Public => heap.alloc(AllocList(["PUBLIC"])),
        VisibilityPatternList::List(specs) => {
            heap.alloc(AllocList(specs.iter().map(|s| match s {
                VisibilityPattern::Parsed(p) => heap.alloc(p.to_string()),
                VisibilityPattern::TargetNameGlob(r) => r.alloc_starlark_value(heap),
            })))
        }
        VisibilityPatternList::Intersection(_) => {
            return Err(internal_error!(
                "Intersection visibility cannot be serialized as attribute"
            ));
        }
    })
}
