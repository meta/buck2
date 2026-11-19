/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The arena's behavior over every kind of value: round-trips through the
//! builder and back, dedup and record sharing down to the byte, identity
//! hashes, interning into the side tables and page-in validation. Kinds
//! whose records are declared beside their attribute types are covered
//! here too, because what these tests pin is the stored form. What a kind
//! means (traversal, configuration, rendering) is tested through the attr
//! suites in `buck2_build_api_tests` and `buck2_node_tests`.

use std::collections::BTreeMap;
use std::rc::Rc;

use buck2_core::provider::label::ProvidersLabel;
use buck2_core::provider::label::testing::ProvidersLabelTestExt;
use buck2_core::target::label::label::TargetLabel;
use buck2_hash::BuckMutMap;
use buck2_util::arc_str::ArcStr;
use buck2_util::arc_str::PackedStr;
use dupe::Dupe;
use pagable::PagableDeserialize;
use pagable::PagableSerialize;

use super::*;
use crate::attrs::arena::storage::INT_INLINE_MAX;
use crate::attrs::arena::storage::INT_INLINE_MIN;
use crate::attrs::arena::storage::OFFSET_MASK;
use crate::attrs::arena::storage::RecordBytes;
use crate::attrs::arena::storage::TAG_WIDTH_SHIFT;
use crate::attrs::attr_type::arg::ArgMacroView;
use crate::attrs::attr_type::arg::ArgPartView;
use crate::attrs::attr_type::arg::MacroDepKind;
use crate::attrs::attr_type::arg::QueryExpansion;
use crate::attrs::attr_type::arg::StringWithMacros;
use crate::attrs::attr_type::arg::StringWithMacrosPart;
use crate::attrs::attr_type::arg::UnconfiguredMacro;
use crate::attrs::attr_type::query::QueryAttrBase;
use crate::attrs::attr_type::query::QueryMacroBase;
use crate::attrs::attr_type::query::ResolvedQueryLiterals;
use crate::attrs::coerced::CoercedAttrRefKind;
use crate::attrs::coerced_path::CoercedDirectory;
use crate::attrs::coerced_path::CoercedPath;
use crate::attrs::display::AttrDisplayWithContextExt;
use crate::attrs::fmt_context::AttrFmtContext;
use crate::configuration::resolved::ConfigurationSettingKey;
use crate::metadata::map::MetadataMap;
use crate::modifiers::CfgModifiers;
use crate::visibility::VisibilitySpecification;

fn label(pkg: &str, name: &str) -> ProvidersLabel {
    ProvidersLabel::testing_new("root", pkg, name, None)
}

/// A test value: the recipe that encodes it into a builder, so one
/// value can be encoded into any number of arenas.
type V = Rc<dyn Fn(&mut CoercedAttrArenaBuilder) -> buck2_error::Result<AttrToken>>;

fn v(f: impl Fn(&mut CoercedAttrArenaBuilder) -> buck2_error::Result<AttrToken> + 'static) -> V {
    Rc::new(f)
}

fn put(b: &mut CoercedAttrArenaBuilder, xs: &[V]) -> buck2_error::Result<Vec<AttrToken>> {
    xs.iter().map(|x| x(b)).collect()
}

