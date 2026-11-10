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
use std::hash::Hash;
use std::hash::Hasher;

use allocative::Allocative;
use buck2_core::cells::CellAliasResolver;
use buck2_core::cells::CellResolver;
use buck2_core::cells::name::CellName;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::soft_error_or_panic;
use buck2_error::buck2_error;
use buck2_hash::BuckHasher;
use buck2_hash::BuckMutInsertionMap;
use buck2_util::arc_str::ArcStr;
use buck2_util::strong_hasher::Blake3StrongHasher;
use buck2_util::strong_hasher::StrongHash128;
use dupe::Dupe;
use pagable::Pagable;
use starlark::values::AllocValue;
use starlark::values::Heap;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::dict::DictRef;
use starlark::values::list::ListRef;
use starlark::values::tuple::TupleRef;
use static_interner::Intern;
use static_interner::interner;
use strong_hash::StrongHash;

use crate::attrs::attr_type::any_matches::AnyMatches;

pub const MODIFIER_METADATA_KEY: &str = "buck.cfg_modifiers";

/// A modifier value in the cfg constructor's grammar: a modifier is `None`,
/// a constraint target, or a `modifiers.match` over constraint targets; a
/// `PACKAGE` value is a list of modifier lists, each tagged with its origin.
/// Text that does not fit the grammar is kept verbatim so the constructor
/// rejects it exactly as it would have.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StrongHash, Allocative, Pagable)]
pub enum Modifier {
    None,
    /// A constraint value or conditional modifier target.
    Constraint(ProvidersLabel),
    /// A string that is not a target label.
    Invalid(ArcStr),
    /// The JSON text of a subtree that does not fit the grammar.
    Malformed(ArcStr),
    /// A modifier list: a target's `modifiers` attribute, a group's
    /// modifiers, or a `PACKAGE`'s groups.
    List(Vec<Modifier>),
    Match(Box<ModifierMatch>),
    /// A `PACKAGE` modifier group.
    Group(Box<ModifierGroup>),
}

/// `modifiers.match`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StrongHash, Allocative, Pagable)]
pub struct ModifierMatch {
    /// In the order the constructor tries them; the first match wins.
    entries: Vec<MatchEntry>,
    default: Option<Modifier>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, StrongHash, Allocative, Pagable)]
struct MatchEntry {
    key: MatchKey,
    value: Modifier,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, StrongHash, Allocative, Pagable)]
enum MatchKey {
    Constraint(ProvidersLabel),
    /// A key that is not a target label.
    Invalid(ArcStr),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, StrongHash, Allocative, Pagable)]
pub struct ModifierGroup {
    location: ModifierLocation,
    rule_name: Option<ArcStr>,
    /// A modifier list.
    modifiers: Modifier,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, StrongHash, Allocative, Pagable)]
pub enum ModifierLocation {
    /// The `PACKAGE` file that set the modifiers. Opaque text written by
    /// the prelude and rendered back verbatim; only error messages read it.
    Package(ArcStr),
    Target,
}

/// Resolves modifier target strings. Modifiers are resolved in the root cell
/// regardless of where they are written.
pub struct ModifierLabelParser<'a> {
    cell: CellName,
    cells: &'a CellResolver,
    aliases: &'a CellAliasResolver,
}

impl<'a> ModifierLabelParser<'a> {
    pub fn new(cells: &'a CellResolver) -> Self {
        Self {
            cell: cells.root_cell(),
            cells,
            aliases: cells.root_cell_cell_alias_resolver(),
        }
    }

    fn label(&self, s: &str) -> Option<ProvidersLabel> {
        ProvidersLabel::parse(s, self.cell, self.cells, self.aliases).ok()
    }

    fn modifier(&self, s: &str) -> Modifier {
        match self.label(s) {
            Some(label) => Modifier::Constraint(label),
            None => Modifier::Invalid(ArcStr::from(s)),
        }
    }

    fn key(&self, s: &str) -> MatchKey {
        match self.label(s) {
            Some(label) => MatchKey::Constraint(label),
            None => MatchKey::Invalid(ArcStr::from(s)),
        }
    }
}

