/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use core::fmt;
use std::borrow::Cow;
use std::fmt::Display;
use std::hash::Hasher;

use buck2_core::configuration::compatibility::IncompatiblePlatformReasonCause;
use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use buck2_core::provider::label::ProvidersLabel;
use buck2_error::buck2_error;
use buck2_error::internal_error;
use buck2_hash::BuckHasherBuilder;
use buck2_hash::BuckMutSet;
use buck2_util::arc_str::ArcStr;
use display_container::fmt_keyed_container;
use itertools::Itertools;
use serde::Serialize;
use serde::Serializer;
use serde::ser::SerializeMap;
use smallvec::SmallVec;
use strong_hash::StrongHash;

use crate::attrs::arena::AttrToken;
use crate::attrs::arena::CoercedAttrArenaBuilder;
use crate::attrs::arena::CoercedAttrRef;
use crate::attrs::arena::Refs;
use crate::attrs::arena::Values;
use crate::attrs::arena::fields::Children;
use crate::attrs::arena::fields::Cursor;
use crate::attrs::arena::fields::Extent;
use crate::attrs::arena::fields::Field;
use crate::attrs::arena::fields::IdHasher;
use crate::attrs::arena::fields::OptChild;
use crate::attrs::arena::fields::RecordWriter;
use crate::attrs::arena::fields::Text;
use crate::attrs::arena::schema::Kind;
use crate::attrs::arena::schema::View;
use crate::attrs::arena::schema::encode_kind;
use crate::attrs::arena::storage::RefRun;
use crate::attrs::attr_type::AttrType;
use crate::attrs::attr_type::AttrTypeInner;
use crate::attrs::attr_type::configuration_dep::ConfigurationDepKind;
use crate::attrs::coerced::configure::ConcatLeaves;
use crate::attrs::coerced::configure::Configure;
use crate::attrs::coerced::configure::TraverseConfigured;
use crate::attrs::coerced::kinds;
use crate::attrs::coerced::render::AttrRefDisplay;
use crate::attrs::coerced::render::Fmt;
use crate::attrs::coerced::render::MatchesAny;
use crate::attrs::coerced::render::SerializeKind;
use crate::attrs::coerced::render::ToJson;
use crate::attrs::coerced::render::any_matches_seq;
use crate::attrs::coerced::render::json_seq;
use crate::attrs::coerced::render::ser_err;
use crate::attrs::coerced::render::ser_keyed;
use crate::attrs::coerced::render::ser_seq;
use crate::attrs::coerced::semantics::Semantics;
use crate::attrs::coerced::traverse::Traverse;
use crate::attrs::configuration_context::AttrConfigurationContext;
use crate::attrs::configured_attr::ConfiguredAttr;
use crate::attrs::display::AttrDisplayWithContextExt;
use crate::attrs::fmt_context::AttrFmtContext;
use crate::configuration::resolved::ConfigurationSettingKey;

#[derive(Debug, Eq, PartialEq)]
pub enum CoercedSelectorKeyRef<'a> {
    Target(&'a ConfigurationSettingKey),
    Default,
}

impl CoercedSelectorKeyRef<'_> {
    pub const DEFAULT_KEY_STR: &'static str = "DEFAULT";
}

impl fmt::Display for CoercedSelectorKeyRef<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoercedSelectorKeyRef::Target(target) => write!(f, "{}", target.0),
            CoercedSelectorKeyRef::Default => write!(f, "{}", Self::DEFAULT_KEY_STR),
        }
    }
}

/// Rejects duplicate keys in a `select()`, including keys that differ only in
/// how they spell the same label.
pub(crate) fn check_select_keys_unique<V>(
    entries: &[(ConfigurationSettingKey, V)],
) -> buck2_error::Result<()> {
    fn duplicate_key(key: &ConfigurationSettingKey) -> buck2_error::Error {
        buck2_error!(
            buck2_error::ErrorTag::Input,
            "duplicate key `{key}` in `select()`"
        )
    }

    // This is possible when select keys are specified like:
    // ```
    // select({
    //   "cell//foo:bar": 2,
    //   "//foo:bar": 1,
    //   ":bar": 3,
    // })
    // ```
    // Keys are unique strings, but resolved to the same target.

    // Quadratic is cheaper than hashing for small `N`.
    // For 32 entries we do 496 comparisons, which is cheaper than 32 hashing operations.
    if entries.len() <= 32 {
        for i in 0..entries.len() {
            for j in i + 1..entries.len() {
                if entries[i].0 == entries[j].0 {
                    return Err(duplicate_key(&entries[i].0));
                }
            }
        }
    } else {
        let mut visited_keys: BuckMutSet<&ConfigurationSettingKey> =
            BuckMutSet::with_capacity_and_hasher(entries.len(), BuckHasherBuilder);
        for (k, _) in entries {
            if !visited_keys.insert(k) {
                return Err(duplicate_key(k));
            }
        }
    }

    Ok(())
}

