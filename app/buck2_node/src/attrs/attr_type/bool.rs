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
use std::fmt::Formatter;

use allocative::Allocative;
use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use dupe::Dupe;
use pagable::Pagable;
use serde::Deserialize;
use serde::Serialize;
use serde::Serializer;
use serde_json::to_value;
use strong_hash::StrongHash;

use crate::attrs::arena::AttrToken;
use crate::attrs::arena::CoercedAttrArenaBuilder;
use crate::attrs::arena::fields::Flag;
use crate::attrs::arena::schema::Kind;
use crate::attrs::arena::schema::View;
use crate::attrs::arena::schema::encode_kind;
use crate::attrs::attr_type::AttrTypeInner;
use crate::attrs::attr_type::any_matches::AnyMatches;
use crate::attrs::coerced::configure::Configure;
use crate::attrs::coerced::configure::TraverseConfigured;
use crate::attrs::coerced::kinds;
use crate::attrs::coerced::render::Fmt;
use crate::attrs::coerced::render::MatchesAny;
use crate::attrs::coerced::render::SerializeKind;
use crate::attrs::coerced::render::ToJson;
use crate::attrs::coerced::semantics::Semantics;
use crate::attrs::coerced::semantics::mismatch;
use crate::attrs::coerced::traverse::Traverse;
use crate::attrs::configured_attr::ConfiguredAttr;

#[derive(
    Debug, Eq, PartialEq, Hash, Pagable, Allocative, Clone, Copy, Dupe, StrongHash
)]
pub struct BoolAttrType;

#[derive(
    Debug,
    Clone,
    Copy,
    Dupe,
    Eq,
    PartialEq,
    Hash,
    StrongHash,
    Allocative,
    Serialize,
    Deserialize,
    Pagable
)]
#[serde(transparent)]
pub struct BoolLiteral(pub bool);

impl Display for BoolLiteral {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.0 {
            write!(f, "True")
        } else {
            write!(f, "False")
        }
    }
}

impl AnyMatches for BoolLiteral {
    fn any_matches(
        &self,
        filter: &dyn Fn(&str) -> buck2_error::Result<bool>,
    ) -> buck2_error::Result<bool> {
        Ok(filter(if self.0 { "True" } else { "False" })?
            || filter(if self.0 { "true" } else { "false" })?)
    }
}

impl Kind for kinds::Bool {
    type Fields = Flag;
}

impl CoercedAttrArenaBuilder {
    pub fn encode_bool(&mut self, b: bool) -> buck2_error::Result<AttrToken> {
        encode_kind::<kinds::Bool>(self, b)
    }
}

impl Semantics for kinds::Bool {
    fn traverse<'a>(_: View<'a, Self>, _: Traverse<'_, 'a>) -> buck2_error::Result<()> {
        Ok(())
    }

    fn configure(
        b: View<'_, Self>,
        Configure { v, t, .. }: Configure<'_, '_>,
    ) -> ResultMaybeCompatible<ConfiguredAttr> {
        match &t.non_optional().0.inner {
            AttrTypeInner::Bool(_) | AttrTypeInner::Any(_) => {
                ResultMaybeCompatible::Compatible(ConfiguredAttr::Bool(BoolLiteral(b)))
            }
            _ => mismatch(v, t).into(),
        }
    }

    fn traverse_configured(
        _: View<'_, Self>,
        _: TraverseConfigured<'_, '_>,
    ) -> ResultMaybeCompatible<()> {
        ResultMaybeCompatible::Compatible(())
    }

    fn to_json(b: View<'_, Self>, _: ToJson<'_>) -> buck2_error::Result<serde_json::Value> {
        Ok(to_value(b)?)
    }

    fn fmt(b: View<'_, Self>, Fmt { f, .. }: Fmt<'_, '_>) -> std::fmt::Result {
        write!(f, "{}", BoolLiteral(b))
    }

    fn any_matches(
        b: View<'_, Self>,
        MatchesAny { filter }: MatchesAny<'_>,
    ) -> buck2_error::Result<bool> {
        BoolLiteral(b).any_matches(filter)
    }

    fn serialize<S: Serializer>(
        b: View<'_, Self>,
        SerializeKind { ser, .. }: SerializeKind<'_, S>,
    ) -> Result<S::Ok, S::Error> {
        ser.serialize_bool(b)
    }
}