/// A modifier value: a target's `modifiers` attribute or a `PACKAGE`'s
/// merged modifiers.
///
/// Interned process-wide, like `ConfigurationData`: equal values are one
/// instance, so every holder (package storage, DICE keys) shares it and
/// `Hash`/`Eq` are pointer operations. Interned values are never freed; the
/// set is bounded by the distinct modifier lists ever coerced.
#[derive(Debug, Clone, Dupe, Copy, PartialEq, Eq, Hash, Allocative, Pagable)]
pub struct CfgModifiers(Intern<CfgModifiersData>);

#[derive(Debug, Allocative, Pagable)]
struct CfgModifiersData {
    /// The enclosing `PACKAGE`'s modifiers, whose groups precede `value`'s
    /// in the merged list. Only `PACKAGE` values have one.
    parent: Option<CfgModifiers>,
    value: Modifier,
    /// A target attribute's JSON text as written, when it renders
    /// differently from `value`: what users see of the attribute.
    // TODO(jtbraun): render users the parsed value, and drop this, once the
    // normalized form is announced.
    source: Option<ArcStr>,
    /// 128 bits of blake3 over the parent's hash, the value and the source,
    /// computed once: the interner's key, and the value's identity across
    /// processes.
    content_hash: StrongHash128,
}

impl PartialEq for CfgModifiersData {
    fn eq(&self, other: &Self) -> bool {
        self.content_hash == other.content_hash
    }
}

impl Eq for CfgModifiersData {}

impl Hash for CfgModifiersData {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.content_hash.hash(state);
    }
}

interner!(INTERNER, BuckHasher, CfgModifiersData);

impl CfgModifiers {
    fn new(parent: Option<CfgModifiers>, value: Modifier) -> Self {
        Self::new_with_source(parent, value, None)
    }

    fn new_with_source(
        parent: Option<CfgModifiers>,
        value: Modifier,
        source: Option<ArcStr>,
    ) -> Self {
        let mut hasher = Blake3StrongHasher::new();
        parent.map(|p| p.content_hash()).hash(&mut hasher);
        value.strong_hash(&mut hasher);
        source.as_deref().hash(&mut hasher);
        Self(INTERNER.intern(CfgModifiersData {
            parent,
            value,
            source,
            content_hash: hasher.finalize128(),
        }))
    }

    /// No modifiers.
    pub fn empty() -> Self {
        Self::new(None, Modifier::List(Vec::new()))
    }

    pub fn content_hash(&self) -> StrongHash128 {
        self.0.content_hash
    }