pub(crate) fn select_fail_to_json(message: &str) -> Result<serde_json::Value, buck2_error::Error> {
    Ok(serde_json::Value::Object(serde_json::Map::from_iter([
        (
            "__type".to_owned(),
            serde_json::Value::String("select_fail".to_owned()),
        ),
        (
            "message".to_owned(),
            serde_json::Value::String(message.to_owned()),
        ),
    ])))
}

pub(crate) fn select_incompatible_to_json(
    message: &str,
) -> Result<serde_json::Value, buck2_error::Error> {
    Ok(serde_json::Value::Object(serde_json::Map::from_iter([
        (
            "__type".to_owned(),
            serde_json::Value::String("select_incompatible".to_owned()),
        ),
        (
            "message".to_owned(),
            serde_json::Value::String(message.to_owned()),
        ),
    ])))
}

impl Kind for kinds::SelectFail {
    type Fields = Text;
}

impl CoercedAttrArenaBuilder {
    pub fn encode_select_fail(&mut self, message: &str) -> buck2_error::Result<AttrToken> {
        encode_kind::<kinds::SelectFail>(self, message)
    }
}

impl Semantics for kinds::SelectFail {
    fn traverse<'a>(_: View<'a, Self>, _: Traverse<'_, 'a>) -> buck2_error::Result<()> {
        Ok(())
    }

    fn configure(m: View<'_, Self>, _: Configure<'_, '_>) -> ResultMaybeCompatible<ConfiguredAttr> {
        select_fail(m)
    }

    fn traverse_configured(
        m: View<'_, Self>,
        _: TraverseConfigured<'_, '_>,
    ) -> ResultMaybeCompatible<()> {
        select_fail(m)
    }

    fn concat_leaves<'a>(m: View<'a, Self>, _: ConcatLeaves<'_, 'a>) -> ResultMaybeCompatible<()> {
        select_fail(m)
    }

    fn to_json(m: View<'_, Self>, _: ToJson<'_>) -> buck2_error::Result<serde_json::Value> {
        select_fail_to_json(m)
    }

    fn fmt(m: View<'_, Self>, Fmt { f, .. }: Fmt<'_, '_>) -> std::fmt::Result {
        write!(f, "select_fail(\"{m}\")")
    }

    fn any_matches(_: View<'_, Self>, _: MatchesAny<'_>) -> buck2_error::Result<bool> {
        Ok(false)
    }

    fn serialize<S: Serializer>(
        m: View<'_, Self>,
        SerializeKind { ser, .. }: SerializeKind<'_, S>,
    ) -> Result<S::Ok, S::Error> {
        select_fail_to_json(m).map_err(ser_err::<S>)?.serialize(ser)
    }
}

impl Kind for kinds::SelectIncompatible {
    type Fields = Text;
}

impl CoercedAttrArenaBuilder {
    pub fn encode_select_incompatible(&mut self, message: &str) -> buck2_error::Result<AttrToken> {
        encode_kind::<kinds::SelectIncompatible>(self, message)
    }
}

