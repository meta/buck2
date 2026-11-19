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
use std::fmt;
use std::hash::Hash;

use allocative::Allocative;
use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use buck2_hash::BuckFixedInsertionSet;
use buck2_hash::BuckMutInsertionSet;
use buck2_util::arc_str::ArcStr;
use pagable::Pagable;
use serde::Serializer;
use serde_json::to_value;
use strong_hash::StrongHash;

use crate::attrs::arena::AttrToken;
use crate::attrs::arena::CoercedAttrArenaBuilder;
use crate::attrs::arena::fields::Text;
use crate::attrs::arena::schema::Kind;
use crate::attrs::arena::schema::View;
use crate::attrs::arena::schema::encode_kind;
use crate::attrs::attr_type::AttrTypeInner;
use crate::attrs::attr_type::string::StringLiteral;
use crate::attrs::coerced::configure::Configure;
use crate::attrs::coerced::configure::TraverseConfigured;
use crate::attrs::coerced::kinds;
use crate::attrs::coerced::render::Fmt;
use crate::attrs::coerced::render::JsonKey;
use crate::attrs::coerced::render::MatchesAny;
use crate::attrs::coerced::render::SerializeKind;
use crate::attrs::coerced::render::ToJson;
use crate::attrs::coerced::render::fmt_text;
use crate::attrs::coerced::semantics::Semantics;
use crate::attrs::coerced::semantics::mismatch;
use crate::attrs::coerced::traverse::Traverse;
use crate::attrs::configured_attr::ConfiguredAttr;

#[derive(Debug, Pagable, Eq, PartialEq, Hash, Allocative)]
pub struct EnumAttrType {
    pub variants: BuckFixedInsertionSet<ArcStr>,
}

impl StrongHash for EnumAttrType {
    fn strong_hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.variants.len().strong_hash(state);
        for v in self.variants.iter() {
            v.strong_hash(state);
        }
    }
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum EnumAttrError {
    #[error("enum.attr() variant names must all be lowercase, got `{0}`")]
    NotLowercase(String),
    #[error("enum.attr() variant names must all be distinct, got repeated `{0}`")]
    DuplicateVariant(String),
}

impl EnumAttrType {
    pub fn new(variants: Vec<String>) -> buck2_error::Result<Self> {
        let mut result = BuckMutInsertionSet::with_capacity(variants.len());
        for x in variants {
            if x != x.to_lowercase() {
                return Err(EnumAttrError::NotLowercase(x).into());
            }
            if result.contains(x.as_str()) {
                return Err(EnumAttrError::DuplicateVariant(x).into());
            }
            result.insert(ArcStr::from(x));
        }
        Ok(Self {
            variants: result.freeze(),
        })
    }

    pub fn fmt_with_arg(&self, f: &mut fmt::Formatter<'_>, arg: &str) -> fmt::Result {
        write!(f, "attrs.enum([")?;
        for (i, x) in self.variants.iter().enumerate() {
            if i != 0 {
                write!(f, ",")?;
            }
            write!(f, "{x:?}")?;
        }
        write!(f, "]{arg})")
    }
}

impl Kind for kinds::EnumVariant {
    type Fields = Text;
}

impl CoercedAttrArenaBuilder {
    pub fn encode_enum_variant(&mut self, s: &str) -> buck2_error::Result<AttrToken> {
        encode_kind::<kinds::EnumVariant>(self, s)
    }
}

impl Semantics for kinds::EnumVariant {
    fn traverse<'a>(_: View<'a, Self>, _: Traverse<'_, 'a>) -> buck2_error::Result<()> {
        Ok(())
    }

    fn configure(
        s: View<'_, Self>,
        Configure { v, t, .. }: Configure<'_, '_>,
    ) -> ResultMaybeCompatible<ConfiguredAttr> {
        match &t.non_optional().0.inner {
            AttrTypeInner::Enum(_) => ResultMaybeCompatible::Compatible(
                ConfiguredAttr::EnumVariant(StringLiteral(ArcStr::from(s))),
            ),
            _ => mismatch(v, t).into(),
        }
    }

    fn traverse_configured(
        _: View<'_, Self>,
        _: TraverseConfigured<'_, '_>,
    ) -> ResultMaybeCompatible<()> {
        ResultMaybeCompatible::Compatible(())
    }

    fn to_json(s: View<'_, Self>, _: ToJson<'_>) -> buck2_error::Result<serde_json::Value> {
        Ok(to_value(s)?)
    }

    fn fmt(s: View<'_, Self>, Fmt { ctx, f }: Fmt<'_, '_>) -> std::fmt::Result {
        fmt_text(s, ctx, f)
    }

    fn any_matches(
        s: View<'_, Self>,
        MatchesAny { filter }: MatchesAny<'_>,
    ) -> buck2_error::Result<bool> {
        filter(s)
    }

    fn json_key<'a>(s: View<'a, Self>, _: JsonKey<'_>) -> buck2_error::Result<Cow<'a, str>> {
        Ok(Cow::Borrowed(s))
    }

    fn serialize<S: Serializer>(
        s: View<'_, Self>,
        SerializeKind { ser, .. }: SerializeKind<'_, S>,
    ) -> Result<S::Ok, S::Error> {
        ser.serialize_str(s)
    }
}
