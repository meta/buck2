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
use buck2_core::cells::cell_path::CellPath;
use buck2_core::package::PackageLabel;
use buck2_core::pattern::package::PackagePattern;
use buck2_core::pattern::pattern::ParsedPattern;
use buck2_core::pattern::pattern_type::TargetPatternExtra;
use buck2_core::target::label::label::TargetLabel;
use buck2_core::target::name_glob::TargetNameGlob;
use buck2_error::internal_error;
use buck2_util::arc_str::ThinArcSlice;
use dupe::Dupe;
use pagable::Pagable;
use serde::Serialize;
use starlark::any::ProvidesStaticType;
use starlark::starlark_simple_value;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::Value;
use starlark::values::starlark_value;
use starlark::values::string::StarlarkStr;
use strong_hash::StrongHash;

use crate::attrs::attr_type::any_matches::AnyMatches;

#[derive(Debug, buck2_error::Error)]
pub enum VisibilityError {
    #[error(
        "`{0}` is not visible to `{1}` (run `buck2 uquery --output-attribute visibility {0}` to check the visibility)"
    )]
    #[buck2(input, tag = Visibility)]
    NotVisibleTo(TargetLabel, TargetLabel),

    #[error(
        "`{0}` is not visible to `{1}` (visibility = {2}). Restricted to {3} by an ancestor PACKAGE's visibility."
    )]
    #[buck2(input, tag = Visibility)]
    NotVisibleToWithIntersection(
        TargetLabel,
        TargetLabel,
        VisibilitySpecification,
        VisibilityIntersection,
    ),
}

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum StarlarkTargetNameGlobError {
    #[error("`target_name_glob(...)` requires at least one entry in `name_globs`")]
    EmptyNameGlobs,
    #[error(
        "Invalid `within` in `target_name_glob`: expected a package pattern \
         (e.g. `//pkg:`) or recursive pattern (e.g. `//pkg/...`), got `{0}`"
    )]
    InvalidWithinScope(String),
    #[error(
        "`PUBLIC` is not a valid `within` scope in `target_name_glob` (it is a \
         target name, not a package pattern); omit `within` to match every package"
    )]
    PublicNotAllowedInWithin,
}

#[derive(
    Debug,
    Eq,
    PartialEq,
    Hash,
    Clone,
    Allocative,
    Pagable,
    StrongHash,
    derive_more::Display
)]
pub enum VisibilityPattern {
    Parsed(ParsedPattern<TargetPatternExtra>),
    TargetNameGlob(TargetNameGlobRecord),
}

/// Convert a parsed pattern into a `within` scope for a `target_name_glob`.
/// `Target` patterns (`//pkg:name`) can never match a package, so they are
/// rejected — the original [`ParsedPattern`] is handed back for error reporting —
/// rather than silently never matching.
///
/// Shared with `visibility_exempt_targets` parsing, so the two cannot drift
/// apart in which pattern kinds they accept.
pub fn within_scope_from_parsed(
    pattern: ParsedPattern<TargetPatternExtra>,
) -> Result<PackagePattern, ParsedPattern<TargetPatternExtra>> {
    match pattern {
        ParsedPattern::Package(p) => Ok(PackagePattern::Package(p)),
        ParsedPattern::Recursive(p) => Ok(PackagePattern::Recursive(p)),
        p @ ParsedPattern::Target(..) => Err(p),
    }
}

#[derive(
    Debug, Eq, PartialEq, Hash, Clone, Dupe, Allocative, Pagable, StrongHash
)]
pub struct TargetNameGlobRecord {
    pub within: ThinArcSlice<PackagePattern>,
    pub name_globs: ThinArcSlice<TargetNameGlob>,
}

/// Renders a `target_name_glob(["..."], within = ["..."])` call that round-trips
/// back into the same Starlark expression. `name_globs` is rendered positionally
/// to match the constructor (where it is positional-only); `within` is
/// pre-rendered to canonical pattern strings by the caller.
fn fmt_target_name_glob<'a>(
    f: &mut Formatter<'_>,
    name_globs: impl Iterator<Item = &'a str>,
    within: impl ExactSizeIterator<Item = String>,
) -> fmt::Result {
    write!(f, "target_name_glob(")?;
    display_container::fmt_container(f, "[", "]", name_globs.map(StarlarkStr::repr))?;
    if within.len() != 0 {
        write!(f, ", within = ")?;
        display_container::fmt_container(f, "[", "]", within.map(|w| StarlarkStr::repr(&w)))?;
    }
    write!(f, ")")
}

impl Display for TargetNameGlobRecord {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        fmt_target_name_glob(
            f,
            self.name_globs.iter().map(|g| g.pattern()),
            self.within.iter().map(ToString::to_string),
        )
    }
}

impl Serialize for TargetNameGlobRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // `to_json` is the single source of truth for the JSON shape; serializing
        // the built `Value` keeps the `to_json` call sites infallible. `Display`
        // and Starlark `repr` render `target_name_glob(...)` call syntax separately.
        self.to_json().serialize(serializer)
    }
}

/// The Starlark value produced by the `target_name_glob(...)` constructor — the
/// raw, pre-coercion form. The globs are validated, but `within` is still the
/// unparsed pattern strings the user passed. The visibility coercer and the
/// `PACKAGE` parser turn this into a [`TargetNameGlobRecord`] via
/// [`StarlarkTargetNameGlob::coerce`], resolving each `within` string against
/// the enclosing file's cell aliases. Keeping this distinct from the coerced
/// record keeps the Starlark layer from depending on the coerced-attr type.
///
/// `NoSerialize`: as a Starlark value a `target_name_glob(...)` has no JSON form,
/// so BXL `json.encode` on it errors. The `uquery --output-attribute ... --json`
/// shape instead comes from [`TargetNameGlobRecord`]'s `Serialize` impl.
#[derive(Debug, ProvidesStaticType, Allocative, StarlarkPagable, NoSerialize)]
pub struct StarlarkTargetNameGlob {
    #[starlark_pagable(pagable)]
    name_globs: ThinArcSlice<TargetNameGlob>,
    #[starlark_pagable(pagable)]
    within: ThinArcSlice<String>,
}

starlark_simple_value!(StarlarkTargetNameGlob);

impl StarlarkTargetNameGlob {
    /// Construct from the raw constructor arguments. Validates `name_globs`;
    /// `within` strings are kept verbatim and parsed later by
    /// [`StarlarkTargetNameGlob::coerce`]. Deferring `within` validation matches
    /// how target labels behave: an invalid scope is reported at each use site
    /// rather than at the (possibly distant) `target_name_glob()` call.
    pub fn try_new(name_globs: Vec<String>, within: Vec<String>) -> buck2_error::Result<Self> {
        if name_globs.is_empty() {
            return Err(StarlarkTargetNameGlobError::EmptyNameGlobs.into());
        }
        let name_globs = name_globs
            .into_iter()
            .map(TargetNameGlob::try_new)
            .collect::<Result<ThinArcSlice<TargetNameGlob>, _>>()?;
        Ok(Self {
            name_globs,
            within: ThinArcSlice::from_iter(within),
        })
    }