impl Semantics for kinds::SelectIncompatible {
    fn traverse<'a>(_: View<'a, Self>, _: Traverse<'_, 'a>) -> buck2_error::Result<()> {
        Ok(())
    }

    fn configure(
        m: View<'_, Self>,
        Configure { ctx, .. }: Configure<'_, '_>,
    ) -> ResultMaybeCompatible<ConfiguredAttr> {
        select_incompatible(m, ctx)
    }

    fn traverse_configured(
        m: View<'_, Self>,
        TraverseConfigured { ctx, .. }: TraverseConfigured<'_, '_>,
    ) -> ResultMaybeCompatible<()> {
        select_incompatible(m, ctx)
    }

    fn concat_leaves<'a>(
        m: View<'a, Self>,
        ConcatLeaves { ctx, .. }: ConcatLeaves<'_, 'a>,
    ) -> ResultMaybeCompatible<()> {
        select_incompatible(m, ctx)
    }

    fn to_json(m: View<'_, Self>, _: ToJson<'_>) -> buck2_error::Result<serde_json::Value> {
        select_incompatible_to_json(m)
    }

    fn fmt(m: View<'_, Self>, Fmt { f, .. }: Fmt<'_, '_>) -> std::fmt::Result {
        write!(f, "select_incompatible(\"{m}\")")
    }

    fn any_matches(_: View<'_, Self>, _: MatchesAny<'_>) -> buck2_error::Result<bool> {
        Ok(false)
    }

    fn serialize<S: Serializer>(
        m: View<'_, Self>,
        SerializeKind { ser, .. }: SerializeKind<'_, S>,
    ) -> Result<S::Ok, S::Error> {
        select_incompatible_to_json(m)
            .map_err(ser_err::<S>)?
            .serialize(ser)
    }
}

fn select_fail<T>(message: &str) -> ResultMaybeCompatible<T> {
    buck2_error!(
        buck2_error::ErrorTag::Input,
        "select resolved to select_fail(): {message}"
    )
    .into()
}

fn select_incompatible<T>(
    message: &str,
    ctx: &dyn AttrConfigurationContext,
) -> ResultMaybeCompatible<T> {
    ResultMaybeCompatible::Incompatible(ctx.incompatible_platform_reason(
        IncompatiblePlatformReasonCause::SelectIncompatible(ArcStr::from(message)),
    ))
}

impl Kind for kinds::Selector {
    type Fields = (SelectorEntries, OptChild);
}

impl CoercedAttrArenaBuilder {
    pub fn encode_selector(
        &mut self,
        entries: &[(ConfigurationSettingKey, AttrToken)],
        default: Option<AttrToken>,
    ) -> buck2_error::Result<AttrToken> {
        encode_kind::<kinds::Selector>(self, (entries, default))
    }
}