    /// A target's `modifiers` attribute: a list of modifiers.
    pub fn from_target<'a>(value: impl ModifierSource<'a>, labels: &ModifierLabelParser) -> Self {
        let parsed = parse_list(value, labels);
        let source = value.json_text();
        let rendered_as_written = serde_json::from_str::<serde_json::Value>(&source)
            .is_ok_and(|written| written == render(&parsed, &mut JsonRenderer));
        let source = (!rendered_as_written).then(|| ArcStr::from(source.as_str()));
        Self::new_with_source(None, parsed, source)
    }

    /// A `PACKAGE`'s merged modifiers: a list of tagged modifier groups,
    /// opening with the groups of the enclosing `PACKAGE` (`parent`). Only
    /// the groups this `PACKAGE` adds are stored; a `PACKAGE` that adds none
    /// is its parent.
    pub fn from_package<'a>(
        value: impl ModifierSource<'a>,
        labels: &ModifierLabelParser,
        parent: Option<CfgModifiers>,
    ) -> Self {
        let Some(groups) = value.list() else {
            return Self::new(None, malformed(value));
        };
        let mut own: Vec<Modifier> = groups
            .into_iter()
            .map(|group| parse_group(group, labels))
            .collect();
        let Some(parent) = parent.filter(|p| p.is_group_list()) else {
            return Self::new(None, Modifier::List(own));
        };
        let Some(added) = parent.strip_groups(&own).map(|rest| rest.len()) else {
            return Self::new(None, Modifier::List(own));
        };
        if added == 0 {
            return parent;
        }
        own.drain(..own.len() - added);
        Self::new(Some(parent), Modifier::List(own))
    }

    /// Whether the value is a list of groups, the shape a chain is made of.
    fn is_group_list(self) -> bool {
        matches!(self.0.value, Modifier::List(_))
    }

    /// The groups this value stores itself, without the parent's.
    fn own_groups(&self) -> &[Modifier] {
        match &self.0.value {
            Modifier::List(groups) => groups,
            _ => &[],
        }
    }

    /// `groups` with the chain's merged groups removed from its front, or
    /// `None` if it does not open with them.
    fn strip_groups<'g>(self, groups: &'g [Modifier]) -> Option<&'g [Modifier]> {
        let groups = match &self.0.parent {
            Some(parent) => parent.strip_groups(groups)?,
            None => groups,
        };
        groups.strip_prefix(self.own_groups())
    }

    pub fn is_empty(&self) -> bool {
        self.0.parent.is_none_or(|p| p.is_empty())
            && match &self.0.value {
                Modifier::None => true,
                Modifier::List(items) => items.is_empty(),
                _ => false,
            }
    }

    fn render<R: ModifierRenderer>(self, renderer: &mut R) -> R::Out {
        if self.0.parent.is_none() {
            return render(&self.0.value, renderer);
        }
        let mut items = Vec::new();
        self.render_groups(renderer, &mut items);
        renderer.list(items)
    }

    fn render_groups<R: ModifierRenderer>(self, renderer: &mut R, items: &mut Vec<R::Out>) {
        if let Some(parent) = &self.0.parent {
            parent.render_groups(renderer, items);
        }
        items.extend(
            self.own_groups()
                .iter()
                .map(|group| render(group, renderer)),
        );
    }

    /// The value as users see it, as JSON.
    pub fn to_json(&self) -> serde_json::Value {
        self.render_for_users(&mut JsonRenderer)
    }

    /// The value as users see it, as a Starlark value.
    pub fn alloc_starlark<'v>(&self, heap: Heap<'v>) -> Value<'v> {
        self.render_for_users(&mut StarlarkRenderer(heap))
    }

    fn render_for_users<R: ModifierRenderer>(self, renderer: &mut R) -> R::Out {
        match &self.0.source {
            Some(source) => renderer.raw_json(source),
            None => self.render(renderer),
        }
    }

    /// Every entry the cfg constructor will reject, with what is wrong with
    /// it, across the whole chain.
    fn unresolvable(&self) -> Vec<(&'static str, &ArcStr)> {
        let mut out = Vec::new();
        if let Some(parent) = &self.0.parent {
            out.extend(parent.unresolvable());
        }
        self.0.value.unresolvable(&mut out);
        out
    }

    /// Reports every entry the cfg constructor will reject, once per call.
    pub fn report_unresolvable(&self) {
        for (what, text) in self.unresolvable() {
            soft_error_or_panic!(
                "cfg_modifier_unresolvable",
                buck2_error!(buck2_error::ErrorTag::Input, "{what}: `{text}`")
            );
        }
    }
}

/// The parsed value in the shape the cfg constructor consumes.
impl<'v> AllocValue<'v> for CfgModifiers {
    type Type = Value<'v>;

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        self.render(&mut StarlarkRenderer(heap)).into()
    }
}

impl Modifier {
    fn unresolvable<'a>(&'a self, out: &mut Vec<(&'static str, &'a ArcStr)>) {
        match self {
            Modifier::None | Modifier::Constraint(_) => {}
            Modifier::Invalid(s) => out.push(("Modifier is not a target label", s)),
            Modifier::Malformed(s) => out.push(("Modifier does not fit the modifier grammar", s)),
            Modifier::List(items) => {
                for item in items {
                    item.unresolvable(out);
                }
            }
            Modifier::Match(m) => {
                for entry in &m.entries {
                    if let MatchKey::Invalid(s) = &entry.key {
                        out.push(("Modifier is not a target label", s));
                    }
                    entry.value.unresolvable(out);
                }
                if let Some(default) = &m.default {
                    default.unresolvable(out);
                }
            }
            Modifier::Group(group) => group.modifiers.unresolvable(out),
        }
    }
}

