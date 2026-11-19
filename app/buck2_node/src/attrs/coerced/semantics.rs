/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! What a coerced attribute value means: the per-kind trait the operations
//! in this module's siblings dispatch to, and the helpers its impls share.

use std::borrow::Cow;

use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use buck2_error::internal_error;
use serde::Serializer;

use crate::attrs::arena::CoercedAttrRef;
use crate::attrs::arena::schema::Kind;
use crate::attrs::arena::schema::View;
use crate::attrs::attr_type::AttrType;
use crate::attrs::coerced::configure::ConcatLeaves;
use crate::attrs::coerced::configure::Configure;
use crate::attrs::coerced::configure::TraverseConfigured;
use crate::attrs::coerced::render::Fmt;
use crate::attrs::coerced::render::JsonKey;
use crate::attrs::coerced::render::MatchesAny;
use crate::attrs::coerced::render::SerializeKind;
use crate::attrs::coerced::render::ToJson;
use crate::attrs::coerced::traverse::Traverse;
use crate::attrs::configured_attr::ConfiguredAttr;
use crate::attrs::display::AttrDisplayWithContextExt;

/// What a kind's values mean: to dependency traversal, configuration,
/// rendering, JSON and substring search. One impl per kind, in the file of
/// the attribute type it belongs to; a kind without one does not compile.
///
/// Each operation receives the value's decoded parts and the operation's
/// operands (the value itself among them) as one struct, so an impl binds
/// only what it uses.
pub(crate) trait Semantics: Kind {
    /// The class of values `+` concatenates: leaves of one class
    /// concatenate; `None` means addition is not supported.
    const CONCAT: Option<&'static str> = None;

    /// Type-threaded traversal over the value's deps.
    fn traverse<'a>(parts: View<'a, Self>, op: Traverse<'_, 'a>) -> buck2_error::Result<()>;

    /// The configured form of the value.
    fn configure(
        parts: View<'_, Self>,
        op: Configure<'_, '_>,
    ) -> ResultMaybeCompatible<ConfiguredAttr>;

    /// The deps of the configured form, without materializing it. Leaves
    /// configure the small configured value and traverse that; containers,
    /// selects and concats recurse.
    fn traverse_configured(
        parts: View<'_, Self>,
        TraverseConfigured {
            v,
            t,
            pkg,
            ctx,
            tr,
            name,
        }: TraverseConfigured<'_, '_>,
    ) -> ResultMaybeCompatible<()> {
        let configured = Self::configure(parts, Configure { v, t, ctx, name })?;
        if let Err(e) = configured.traverse(pkg, tr) {
            return e.into();
        }
        ResultMaybeCompatible::Compatible(())
    }

    /// The leaves `ConfiguredAttr::concat` would see: selects resolved,
    /// nested concats inlined. A plain value is its own leaf.
    fn concat_leaves<'a>(
        _parts: View<'a, Self>,
        ConcatLeaves { v, out, .. }: ConcatLeaves<'_, 'a>,
    ) -> ResultMaybeCompatible<()> {
        out.push(v);
        ResultMaybeCompatible::Compatible(())
    }

    fn to_json(parts: View<'_, Self>, op: ToJson<'_>) -> buck2_error::Result<serde_json::Value>;

    fn fmt(parts: View<'_, Self>, op: Fmt<'_, '_>) -> std::fmt::Result;

    /// Whether any leaf's string form matches the filter.
    fn any_matches(parts: View<'_, Self>, op: MatchesAny<'_>) -> buck2_error::Result<bool>;

    /// The value's rendering as a JSON object key; an error for values that
    /// render as anything but a string.
    fn json_key<'a>(_parts: View<'a, Self>, _op: JsonKey<'_>) -> buck2_error::Result<Cow<'a, str>> {
        Err(internal_error!("dict keys must render to JSON strings"))
    }

    /// Serialize the value's JSON form without building a `serde_json::Value`.
    fn serialize<S: Serializer>(
        parts: View<'_, Self>,
        op: SerializeKind<'_, S>,
    ) -> Result<S::Ok, S::Error>;
}

/// A value whose kind does not fit the attribute type it is read with:
/// the coercer established the pairing, so this is a logic error.
pub(crate) fn mismatch(v: CoercedAttrRef<'_>, t: &AttrType) -> buck2_error::Error {
    internal_error!(
        "attr arena: value `{}` does not fit attribute type `{}`",
        v.as_display_no_ctx(),
        t
    )
}