impl Semantics for kinds::Selector {
    fn traverse<'a>(
        (entries, default): View<'a, Self>,
        Traverse { t, pkg, tr, .. }: Traverse<'_, 'a>,
    ) -> buck2_error::Result<()> {
        for (key, val) in entries.iter() {
            tr.configuration_dep(key, ConfigurationDepKind::SelectKey)?;
            val.traverse(t, pkg, tr)?;
        }
        if let Some(d) = default {
            d.traverse(t, pkg, tr)?;
        }
        Ok(())
    }

    fn configure(
        (entries, default): View<'_, Self>,
        Configure { t, ctx, name, .. }: Configure<'_, '_>,
    ) -> ResultMaybeCompatible<ConfiguredAttr> {
        match CoercedAttrRef::select_view(ctx, entries, default)? {
            Some(selected) => selected.configure(t, ctx, name),
            None => CoercedAttrRef::select_no_match(ctx, entries, name),
        }
    }

    fn traverse_configured(
        (entries, default): View<'_, Self>,
        TraverseConfigured {
            t,
            pkg,
            ctx,
            tr,
            name,
            ..
        }: TraverseConfigured<'_, '_>,
    ) -> ResultMaybeCompatible<()> {
        match CoercedAttrRef::select_view(ctx, entries, default) {
            Ok(Some(selected)) => selected.traverse_configured(t, pkg, ctx, tr, name),
            Ok(None) => CoercedAttrRef::select_no_match(ctx, entries, name),
            Err(e) => e.into(),
        }
    }

    fn concat_leaves<'a>(
        (entries, default): View<'a, Self>,
        ConcatLeaves { ctx, out, .. }: ConcatLeaves<'_, 'a>,
    ) -> ResultMaybeCompatible<()> {
        match CoercedAttrRef::select_view(ctx, entries, default) {
            Ok(Some(selected)) => selected.concat_leaves(ctx, out),
            Ok(None) => CoercedAttrRef::select_no_match(ctx, entries, None),
            Err(e) => e.into(),
        }
    }

    fn to_json(
        (entries, default): View<'_, Self>,
        ToJson { ctx }: ToJson<'_>,
    ) -> buck2_error::Result<serde_json::Value> {
        let mut map = serde_json::Map::new();
        for (key, value) in entries.iter() {
            map.insert(key.to_string(), value.to_json(ctx)?);
        }
        if let Some(default) = default {
            map.insert("DEFAULT".to_owned(), default.to_json(ctx)?);
        }
        Ok(serde_json::Value::Object(serde_json::Map::from_iter([
            (
                "__type".to_owned(),
                serde_json::Value::String("selector".to_owned()),
            ),
            ("entries".to_owned(), serde_json::Value::Object(map)),
        ])))
    }

    fn fmt((entries, default): View<'_, Self>, Fmt { ctx, f }: Fmt<'_, '_>) -> std::fmt::Result {
        write!(f, "select(")?;
        fmt_keyed_container(
            f,
            "{",
            "}",
            ": ",
            entries
                .iter()
                .map(|(k, val)| (format!("\"{k}\""), AttrRefDisplay { ctx, r: val }))
                .chain(default.map(|d| ("\"DEFAULT\"".to_owned(), AttrRefDisplay { ctx, r: d }))),
        )?;
        write!(f, ")")
    }

    fn any_matches(
        (entries, default): View<'_, Self>,
        MatchesAny { filter }: MatchesAny<'_>,
    ) -> buck2_error::Result<bool> {
        for (_, val) in entries.iter() {
            if val.any_matches(filter)? {
                return Ok(true);
            }
        }
        if let Some(default) = default {
            if default.any_matches(filter)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn serialize<S: Serializer>(
        (entries, default): View<'_, Self>,
        SerializeKind { ctx, ser }: SerializeKind<'_, S>,
    ) -> Result<S::Ok, S::Error> {
        struct Entries<'a> {
            entries: AttrRefSelectorEntries<'a>,
            default: Option<CoercedAttrRef<'a>>,
            ctx: &'a AttrFmtContext,
        }
        impl Serialize for Entries<'_> {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                ser_keyed(
                    self.entries
                        .iter()
                        .map(|(k, v)| (Cow::Owned(k.to_string()), v))
                        .chain(self.default.map(|d| (Cow::Borrowed("DEFAULT"), d))),
                    self.ctx,
                    s,
                )
            }
        }
        let mut map = ser.serialize_map(Some(2))?;
        map.serialize_entry("__type", "selector")?;
        map.serialize_entry(
            "entries",
            &Entries {
                entries,
                default,
                ctx,
            },
        )?;
        map.end()
    }
}

impl Kind for kinds::Concat {
    type Fields = Children;
}

impl CoercedAttrArenaBuilder {
    pub fn encode_concat(&mut self, items: &[AttrToken]) -> buck2_error::Result<AttrToken> {
        encode_kind::<kinds::Concat>(self, items)
    }
}