/// A value a modifier tree is read from: the Starlark value a `PACKAGE`
/// file or a target sets, or its JSON rendering.
pub trait ModifierSource<'a>: Copy {
    fn is_none(self) -> bool;
    fn as_str(self) -> Option<&'a str>;
    fn list(self) -> Option<Vec<Self>>;
    /// Dict entries with string keys; `None` for a non-dict or a non-string
    /// key.
    fn entries(self) -> Option<Vec<(&'a str, Self)>>;
    /// The value as JSON text.
    fn json_text(self) -> String;
}

impl<'a> ModifierSource<'a> for &'a serde_json::Value {
    fn is_none(self) -> bool {
        self.is_null()
    }

    fn as_str(self) -> Option<&'a str> {
        self.as_str()
    }

    fn list(self) -> Option<Vec<Self>> {
        self.as_array().map(|items| items.iter().collect())
    }

    fn entries(self) -> Option<Vec<(&'a str, Self)>> {
        self.as_object()
            .map(|map| map.iter().map(|(k, v)| (k.as_str(), v)).collect())
    }

    fn json_text(self) -> String {
        self.to_string()
    }
}

impl<'v> ModifierSource<'v> for Value<'v> {
    fn is_none(self) -> bool {
        Value::is_none(self)
    }

    fn as_str(self) -> Option<&'v str> {
        self.unpack_str()
    }

    fn list(self) -> Option<Vec<Self>> {
        ListRef::from_value(self)
            .map(|l| l.content().to_vec())
            .or_else(|| TupleRef::from_value(self).map(|t| t.content().to_vec()))
    }

    fn entries(self) -> Option<Vec<(&'v str, Self)>> {
        DictRef::from_value(self)?
            .iter()
            .map(|(k, v)| Some((k.unpack_str()?, v)))
            .collect()
    }

    fn json_text(self) -> String {
        match self.to_json_value() {
            Ok(json) => json.to_string(),
            Err(_) => serde_json::Value::String(self.to_repr()).to_string(),
        }
    }
}

fn malformed<'a>(value: impl ModifierSource<'a>) -> Modifier {
    Modifier::Malformed(ArcStr::from(value.json_text().as_str()))
}

impl Display for CfgModifiers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.to_json(), f)
    }
}

impl StrongHash for CfgModifiers {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        self.content_hash().strong_hash(state);
    }
}

impl AnyMatches for CfgModifiers {
    fn any_matches(
        &self,
        filter: &dyn Fn(&str) -> buck2_error::Result<bool>,
    ) -> buck2_error::Result<bool> {
        self.to_json().any_matches(filter)
    }
}

/// A modifier list: `None`, a list of modifiers, or malformed text.
fn parse_list<'a>(value: impl ModifierSource<'a>, labels: &ModifierLabelParser) -> Modifier {
    if value.is_none() {
        return Modifier::None;
    }
    match value.list() {
        Some(items) => Modifier::List(
            items
                .into_iter()
                .map(|item| parse_modifier(item, labels))
                .collect(),
        ),
        None => malformed(value),
    }
}

fn parse_modifier<'a>(value: impl ModifierSource<'a>, labels: &ModifierLabelParser) -> Modifier {
    if value.is_none() {
        return Modifier::None;
    }
    if let Some(s) = value.as_str() {
        return labels.modifier(s);
    }
    let Some(mut entries) = value.entries().filter(|e| is_tagged(e, "ModifiersMatch")) else {
        return malformed(value);
    };
    // The constructor tries entries in this order, first match wins.
    entries.sort_by(|a, b| a.0.cmp(b.0));
    let mut m = ModifierMatch {
        entries: Vec::new(),
        default: None,
    };
    for (key, sub) in entries {
        match key {
            "_type" => {}
            "DEFAULT" => m.default = Some(parse_modifier(sub, labels)),
            key => {
                // Two spellings of one target are one key once canonical.
                // The constructor tries entries in order and the same
                // target cannot match twice, so only the first can win.
                let key = labels.key(key);
                if m.entries.iter().any(|e| e.key == key) {
                    continue;
                }
                let value = parse_modifier(sub, labels);
                m.entries.push(MatchEntry { key, value });
            }
        }
    }
    Modifier::Match(Box::new(m))
}