    /// Coerce into the [`TargetNameGlobRecord`], parsing each `within` string
    /// into a [`PackagePattern`] with the supplied cell-aware pattern parser.
    /// The visibility coercer passes `ctx.coerce_target_pattern`; the `PACKAGE`
    /// parser passes `ParsedPattern::parse_precise`.
    pub fn coerce(
        &self,
        parse_within: impl Fn(&str) -> buck2_error::Result<ParsedPattern<TargetPatternExtra>>,
    ) -> buck2_error::Result<TargetNameGlobRecord> {
        let within = self
            .within
            .iter()
            .map(|within_str| {
                // Reject `PUBLIC` before parsing so the error names it
                // specifically (it is a target name, not a package pattern).
                if within_str.as_str() == VisibilityPattern::PUBLIC {
                    return Err(StarlarkTargetNameGlobError::PublicNotAllowedInWithin.into());
                }
                within_scope_from_parsed(parse_within(within_str)?).map_err(|_| {
                    StarlarkTargetNameGlobError::InvalidWithinScope(within_str.clone()).into()
                })
            })
            .collect::<buck2_error::Result<ThinArcSlice<PackagePattern>>>()?;
        Ok(TargetNameGlobRecord {
            within,
            name_globs: self.name_globs.dupe(),
        })
    }
}

impl Display for StarlarkTargetNameGlob {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        fmt_target_name_glob(
            f,
            self.name_globs.iter().map(|g| g.pattern()),
            self.within.iter().cloned(),
        )
    }
}

#[starlark_value(type = "target_name_glob")]
impl<'v> StarlarkValue<'v> for StarlarkTargetNameGlob {}

impl VisibilityPattern {
    pub const PUBLIC: &'static str = "PUBLIC";

    pub fn testing_new(pattern: &str) -> VisibilityPattern {
        VisibilityPattern::Parsed(ParsedPattern::testing_parse(pattern))
    }

    pub fn testing_new_record(within: &[&str], name_globs: &[&str]) -> VisibilityPattern {
        let within_scopes: ThinArcSlice<PackagePattern> = within
            .iter()
            .map(|s| {
                within_scope_from_parsed(ParsedPattern::testing_parse(s))
                    .expect("testing within pattern must be Package or Recursive, not Target")
            })
            .collect();
        let globs: ThinArcSlice<TargetNameGlob> = name_globs
            .iter()
            .map(|g| TargetNameGlob::try_new((*g).to_owned()).unwrap())
            .collect();
        VisibilityPattern::TargetNameGlob(TargetNameGlobRecord {
            within: within_scopes,
            name_globs: globs,
        })
    }
}

impl TargetNameGlobRecord {
    /// Canonical JSON form for `uquery --output-attribute` / BXL: a
    /// `__type`-tagged object that round-trips back into `target_name_glob(...)`.
    /// The single source of truth for the JSON shape (`Serialize` delegates
    /// here); building the `Value` is infallible, so callers need no `Result`.
    pub(crate) fn to_json(&self) -> serde_json::Value {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "__type".to_owned(),
            <StarlarkTargetNameGlob as StarlarkValue>::TYPE.into(),
        );
        obj.insert(
            "name_globs".to_owned(),
            self.name_globs.iter().map(|g| g.pattern()).collect(),
        );
        obj.insert(
            "within".to_owned(),
            self.within.iter().map(|p| p.to_string()).collect(),
        );
        obj.into()
    }

    pub fn matches(&self, target: &TargetLabel) -> buck2_error::Result<bool> {
        let within_matches =
            self.within.is_empty() || self.within.iter().any(|scope| scope.matches(target.pkg()));
        if !within_matches {
            return Ok(false);
        }
        for g in self.name_globs.iter() {
            if g.matches(target.name())? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn alloc_starlark_value<'v>(&self, heap: Heap<'v>) -> Value<'v> {
        // `within` scopes render back to their canonical string form.
        heap.alloc(StarlarkTargetNameGlob {
            name_globs: self.name_globs.dupe(),
            within: self.within.iter().map(ToString::to_string).collect(),
        })
    }
}

struct VisibilityPatternQuoted<'a>(&'a VisibilityPattern);

impl Display for VisibilityPatternQuoted<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        // `Parsed` patterns need surrounding quotes when they appear inside a
        // visibility list literal (`["//pkg:", ...]`). `TargetNameGlob` already
        // emits its own `target_name_glob(...)` syntax and must not be quoted.
        match self.0 {
            VisibilityPattern::Parsed(p) => write!(f, "\"{}\"", p),
            VisibilityPattern::TargetNameGlob(r) => Display::fmt(r, f),
        }
    }
}

#[derive(
    Debug, Eq, PartialEq, Hash, Clone, Dupe, Allocative, Pagable, StrongHash
)]
pub enum VisibilityPatternList {
    Public,
    List(ThinArcSlice<VisibilityPattern>),
    /// All sub-lists must match.
    Intersection(ThinArcSlice<VisibilityPatternList>),
}