impl Semantics for kinds::Concat {
    fn traverse<'a>(
        items: View<'a, Self>,
        Traverse { t, pkg, tr, .. }: Traverse<'_, 'a>,
    ) -> buck2_error::Result<()> {
        for item in items.iter() {
            item.traverse(t, pkg, tr)?;
        }
        Ok(())
    }

    fn configure(
        items: View<'_, Self>,
        Configure { t, ctx, name, .. }: Configure<'_, '_>,
    ) -> ResultMaybeCompatible<ConfiguredAttr> {
        let singleton = items.len() == 1;
        let mut configured = Vec::with_capacity(items.len());
        for item in items.iter() {
            configured.push(item.configure(t, ctx, name)?);
        }
        let mut iter = configured.into_iter();
        let first = match iter.next() {
            Some(first) => first,
            None => return internal_error!("concat with no items").into(),
        };
        ResultMaybeCompatible::Compatible(if singleton {
            first
        } else {
            first.concat(t, &mut iter.map(Ok))?
        })
    }

    fn traverse_configured(
        items: View<'_, Self>,
        TraverseConfigured {
            t,
            pkg,
            ctx,
            tr,
            name,
            ..
        }: TraverseConfigured<'_, '_>,
    ) -> ResultMaybeCompatible<()> {
        let mut leaves: SmallVec<[CoercedAttrRef<'_>; 4]> = SmallVec::new();
        for item in items.iter() {
            item.concat_leaves(ctx, &mut leaves)?;
        }
        if leaves.len() > 1
            && let Err(e) = validate_concat(t, &leaves)
        {
            return e.into();
        }
        for leaf in leaves {
            leaf.traverse_configured(t, pkg, ctx, tr, name)?;
        }
        ResultMaybeCompatible::Compatible(())
    }

    fn concat_leaves<'a>(
        items: View<'a, Self>,
        ConcatLeaves { ctx, out, .. }: ConcatLeaves<'_, 'a>,
    ) -> ResultMaybeCompatible<()> {
        for item in items.iter() {
            item.concat_leaves(ctx, out)?;
        }
        ResultMaybeCompatible::Compatible(())
    }

    fn to_json(
        items: View<'_, Self>,
        ToJson { ctx }: ToJson<'_>,
    ) -> buck2_error::Result<serde_json::Value> {
        Ok(serde_json::Value::Object(serde_json::Map::from_iter([
            (
                "__type".to_owned(),
                serde_json::Value::String("concat".to_owned()),
            ),
            ("items".to_owned(), json_seq(items, ctx)?),
        ])))
    }

    fn fmt(items: View<'_, Self>, Fmt { ctx, f }: Fmt<'_, '_>) -> std::fmt::Result {
        items
            .iter()
            .map(|a| AttrRefDisplay { ctx, r: a })
            .format("+")
            .fmt(f)
    }

    fn any_matches(
        items: View<'_, Self>,
        MatchesAny { filter }: MatchesAny<'_>,
    ) -> buck2_error::Result<bool> {
        any_matches_seq(items, filter)
    }

    fn serialize<S: Serializer>(
        items: View<'_, Self>,
        SerializeKind { ctx, ser }: SerializeKind<'_, S>,
    ) -> Result<S::Ok, S::Error> {
        struct Items<'a> {
            items: Refs<'a, Values>,
            ctx: &'a AttrFmtContext,
        }
        impl Serialize for Items<'_> {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                ser_seq(self.items, self.ctx, s)
            }
        }
        let mut map = ser.serialize_map(Some(2))?;
        map.serialize_entry("__type", "concat")?;
        map.serialize_entry("items", &Items { items, ctx })?;
        map.end()
    }
}

/// The checks `ConfiguredAttr::concat` makes on its items, applied to the
/// resolved leaves: one concat class throughout, equal oneof variants, no
/// dict key on both sides.
fn validate_concat<'a>(t: &AttrType, leaves: &[CoercedAttrRef<'a>]) -> buck2_error::Result<()> {
    let Some((first, rest)) = leaves.split_first() else {
        return Ok(());
    };
    let t = t.non_optional();
    if let AttrTypeInner::OneOf(oneof) = &t.0.inner {
        let mut inner: SmallVec<[CoercedAttrRef<'a>; 4]> = SmallVec::with_capacity(leaves.len());
        let mut variant = None;
        for leaf in leaves {
            let Some((i, v)) = leaf.downcast::<kinds::OneOf>() else {
                return Err(internal_error!(
                    "expecting oneof variant, got: {}",
                    leaf.as_display_no_ctx()
                ));
            };
            match variant {
                None => variant = Some(i),
                Some(first_i) if first_i != i => {
                    return Err(buck2_error!(
                        buck2_error::ErrorTag::Input,
                        "Cannot concatenate values coerced/configured \
                        to different oneof variants: `{}` and `{}`",
                        oneof.get(first_i)?,
                        oneof.get(i)?
                    ));
                }
                Some(_) => {}
            }
            inner.push(v);
        }
        return match variant {
            Some(i) => validate_concat(oneof.get(i)?, &inner),
            None => Ok(()),
        };
    }
    if first.downcast::<kinds::OneOf>().is_some() {
        return Err(internal_error!(
            "Inconsistent attr value (`{}`) and attr type (`{}`)",
            first.as_display_no_ctx(),
            t
        ));
    }
    let Some(class) = first.concat_class() else {
        return Err(buck2_error!(
            buck2_error::ErrorTag::Input,
            "addition not supported for this attribute type `{}`",
            first.as_display_no_ctx()
        ));
    };
    for leaf in rest {
        if leaf.concat_class() != Some(class) {
            return Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "addition not supported for these attribute type `{}` and value `{}`",
                class,
                leaf.as_display_no_ctx()
            ));
        }
    }
    if let Some(first_pairs) = first.downcast::<kinds::Dict>() {
        let mut seen: Vec<CoercedAttrRef<'a>> = first_pairs.iter().map(|(k, _)| k).collect();
        for leaf in rest {
            let pairs = leaf
                .downcast::<kinds::Dict>()
                .expect("should be a dict: same concat class as the first leaf");
            for (k, _) in pairs.iter() {
                if seen
                    .iter()
                    .any(|s| s.word() == k.word() || s.content_eq(&k))
                {
                    return Err(buck2_error!(
                        buck2_error::ErrorTag::Input,
                        "got same key in both sides of dictionary concat (key `{}`)",
                        k.as_display_no_ctx()
                    ));
                }
                seen.push(k);
            }
        }
    }
    Ok(())
}

