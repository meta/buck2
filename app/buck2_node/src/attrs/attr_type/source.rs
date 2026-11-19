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

use allocative::Allocative;
use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use buck2_core::package::source_path::SourcePathRef;
use buck2_core::provider::label::ProvidersLabel;
use buck2_error::internal_error;
use dupe::Dupe;
use pagable::Pagable;
use serde::Serializer;
use serde_json::to_value;
use strong_hash::StrongHash;

use crate::attrs::arena::AttrToken;
use crate::attrs::arena::CoercedAttrArenaBuilder;
use crate::attrs::arena::fields::Label;
use crate::attrs::arena::fields::Path;
use crate::attrs::arena::schema::Kind;
use crate::attrs::arena::schema::View;
use crate::attrs::arena::schema::encode_kind;
use crate::attrs::attr_type::AttrTypeInner;
use crate::attrs::attr_type::attr_config::source_file_display;
use crate::attrs::coerced::configure::Configure;
use crate::attrs::coerced::configure::TraverseConfigured;
use crate::attrs::coerced::kinds;
use crate::attrs::coerced::render::Fmt;
use crate::attrs::coerced::render::JsonKey;
use crate::attrs::coerced::render::MatchesAny;
use crate::attrs::coerced::render::SerializeKind;
use crate::attrs::coerced::render::ToJson;
use crate::attrs::coerced::render::json_label;
use crate::attrs::coerced::semantics::Semantics;
use crate::attrs::coerced::semantics::mismatch;
use crate::attrs::coerced::traverse::Traverse;
use crate::attrs::coerced_path::CoercedPath;
use crate::attrs::configured_attr::ConfiguredAttr;

#[derive(
    Debug, Pagable, Eq, PartialEq, Hash, Allocative, Clone, Copy, Dupe, StrongHash
)]
pub struct SourceAttrType {
    pub allow_directory: bool,
}

impl Kind for kinds::SourceLabel {
    type Fields = Label;
}

impl CoercedAttrArenaBuilder {
    pub fn encode_source_label(&mut self, l: &ProvidersLabel) -> buck2_error::Result<AttrToken> {
        encode_kind::<kinds::SourceLabel>(self, l)
    }
}

impl Semantics for kinds::SourceLabel {
    fn traverse<'a>(
        l: View<'a, Self>,
        Traverse { v, t, tr, .. }: Traverse<'_, 'a>,
    ) -> buck2_error::Result<()> {
        match &t.non_optional().0.inner {
            AttrTypeInner::Source(_) => tr.dep(l),
            _ => Err(mismatch(v, t)),
        }
    }

    fn configure(
        l: View<'_, Self>,
        Configure { v, t, ctx, .. }: Configure<'_, '_>,
    ) -> ResultMaybeCompatible<ConfiguredAttr> {
        match &t.non_optional().0.inner {
            AttrTypeInner::Source(_) => ResultMaybeCompatible::Compatible(
                ConfiguredAttr::SourceLabel(l.configure_pair(ctx.cfg().cfg_pair().dupe())),
            ),
            _ => mismatch(v, t).into(),
        }
    }

    fn traverse_configured(
        l: View<'_, Self>,
        TraverseConfigured { v, t, ctx, tr, .. }: TraverseConfigured<'_, '_>,
    ) -> ResultMaybeCompatible<()> {
        match &t.non_optional().0.inner {
            AttrTypeInner::Source(_) => {
                let configured = l.configure_pair(ctx.cfg().cfg_pair().dupe());
                if let Err(e) = tr.dep(&configured) {
                    return e.into();
                }
            }
            _ => return mismatch(v, t).into(),
        }
        ResultMaybeCompatible::Compatible(())
    }

    fn to_json(l: View<'_, Self>, _: ToJson<'_>) -> buck2_error::Result<serde_json::Value> {
        json_label(l)
    }

    fn fmt(l: View<'_, Self>, Fmt { f, .. }: Fmt<'_, '_>) -> std::fmt::Result {
        write!(f, "\"{l}\"")
    }

    fn any_matches(
        l: View<'_, Self>,
        MatchesAny { filter }: MatchesAny<'_>,
    ) -> buck2_error::Result<bool> {
        filter(&l.to_string())
    }

    fn json_key<'a>(l: View<'a, Self>, _: JsonKey<'_>) -> buck2_error::Result<Cow<'a, str>> {
        Ok(Cow::Owned(l.to_string()))
    }

    fn serialize<S: Serializer>(
        l: View<'_, Self>,
        SerializeKind { ser, .. }: SerializeKind<'_, S>,
    ) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&l.to_string())
    }
}

impl Kind for kinds::SourceFile {
    type Fields = Path;
}

impl CoercedAttrArenaBuilder {
    pub fn encode_source_file(&mut self, p: &CoercedPath) -> buck2_error::Result<AttrToken> {
        encode_kind::<kinds::SourceFile>(self, p)
    }
}

impl Semantics for kinds::SourceFile {
    fn traverse<'a>(
        p: View<'a, Self>,
        Traverse { v, t, pkg, tr }: Traverse<'_, 'a>,
    ) -> buck2_error::Result<()> {
        match &t.non_optional().0.inner {
            AttrTypeInner::Source(_) => {
                let pkg = pkg.ok_or_else(|| {
                    internal_error!("Expected a package when traversing coerced source attribute")
                })?;
                for x in p.inputs() {
                    tr.input(SourcePathRef::new(pkg, x))?;
                }
                Ok(())
            }
            _ => Err(mismatch(v, t)),
        }
    }

    fn configure(
        p: View<'_, Self>,
        Configure { v, t, .. }: Configure<'_, '_>,
    ) -> ResultMaybeCompatible<ConfiguredAttr> {
        match &t.non_optional().0.inner {
            AttrTypeInner::Source(_) => {
                ResultMaybeCompatible::Compatible(ConfiguredAttr::SourceFile(p.clone()))
            }
            _ => mismatch(v, t).into(),
        }
    }

    fn traverse_configured(
        p: View<'_, Self>,
        TraverseConfigured { v, t, pkg, tr, .. }: TraverseConfigured<'_, '_>,
    ) -> ResultMaybeCompatible<()> {
        match &t.non_optional().0.inner {
            AttrTypeInner::Source(_) => {
                for x in p.inputs() {
                    if let Err(e) = tr.input(SourcePathRef::new(pkg, x)) {
                        return e.into();
                    }
                }
            }
            _ => return mismatch(v, t).into(),
        }
        ResultMaybeCompatible::Compatible(())
    }

    fn to_json(
        p: View<'_, Self>,
        ToJson { ctx }: ToJson<'_>,
    ) -> buck2_error::Result<serde_json::Value> {
        Ok(to_value(source_file_display(ctx, p).to_string())?)
    }

    fn fmt(p: View<'_, Self>, Fmt { ctx, f }: Fmt<'_, '_>) -> std::fmt::Result {
        write!(f, "\"{}\"", source_file_display(ctx, p))
    }

    fn any_matches(
        p: View<'_, Self>,
        MatchesAny { filter }: MatchesAny<'_>,
    ) -> buck2_error::Result<bool> {
        filter(&p.path().to_string())
    }

    fn json_key<'a>(
        p: View<'a, Self>,
        JsonKey { ctx }: JsonKey<'_>,
    ) -> buck2_error::Result<Cow<'a, str>> {
        Ok(Cow::Owned(source_file_display(ctx, p).to_string()))
    }

    fn serialize<S: Serializer>(
        p: View<'_, Self>,
        SerializeKind { ctx, ser }: SerializeKind<'_, S>,
    ) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&source_file_display(ctx, p).to_string())
    }
}