impl VisibilityPatternList {
    fn is_empty(&self) -> bool {
        match self {
            VisibilityPatternList::Public => false,
            VisibilityPatternList::List(patterns) => patterns.is_empty(),
            VisibilityPatternList::Intersection(sub_lists) => {
                sub_lists.iter().any(|s| s.is_empty())
            }
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        let list = match self {
            VisibilityPatternList::Public => vec![serde_json::Value::String(
                VisibilityPattern::PUBLIC.to_owned(),
            )],
            VisibilityPatternList::List(patterns) => patterns
                .iter()
                .map(|p| match p {
                    VisibilityPattern::Parsed(p) => serde_json::Value::String(p.to_string()),
                    VisibilityPattern::TargetNameGlob(r) => r.to_json(),
                })
                .collect(),
            VisibilityPatternList::Intersection(sub_lists) => {
                let parts: Vec<serde_json::Value> = sub_lists.iter().map(|s| s.to_json()).collect();
                return serde_json::json!({ "intersection": parts });
            }
        };
        serde_json::Value::Array(list)
    }

    fn extend_with(
        &self,
        other: &VisibilityPatternList,
    ) -> buck2_error::Result<VisibilityPatternList> {
        match (self, other) {
            (VisibilityPatternList::Intersection(_), _)
            | (_, VisibilityPatternList::Intersection(_)) => Err(internal_error!(
                "`extend_with` does not support Intersection"
            )),
            (VisibilityPatternList::Public, _) | (_, VisibilityPatternList::Public) => {
                Ok(VisibilityPatternList::Public)
            }
            (VisibilityPatternList::List(this), VisibilityPatternList::List(other)) => Ok(
                VisibilityPatternList::List(this.iter().chain(other).cloned().collect()),
            ),
        }
    }

    pub fn intersect_with(&self, other: &VisibilityPatternList) -> VisibilityPatternList {
        match (self, other) {
            (VisibilityPatternList::Public, x) | (x, VisibilityPatternList::Public) => x.dupe(),
            _ => Self::intersect_many(vec![self.dupe(), other.dupe()]),
        }
    }

    fn intersect_many(constraints: Vec<VisibilityPatternList>) -> VisibilityPatternList {
        let mut flat: Vec<VisibilityPatternList> = Vec::with_capacity(constraints.len());
        for c in constraints {
            match c {
                VisibilityPatternList::Public => {}
                VisibilityPatternList::List(_) => flat.push(c),
                VisibilityPatternList::Intersection(sub_lists) => {
                    flat.extend(sub_lists.iter().map(Dupe::dupe));
                }
            }
        }
        match flat.len() {
            0 => VisibilityPatternList::Public,
            1 => flat.pop().unwrap(),
            _ => VisibilityPatternList::Intersection(flat.into_iter().collect()),
        }
    }

    fn testing_parse(patterns: &[&str]) -> VisibilityPatternList {
        if patterns.contains(&VisibilityPattern::PUBLIC) {
            VisibilityPatternList::Public
        } else {
            VisibilityPatternList::List(
                patterns
                    .iter()
                    .map(|p| VisibilityPattern::testing_new(p))
                    .collect(),
            )
        }
    }

    /// Whether this list is the intersection identity: `Public` contributes
    /// nothing when layered (`VisibilityIntersection::with_layer` drops such
    /// layers along with their exemptions).
    pub fn is_intersection_identity(&self) -> bool {
        matches!(self, VisibilityPatternList::Public)
    }

    pub fn matches_target(&self, target: &TargetLabel) -> buck2_error::Result<bool> {
        match self {
            VisibilityPatternList::Public => Ok(true),
            VisibilityPatternList::List(patterns) => {
                for pattern in patterns.iter() {
                    let matches = match pattern {
                        VisibilityPattern::Parsed(p) => p.matches(target),
                        VisibilityPattern::TargetNameGlob(r) => r.matches(target)?,
                    };
                    if matches {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            VisibilityPatternList::Intersection(sub_lists) => {
                for s in sub_lists.iter() {
                    if !s.matches_target(target)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
        }
    }

    /// Whether this list matches the package of a loading file, for load
    /// visibility. `target_name_glob(...)` patterns are target-oriented and never
    /// match a bare package, so they are treated as non-matching (they also
    /// cannot be produced by the `visibility()` builtin, which only accepts
    /// strings).
    pub fn matches_package(&self, package: CellPath) -> bool {
        match self {
            VisibilityPatternList::Public => true,
            VisibilityPatternList::List(patterns) => patterns.iter().any(|pattern| match pattern {
                VisibilityPattern::Parsed(p) => p.matches_package(package),
                VisibilityPattern::TargetNameGlob(_) => false,
            }),
            VisibilityPatternList::Intersection(sub_lists) => {
                sub_lists.iter().all(|s| s.matches_package(package))
            }
        }
    }
}

impl Display for VisibilityPatternList {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            VisibilityPatternList::Public => write!(f, "[\"{}\"]", VisibilityPattern::PUBLIC),
            VisibilityPatternList::List(patterns) => display_container::fmt_container(
                f,
                "[",
                "]",
                patterns.iter().map(VisibilityPatternQuoted),
            ),
            VisibilityPatternList::Intersection(sub_lists) => {
                let mut first = true;
                for s in sub_lists.iter() {
                    if !first {
                        write!(f, " AND ")?;
                    }
                    first = false;
                    Display::fmt(s, f)?;
                }
                Ok(())
            }
        }
    }
}

impl AnyMatches for VisibilityPatternList {
    fn any_matches(
        &self,
        filter: &dyn Fn(&str) -> buck2_error::Result<bool>,
    ) -> buck2_error::Result<bool> {
        match self {
            VisibilityPatternList::Public => filter(VisibilityPattern::PUBLIC),
            VisibilityPatternList::List(patterns) => {
                for p in patterns {
                    match p {
                        VisibilityPattern::Parsed(parsed) => {
                            if filter(&parsed.to_string())? {
                                return Ok(true);
                            }
                        }
                        VisibilityPattern::TargetNameGlob(record) => {
                            for g in record.name_globs.iter() {
                                if filter(g.pattern())? {
                                    return Ok(true);
                                }
                            }
                        }
                    }
                }
                Ok(false)
            }
            VisibilityPatternList::Intersection(_) => Err(internal_error!(
                "Intersection not supported in `any_matches`"
            )),
        }
    }
}

/// An exemption skips only this layer, for targets defined in a matching
/// package. It cannot weaken any other layer's restriction, and it never
/// grants consumer visibility.
#[derive(Debug, Eq, PartialEq, Hash, Clone, Dupe, Allocative, Pagable)]
pub struct VisibilityIntersectionLayer {
    pub(crate) patterns: VisibilityPatternList,
    pub(crate) exemptions: ThinArcSlice<PackagePattern>,
    pub(crate) origin: VisibilityLayerOrigin,
}

/// Why a `PACKAGE`'s visibility became an intersection layer.
#[derive(Debug, Eq, PartialEq, Hash, Clone, Copy, Dupe, Allocative, Pagable)]
pub enum VisibilityLayerOrigin {
    /// The `PACKAGE` called `enforce_visibility_intersection()`: enforced in
    /// every mode, including `audit`.
    Marker,
    /// Contributed only because `package_visibility.default_intersection` is
    /// `audit` or `enforce`; `audit` reports rather than fails its blocks.
    Default,
}

impl VisibilityIntersectionLayer {
    pub fn patterns(&self) -> &VisibilityPatternList {
        &self.patterns
    }

    pub fn exemptions(&self) -> &[PackagePattern] {
        &self.exemptions
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "patterns": self.patterns.to_json(),
            "exempt_targets": self
                .exemptions
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        })
    }
}

/// The visibility intersection propagated from `enforce_visibility_intersection()`
/// boundaries (or, when default intersection is enabled, from every explicit
/// `package(visibility=...)`). A target is visible to a consumer only if its
/// own visibility and every non-exempt layer match the consumer.
///
/// Empty layers = no restriction.
#[derive(Debug, Eq, PartialEq, Hash, Clone, Dupe, Allocative, Pagable)]
pub struct VisibilityIntersection {
    layers: ThinArcSlice<VisibilityIntersectionLayer>,
}

impl VisibilityIntersection {
    pub fn unrestricted() -> Self {
        Self {
            layers: ThinArcSlice::empty(),
        }
    }

    pub fn is_unrestricted(&self) -> bool {
        self.layers.is_empty()
    }

    pub fn layers(&self) -> &[VisibilityIntersectionLayer] {
        &self.layers
    }

    /// Intersect one more `PACKAGE`'s explicit visibility into this intersection.
    /// `Public` is the identity and contributes nothing (any exemptions on a
    /// `Public` layer are vacuous and dropped).
    pub fn with_layer(
        &self,
        patterns: VisibilityPatternList,
        exemptions: ThinArcSlice<PackagePattern>,
        origin: VisibilityLayerOrigin,
    ) -> Self {
        if patterns.is_intersection_identity() {
            return self.dupe();
        }
        let mut layers = Vec::with_capacity(self.layers.len() + 1);
        layers.extend(self.layers.iter().cloned());
        layers.push(VisibilityIntersectionLayer {
            patterns,
            exemptions,
            origin,
        });
        Self {
            layers: ThinArcSlice::from_iter(layers),
        }
    }

    /// The layers that apply to targets defined in `defining_package`: layers
    /// exempting it are dropped. Exemptions key off the defining package only;
    /// they never grant a consumer visibility by themselves. Each build file
    /// computes this once for its `Package`, so per-edge checks never consult
    /// exemptions.
    pub fn for_defining_package(&self, defining_package: PackageLabel) -> Self {
        let exempts = |layer: &VisibilityIntersectionLayer| {
            layer
                .exemptions
                .iter()
                .any(|exemption| exemption.matches(defining_package))
        };
        if !self.layers.iter().any(exempts) {
            return self.dupe();
        }
        Self {
            layers: self
                .layers
                .iter()
                .filter(|layer| !exempts(layer))
                .cloned()
                .collect(),
        }
    }

    /// Whether `consumer` passes every layer. Exemptions are not consulted:
    /// call this on the result of
    /// [`for_defining_package`](Self::for_defining_package).
    pub fn matches(&self, consumer: &TargetLabel) -> buck2_error::Result<bool> {
        self.matches_layers(*consumer, |_| true)
    }

    /// Like [`matches`](Self::matches), but only `Marker` layers can block.
    pub fn matches_marker_layers(&self, consumer: &TargetLabel) -> buck2_error::Result<bool> {
        self.matches_layers(*consumer, |layer| {
            layer.origin == VisibilityLayerOrigin::Marker
        })
    }

    fn matches_layers(
        &self,
        consumer: TargetLabel,
        include: impl Fn(&VisibilityIntersectionLayer) -> bool,
    ) -> buck2_error::Result<bool> {
        for layer in self.layers.iter().filter(|layer| include(layer)) {
            if !layer.patterns.matches_target(&consumer)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// One object per layer; never collapses, unlike `to_legacy_json`.
    pub fn to_json(&self) -> serde_json::Value {
        self.layers
            .iter()
            .map(VisibilityIntersectionLayer::to_json)
            .collect()
    }

    /// Legacy collapsed shape, for the `visibility_cap` back-compat attr only:
    /// exemption-free layers collapse exactly as before, so existing output is
    /// unchanged. Exemptions are ignored here (exact for exemption-free
    /// intersections, approximate otherwise); new consumers must read `to_json`.
    pub fn to_legacy_json(&self) -> serde_json::Value {
        let parts = self
            .layers
            .iter()
            .map(|layer| layer.patterns.to_json())
            .collect::<Vec<_>>();
        match parts.as_slice() {
            [] => serde_json::json!([VisibilityPattern::PUBLIC]),
            [single] => single.clone(),
            many => serde_json::json!({ "intersection": many }),
        }
    }
}

struct VisibilityIntersectionLayerQuoted<'a>(&'a VisibilityIntersectionLayer);

struct QuotedExemption<'a>(&'a PackagePattern);

impl Display for QuotedExemption<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "\"{}\"", self.0)
    }
}