/// View of a selector value's (configuration setting key, value) entries.
#[derive(Copy, Clone)]
pub struct AttrRefSelectorEntries<'a> {
    /// Providers-label table indices, from the shared `SelectorKeys` record.
    keys: RefRun<'a>,
    /// Value refs, one per key, stored in the selector record.
    values: RefRun<'a>,
}
impl<'a> AttrRefSelectorEntries<'a> {
    pub fn len(&self) -> usize {
        self.values.len() as usize
    }

    pub fn iter(
        &self,
    ) -> impl ExactSizeIterator<Item = (&'a ProvidersLabel, CoercedAttrRef<'a>)> + 'a {
        let arena = self.values.arena;
        self.keys
            .iter_raw()
            .zip(self.values.iter())
            .map(move |(k, v)| (&arena.providers_labels[k as usize], arena.view(v)))
    }
}
/// A selector's entries: a count head field, a ref to the shared
/// `SelectorKeys` record, then that many value refs.
pub struct SelectorEntries;

impl Field for SelectorEntries {
    const DEPS: bool = true;
    const INLINE: bool = false;
    const HAS_TEXT: bool = false;
    type View<'a> = AttrRefSelectorEntries<'a>;
    type Spec<'s> = &'s [(ConfigurationSettingKey, AttrToken)];

    fn shape(peek: &dyn Fn(u32) -> Option<u32>, at: u32) -> Option<Extent> {
        Some(Extent {
            head: 1,
            refs: Some(peek(at)?.checked_add(1)?),
            text: None,
        })
    }

    fn decode<'a>(c: &mut Cursor<'a>) -> AttrRefSelectorEntries<'a> {
        let n = c.raw();
        let keys = c
            .child()
            .expect("should be present: a selector always has a keys record");
        AttrRefSelectorEntries {
            keys: <kinds::SelectorKeys as Kind>::decode(c.arena, keys).keys,
            values: c.refs(n),
        }
    }

    fn encode<'s>(
        entries: &'s [(ConfigurationSettingKey, AttrToken)],
        b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'s>,
    ) -> buck2_error::Result<()> {
        check_select_keys_unique(entries)?;
        let AttrToken(keys, keys_hash) = encode_kind::<kinds::SelectorKeys>(b, entries)?;
        w.head(entries.len() as u32);
        w.r(keys);
        w.hash(&keys_hash);
        for (_, v) in entries {
            w.r(v.0);
            w.hash(&v.1);
        }
        Ok(())
    }

    fn reemit<'a>(
        view: AttrRefSelectorEntries<'a>,
        b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'a>,
    ) -> buck2_error::Result<()> {
        let AttrToken(keys, keys_hash) =
            <kinds::SelectorKeys as Kind>::copy_into(KeysParts { keys: view.keys }, b)?;
        w.head(view.len() as u32);
        w.r(keys);
        w.hash(&keys_hash);
        for (_, v) in view.iter() {
            let t = b.encode_view(v)?;
            w.r(t.0);
            w.hash(&t.1);
        }
        Ok(())
    }

    fn identity(view: AttrRefSelectorEntries<'_>, h: &mut IdHasher<'_>) {
        h.h.write_usize(view.len());
        for (k, v) in view.iter() {
            k.strong_hash(&mut h.h);
            h.child(v);
        }
    }

    fn same(a: AttrRefSelectorEntries<'_>, b: AttrRefSelectorEntries<'_>) -> bool {
        a.len() == b.len()
            && a.iter()
                .zip(b.iter())
                .all(|((ka, va), (kb, vb))| ka == kb && va == vb)
    }
}
/// The label set of a `SelectorKeys` record: `[n, idx...]`.
#[derive(Copy, Clone)]
pub struct KeysParts<'a> {
    keys: RefRun<'a>,
}
impl<'a> KeysParts<'a> {
    fn iter(&self) -> impl ExactSizeIterator<Item = &'a ProvidersLabel> + 'a {
        let arena = self.keys.arena;
        self.keys
            .iter_raw()
            .map(move |k| &arena.providers_labels[k as usize])
    }
}
/// A selector's key set: a count, then that many label indices. The spec
/// is the selector's entries, whose keys this stores.
pub struct Keys;