fn none() -> V {
    v(|b| b.encode_none())
}
fn boolean(x: bool) -> V {
    v(move |b| b.encode_bool(x))
}
fn int(x: i64) -> V {
    v(move |b| b.encode_int(x))
}
fn s(x: &'static str) -> V {
    v(move |b| b.encode_string(x))
}
fn enum_variant(x: &'static str) -> V {
    v(move |b| b.encode_enum_variant(x))
}
fn select_fail(x: &'static str) -> V {
    v(move |b| b.encode_select_fail(x))
}
fn select_incompatible(x: &'static str) -> V {
    v(move |b| b.encode_select_incompatible(x))
}
fn arg_simple(x: &'static str) -> V {
    arg_value(StringWithMacros::StringPart(ArcStr::from(x)))
}
fn arg(parts: Vec<StringWithMacrosPart<ProvidersLabel>>) -> V {
    arg_value(StringWithMacros::ManyParts(parts.into_boxed_slice()))
}
fn arg_value(x: StringWithMacros<ProvidersLabel>) -> V {
    v(move |b| b.encode_arg(&x))
}
fn query(x: QueryAttrBase<ProvidersLabel>) -> V {
    v(move |b| b.encode_query(&x))
}
fn list(xs: Vec<V>) -> V {
    v(move |b| {
        let t = put(b, &xs)?;
        b.encode_list(&t)
    })
}
fn tuple(xs: Vec<V>) -> V {
    v(move |b| {
        let t = put(b, &xs)?;
        b.encode_tuple(&t)
    })
}
fn concat(xs: Vec<V>) -> V {
    v(move |b| {
        let t = put(b, &xs)?;
        b.encode_concat(&t)
    })
}
fn dict(pairs: Vec<(V, V)>) -> V {
    v(move |b| {
        let mut t = Vec::with_capacity(pairs.len());
        for (k, x) in &pairs {
            t.push((k(b)?, x(b)?));
        }
        b.encode_dict(&t)
    })
}
fn one_of(x: V, idx: u32) -> V {
    v(move |b| {
        let t = x(b)?;
        b.encode_one_of(idx, t)
    })
}
/// A selector keyed by `root//cfg:<key>` labels.
fn sel(entries: Vec<(&'static str, V)>, default: Option<V>) -> V {
    v(move |b| {
        let mut t = Vec::with_capacity(entries.len());
        for (k, x) in &entries {
            t.push((ConfigurationSettingKey(label("cfg", k)), x(b)?));
        }
        let d = default.as_ref().map(|d| d(b)).transpose()?;
        b.encode_selector(&t, d)
    })
}
fn dep(l: ProvidersLabel) -> V {
    v(move |b| b.encode_dep(&l))
}
fn label_value(l: ProvidersLabel) -> V {
    v(move |b| b.encode_label(&l))
}
fn source_label(l: ProvidersLabel) -> V {
    v(move |b| b.encode_source_label(&l))
}
fn configuration_dep(l: ProvidersLabel) -> V {
    v(move |b| b.encode_configuration_dep(&l))
}
fn split_transition_dep(l: ProvidersLabel) -> V {
    v(move |b| b.encode_split_transition_dep(&l))
}
fn explicit_configured_dep(l: ProvidersLabel, platform: TargetLabel) -> V {
    v(move |b| b.encode_explicit_configured_dep(&l, &platform))
}
fn transition_dep(l: ProvidersLabel, transition: Option<ProvidersLabel>) -> V {
    v(move |b| b.encode_transition_dep(&l, transition.as_ref()))
}
fn source_file(p: CoercedPath) -> V {
    v(move |b| b.encode_source_file(&p))
}
fn visibility(x: VisibilitySpecification) -> V {
    v(move |b| b.encode_visibility(&x))
}
fn metadata(x: MetadataMap) -> V {
    v(move |b| b.encode_metadata(&x))
}
fn target_modifiers(x: CfgModifiers) -> V {
    v(move |b| b.encode_target_modifiers(&x))
}

/// Encode each value and check what every value must satisfy:
/// re-encoding it into the same arena dedups to the same ref, and a
/// copy into a fresh arena is content-equal with the same identity hash
/// and rendering.
fn roundtrip(values: &[V]) -> CoercedAttrArena {
    let (arena, refs) = encode_all(values);
    assert_copies(&arena, &refs);
    arena
}

/// Encode `values` into one arena, in order.
fn encode_all(values: &[V]) -> (CoercedAttrArena, Vec<AttrWord>) {
    let mut b = CoercedAttrArena::builder();
    let refs: Vec<AttrWord> = values.iter().map(|x| x(&mut b).unwrap().0).collect();
    for (x, &r) in values.iter().zip(&refs) {
        assert_eq!(
            x(&mut b).unwrap().0,
            r,
            "re-encoding should dedup to the same ref: {}",
            b.view(r).as_display_no_ctx()
        );
    }
    (b.finish(), refs)
}

/// Encode one value into a fresh arena.
fn encode_one(value: &V) -> (CoercedAttrArena, AttrWord) {
    let (arena, refs) = encode_all(std::slice::from_ref(value));
    (arena, refs[0])
}

/// The query expressions a value holds, as `attrs.query()` values or
/// query macros: each as its reprint and literal bindings.
fn queries_of(view: CoercedAttrRef<'_>) -> Vec<(String, ResolvedQueryLiterals<ProvidersLabel>)> {
    match view.kind() {
        CoercedAttrRefKind::Query(q) => vec![q.query()],
        CoercedAttrRefKind::Arg(parts) => parts
            .iter()
            .filter_map(|p| match p {
                ArgPartView::Macro {
                    mac: ArgMacroView::Query(m),
                    ..
                } => Some(m.query()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Round-trip for values whose decode is a canonical reprint (query
/// expressions): bytes may differ from the source, but every literal
/// label must survive with a span that slices the reprint to that
/// label's rendering, and the reprint must be a fixed point.
fn roundtrip_canonical(values: &[V]) -> CoercedAttrArena {
    let (arena, refs) = encode_all(values);
    assert_copies(&arena, &refs);
    for &r in &refs {
        for (reprint, lits) in queries_of(arena.view(r)) {
            for ((off, len), l) in lits.0.iter() {
                assert_eq!(
                    &reprint[*off..*off + *len],
                    l.to_string(),
                    "literal span should slice the reprint to its label"
                );
            }
            let again = QueryAttrBase {
                query: reprint.clone(),
                resolved_literals: lits.clone(),
            };
            let mut b2 = CoercedAttrArena::builder();
            let r2 = b2.encode_query(&again).unwrap().0;
            let arena2 = b2.finish();
            assert_eq!(
                queries_of(arena2.view(r2)),
                vec![(reprint, lits)],
                "canonical form should be a fixed point"
            );
        }
    }
    arena
}

/// `encode_view` into a fresh arena reproduces each value: same content,
/// same identity hash, same rendering.
fn assert_copies(arena: &CoercedAttrArena, refs: &[AttrWord]) {
    for &r in refs {
        let mut b = CoercedAttrArena::builder();
        let AttrToken(c, _) = b.encode_view(arena.view(r)).unwrap();
        let copy = b.finish();
        assert!(
            copy.view(c).content_eq(&arena.view(r)),
            "copy should have the same content: {}",
            arena.view(r).as_display_no_ctx()
        );
        assert_eq!(
            copy.strong_of(c, &mut BuckMutMap::default()),
            arena.strong_of(r, &mut BuckMutMap::default()),
            "copy should have the same identity hash: {}",
            arena.view(r).as_display_no_ctx()
        );
        assert_eq!(
            copy.view(c).as_display_no_ctx().to_string(),
            arena.view(r).as_display_no_ctx().to_string()
        );
    }
}

/// Near-miss values must stay distinct under both the identity hash and
/// content equality: the shared decomposition may not coarsen either.
#[test]
fn test_near_miss_values_are_distinct() {
    let l = label("pkg", "t");
    let values = vec![
        none(),
        boolean(true),
        boolean(false),
        int(1),
        int(2),
        s("a"),
        s(""),
        enum_variant("a"),
        arg_simple("a"),
        select_fail("a"),
        select_incompatible("a"),
        list(vec![s("a")]),
        list(vec![s("a"), s("a")]),
        tuple(vec![s("a")]),
        concat(vec![s("a")]),
        dict(vec![(s("a"), int(1))]),
        dep(l.dupe()),
        label_value(l.dupe()),
        source_label(l.dupe()),
        configuration_dep(l.dupe()),
        split_transition_dep(l.dupe()),
        one_of(s("a"), 0),
        one_of(s("a"), 1),
        sel(vec![("a", s("x"))], None),
        sel(vec![("a", s("x"))], Some(s("d"))),
    ];
    roundtrip(&values);
    let mut b = CoercedAttrArena::builder();
    let refs: Vec<AttrWord> = values.iter().map(|x| x(&mut b).unwrap().0).collect();
    let arena = b.finish();
    let mut memo = BuckMutMap::default();
    for (i, &ri) in refs.iter().enumerate() {
        for &rj in &refs[i + 1..] {
            let (vi, vj) = (arena.view(ri), arena.view(rj));
            assert!(
                !vi.content_eq(&vj),
                "{} and {} must differ",
                vi.as_display_no_ctx(),
                vj.as_display_no_ctx()
            );
            assert_ne!(
                arena.strong_of(ri, &mut memo),
                arena.strong_of(rj, &mut memo),
                "{} and {} must hash apart",
                vi.as_display_no_ctx(),
                vj.as_display_no_ctx()
            );
        }
    }
}

#[test]
fn test_page_in_validates_text_records() {
    use pagable::testing::TestingDeserializer;
    use pagable::testing::TestingSerializer;

    let mut b = CoercedAttrArena::builder();
    let r = b.encode_string("héllo").unwrap().0;
    let arena = b.finish();

    let mut ser = TestingSerializer::new();
    arena.pagable_serialize(&mut ser).unwrap();
    let blob = ser.finish();
    let back = CoercedAttrArena::pagable_deserialize(&mut TestingDeserializer::new(&blob)).unwrap();
    assert_eq!(back.str_at(r), "héllo");

    let text_off = arena.bytes_at(r).as_ptr() as usize - arena.bytes.as_ptr() as usize;
    let mut corrupt = arena.bytes.0.clone();
    corrupt[text_off + 1] = 0xFF;
    let mut ser = TestingSerializer::new();
    RecordBytes(corrupt).pagable_serialize(&mut ser).unwrap();
    let blob = ser.finish();
    let err = RecordBytes::pagable_deserialize(&mut TestingDeserializer::new(&blob))
        .expect_err("corrupt text must be rejected");
    assert!(err.to_string().contains("not UTF-8"), "{err}");
}

#[test]
fn test_page_in_validates_ref_targets() {
    use pagable::testing::TestingDeserializer;
    use pagable::testing::TestingSerializer;

    let mut b = CoercedAttrArena::builder();
    let r = list(vec![s("héllo")])(&mut b).unwrap().0;
    let arena = b.finish();

    // The list record's single ref field follows its tag byte and one
    // head field; point it further back than the buffer start.
    let list_off = (r.0 & OFFSET_MASK) as usize;
    let width = (arena.bytes[list_off] >> TAG_WIDTH_SHIFT) as usize + 1;
    let mut corrupt = arena.bytes.0.clone();
    corrupt[list_off + 1 + width] = 0xFE;
    let mut ser = TestingSerializer::new();
    RecordBytes(corrupt).pagable_serialize(&mut ser).unwrap();
    let blob = ser.finish();
    let err = RecordBytes::pagable_deserialize(&mut TestingDeserializer::new(&blob))
        .expect_err("dangling ref must be rejected");
    assert!(err.to_string().contains("not an earlier record"), "{err}");
}

#[test]
fn test_immediates_roundtrip_and_cost_no_bytes() {
    let arena = roundtrip(&[
        none(),
        boolean(true),
        boolean(false),
        int(0),
        int(-1),
        int(INT_INLINE_MAX),
        int(INT_INLINE_MIN),
        dep(label("foo", "bar")),
    ]);
    assert_eq!(
        arena.byte_len(),
        0,
        "inline leaves should use no arena bytes"
    );
}

#[test]
fn test_root_strong_hash_identity() {
    let value = list(vec![s("x"), int(3), dep(label("foo", "bar"))]);
    let other = list(vec![s("y")]);

    // Same value in two arenas, interleaved with different neighbors:
    // strong hashes must agree (identity is content, not placement).
    let mut b1 = CoercedAttrArena::builder();
    let t = value(&mut b1).unwrap();
    let r1 = b1.root(t);
    let a1 = b1.finish();

    let mut b2 = CoercedAttrArena::builder();
    let t = other(&mut b2).unwrap();
    let o2 = b2.root(t);
    let t = value(&mut b2).unwrap();
    let r2 = b2.root(t);
    let a2 = b2.finish();

    assert_eq!(
        a1.root_strong_hash(r1).unwrap(),
        a2.root_strong_hash(r2).unwrap(),
        "equal content should have equal strong hashes across arenas"
    );
    assert_ne!(
        a2.root_strong_hash(o2).unwrap(),
        a2.root_strong_hash(r2).unwrap(),
        "different content should have different strong hashes"
    );

    // Non-roots retain no strong hash.
    let mut b3 = CoercedAttrArena::builder();
    let interior = value(&mut b3).unwrap().0;
    let a3 = b3.finish();
    assert_eq!(
        a3.root_strong_hash(interior),
        None,
        "non-root values should not retain strong hashes"
    );

    // OwnedCoercedAttr agrees with a package arena holding the same value.
    let sealed = OwnedCoercedAttr::new_with(|b| value(b)).unwrap();
    assert_eq!(
        sealed.strong_hash(),
        a1.root_strong_hash(r1).unwrap(),
        "sealed storage should produce the same identity hash"
    );
}

#[test]
fn test_big_ints() {
    let arena = roundtrip(&[
        int(INT_INLINE_MAX + 1),
        int(INT_INLINE_MIN - 1),
        int(i64::MAX),
        int(i64::MIN),
    ]);
    // Each is a tag plus the two 32-bit halves of the zigzag value, at
    // four bytes each once the low half needs them.
    assert_eq!(arena.byte_len(), 4 * (1 + 2 * 4));
}

#[test]
fn test_strings() {
    roundtrip(&[
        s(""),
        s("a"),
        s("abcd"),
        s("abcde"),
        s("unicode: \u{1F600} end"),
    ]);
}

#[test]
fn test_containers() {
    let list_v = list(vec![s("x"), int(3), none()]);
    let tuple_v = tuple(vec![list_v.clone(), boolean(false)]);
    let dict_v = dict(vec![(s("k1"), list_v.clone()), (s("k2"), tuple_v.clone())]);
    let oneof_v = one_of(dict_v.clone(), 7);
    roundtrip(&[list_v, tuple_v, dict_v.clone(), oneof_v]);
    let (arena, r) = encode_one(&dict_v);
    assert_eq!(
        arena.view(r).as_display_no_ctx().to_string(),
        "{\"k1\": [\"x\", 3, None], \"k2\": ([\"x\", 3, None], False)}"
    );
}

#[test]
fn test_labels_and_selector() {
    let dep_v = dep(label("foo", "bar"));
    let sel_full = sel(
        vec![("a", s("on-a")), ("b", dep_v.clone())],
        Some(s("dflt")),
    );
    let sel_no_default = sel(vec![("a", s("v"))], None);
    let concat_v = concat(vec![sel_full.clone(), s("suffix")]);
    roundtrip(&[dep_v, sel_full, sel_no_default, concat_v.clone()]);
    let (arena, r) = encode_one(&concat_v);
    assert_eq!(
        arena.view(r).as_display_no_ctx().to_string(),
        "select({\"root//cfg:a\": \"on-a\", \"root//cfg:b\": \"root//foo:bar\", \"DEFAULT\": \"dflt\"})+\"suffix\""
    );
}

/// Whole-value leaves intern into their own tables, one entry per
/// distinct value.
#[test]
fn test_whole_value_leaves_intern_into_tables() {
    let vis = visibility(VisibilitySpecification::testing_parse(&["root//a/..."]));
    let meta = metadata(Default::default());
    let arena = roundtrip(&[
        vis.clone(),
        visibility(VisibilitySpecification::DEFAULT),
        vis.clone(),
        meta.clone(),
        meta.clone(),
    ]);
    assert_eq!(arena.visibility.len(), 2);
    assert_eq!(arena.metadata.len(), 1);
}

#[test]
fn test_dep_like_leaves_encode_natively() {
    let configured_dep = explicit_configured_dep(
        label("pkg", "dep"),
        label("platforms", "linux").target().dupe(),
    );
    let fixed_transition = transition_dep(label("pkg", "dep"), None);
    let dynamic_transition = transition_dep(label("pkg", "dep"), Some(label("transitions", "t")));
    roundtrip(&[
        configured_dep,
        fixed_transition.clone(),
        dynamic_transition.clone(),
    ]);

    // Same dep with and without a dynamic transition: distinct records.
    let mut b = CoercedAttrArena::builder();
    let AttrToken(r1, h1) = fixed_transition(&mut b).unwrap();
    let AttrToken(r2, h2) = dynamic_transition(&mut b).unwrap();
    assert_ne!(r1, r2);
    assert_ne!(h1, h2);
}

fn modifiers(json: serde_json::Value) -> CfgModifiers {
    use buck2_core::cells::CellResolver;
    use buck2_core::cells::cell_root_path::CellRootPathBuf;
    use buck2_core::cells::name::CellName;

    use crate::modifiers::ModifierLabelParser;
    let cells = CellResolver::testing_with_names_and_paths(&[
        (
            CellName::testing_new("root"),
            CellRootPathBuf::testing_new(""),
        ),
        (
            CellName::testing_new("cfg"),
            CellRootPathBuf::testing_new("cfg"),
        ),
    ]);
    CfgModifiers::from_target(&json, &ModifierLabelParser::new(&cells))
}

/// `modifiers` values intern into the modifiers table: equal values
/// share one entry, and the view hands out the shared value.
#[test]
fn test_modifiers_intern_into_table() {
    use serde_json::json;
    let json = json!([
        "root//os:linux",
        null,
        "not-a-label",
        42,
        {
            "_type": "ModifiersMatch",
            "DEFAULT": "cfg//compiler:gcc",
            "cfg//os:linux": {"_type": "ModifiersMatch", "cfg//arch:arm": null},
            "cfg//os:windows": "cfg//compiler:msvc[sub]",
        },
    ]);
    let value = target_modifiers(modifiers(json.clone()));
    let non_list = target_modifiers(modifiers(json!({"a": 1})));
    let empty = target_modifiers(CfgModifiers::empty());
    let arena = roundtrip(&[value.clone(), non_list.clone(), empty]);
    assert_eq!(arena.modifiers.len(), 3);

    let mut b = CoercedAttrArena::builder();
    let AttrToken(r, _) = value(&mut b).unwrap();
    let AttrToken(again, _) = target_modifiers(modifiers(json.clone()))(&mut b).unwrap();
    assert_eq!(r, again, "an equal value should intern to the same entry");
    let AttrToken(r2, _) = non_list(&mut b).unwrap();
    let arena = b.finish();
    assert_eq!(arena.modifiers.len(), 2);
    let CoercedAttrRefKind::Modifiers(m) = arena.view(r).kind() else {
        panic!("should decode as modifiers");
    };
    assert_eq!(m.to_json(), json);
    assert_eq!(
        arena.view(r2).to_json(&AttrFmtContext::NO_CONTEXT).unwrap(),
        json!({"a": 1})
    );
}

/// `PACKAGE` modifier groups served as `buck.package_cfg_modifiers`
/// share the package's value.
#[test]
fn test_package_modifier_groups_intern_into_table() {
    use buck2_core::cells::CellResolver;
    use buck2_core::cells::cell_root_path::CellRootPathBuf;
    use buck2_core::cells::name::CellName;
    use serde_json::json;

    use crate::modifiers::ModifierLabelParser;
    let cells = CellResolver::testing_with_names_and_paths(&[(
        CellName::testing_new("root"),
        CellRootPathBuf::testing_new(""),
    )]);
    let json = json!([
        {
            "_type": "TaggedModifiers",
            "location": {"_type": "ModifierPackageLocation", "package_path": "root//foo/PACKAGE"},
            "modifiers": ["root//cfg:linux"],
            "rule_name": null,
        },
        {
            "_type": "TaggedModifiers",
            "location": {"_type": "ModifierTargetLocation"},
            "modifiers": [],
            "rule_name": "python_binary",
        },
    ]);
    let package = CfgModifiers::from_package(&json, &ModifierLabelParser::new(&cells), None);
    let value = target_modifiers(package);
    roundtrip(std::slice::from_ref(&value));
    let mut b = CoercedAttrArena::builder();
    let AttrToken(r, _) = value(&mut b).unwrap();
    let arena = b.finish();
    assert_eq!(
        arena.view(r).to_json(&AttrFmtContext::NO_CONTEXT).unwrap(),
        json
    );
}

#[test]
fn test_select_outcomes_are_text_records() {
    let fail = select_fail("nope");
    let incompatible = select_incompatible("nope");
    roundtrip(&[fail.clone(), incompatible.clone()]);

    // Same message, different outcome: distinct values.
    let mut b = CoercedAttrArena::builder();
    let AttrToken(r1, h1) = fail(&mut b).unwrap();
    let AttrToken(r2, h2) = incompatible(&mut b).unwrap();
    assert_ne!(r1, r2);
    assert_ne!(h1, h2);
    let arena = b.finish();
    assert_eq!(
        arena.view(r1).as_display_no_ctx().to_string(),
        "select_fail(\"nope\")"
    );
    assert_eq!(
        arena.view(r2).as_display_no_ctx().to_string(),
        "select_incompatible(\"nope\")"
    );
}

#[test]
fn test_selector_keys_shared_across_values() {
    let entries = |x: &'static str| vec![("a", s(x)), ("b", none())];
    let sel1 = sel(entries("one"), None);
    let sel2 = sel(entries("two"), None);
    roundtrip(&[sel1.clone(), sel2.clone()]);

    let mut b = CoercedAttrArena::builder();
    let AttrToken(r1, _) = sel1(&mut b).unwrap();
    let bytes_after_first = b.arena.bytes.len();
    let AttrToken(r2, _) = sel2(&mut b).unwrap();
    // Second selector shares the keys record and the None value: it
    // adds only its own record (tag, count, 4 one-byte refs: keys,
    // absent default, 2 values) plus the new "two" text record (tag,
    // length, 3 bytes) -- NOT a second keys record.
    let added = b.arena.bytes.len() - bytes_after_first;
    assert_eq!(
        added,
        (1 + 1 + 4) + (1 + 1 + 3),
        "expected keys record to be shared"
    );
    let keys_of =
        |b: &CoercedAttrArenaBuilder, r: AttrWord| fields::Cursor::new(&b.arena, r).refs(1).get(0);
    assert_eq!(
        keys_of(&b, r1),
        keys_of(&b, r2),
        "selectors should share one keys record"
    );
}

#[test]
fn test_arg_simple_and_many_parts_encode_natively() {
    roundtrip(&[arg_simple("-DFOO=1"), arg(vec![])]);
    let (arena, r) = encode_one(&arg_simple("-DFOO=1"));
    assert!(matches!(
        arena.view(r).kind(),
        CoercedAttrRefKind::ArgSimple("-DFOO=1")
    ));
}

/// An `attrs.query()` value and a query macro over the same expression
/// share one expression record, so `queries()` sees one query.
#[test]
fn test_query_attr_shares_expression_with_query_macro() {
    let text = "deps(:dep)";
    let lits = || ResolvedQueryLiterals(BTreeMap::from([((5, 4), label("pkg", "dep"))]));
    let attr = query(QueryAttrBase {
        query: text.to_owned(),
        resolved_literals: lits(),
    });
    let arg = arg(vec![StringWithMacrosPart::Macro(
        false,
        UnconfiguredMacro::Query(Box::new(QueryMacroBase {
            expansion_type: QueryExpansion::Target,
            query: QueryAttrBase {
                query: text.to_owned(),
                resolved_literals: lits(),
            },
        })),
    )]);
    roundtrip_canonical(&[attr.clone(), arg.clone()]);

    let mut b = CoercedAttrArena::builder();
    let AttrToken(ra, _) = attr(&mut b).unwrap();
    let AttrToken(rg, _) = arg(&mut b).unwrap();
    let arena = b.finish();
    let CoercedAttrRefKind::Query(q) = arena.view(ra).kind() else {
        panic!("should decode as a query");
    };
    let CoercedAttrRefKind::Arg(parts) = arena.view(rg).kind() else {
        panic!("should decode as arg parts");
    };
    let ArgPartView::Macro {
        mac: ArgMacroView::Query(m),
        ..
    } = parts.iter().next().unwrap()
    else {
        panic!("should decode as a query macro");
    };
    assert_eq!(q.r, m.expr().r, "expression record should be shared");
    let (reprint, lits) = q.query();
    assert_eq!(reprint, "deps(root//pkg:dep)");
    assert_eq!(lits.0.len(), 1);
    let ((off, len), l) = lits.0.iter().next().unwrap();
    assert_eq!(&reprint[*off..*off + *len], l.to_string());
}

#[test]
fn test_arg_many_parts_roundtrip_native() {
    let parts = vec![
        StringWithMacrosPart::String(ArcStr::from("--flag=")),
        StringWithMacrosPart::Macro(
            false,
            UnconfiguredMacro::Location {
                label: label("foo", "gen"),
                dep_kind: MacroDepKind::Toolchain,
            },
        ),
        StringWithMacrosPart::Macro(
            true,
            UnconfiguredMacro::Exe {
                label: label("tools", "compiler"),
                exec_dep: true,
            },
        ),
        StringWithMacrosPart::Macro(
            false,
            UnconfiguredMacro::UserUnkeyedPlaceholder(Box::from("CXX")),
        ),
        StringWithMacrosPart::Macro(
            false,
            UnconfiguredMacro::UserKeyedPlaceholder(Box::new((
                Box::from("cxxppflags"),
                label("foo", "hdrs"),
                None,
            ))),
        ),
        StringWithMacrosPart::Macro(
            false,
            UnconfiguredMacro::UserKeyedPlaceholder(Box::new((
                Box::from("cxxflags"),
                label("foo", "deps"),
                Some(Box::from("--x")),
            ))),
        ),
    ];
    let value = arg(parts);
    roundtrip(std::slice::from_ref(&value));
    let (arena, r) = encode_one(&value);
    let CoercedAttrRefKind::Arg(decoded) = arena.view(r).kind() else {
        panic!("should decode as arg parts");
    };
    let decoded: Vec<ArgPartView<'_>> = decoded.iter().collect();
    assert_eq!(decoded.len(), 6);
    assert!(decoded[0] == ArgPartView::String("--flag="));
    assert!(
        decoded[1]
            == ArgPartView::Macro {
                write_to_file: false,
                mac: ArgMacroView::Location {
                    label: &label("foo", "gen"),
                    dep_kind: MacroDepKind::Toolchain,
                },
            }
    );
    assert!(
        decoded[2]
            == ArgPartView::Macro {
                write_to_file: true,
                mac: ArgMacroView::Exe {
                    label: &label("tools", "compiler"),
                    exec_dep: true,
                },
            }
    );
    assert!(
        decoded[5]
            == ArgPartView::Macro {
                write_to_file: false,
                mac: ArgMacroView::UserKeyedPlaceholder {
                    var: "cxxflags",
                    label: &label("foo", "deps"),
                    arg: Some("--x"),
                },
            }
    );
}

#[test]
fn test_arg_query_macro_native_roundtrip_and_shared_record() {
    let mk_query = |expansion: QueryExpansion| {
        let mut lits = BTreeMap::new();
        lits.insert((5usize, 10usize), label("foo", "dep1"));
        lits.insert((23usize, 10usize), label("bar", "dep2"));
        UnconfiguredMacro::Query(Box::new(QueryMacroBase {
            expansion_type: expansion,
            query: QueryAttrBase {
                query: "deps(//foo:dep1) + set(//bar:dep2)".to_owned(),
                resolved_literals: ResolvedQueryLiterals(lits),
            },
        }))
    };
    let query_arg = |prefix: &str, expansion| {
        arg(vec![
            StringWithMacrosPart::String(ArcStr::from(prefix)),
            StringWithMacrosPart::Macro(false, mk_query(expansion)),
        ])
    };
    let no_lits = arg(vec![StringWithMacrosPart::Macro(
        true,
        UnconfiguredMacro::Query(Box::new(QueryMacroBase {
            expansion_type: QueryExpansion::Output,
            query: QueryAttrBase {
                query: "deps(:x)".to_owned(),
                resolved_literals: ResolvedQueryLiterals(BTreeMap::new()),
            },
        })),
    )]);
    roundtrip_canonical(&[
        query_arg("--deps=", QueryExpansion::Target),
        query_arg(
            "--deps=",
            QueryExpansion::TargetAndOutput(Some("=".to_owned())),
        ),
        query_arg("--deps=", QueryExpansion::TargetAndOutput(None)),
        no_lits,
    ]);
    let (arena, r) = encode_one(&query_arg("--deps=", QueryExpansion::Target));
    let (reprint, lits) = queries_of(arena.view(r)).remove(0);
    assert_eq!(reprint, "( deps(root//foo:dep1) + set(root//bar:dep2))");
    assert_eq!(lits.0.len(), 2);

    // Same macro in a different arg: only the new arg record and the
    // new text record allocate; the query macro record is shared.
    let mut b = CoercedAttrArena::builder();
    query_arg("--deps=", QueryExpansion::Target)(&mut b).unwrap();
    let bytes_after = b.arena.bytes.len();
    let AttrToken(r2, _) = query_arg("--other=", QueryExpansion::Target)(&mut b).unwrap();
    let (_, width) = b.arena.tag(r2);
    assert_eq!(
        b.arena.bytes.len(),
        bytes_after + (1 + 1 + 8) + (1 + 1 + 2 * width as usize),
        "second arg should add one 8-byte text record and its own [tag, count, 2 refs] record"
    );
}

/// Coercion records a repeated literal once (first occurrence's span);
/// every occurrence must still encode as that label, or the reprint
/// would carry the raw spelling that analysis cannot resolve.
#[test]
fn test_arg_query_macro_repeated_literal_resolves_every_occurrence() {
    let query = "deps(set(//foo:dep1 //foo:dep1 //bar:dep2 //foo:dep1))".to_owned();
    let mut lits = BTreeMap::new();
    lits.insert((9usize, 10usize), label("foo", "dep1"));
    lits.insert((31usize, 10usize), label("bar", "dep2"));
    let value = arg(vec![StringWithMacrosPart::Macro(
        false,
        UnconfiguredMacro::Query(Box::new(QueryMacroBase {
            expansion_type: QueryExpansion::Target,
            query: QueryAttrBase {
                query,
                resolved_literals: ResolvedQueryLiterals(lits),
            },
        })),
    )]);
    let mut b = CoercedAttrArena::builder();
    let r = value(&mut b).unwrap().0;
    let arena = b.finish();
    let (reprint, lits) = queries_of(arena.view(r)).remove(0);
    assert_eq!(
        reprint, "deps(set(root//foo:dep1 root//foo:dep1 root//bar:dep2 root//foo:dep1))",
        "every occurrence should reprint as the resolved label"
    );
    assert_eq!(
        lits.0.len(),
        4,
        "each occurrence should carry its own literal span"
    );
}

#[test]
fn test_source_file_pointer_dedup() {
    use buck2_core::package::package_relative_path::PackageRelativePath;
    let shared = PackedStr::new(PackageRelativePath::new("hdrs/common.h").unwrap());
    let a = source_file(CoercedPath::File(shared.dupe()));
    let b = source_file(CoercedPath::File(shared));
    let other = source_file(CoercedPath::File(PackedStr::new(
        PackageRelativePath::new("other.h").unwrap(),
    )));
    let mut bld = CoercedAttrArena::builder();
    let AttrToken(r1, h1) = a(&mut bld).unwrap();
    let AttrToken(r2, h2) = b(&mut bld).unwrap();
    let AttrToken(r3, _) = other(&mut bld).unwrap();
    assert_eq!(r1, r2, "same listing allocation should dedup by pointer");
    assert_eq!(h1, h2);
    assert_ne!(r1, r3);
    assert_eq!(bld.arena.paths.len(), 2);
}

#[test]
fn test_source_directory_pointer_dedup() {
    use buck2_core::package::package_relative_path::PackageRelativePath;
    let path = |s: &str| PackedStr::new(PackageRelativePath::new(s).unwrap());
    let dir = path("models");
    let files = [path("models/a.py"), path("models/b.py")];
    let directory = |files: &[PackedStr<PackageRelativePath>]| {
        source_file(CoercedPath::Directory(Box::new(CoercedDirectory {
            dir: dir.dupe(),
            files: files.into(),
        })))
    };
    let a = directory(&files);
    let b = directory(&files);
    let fewer_files = directory(&files[..1]);
    let mut bld = CoercedAttrArena::builder();
    let AttrToken(r1, h1) = a(&mut bld).unwrap();
    let AttrToken(r2, h2) = b(&mut bld).unwrap();
    let AttrToken(r3, _) = fewer_files(&mut bld).unwrap();
    assert_eq!(
        r1, r2,
        "the same directory and file allocations should dedup by pointer"
    );
    assert_eq!(h1, h2);
    assert_ne!(
        r1, r3,
        "the same directory with other files is another value"
    );
    assert_eq!(bld.arena.paths.len(), 2);
}

#[test]
fn test_arg_parts_dedup_across_args() {
    // Same template, different exe label: the literal chunk is shared;
    // the second arg adds only its parent node and the new macro record.
    let exe_arg = |name: &str| {
        arg(vec![
            StringWithMacrosPart::String(ArcStr::from("-o $OUT --long-shared-literal-chunk")),
            StringWithMacrosPart::Macro(
                false,
                UnconfiguredMacro::Exe {
                    label: label("tools", name),
                    exec_dep: true,
                },
            ),
        ])
    };
    let mut b = CoercedAttrArena::builder();
    let AttrToken(r1, h1) = exe_arg("gen1")(&mut b).unwrap();
    let bytes_after_first = b.arena.bytes.len();
    let AttrToken(r2, h2) = exe_arg("gen2")(&mut b).unwrap();
    assert_ne!(r1, r2);
    assert_ne!(h1, h2);
    let (_, width) = b.arena.tag(r2);
    // The macro record is [tag, header, label index, 0 refs] (4 bytes);
    // the arg record is [tag, count, 2 refs]; the text record is shared.
    assert_eq!(
        b.arena.bytes.len(),
        bytes_after_first + 4 + (1 + 1 + 2 * width as usize),
        "second arg should add its own record and one macro record; the text record is shared"
    );
    let AttrToken(r3, _) = exe_arg("gen1")(&mut b).unwrap();
    assert_eq!(r1, r3, "identical arg should dedup to one node");
}

#[test]
fn test_dedup_shares_nodes_and_labels() {
    let shared = list(vec![s("shared"), dep(label("foo", "bar"))]);
    let mut b = CoercedAttrArena::builder();
    let r1 = shared(&mut b).unwrap().0;
    let bytes_after = b.arena.bytes.len();
    let r2 = shared(&mut b).unwrap().0;
    assert_eq!(r1, r2, "structurally equal values should dedup to one ref");
    assert_eq!(
        bytes_after,
        b.arena.bytes.len(),
        "dedup hit should allocate nothing"
    );
    let r3 = b.encode_label(&label("foo", "bar")).unwrap().0;
    // A different kind over the same label is an inline leaf sharing
    // the table entry: no bytes.
    assert_eq!(b.arena.bytes.len(), bytes_after);
    let arena = b.finish();
    assert_eq!(
        arena.providers_labels.len(),
        1,
        "same label via Dep and Label should share one table entry"
    );
    // Distinct-kind refs to the same label index must stay distinct.
    assert_ne!(r1, r3);
}
/// A label outside the unquoted-word grammar reprints quoted, so the
/// reprint parses back to the same expression.
#[test]
fn test_query_reprint_quotes_labels_outside_word_grammar() {
    let text = "deps('//x:libc++') + set('//x:y[sub]')";
    let lits = ResolvedQueryLiterals(BTreeMap::from([
        ((6, 10), label("x", "libc++")),
        (
            (26, 10),
            ProvidersLabel::testing_new("root", "x", "y", Some(&["sub"])),
        ),
    ]));
    let attr = query(QueryAttrBase {
        query: text.to_owned(),
        resolved_literals: lits,
    });
    roundtrip_canonical(std::slice::from_ref(&attr));

    let mut b = CoercedAttrArena::builder();
    let AttrToken(r, _) = attr(&mut b).unwrap();
    let arena = b.finish();
    let CoercedAttrRefKind::Query(q) = arena.view(r).kind() else {
        panic!("should decode as a query");
    };
    assert_eq!(
        q.query().0,
        "( deps('root//x:libc++') + set('root//x:y[sub]'))"
    );
}
