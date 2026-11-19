/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::fmt::Display;

use allocative::Allocative;
use buck2_core::package::source_path::SourcePathRef;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_hash::BuckFixedSortedSet;
use buck2_hash::BuckMutInsertionSet;
use dupe::Dupe;
use pagable::Pagable;
use starlark_map::Hashed;

use crate::attrs::arena::AttrToken;
use crate::attrs::arena::CoercedAttrArenaBuilder;
use crate::attrs::arena::CoercedAttrRef;
use crate::attrs::arena::OwnedCoercedAttr;
use crate::attrs::attr_type::AttrType;
use crate::attrs::attr_type::configuration_dep::ConfigurationDepKind;
use crate::attrs::display::AttrDisplayWithContextExt;
use crate::attrs::traversal::CoercedAttrTraversal;

#[derive(Clone, Debug, Hash, Eq, PartialEq, Pagable, Allocative)]
enum AttributeDefault {
    No,
    Yes(OwnedCoercedAttr),
    YesWithAllowed {
        default: OwnedCoercedAttr,
        allowed_deps: Hashed<BuckFixedSortedSet<TargetLabel>>,
    },
    // N.B. DefaultOnly attributes are not checked for within_view, so we don't have to track allowed_deps for them
    DefaultOnly(OwnedCoercedAttr),
}

/// Starlark compatible container for results from e.g. `attrs.string()`
#[derive(Clone, Debug, Eq, PartialEq, Hash, Pagable, Allocative)]
pub struct Attribute {
    /// The default value. If None, the value is not optional and must be provided by the user
    default: AttributeDefault,
    /// Documentation for what the attribute actually means
    doc: String,
    /// The coercer to take this parameter's value from Starlark value -> an
    /// internal representation
    coercer: AttrType,
}

/// Encodes an attribute's constant default into the storage that will hold it.
pub type DefaultEncoder = fn(&mut CoercedAttrArenaBuilder) -> buck2_error::Result<AttrToken>;

impl Attribute {
    pub fn new_const(default: Option<DefaultEncoder>, doc: &str, coercer: AttrType) -> Self {
        Self::new(default, doc, coercer).expect("Attribute::new_const failed")
    }

    pub fn new(
        default: Option<DefaultEncoder>,
        doc: &str,
        coercer: AttrType,
    ) -> buck2_error::Result<Self> {
        let default = default.map(OwnedCoercedAttr::new_with).transpose()?;
        Self::new_sealed(default, doc, coercer)
    }

    /// Like [`Self::new`], for a default coerced through an arena-encoding
    /// context: the token was minted by the paired builder, which becomes
    /// the default's own sealed storage.
    pub fn new_from_builder(
        default: Option<(CoercedAttrArenaBuilder, AttrToken)>,
        doc: &str,
        coercer: AttrType,
    ) -> buck2_error::Result<Self> {
        let default = default
            .map(|(builder, d)| OwnedCoercedAttr::seal_from_builder(builder, d))
            .transpose()?;
        Self::new_sealed(default, doc, coercer)
    }

    fn new_sealed(
        default: Option<OwnedCoercedAttr>,
        doc: &str,
        coercer: AttrType,
    ) -> buck2_error::Result<Self> {
        Ok(Attribute {
            default: match default {
                Some(default) => {
                    let allowed_deps = collect_default_deps(default.as_ref(), &coercer)?;
                    if allowed_deps.is_empty() {
                        AttributeDefault::Yes(default)
                    } else {
                        AttributeDefault::YesWithAllowed {
                            default,
                            allowed_deps: Hashed::new(allowed_deps),
                        }
                    }
                }
                None => AttributeDefault::No,
            },
            doc: doc.to_owned(),
            coercer,
        })
    }

    pub fn new_default_only(default: OwnedCoercedAttr, doc: &str, coercer: AttrType) -> Self {
        Attribute {
            default: AttributeDefault::DefaultOnly(default),
            doc: doc.to_owned(),
            coercer,
        }
    }

    pub fn coercer(&self) -> &AttrType {
        &self.coercer
    }

    pub fn is_default_only(&self) -> bool {
        matches!(self.default, AttributeDefault::DefaultOnly(_))
    }

    /// The default value as a view into its sealed storage.
    pub fn default_as_ref(&self) -> Option<CoercedAttrRef<'_>> {
        self.default_as_owned().map(OwnedCoercedAttr::as_ref)
    }

    /// The default value in its owned form, for constructing another
    /// attribute around the same default (`attrs.default_only`).
    pub fn default_as_owned(&self) -> Option<&OwnedCoercedAttr> {
        match &self.default {
            AttributeDefault::Yes(x)
            | AttributeDefault::YesWithAllowed { default: x, .. }
            | AttributeDefault::DefaultOnly(x) => Some(x),
            AttributeDefault::No => None,
        }
    }

    pub fn default_allowed_deps(&self) -> Option<&BuckFixedSortedSet<TargetLabel>> {
        match &self.default {
            AttributeDefault::YesWithAllowed { allowed_deps, .. } => Some(&**allowed_deps),
            AttributeDefault::Yes(_) | AttributeDefault::DefaultOnly(_) | AttributeDefault::No => {
                None
            }
        }
    }

    pub fn doc(&self) -> &str {
        &self.doc
    }
}

impl Display for Attribute {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.coercer.fmt_with_default(
            f,
            self.default_as_ref()
                .map(|x| x.as_display_no_ctx().to_string())
                .as_deref(),
        )
    }
}

/// Attribute which may be either a custom value supplied by the user, or missing/None to indicate use the default.
#[derive(Eq, PartialEq)]
pub enum CoercedValue {
    Custom(AttrToken),
    Default,
}

fn collect_default_deps(
    default: CoercedAttrRef<'_>,
    attr_type: &AttrType,
) -> buck2_error::Result<BuckFixedSortedSet<TargetLabel>> {
    struct CollectDefaultsTraversal<'a> {
        deps: &'a mut BuckMutInsertionSet<TargetLabel>,
    }

    // N.B. the traversal here needs to match the `dep` cases that check_within_view checks
    impl<'a> CoercedAttrTraversal<'a> for CollectDefaultsTraversal<'a> {
        fn dep(&mut self, dep: &ProvidersLabel) -> buck2_error::Result<()> {
            self.deps.insert(dep.target().dupe());
            Ok(())
        }

        fn configuration_dep(
            &mut self,
            dep: &ProvidersLabel,
            t: ConfigurationDepKind,
        ) -> buck2_error::Result<()> {
            match t {
                // Skip some configuration deps
                ConfigurationDepKind::CompatibilityAttribute
                | ConfigurationDepKind::DefaultTargetPlatform
                | ConfigurationDepKind::SelectKey => (),
                ConfigurationDepKind::ConfiguredDepPlatform | ConfigurationDepKind::Transition => {
                    self.deps.insert(dep.target().dupe());
                }
            }
            Ok(())
        }

        fn input(&mut self, _input: SourcePathRef) -> buck2_error::Result<()> {
            Ok(())
        }
    }

    let mut default_deps = BuckMutInsertionSet::default();
    default.traverse(
        attr_type,
        None,
        &mut CollectDefaultsTraversal {
            deps: &mut default_deps,
        },
    )?;
    Ok(default_deps.into())
}