/// A `PACKAGE` modifier group, or malformed text.
fn parse_group<'a>(value: impl ModifierSource<'a>, labels: &ModifierLabelParser) -> Modifier {
    let Some(group) = value.entries().filter(|e| is_tagged(e, "TaggedModifiers")) else {
        return malformed(value);
    };
    let field = |name: &str| group.iter().find(|(k, _)| *k == name).map(|(_, v)| *v);
    let location = match field("location").and_then(|l| l.entries()) {
        Some(l) if is_tagged(&l, "ModifierTargetLocation") => ModifierLocation::Target,
        Some(l) if is_tagged(&l, "ModifierPackageLocation") => {
            match l
                .iter()
                .find(|(k, _)| *k == "package_path")
                .and_then(|(_, v)| v.as_str())
            {
                Some(path) => ModifierLocation::Package(ArcStr::from(path)),
                None => return malformed(value),
            }
        }
        _ => return malformed(value),
    };
    let rule_name = match field("rule_name") {
        None => None,
        Some(v) if v.is_none() => None,
        Some(v) => match v.as_str() {
            Some(name) => Some(ArcStr::from(name)),
            None => return malformed(value),
        },
    };
    let Some(modifiers) = field("modifiers") else {
        return malformed(value);
    };
    Modifier::Group(Box::new(ModifierGroup {
        location,
        rule_name,
        modifiers: parse_list(modifiers, labels),
    }))
}