impl Display for VisibilityIntersectionLayerQuoted<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0.patterns, f)?;
        if !self.0.exemptions.is_empty() {
            write!(f, " except ")?;
            display_container::fmt_container(
                f,
                "[",
                "]",
                self.0.exemptions.iter().map(QuotedExemption),
            )?;
        }
        Ok(())
    }
}

impl Display for VisibilityIntersection {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.layers.is_empty() {
            return write!(f, "[\"{}\"]", VisibilityPattern::PUBLIC);
        }
        let mut first = true;
        for layer in self.layers.iter() {
            if !first {
                write!(f, " AND ")?;
            }
            first = false;
            Display::fmt(&VisibilityIntersectionLayerQuoted(layer), f)?;
        }
        Ok(())
    }
}

/// Represents the visibility spec of a target. Note that targets in the same package will ignore the
/// visibility spec of each other.
#[derive(
    Debug, Eq, PartialEq, Hash, Clone, Dupe, Allocative, Pagable, StrongHash
)]
pub struct VisibilitySpecification(pub VisibilityPatternList);

impl Default for VisibilitySpecification {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[derive(
    Debug, Eq, PartialEq, Hash, Clone, Dupe, Allocative, Pagable, StrongHash
)]
pub struct WithinViewSpecification(pub VisibilityPatternList);

impl Default for WithinViewSpecification {
    fn default() -> Self {
        Self::PUBLIC
    }
}

impl VisibilitySpecification {
    pub const DEFAULT: VisibilitySpecification =
        VisibilitySpecification(VisibilityPatternList::List(ThinArcSlice::empty()));

    pub fn to_json(&self) -> serde_json::Value {
        self.0.to_json()
    }

    pub fn extend_with(
        &self,
        other: &VisibilitySpecification,
    ) -> buck2_error::Result<VisibilitySpecification> {
        Ok(VisibilitySpecification(self.0.extend_with(&other.0)?))
    }

    pub fn testing_parse(patterns: &[&str]) -> VisibilitySpecification {
        VisibilitySpecification(VisibilityPatternList::testing_parse(patterns))
    }
}

impl Display for VisibilitySpecification {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl WithinViewSpecification {
    pub const PUBLIC: WithinViewSpecification =
        WithinViewSpecification(VisibilityPatternList::Public);

    pub fn extend_with(
        &self,
        other: &WithinViewSpecification,
    ) -> buck2_error::Result<WithinViewSpecification> {
        Ok(WithinViewSpecification(self.0.extend_with(&other.0)?))
    }

    pub fn to_json(&self) -> serde_json::Value {
        self.0.to_json()
    }
}

impl Display for WithinViewSpecification {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

/// The load visibility of a `.bzl` file: the set of packages whose `BUCK`/`.bzl`
/// files may `load()` it, as declared by the `visibility()` builtin. Unlike
/// target visibility this is matched against the *package* of the loading file
/// rather than a target label. When a `.bzl` file never calls `visibility()`
/// there is no `LoadVisibility` at all, which is treated as public (matching
/// Bazel).
#[derive(Debug, Eq, PartialEq, Hash, Clone, Dupe, Allocative, Pagable)]
pub enum LoadVisibility {
    /// `visibility("private")`: loadable only from the same package (enforced
    /// separately by the load-visibility check). Kept distinct from an empty
    /// pattern list, which has the same meaning, so it renders as written.
    Private,
    /// `visibility([...])`, including `[]` and `["PUBLIC"]`.
    Patterns(VisibilityPatternList),
}

impl LoadVisibility {
    pub fn new(list: VisibilityPatternList) -> LoadVisibility {
        LoadVisibility::Patterns(list)
    }

    /// `visibility(["PUBLIC"])` / `visibility("public")`: loadable from anywhere.
    pub fn public() -> LoadVisibility {
        LoadVisibility::Patterns(VisibilityPatternList::Public)
    }

    /// `visibility("private")`: loadable only from the same package.
    pub fn private() -> LoadVisibility {
        LoadVisibility::Private
    }

    /// Whether this is `visibility(["PUBLIC"])` / `visibility("public")`.
    pub fn is_public(&self) -> bool {
        matches!(
            self,
            LoadVisibility::Patterns(VisibilityPatternList::Public)
        )
    }

    /// Whether a file in `package` may load the `.bzl` file that declared this
    /// visibility.
    pub fn matches_package(&self, package: CellPath) -> bool {
        match self {
            LoadVisibility::Private => false,
            LoadVisibility::Patterns(list) => list.matches_package(package),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        match self {
            LoadVisibility::Private => serde_json::json!("private"),
            LoadVisibility::Patterns(list) => list.to_json(),
        }
    }
}

/// Renders the argument as it would be passed to `visibility()`.
impl Display for LoadVisibility {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            LoadVisibility::Private => write!(f, "\"private\""),
            LoadVisibility::Patterns(list) => Display::fmt(list, f),
        }
    }
}

impl AnyMatches for VisibilitySpecification {
    fn any_matches(
        &self,
        filter: &dyn Fn(&str) -> buck2_error::Result<bool>,
    ) -> buck2_error::Result<bool> {
        self.0.any_matches(filter)
    }
}

impl AnyMatches for WithinViewSpecification {
    fn any_matches(
        &self,
        filter: &dyn Fn(&str) -> buck2_error::Result<bool>,
    ) -> buck2_error::Result<bool> {
        self.0.any_matches(filter)
    }
}

pub struct VisibilityWithinViewBuilder {
    cap: usize,
    seen_public: bool,
    patterns: Option<Vec<VisibilityPattern>>,
}

impl VisibilityWithinViewBuilder {
    pub fn with_capacity(cap: usize) -> VisibilityWithinViewBuilder {
        VisibilityWithinViewBuilder {
            cap,
            seen_public: false,
            patterns: None,
        }
    }

