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

use allocative::Allocative;
use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use pagable::Pagable;
use serde::Serializer;
use strong_hash::StrongHash;

use crate::attrs::arena::AttrToken;
use crate::attrs::arena::CoercedAttrArenaBuilder;
use crate::attrs::arena::schema::Kind;
use crate::attrs::arena::schema::View;
use crate::attrs::arena::schema::encode_kind;
use crate::attrs::attr_type::AttrType;
use crate::attrs::coerced::configure::Configure;
use crate::attrs::coerced::configure::TraverseConfigured;
use crate::attrs::coerced::kinds;
use crate::attrs::coerced::render::Fmt;
use crate::attrs::coerced::render::MatchesAny;
use crate::attrs::coerced::render::SerializeKind;
use crate::attrs::coerced::render::ToJson;
use crate::attrs::coerced::semantics::Semantics;
use crate::attrs::coerced::traverse::Traverse;
use crate::attrs::configured_attr::ConfiguredAttr;

#[derive(Debug, Eq, PartialEq, Hash, Pagable, Allocative, StrongHash)]
pub struct OptionAttrType {
    pub inner: AttrType,
}

impl OptionAttrType {
    pub fn new(inner: AttrType) -> Self {
        Self { inner }
    }

    pub(crate) fn fmt_with_arg(&self, f: &mut fmt::Formatter<'_>, arg: &str) -> fmt::Result {
        write!(f, "attrs.option({}{})", self.inner, arg)
    }
}

impl Kind for kinds::None {
    type Fields = ();
}

impl CoercedAttrArenaBuilder {
    pub fn encode_none(&mut self) -> buck2_error::Result<AttrToken> {
        encode_kind::<kinds::None>(self, ())
    }
}

impl Semantics for kinds::None {
    fn traverse<'a>((): View<'a, Self>, _: Traverse<'_, 'a>) -> buck2_error::Result<()> {
        Ok(())
    }

    fn configure(
        (): View<'_, Self>,
        _: Configure<'_, '_>,
    ) -> ResultMaybeCompatible<ConfiguredAttr> {
        ResultMaybeCompatible::Compatible(ConfiguredAttr::None)
    }

    fn traverse_configured(
        (): View<'_, Self>,
        _: TraverseConfigured<'_, '_>,
    ) -> ResultMaybeCompatible<()> {
        ResultMaybeCompatible::Compatible(())
    }

    fn to_json((): View<'_, Self>, _: ToJson<'_>) -> buck2_error::Result<serde_json::Value> {
        Ok(serde_json::Value::Null)
    }

    fn fmt((): View<'_, Self>, Fmt { f, .. }: Fmt<'_, '_>) -> std::fmt::Result {
        write!(f, "None")
    }

    fn any_matches((): View<'_, Self>, _: MatchesAny<'_>) -> buck2_error::Result<bool> {
        Ok(false)
    }

    fn serialize<S: Serializer>(
        (): View<'_, Self>,
        SerializeKind { ser, .. }: SerializeKind<'_, S>,
    ) -> Result<S::Ok, S::Error> {
        ser.serialize_unit()
    }
}