fn is_tagged<'a, S: ModifierSource<'a>>(entries: &[(&'a str, S)], tag: &str) -> bool {
    entries
        .iter()
        .any(|(k, v)| *k == "_type" && v.as_str() == Some(tag))
}

/// Builds the nodes of one output representation as `render` walks the
/// value.
trait ModifierRenderer {
    type Out;
    fn none(&mut self) -> Self::Out;
    fn string(&mut self, s: &str) -> Self::Out;
    fn list(&mut self, items: Vec<Self::Out>) -> Self::Out;
    /// `entries` arrive in the order the constructor must see them.
    fn dict(&mut self, entries: Vec<(String, Self::Out)>) -> Self::Out;
    fn raw_json(&mut self, json: &str) -> Self::Out;
}

fn render<R: ModifierRenderer>(modifier: &Modifier, renderer: &mut R) -> R::Out {
    match modifier {
        Modifier::None => renderer.none(),
        Modifier::Constraint(label) => renderer.string(&label.to_string()),
        Modifier::Invalid(s) => renderer.string(s),
        Modifier::Malformed(json) => renderer.raw_json(json),
        Modifier::List(items) => {
            let items = items.iter().map(|item| render(item, renderer)).collect();
            renderer.list(items)
        }
        Modifier::Match(m) => {
            let mut dict: Vec<(String, R::Out)> = Vec::with_capacity(m.entries.len() + 2);
            for entry in &m.entries {
                let key = match &entry.key {
                    MatchKey::Constraint(label) => label.to_string(),
                    MatchKey::Invalid(s) => s.to_string(),
                };
                dict.push((key, render(&entry.value, renderer)));
            }
            if let Some(default) = &m.default {
                dict.push(("DEFAULT".to_owned(), render(default, renderer)));
            }
            let tag = renderer.string("ModifiersMatch");
            dict.push(("_type".to_owned(), tag));
            renderer.dict(dict)
        }
        Modifier::Group(group) => {
            let location = match &group.location {
                ModifierLocation::Target => {
                    let tag = renderer.string("ModifierTargetLocation");
                    renderer.dict(vec![("_type".to_owned(), tag)])
                }
                ModifierLocation::Package(path) => {
                    let tag = renderer.string("ModifierPackageLocation");
                    let path = renderer.string(path);
                    renderer.dict(vec![
                        ("_type".to_owned(), tag),
                        ("package_path".to_owned(), path),
                    ])
                }
            };
            let modifiers = render(&group.modifiers, renderer);
            let rule_name = match &group.rule_name {
                Some(name) => renderer.string(name),
                None => renderer.none(),
            };
            let tag = renderer.string("TaggedModifiers");
            renderer.dict(vec![
                ("_type".to_owned(), tag),
                ("location".to_owned(), location),
                ("modifiers".to_owned(), modifiers),
                ("rule_name".to_owned(), rule_name),
            ])
        }
    }
}

struct JsonRenderer;

impl ModifierRenderer for JsonRenderer {
    type Out = serde_json::Value;

    fn none(&mut self) -> serde_json::Value {
        serde_json::Value::Null
    }

    fn string(&mut self, s: &str) -> serde_json::Value {
        serde_json::Value::String(s.to_owned())
    }

    fn list(&mut self, items: Vec<serde_json::Value>) -> serde_json::Value {
        serde_json::Value::Array(items)
    }

    fn dict(&mut self, entries: Vec<(String, serde_json::Value)>) -> serde_json::Value {
        serde_json::Value::Object(entries.into_iter().collect())
    }

    fn raw_json(&mut self, json: &str) -> serde_json::Value {
        serde_json::from_str(json).expect("should parse: the text was written by serde_json")
    }
}

struct StarlarkRenderer<'v>(Heap<'v>);

impl<'v> ModifierRenderer for StarlarkRenderer<'v> {
    type Out = Value<'v>;

    fn none(&mut self) -> Value<'v> {
        Value::new_none()
    }

    fn string(&mut self, s: &str) -> Value<'v> {
        self.0.alloc_str(s).to_value()
    }

    fn list(&mut self, items: Vec<Value<'v>>) -> Value<'v> {
        self.0.alloc(items)
    }

    fn dict(&mut self, entries: Vec<(String, Value<'v>)>) -> Value<'v> {
        self.0.alloc(
            entries
                .into_iter()
                .collect::<BuckMutInsertionMap<String, Value<'v>>>(),
        )
    }

    fn raw_json(&mut self, json: &str) -> Value<'v> {
        self.0.alloc(JsonRenderer.raw_json(json))
    }
}

#[cfg(test)]
mod tests {
    use buck2_core::cells::CellResolver;
    use buck2_core::cells::cell_root_path::CellRootPathBuf;
    use buck2_core::cells::name::CellName;
    use serde_json::json;
    use starlark::values::tuple::AllocTuple;

    use super::*;

    fn cells() -> CellResolver {
        CellResolver::testing_with_names_and_paths(&[
            (
                CellName::testing_new("root"),
                CellRootPathBuf::testing_new(""),
            ),
            (
                CellName::testing_new("cfg"),
                CellRootPathBuf::testing_new("cfg"),
            ),
        ])
    }

    /// What the cfg constructor sees.
    fn parsed(m: CfgModifiers) -> serde_json::Value {
        m.render(&mut JsonRenderer)
    }

    fn target(json: serde_json::Value) -> CfgModifiers {
        CfgModifiers::from_target(&json, &ModifierLabelParser::new(&cells()))
    }

    fn package(json: serde_json::Value) -> CfgModifiers {
        CfgModifiers::from_package(&json, &ModifierLabelParser::new(&cells()), None)
    }

    fn child(json: serde_json::Value, parent: CfgModifiers) -> CfgModifiers {
        CfgModifiers::from_package(&json, &ModifierLabelParser::new(&cells()), Some(parent))
    }

    fn group(path: &str, modifiers: serde_json::Value) -> serde_json::Value {
        json!({
            "_type": "TaggedModifiers",
            "location": {"_type": "ModifierPackageLocation", "package_path": path},
            "modifiers": modifiers,
            "rule_name": null,
        })
    }

    #[test]
    fn test_target_modifiers_round_trip() {
        let json = json!([
            "cfg//os:linux",
            null,
            {
                "_type": "ModifiersMatch",
                "cfg//os:windows": "cfg//compiler:msvc[sub]",
                "cfg//os:linux": {
                    "_type": "ModifiersMatch",
                    "DEFAULT": null,
                    "cfg//arch:arm": "cfg//compiler:clang",
                },
                "DEFAULT": "cfg//compiler:gcc",
            },
        ]);
        let m = target(json.clone());
        assert_eq!(m.to_json(), json);
        assert!(!m.is_empty());
        assert!(
            m.unresolvable().is_empty(),
            "every string should have parsed as a label: {:?}",
            m.unresolvable()
        );
    }

    #[test]
    fn test_unparseable_text_survives_verbatim() {
        let json = json!([
            "not-a-label",
            42,
            {"_type": "Unknown", "x": 1},
            {"_type": "ModifiersMatch", "also-not-a-label": "cfg//a:b"},
        ]);
        let m = target(json.clone());
        assert_eq!(m.to_json(), json);
        assert_eq!(
            m.unresolvable().len(),
            4,
            "two invalid labels and two malformed subtrees"
        );
    }

    /// Users see a target's modifiers as written; the cfg constructor sees
    /// the parsed value, with canonical labels and duplicate keys merged.
    #[test]
    fn test_non_canonical_modifiers_render_as_written() {
        let written = json!([
            "//os:linux",
            {"_type": "ModifiersMatch", "//os:linux": "cfg//a:b", "root//os:linux": "cfg//a:c"},
        ]);
        let m = target(written.clone());
        assert_eq!(m.to_json(), written);
        assert_eq!(m.to_string(), written.to_string());
        Heap::temp(|heap| {
            assert_eq!(
                m.alloc_starlark(heap).to_json_value().unwrap(),
                written,
                "BXL and rules see the value as written"
            );
            let parsed = heap.alloc(m).to_json_value().unwrap();
            assert_eq!(parsed[0], json!("root//os:linux"));
            assert_eq!(
                parsed[1].as_object().unwrap().len(),
                2,
                "the cfg constructor sees one key for one target: {parsed}"
            );
        });
        assert_ne!(
            m,
            target(json!([
                "root//os:linux",
                {"_type": "ModifiersMatch", "root//os:linux": "cfg//a:c"},
            ])),
            "a different spelling is a different value"
        );
    }

    #[test]
    fn test_starlark_tuple_is_a_list() {
        Heap::temp(|heap| {
            let cells = cells();
            let labels = ModifierLabelParser::new(&cells);
            assert_eq!(
                CfgModifiers::from_target(heap.alloc(AllocTuple(["cfg//os:linux"])), &labels),
                CfgModifiers::from_target(heap.alloc(vec!["cfg//os:linux"]), &labels),
            );
            assert!(CfgModifiers::from_target(heap.alloc(AllocTuple::EMPTY), &labels).is_empty());
            assert_eq!(
                CfgModifiers::from_package(heap.alloc(AllocTuple::EMPTY), &labels, None),
                CfgModifiers::from_package(heap.alloc(Vec::<&str>::new()), &labels, None),
            );
        });
    }

    #[test]
    fn test_empty_and_non_list() {
        assert!(target(json!([])).is_empty());
        assert!(target(json!(null)).is_empty());
        assert_eq!(target(json!({"a": 1})).to_json(), json!({"a": 1}));
    }

    #[test]
    fn test_package_modifiers_round_trip() {
        let json = json!([
            {
                "_type": "TaggedModifiers",
                "location": {"_type": "ModifierPackageLocation", "package_path": "root//foo/PACKAGE"},
                "modifiers": ["cfg//os:linux"],
                "rule_name": null,
            },
            {
                "_type": "TaggedModifiers",
                "location": {"_type": "ModifierTargetLocation"},
                "modifiers": [],
                "rule_name": "python_binary",
            },
            "garbage",
        ]);
        assert_eq!(package(json.clone()).to_json(), json);
        assert_eq!(
            package(json!(["aaabbbccc"])).to_json(),
            json!(["aaabbbccc"])
        );
    }

    /// Two spellings of one target inside a `modifiers.match` are one key
    /// after canonicalization; the first entry wins, as it does when the
    /// constructor tries them in order.
    #[test]
    fn test_match_keys_that_canonicalize_together_keep_the_first() {
        let m = target(json!([{
            "_type": "ModifiersMatch",
            "//os:linux": "cfg//compiler:clang",
            "root//os:linux": "cfg//compiler:gcc",
        }]));
        let canonical = target(json!([{
            "_type": "ModifiersMatch",
            "root//os:linux": "cfg//compiler:clang",
        }]));
        assert_eq!(parsed(m), parsed(canonical));
        assert_eq!(
            parsed(canonical),
            json!([{
                "_type": "ModifiersMatch",
                "root//os:linux": "cfg//compiler:clang",
            }])
        );
    }

    /// A `PACKAGE` whose merged list opens with its parent's groups stores
    /// only its own groups and renders the whole chain.
    #[test]
    fn test_package_chain_shares_parent() {
        let root = group("root//PACKAGE", json!(["cfg//os:linux"]));
        let mid = group(
            "root//a/PACKAGE",
            json!([{"_type": "ModifiersMatch", "cfg//os:linux": "cfg//compiler:clang", "DEFAULT": null}]),
        );
        let leaf = group("root//a/b/PACKAGE", json!(["cfg//arch:arm", "not-a-label"]));

        let p0 = package(json!([root.clone()]));
        let p1 = child(json!([root.clone(), mid.clone()]), p0);
        let p2 = child(json!([root.clone(), mid.clone(), leaf.clone()]), p1);

        assert_eq!(p1.to_json(), json!([root, mid]));
        assert_eq!(p2.to_json(), json!([root, mid, leaf]));
        assert_eq!(p2.own_groups().len(), 1, "only the leaf's own group");
        assert!(std::ptr::eq(&*p2.0.parent.unwrap().0, &*p1.0));
        assert!(std::ptr::eq(&*p1.0.parent.unwrap().0, &*p0.0));
        assert!(!p2.is_empty());

        let flat = package(p2.to_json());
        assert_eq!(flat.to_json(), p2.to_json());
        assert_ne!(flat, p2, "flat and chained are different interned values");
    }

    /// A `PACKAGE` that adds no groups is its parent; one that does not
    /// extend its parent stands alone.
    #[test]
    fn test_package_chain_collapse_and_fallback() {
        let root = group("root//PACKAGE", json!(["cfg//os:linux"]));
        let other = group("root//x/PACKAGE", json!(["cfg//os:windows"]));

        let p0 = package(json!([root.clone()]));
        let same = child(json!([root.clone()]), p0);
        assert!(std::ptr::eq(&*same.0, &*p0.0));

        let replaced = child(json!([other.clone()]), p0);
        assert!(replaced.0.parent.is_none());
        assert_eq!(replaced.to_json(), json!([other]));

        let empty_root = package(json!([]));
        assert!(empty_root.is_empty());
        let under_empty = child(json!([root.clone()]), empty_root);
        assert!(std::ptr::eq(
            &*under_empty.0.parent.unwrap().0,
            &*empty_root.0
        ));
        assert!(!under_empty.is_empty());
        assert_eq!(under_empty.to_json(), json!([root]));

        assert!(child(json!("garbage"), p0).0.parent.is_none());
    }

    #[test]
    fn test_identity_is_content() {
        let a = target(json!(["cfg//os:linux", "cfg//os:windows"]));
        let b = target(json!(["cfg//os:linux", "cfg//os:windows"]));
        let c = target(json!(["cfg//os:windows", "cfg//os:linux"]));
        assert_eq!(a, b);
        assert!(
            std::ptr::eq(&*a.0, &*b.0),
            "equal values should be one interned instance"
        );
        assert_ne!(a, c, "order is part of the value");
    }
}