    pub fn add_public(&mut self) {
        self.seen_public = true;
    }

    pub fn add(&mut self, pattern: VisibilityPattern) {
        if !self.seen_public {
            self.patterns
                .get_or_insert_with(|| Vec::with_capacity(self.cap))
                .push(pattern);
        }
    }

    fn build_list(self) -> VisibilityPatternList {
        if self.seen_public {
            VisibilityPatternList::Public
        } else {
            VisibilityPatternList::List(ThinArcSlice::from_iter(self.patterns.unwrap_or_default()))
        }
    }

    pub fn build_visibility(self) -> VisibilitySpecification {
        VisibilitySpecification(self.build_list())
    }

    pub fn build_within_view(self) -> WithinViewSpecification {
        let list = self.build_list();
        if list.is_empty() {
            WithinViewSpecification::PUBLIC
        } else {
            WithinViewSpecification(list)
        }
    }
}

#[cfg(test)]
mod tests {
    use buck2_core::pattern::pattern::ParsedPattern;
    use buck2_core::target::label::label::TargetLabel;

    use super::*;

    fn label(s: &str) -> TargetLabel {
        ParsedPattern::testing_parse(s)
            .as_target_label(s)
            .expect("test label parses")
    }

    #[test]
    fn intersect_with_public_is_identity() {
        let list = VisibilityPatternList::testing_parse(&["root//foo:"]);
        assert_eq!(VisibilityPatternList::Public.intersect_with(&list), list);
        assert_eq!(list.intersect_with(&VisibilityPatternList::Public), list);
    }

    #[test]
    fn intersect_with_lists_requires_both_to_match() {
        let a = VisibilityPatternList::testing_parse(&["root//foo:"]);
        let b = VisibilityPatternList::testing_parse(&["root//bar:"]);
        let a_and_b = a.intersect_with(&b);

        assert!(matches!(a_and_b, VisibilityPatternList::Intersection(_)));
        assert!(!a_and_b.matches_target(&label("root//foo:foo")).unwrap());
        assert!(!a_and_b.matches_target(&label("root//bar:bar")).unwrap());

        let overlap = VisibilityPatternList::testing_parse(&["root//foo:", "root//bar:"]);
        assert!(
            overlap
                .intersect_with(&a)
                .matches_target(&label("root//foo:foo"))
                .unwrap()
        );
        assert!(
            !overlap
                .intersect_with(&a)
                .matches_target(&label("root//bar:bar"))
                .unwrap()
        );
    }

    #[test]
    fn intersect_many_matches_all_constraints() {
        let a = VisibilityPatternList::testing_parse(&["root//baz:", "root//foo:"]);
        let b = VisibilityPatternList::testing_parse(&["root//baz:", "root//bar:"]);
        let c = VisibilityPatternList::testing_parse(&["root//baz:"]);
        let intersection = a.intersect_with(&b).intersect_with(&c);

        assert!(
            intersection
                .matches_target(&label("root//baz:baz"))
                .unwrap()
        );
        assert!(
            !intersection
                .matches_target(&label("root//foo:foo"))
                .unwrap()
        );
        assert!(
            !intersection
                .matches_target(&label("root//bar:bar"))
                .unwrap()
        );
    }

    fn pkg(s: &str) -> buck2_core::cells::cell_path::CellPath {
        label(s).pkg().as_cell_path().to_owned()
    }

    #[test]
    fn matches_package_public_and_private() {
        assert!(VisibilityPatternList::Public.matches_package(pkg("root//anywhere:x")));

        // Empty list is private: matches nothing.
        let private = VisibilityPatternList::List(ThinArcSlice::empty());
        assert!(!private.matches_package(pkg("root//anywhere:x")));
    }

    #[test]
    fn matches_package_recursive_and_exact() {
        let list = VisibilityPatternList::testing_parse(&["root//foo/...", "root//bar:"]);
        // `//foo/...` matches the package itself and its subpackages.
        assert!(list.matches_package(pkg("root//foo:x")));
        assert!(list.matches_package(pkg("root//foo/sub:x")));
        // `//bar:` matches only that exact package, not subpackages.
        assert!(list.matches_package(pkg("root//bar:x")));
        assert!(!list.matches_package(pkg("root//bar/sub:x")));
        // Unrelated package does not match.
        assert!(!list.matches_package(pkg("root//other:x")));
    }

    #[test]
    fn matches_package_target_pattern_never_matches() {
        // A `//foo:bar` target pattern names a target, not a package.
        let list = VisibilityPatternList::testing_parse(&["root//foo:bar"]);
        assert!(!list.matches_package(pkg("root//foo:anything")));
    }

    #[test]
    fn load_visibility_public_and_private() {
        assert!(LoadVisibility::public().matches_package(pkg("root//anywhere:x")));
        assert!(!LoadVisibility::private().matches_package(pkg("root//anywhere:x")));
    }

    #[test]
    fn load_visibility_display() {
        assert_eq!(LoadVisibility::private().to_string(), "\"private\"");
        assert_eq!(LoadVisibility::public().to_string(), "[\"PUBLIC\"]");
        assert_eq!(
            LoadVisibility::new(VisibilityPatternList::List(ThinArcSlice::empty())).to_string(),
            "[]"
        );
    }

    #[test]
    fn extend_with_intersection_returns_internal_error() {
        let a = VisibilityPatternList::testing_parse(&["root//foo:"]);
        let b = VisibilityPatternList::testing_parse(&["root//bar:"]);
        let intersection = VisibilityPatternList::Intersection([a, b].into_iter().collect());
        let public = VisibilityPatternList::Public;

        assert!(public.extend_with(&intersection).is_err());
        assert!(intersection.extend_with(&public).is_err());
    }

    #[test]
    fn visibility_record_matches_unscoped_target_names() {
        let list = VisibilityPatternList::List(
            [VisibilityPattern::testing_new_record(&[], &["*-test"])]
                .into_iter()
                .collect(),
        );

        assert!(list.matches_target(&label("root//foo:unit-test")).unwrap());
        assert!(list.matches_target(&label("root//bar:unit-test")).unwrap());
        // Negative: full-match semantics — `-testfoo` ends after `-test`, so
        // suffix `-test` must terminate the string, not appear in the middle.
        assert!(
            !list
                .matches_target(&label("root//foo:unit-testfoo"))
                .unwrap()
        );
        assert!(!list.matches_target(&label("root//foo:library")).unwrap());
    }

