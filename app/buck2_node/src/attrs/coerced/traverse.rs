/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Dependency traversal of coerced values.

use std::fmt::Display;

use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use buck2_core::package::PackageLabel;
use buck2_core::package::source_path::SourcePathRef;
use buck2_core::provider::label::ProvidersLabel;

use crate::attrs::arena::CoercedAttrRef;
use crate::attrs::arena::schema::View;
use crate::attrs::attr_type::AttrType;
use crate::attrs::coerced::configure::TraverseConfigured;
use crate::attrs::coerced::kinds::ValueVisitor;
use crate::attrs::coerced::kinds::dispatch_value;
use crate::attrs::coerced::semantics::Semantics;
use crate::attrs::traversal::CoercedAttrTraversal;

impl<'a> CoercedAttrRef<'a> {
    /// Type-threaded traversal over the value's deps. Callback borrows come
    /// from the arena's side tables, so they carry the arena's lifetime.
    pub fn traverse(
        &self,
        t: &AttrType,
        pkg: Option<PackageLabel>,
        traversal: &mut dyn CoercedAttrTraversal<'a>,
    ) -> buck2_error::Result<()> {
        let kind = self.arena.kind_of(self.r);
        if kind.is_scalar() {
            return Ok(());
        }
        dispatch_value(
            *self,
            Traverse {
                v: *self,
                t,
                pkg,
                tr: traversal,
            },
        )
    }

    /// The first `$(exe)`, `$(location_exec)` or `$(location_toolchain)` macro within the args
    /// of this value whose `attrs.arg()` has no `exec_group`, every `select()` branch included.
    /// `t` is the type the value was coerced to.
    pub fn first_ungrouped_tool_macro(
        &self,
        t: &AttrType,
        pkg: Option<PackageLabel>,
    ) -> buck2_error::Result<Option<String>> {
        struct FirstUngroupedToolMacro(Option<String>);

        impl CoercedAttrTraversal<'_> for FirstUngroupedToolMacro {
            fn dep(&mut self, _dep: &ProvidersLabel) -> buck2_error::Result<()> {
                Ok(())
            }

            fn input(&mut self, _input: SourcePathRef) -> buck2_error::Result<()> {
                Ok(())
            }

            fn ungrouped_tool_macro(&mut self, m: &dyn Display) -> buck2_error::Result<()> {
                if self.0.is_none() {
                    self.0 = Some(m.to_string());
                }
                Ok(())
            }
        }

        let mut first = FirstUngroupedToolMacro(None);
        self.traverse(t, pkg, &mut first)?;
        Ok(first.0)
    }
}

/// The operands of one value operation, as [`dispatch_value`] hands them to
/// the kind's [`Semantics`]; `v` is the value itself.
pub(crate) struct Traverse<'x, 'a> {
    pub(crate) v: CoercedAttrRef<'a>,
    pub(crate) t: &'x AttrType,
    pub(crate) pkg: Option<PackageLabel>,
    pub(crate) tr: &'x mut dyn CoercedAttrTraversal<'a>,
}

impl<'a> ValueVisitor<'a> for Traverse<'_, 'a> {
    type Out = buck2_error::Result<()>;
    fn visit<K: Semantics + 'static>(self, view: View<'a, K>) -> Self::Out {
        K::traverse(view, self)
    }
}

impl<'a> ValueVisitor<'a> for TraverseConfigured<'_, 'a> {
    type Out = ResultMaybeCompatible<()>;
    fn visit<K: Semantics + 'static>(self, view: View<'a, K>) -> Self::Out {
        K::traverse_configured(view, self)
    }
}