impl Field for Keys {
    const DEPS: bool = true;
    const INLINE: bool = false;
    const HAS_TEXT: bool = false;
    type View<'a> = KeysParts<'a>;
    type Spec<'s> = &'s [(ConfigurationSettingKey, AttrToken)];

    fn shape(peek: &dyn Fn(u32) -> Option<u32>, at: u32) -> Option<Extent> {
        Some(Extent::head(1u32.checked_add(peek(at)?)?))
    }

    fn decode<'a>(c: &mut Cursor<'a>) -> KeysParts<'a> {
        let n = c.raw();
        KeysParts { keys: c.raws(n) }
    }

    fn encode<'s>(
        entries: &'s [(ConfigurationSettingKey, AttrToken)],
        b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'s>,
    ) -> buck2_error::Result<()> {
        write_keys(b, w, entries.iter().map(|(k, _)| &k.0))
    }

    fn reemit<'a>(
        parts: KeysParts<'a>,
        b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'a>,
    ) -> buck2_error::Result<()> {
        write_keys(b, w, parts.iter())
    }

    fn identity(parts: KeysParts<'_>, h: &mut IdHasher<'_>) {
        h.h.write_u32(parts.keys.len());
        for k in parts.iter() {
            k.strong_hash(&mut h.h);
        }
    }

    fn same(a: KeysParts<'_>, b: KeysParts<'_>) -> bool {
        a.keys.len() == b.keys.len() && a.iter().zip(b.iter()).all(|(x, y)| x == y)
    }
}

fn write_keys<'k>(
    b: &mut CoercedAttrArenaBuilder,
    w: &mut RecordWriter<'_>,
    keys: impl ExactSizeIterator<Item = &'k ProvidersLabel>,
) -> buck2_error::Result<()> {
    w.head(keys.len() as u32);
    for key in keys {
        let (idx, hash) = b.providers_label_idx_and_hash(key);
        w.head(idx);
        w.hash(&hash);
    }
    Ok(())
}

impl Kind for kinds::SelectorKeys {
    type Fields = Keys;
}

impl<'a> CoercedAttrRef<'a> {
    /// Whether evaluating this value can ever yield `None`, directly or
    /// through a `select()` branch.
    pub fn may_return_none(&self) -> bool {
        self.downcast::<kinds::None>().is_some()
            || self
                .downcast::<kinds::Selector>()
                .is_some_and(|(entries, default)| {
                    entries.iter().any(|(_, v)| v.may_return_none())
                        || default.is_some_and(|d| d.may_return_none())
                })
    }
}

#[cfg(test)]
mod tests {
    use dupe::Dupe;

    use crate::attrs::coerced::select::check_select_keys_unique;
    use crate::configuration::resolved::ConfigurationSettingKey;

    #[test]
    fn test_check_select_keys_unique_small() {
        let a = (ConfigurationSettingKey::testing_parse("foo//:a"), ());
        let b = (ConfigurationSettingKey::testing_parse("foo//:b"), ());
        let c = (ConfigurationSettingKey::testing_parse("foo//:c"), ());
        assert!(check_select_keys_unique(&[a.dupe(), b.dupe(), a.dupe()]).is_err());
        assert!(check_select_keys_unique(&[a.dupe(), b.dupe(), b.dupe()]).is_err());
        assert!(check_select_keys_unique(&[a.dupe(), a.dupe(), b.dupe()]).is_err());
        assert!(check_select_keys_unique(&[a, b, c]).is_ok());
    }

    #[test]
    fn test_check_select_keys_unique_large() {
        let mut long = (0..100)
            .map(|i| {
                (
                    ConfigurationSettingKey::testing_parse(&format!("foo//:{i}")),
                    (),
                )
            })
            .collect::<Vec<_>>();
        assert!(check_select_keys_unique(&long).is_ok());
        long[10].0 = long[0].0.dupe();
        assert!(check_select_keys_unique(&long).is_err());
    }
}