    #[test]
    fn visibility_record_matches_package_scope() {
        let list = VisibilityPatternList::List(
            [VisibilityPattern::testing_new_record(
                &["root//foo:"],
                &["*-test"],
            )]
            .into_iter()
            .collect(),
        );

        assert!(list.matches_target(&label("root//foo:unit-test")).unwrap());
        // Negative: glob would match if scope ignored, but package mismatch rules it out.
        assert!(
            !list
                .matches_target(&label("root//foo:unit-testfoo"))
                .unwrap()
        );
        assert!(
            !list
                .matches_target(&label("root//foo/sub:unit-test"))
                .unwrap()
        );
        assert!(!list.matches_target(&label("root//bar:unit-test")).unwrap());
    }

    #[test]
    fn visibility_record_matches_recursive_scope() {
        let list = VisibilityPatternList::List(
            [VisibilityPattern::testing_new_record(
                &["root//foo/..."],
                &["*-test"],
            )]
            .into_iter()
            .collect(),
        );

        assert!(list.matches_target(&label("root//foo:unit-test")).unwrap());
        assert!(
            list.matches_target(&label("root//foo/sub:unit-test"))
                .unwrap()
        );
        // Negative: full-match within the recursive scope.
        assert!(
            !list
                .matches_target(&label("root//foo:unit-testfoo"))
                .unwrap()
        );
        assert!(!list.matches_target(&label("root//bar:unit-test")).unwrap());
    }

    #[test]
    fn visibility_record_matches_any_of_multiple_globs() {
        // The list semantics is "any glob matches" — both `*-test` and
        // `*-deprecated` count as hits, but unrelated names do not.
        let list = VisibilityPatternList::List(
            [VisibilityPattern::testing_new_record(
                &[],
                &["*-test", "*-deprecated"],
            )]
            .into_iter()
            .collect(),
        );

        assert!(list.matches_target(&label("root//foo:unit-test")).unwrap());
        assert!(
            list.matches_target(&label("root//bar:legacy-deprecated"))
                .unwrap()
        );
        // Negative: neither glob matches.
        assert!(!list.matches_target(&label("root//foo:library")).unwrap());
        // Negative: full-match — the `-test` glob must end the string.
        assert!(
            !list
                .matches_target(&label("root//foo:unit-testfoo"))
                .unwrap()
        );
    }

    #[test]
    fn visibility_record_intersection_requires_all_constraints() {
        let glob = VisibilityPatternList::List(
            [VisibilityPattern::testing_new_record(&[], &["*-test"])]
                .into_iter()
                .collect(),
        );
        let package = VisibilityPatternList::testing_parse(&["root//foo:"]);
        let intersection = VisibilityIntersection::unrestricted()
            .with_layer(glob, ThinArcSlice::empty(), VisibilityLayerOrigin::Default)
            .with_layer(
                package,
                ThinArcSlice::empty(),
                VisibilityLayerOrigin::Default,
            );

        assert!(intersection.matches(&label("root//foo:unit-test")).unwrap());
        assert!(!intersection.matches(&label("root//foo:library")).unwrap());
        assert!(!intersection.matches(&label("root//bar:unit-test")).unwrap());
    }

    #[test]
    fn target_name_glob_record_serializes_empty_within() {
        let VisibilityPattern::TargetNameGlob(record) =
            VisibilityPattern::testing_new_record(&[], &["*-test"])
        else {
            panic!("testing_new_record builds a Record");
        };

        assert_eq!(
            record.to_json(),
            serde_json::json!({
                "__type": "target_name_glob",
                "name_globs": ["*-test"],
                "within": [],
            })
        );
    }

    #[test]
    fn starlark_target_name_glob_rejects_empty_list() {
        let err = StarlarkTargetNameGlob::try_new(vec![], vec![]).expect_err("empty list rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("at least one entry"),
            "expected empty-list error, got: {msg}"
        );
    }

    #[test]
    fn starlark_target_name_glob_repr_escapes_within() {
        // The raw `within` strings are unvalidated user input (coercion happens
        // later), so `repr`/`Display` must emit a properly escaped Starlark
        // string literal — not Rust's `Debug` form.
        let value =
            StarlarkTargetNameGlob::try_new(vec!["*-test".to_owned()], vec!["a\"b".to_owned()])
                .expect("valid globs");
        let repr = value.to_string();
        assert!(
            repr.contains(r#"["a\"b"]"#),
            "within must be escaped as a Starlark string literal, got: {repr}"
        );
    }

    #[test]
    fn package_pattern_recursive_cell_root_round_trips() {
        // Cell-root recursive `within` must render as `root//...` (the empty-path
        // branch of `PackagePattern`'s `Display`).
        let VisibilityPattern::TargetNameGlob(record) =
            VisibilityPattern::testing_new_record(&["root//..."], &["*-test"])
        else {
            panic!("testing_new_record builds a Record");
        };
        assert_eq!(
            record.to_json(),
            serde_json::json!({
                "__type": "target_name_glob",
                "name_globs": ["*-test"],
                "within": ["root//..."],
            })
        );
    }

    #[test]
    fn within_view_preserves_single_record() {
        // A `within_view` holding only a `target_name_glob` must NOT collapse to
        // PUBLIC — only a genuinely empty list does.
        let mut builder = VisibilityWithinViewBuilder::with_capacity(1);
        builder.add(VisibilityPattern::testing_new_record(&[], &["*-test"]));
        let within_view = builder.build_within_view();
        assert!(
            matches!(within_view.0, VisibilityPatternList::List(_)),
            "single-record within_view must stay a List, not become Public"
        );
        assert!(
            within_view
                .0
                .matches_target(&label("root//foo:unit-test"))
                .unwrap()
        );
    }

    #[test]
    fn extend_with_carries_records() {
        // Extending a list that contains a `Record` must carry the Record through.
        let globs = VisibilityPatternList::List(
            [VisibilityPattern::testing_new_record(&[], &["*-test"])]
                .into_iter()
                .collect(),
        );
        let other = VisibilityPatternList::testing_parse(&["root//foo:"]);
        let extended = globs.extend_with(&other).unwrap();

        // The Record (name glob) and the plain pattern both still match.
        assert!(
            extended
                .matches_target(&label("root//bar:unit-test"))
                .unwrap()
        );
        assert!(extended.matches_target(&label("root//foo:lib")).unwrap());
        // Negative: neither matches.
        assert!(!extended.matches_target(&label("root//bar:lib")).unwrap());
    }

    #[test]
    fn visibility_record_matches_any_of_multiple_within_scopes() {
        // `within` is OR-ed: a target matches if its package is in ANY listed
        // scope (and its name matches a glob).
        let list = VisibilityPatternList::List(
            [VisibilityPattern::testing_new_record(
                &["root//foo:", "root//bar/..."],
                &["*-test"],
            )]
            .into_iter()
            .collect(),
        );

        // First scope (exact package).
        assert!(list.matches_target(&label("root//foo:unit-test")).unwrap());
        // Second scope (recursive prefix).
        assert!(
            list.matches_target(&label("root//bar/sub:unit-test"))
                .unwrap()
        );
        // Negative: name matches a glob but package is in neither scope.
        assert!(!list.matches_target(&label("root//baz:unit-test")).unwrap());
        // Negative: package is in a scope but the name matches no glob.
        assert!(!list.matches_target(&label("root//foo:library")).unwrap());
    }

    #[test]
    fn starlark_target_name_glob_serializes_within_as_list() {
        let VisibilityPattern::TargetNameGlob(record) =
            VisibilityPattern::testing_new_record(&["root//foo:", "root//bar/..."], &["*-test"])
        else {
            panic!("testing_new_record builds a Record");
        };

        assert_eq!(
            record.to_json(),
            serde_json::json!({
                "__type": "target_name_glob",
                "name_globs": ["*-test"],
                "within": ["root//foo:", "root//bar/..."],
            })
        );
    }

    #[test]
    fn starlark_target_name_glob_coerce_resolves_within() {
        let value = StarlarkTargetNameGlob::try_new(
            vec!["*-test".to_owned()],
            vec!["root//foo:".to_owned(), "root//bar/...".to_owned()],
        )
        .expect("valid globs");

        let record = value
            .coerce(|s| Ok(ParsedPattern::testing_parse(s)))
            .expect("within scopes resolve");

        // OR over the two resolved scopes (plus the name glob).
        assert!(record.matches(&label("root//foo:unit-test")).unwrap());
        assert!(record.matches(&label("root//bar/sub:unit-test")).unwrap());
        // Negative: name matches but package is outside both scopes.
        assert!(!record.matches(&label("root//baz:unit-test")).unwrap());
        // Negative: package in scope but name matches no glob.
        assert!(!record.matches(&label("root//foo:library")).unwrap());
    }

    #[test]
    fn starlark_target_name_glob_coerce_rejects_target_within() {
        let value = StarlarkTargetNameGlob::try_new(
            vec!["*-test".to_owned()],
            // A target pattern (not package/recursive) is not a valid `within`.
            vec!["root//foo:bar".to_owned()],
        )
        .expect("valid globs");

        let err = value
            .coerce(|s| Ok(ParsedPattern::testing_parse(s)))
            .expect_err("target pattern in within must be rejected");
        assert!(
            format!("{err:#}").contains("expected a package pattern"),
            "expected InvalidWithinScope error, got: {err:#}"
        );
    }

    #[test]
    fn starlark_target_name_glob_coerce_rejects_public_within() {
        // `PUBLIC` is a valid target *name* (so it is fine in `name_globs`), but
        // it is not a valid `within` scope and must be rejected with a clear
        // message rather than a generic parse error.
        let value =
            StarlarkTargetNameGlob::try_new(vec!["*-test".to_owned()], vec!["PUBLIC".to_owned()])
                .expect("valid globs");

        let err = value
            .coerce(|s| Ok(ParsedPattern::testing_parse(s)))
            .expect_err("`PUBLIC` must be rejected as a within scope");
        assert!(
            format!("{err:#}").contains("`PUBLIC` is not a valid `within`"),
            "expected PublicNotAllowedInWithin error, got: {err:#}"
        );
    }

    #[test]
    fn starlark_target_name_glob_repr_renders_name_globs_positionally() {
        // `name_globs` is positional-only in the constructor, so the repr must
        // render it positionally (no `name_globs = `) to round-trip.
        let value = StarlarkTargetNameGlob::try_new(
            vec!["*-test".to_owned()],
            vec!["root//foo:".to_owned()],
        )
        .expect("valid globs");
        assert_eq!(
            value.to_string(),
            r#"target_name_glob(["*-test"], within = ["root//foo:"])"#,
        );
    }

    #[test]
    fn any_matches_yields_name_globs_not_rendered_call() {
        // `any_matches` backs `attrfilter(visibility, ...)`; it must yield the
        // constituent `name_globs` so `*-test` finds a glob entry, not the
        // rendered `target_name_glob(...)` call string.
        let list = VisibilityPatternList::List(
            [VisibilityPattern::testing_new_record(&[], &["*-test"])]
                .into_iter()
                .collect(),
        );
        // Positive: filter on the glob pattern finds the entry.
        assert!(
            list.any_matches(&|s| Ok(s == "*-test")).unwrap(),
            "any_matches must yield name_globs"
        );
        // Negative: filter on the rendered call must not be required.
        assert!(
            !list
                .any_matches(&|s| Ok(s == "target_name_glob([\"*-test\"])"))
                .unwrap(),
            "any_matches must not require the rendered call string"
        );
    }

    #[test]
    fn any_matches_yields_parsed_pattern() {
        let list = VisibilityPatternList::testing_parse(&["root//foo:"]);
        assert!(list.any_matches(&|s| Ok(s == "root//foo:")).unwrap(),);
        assert!(!list.any_matches(&|s| Ok(s == "root//bar:")).unwrap(),);
    }

    fn exempt(s: &str) -> PackagePattern {
        within_scope_from_parsed(ParsedPattern::testing_parse(s))
            .unwrap_or_else(|_| panic!("test exemption must be a package pattern, got: {s}"))
    }

    fn exempt_slice(patterns: &[&str]) -> ThinArcSlice<PackagePattern> {
        patterns.iter().map(|s| exempt(s)).collect()
    }

    fn single_layer_intersection(patterns: &[&str], exemptions: &[&str]) -> VisibilityIntersection {
        VisibilityIntersection::unrestricted().with_layer(
            VisibilityPatternList::testing_parse(patterns),
            exempt_slice(exemptions),
            VisibilityLayerOrigin::Default,
        )
    }

    fn defining_package(target: &str) -> PackageLabel {
        label(target).pkg()
    }

    #[test]
    fn visibility_intersection_public_matches_everything() {
        let intersection = VisibilityIntersection::unrestricted();
        assert!(intersection.is_unrestricted());
        assert!(
            intersection
                .for_defining_package(defining_package("root//other:thing"))
                .matches(&label("root//any:thing"))
                .unwrap()
        );
        assert_eq!(intersection.to_json(), serde_json::json!([]));
        assert_eq!(intersection.to_legacy_json(), serde_json::json!(["PUBLIC"]));
        assert_eq!(intersection.to_string(), r#"["PUBLIC"]"#);
    }

    #[test]
    fn visibility_intersection_with_layer_public_is_identity() {
        let intersection = VisibilityIntersection::unrestricted().with_layer(
            VisibilityPatternList::Public,
            exempt_slice(&["root//foo:"]),
            VisibilityLayerOrigin::Default,
        );
        assert!(intersection.is_unrestricted());
        assert_eq!(intersection, VisibilityIntersection::unrestricted());
    }

    #[test]
    fn visibility_intersection_marker_layers_ignore_default_layers() {
        let intersection = VisibilityIntersection::unrestricted()
            .with_layer(
                VisibilityPatternList::testing_parse(&["root//a/..."]),
                ThinArcSlice::empty(),
                VisibilityLayerOrigin::Marker,
            )
            .with_layer(
                VisibilityPatternList::testing_parse(&["root//a/b/..."]),
                ThinArcSlice::empty(),
                VisibilityLayerOrigin::Default,
            );
        let default_blocked = label("root//a/c:consumer");
        assert!(!intersection.matches(&default_blocked).unwrap());
        assert!(
            intersection
                .matches_marker_layers(&default_blocked)
                .unwrap()
        );
        let marker_blocked = label("root//other:consumer");
        assert!(!intersection.matches_marker_layers(&marker_blocked).unwrap());
    }

    #[test]
    fn visibility_intersection_layer_blocks_non_matching_consumer() {
        let intersection = single_layer_intersection(&["root//allowed/..."], &[]);
        assert!(!intersection.is_unrestricted());
        assert!(
            intersection
                .for_defining_package(defining_package("root//etc:lib"))
                .matches(&label("root//allowed:lib"))
                .unwrap()
        );
        assert!(
            !intersection
                .for_defining_package(defining_package("root//etc:lib"))
                .matches(&label("root//other:lib"))
                .unwrap()
        );
    }

    #[test]
    fn visibility_intersection_exact_exemption_skips_only_matching_definer() {
        let intersection =
            single_layer_intersection(&["root//allowed/..."], &["root//etc/legacy:"]);
        // Exempt definer skips the layer, even for an otherwise-blocked consumer.
        assert!(
            intersection
                .for_defining_package(defining_package("root//etc/legacy:lib"))
                .matches(&label("root//other:lib"))
                .unwrap()
        );
        assert!(
            !intersection
                .for_defining_package(defining_package("root//etc/other:lib"))
                .matches(&label("root//other:lib"))
                .unwrap()
        );
        assert!(
            intersection
                .for_defining_package(defining_package("root//etc/other:lib"))
                .matches(&label("root//allowed:lib"))
                .unwrap()
        );
    }

    #[test]
    fn visibility_intersection_recursive_exemption_covers_nested_packages() {
        let intersection =
            single_layer_intersection(&["root//allowed/..."], &["root//etc/generated/..."]);
        assert!(
            intersection
                .for_defining_package(defining_package("root//etc/generated:lib"))
                .matches(&label("root//other:lib"))
                .unwrap()
        );
        assert!(
            intersection
                .for_defining_package(defining_package("root//etc/generated/sub:lib"))
                .matches(&label("root//other:lib"))
                .unwrap()
        );
        assert!(
            !intersection
                .for_defining_package(defining_package("root//etc/handwritten:lib"))
                .matches(&label("root//other:lib"))
                .unwrap()
        );
    }

    #[test]
    fn visibility_intersection_exemption_cannot_weaken_other_layers() {
        let intersection = single_layer_intersection(&["root//allowed/..."], &[]).with_layer(
            VisibilityPatternList::testing_parse(&["root//extra/..."]),
            exempt_slice(&["root//etc/sub:"]),
            VisibilityLayerOrigin::Default,
        );
        // Exempt from the second layer only: the first still blocks outsiders.
        assert!(
            !intersection
                .for_defining_package(defining_package("root//etc/sub:lib"))
                .matches(&label("root//other:lib"))
                .unwrap()
        );
        assert!(
            intersection
                .for_defining_package(defining_package("root//etc/sub:lib"))
                .matches(&label("root//allowed:lib"))
                .unwrap()
        );
    }

    #[test]
    fn visibility_intersection_deep_nesting_stays_flat_and_conjunctive() {
        let intersection = single_layer_intersection(&["root//a/..."], &[])
            .with_layer(
                VisibilityPatternList::testing_parse(&["root//a/b/..."]),
                ThinArcSlice::empty(),
                VisibilityLayerOrigin::Default,
            )
            .with_layer(
                VisibilityPatternList::testing_parse(&["root//a/b/c/..."]),
                exempt_slice(&["root//a/b/c/legacy:"]),
                VisibilityLayerOrigin::Default,
            );
        assert!(
            intersection
                .for_defining_package(defining_package("root//a/b/c:lib"))
                .matches(&label("root//a/b/c:lib"))
                .unwrap()
        );
        // Outside the middle layer: blocked even though the outer allows.
        assert!(
            !intersection
                .for_defining_package(defining_package("root//a/b/c:lib"))
                .matches(&label("root//a/other:lib"))
                .unwrap()
        );
        // Exempt from the innermost layer only: outer layers still block.
        assert!(
            !intersection
                .for_defining_package(defining_package("root//a/b/c/legacy:lib"))
                .matches(&label("root//elsewhere:lib"))
                .unwrap()
        );
        assert!(
            intersection
                .for_defining_package(defining_package("root//a/b/c/legacy:lib"))
                .matches(&label("root//a/b/c:lib"))
                .unwrap()
        );
    }

    #[test]
    fn visibility_intersection_legacy_json_preserves_legacy_shapes() {
        assert_eq!(
            VisibilityIntersection::unrestricted().to_legacy_json(),
            serde_json::json!(["PUBLIC"])
        );
        assert_eq!(
            single_layer_intersection(&["root//some_dir/..."], &[]).to_legacy_json(),
            serde_json::json!(["root//some_dir/..."])
        );
        assert_eq!(
            single_layer_intersection(&["root//a/..."], &[])
                .with_layer(
                    VisibilityPatternList::testing_parse(&["root//b/..."]),
                    ThinArcSlice::empty(),
                    VisibilityLayerOrigin::Default
                )
                .to_legacy_json(),
            serde_json::json!({"intersection": [["root//a/..."], ["root//b/..."]]})
        );
    }

    #[test]
    fn visibility_intersection_to_json_carries_exemptions() {
        assert_eq!(
            single_layer_intersection(
                &["root//allowed/..."],
                &["root//etc/legacy:", "root//etc/generated/..."]
            )
            .to_json(),
            serde_json::json!([{
                "patterns": ["root//allowed/..."],
                "exempt_targets": ["root//etc/legacy:", "root//etc/generated/..."],
            }])
        );
    }

    #[test]
    fn visibility_intersection_to_json_is_uniform_per_layer() {
        assert_eq!(
            single_layer_intersection(&["root//a/..."], &[]).to_json(),
            serde_json::json!([{
                "patterns": ["root//a/..."],
                "exempt_targets": [],
            }])
        );
        assert_eq!(
            single_layer_intersection(&["root//a/..."], &[])
                .with_layer(
                    VisibilityPatternList::testing_parse(&["root//b/..."]),
                    ThinArcSlice::empty(),
                    VisibilityLayerOrigin::Default
                )
                .to_json(),
            serde_json::json!([
                {"patterns": ["root//a/..."], "exempt_targets": []},
                {"patterns": ["root//b/..."], "exempt_targets": []},
            ])
        );
    }

    #[test]
    fn visibility_intersection_display_renders_exemptions() {
        let intersection =
            single_layer_intersection(&["root//allowed/..."], &["root//etc/legacy:"]).with_layer(
                VisibilityPatternList::testing_parse(&["root//extra/..."]),
                ThinArcSlice::empty(),
                VisibilityLayerOrigin::Default,
            );
        assert_eq!(
            intersection.to_string(),
            r#"["root//allowed/..."] except ["root//etc/legacy:"] AND ["root//extra/..."]"#,
        );
    }

    #[test]
    fn visibility_intersection_display_lists_every_exemption() {
        let intersection = single_layer_intersection(
            &["root//allowed/..."],
            &["root//etc/legacy:", "root//etc/generated/..."],
        );
        assert_eq!(
            intersection.to_string(),
            r#"["root//allowed/..."] except ["root//etc/legacy:", "root//etc/generated/..."]"#,
        );
    }

    #[test]
    fn visibility_intersection_legacy_json_ignores_exemptions() {
        assert_eq!(
            single_layer_intersection(&["root//allowed/..."], &["root//etc/legacy:"])
                .to_legacy_json(),
            serde_json::json!(["root//allowed/..."])
        );
    }

    #[test]
    fn visibility_intersection_empty_layer_blocks_everyone() {
        let intersection = single_layer_intersection(&[], &[]);
        assert!(
            !intersection
                .for_defining_package(defining_package("root//any:lib"))
                .matches(&label("root//any:lib"))
                .unwrap()
        );
    }
}
