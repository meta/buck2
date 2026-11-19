/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;

use allocative::Allocative;
use buck2_hash::BuckFixedInsertionMap;
use buck2_hash::BuckHasher;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::index_map::Entry;
use buck2_util::size_assert;
use derive_more::Display;
use dupe::Dupe;
use pagable::Pagable;
use pagable::PagableDeserialize;
use pagable::PagableDeserializer;
use pagable::PagableSerialize;
use pagable::PagableSerializer;
use static_interner::Intern;
use static_interner::interner;

use crate::bzl_global::BzlGlobalId;
use crate::cells::cell_path::CellPath;
use crate::target::label::label::TargetLabel;

#[derive(
    Clone,
    Dupe,
    Debug,
    Display,
    Eq,
    PartialEq,
    Hash,
    Ord,
    PartialOrd,
    Allocative,
    strong_hash::StrongHash,
    Pagable
)]
pub struct PluginKind(BzlGlobalId);

impl PluginKind {
    /// Creates a new `PluginKind` instance.
    pub fn new(name: String, cell: CellPath) -> Self {
        Self(BzlGlobalId::new(name, cell))
    }

    pub fn as_str(&self) -> &str {
        self.0.name()
    }
}

/// This type is pretty tailor-made for storing the values of `pulls_plugins` and
/// `pulls_and_pushes_plugins` on `attrs.dep()`.
///
/// It stores a list of plugin kinds and a bool for each indicating whether that kind is pushed in
/// addition to pulled.
#[derive(Copy, Clone, Dupe)]
pub struct PluginKindSet(*const ());

impl PagableSerialize for PluginKindSet {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> pagable::Result<()> {
        self.unpack().pagable_serialize(serializer)
    }
}

impl<'de> PagableDeserialize<'de> for PluginKindSet {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        Ok(PluginKindSet::pack(
            PluginKindSetUnpacked::pagable_deserialize(deserializer)?,
        ))
    }
}

/// We'd ideally like to just let this type be the definition of `PluginKindSet`. Unfortunately,
/// this type is 16 bytes in size. So instead, we store `PluginKindSet` as a pointer with `0`
/// indicating `None` and `1` indicating `All`
#[derive(
    Clone,
    Dupe,
    Debug,
    Eq,
    PartialEq,
    Hash,
    Ord,
    PartialOrd,
    Allocative,
    strong_hash::StrongHash,
    Pagable
)]
enum PluginKindSetUnpacked {
    None,
    All,
    Interned(Intern<PluginKindSetData>),
}

#[derive(
    Clone,
    Debug,
    Eq,
    PartialEq,
    Hash,
    Ord,
    PartialOrd,
    Allocative,
    strong_hash::StrongHash,
    Pagable
)]
struct PluginKindSetData(Vec<(PluginKind, bool)>);

size_assert::words_of_type!(PluginKindSet, 1);
size_assert::words_of_type!(PluginKindSetUnpacked, 2);

interner!(
    PLUGIN_KIND_SET_INTERNER,
    BuckHasher,
    PluginKindSetData,
    Vec<(PluginKind, bool)>,
    [(PluginKind, bool)]
);

impl PluginKindSet {
    pub const EMPTY: Self = Self::pack(PluginKindSetUnpacked::None);
    pub const ALL: Self = Self::pack(PluginKindSetUnpacked::All);

    pub fn new(
        pulls: Vec<PluginKind>,
        pulls_and_pushes: Vec<PluginKind>,
    ) -> buck2_error::Result<Self> {
        if pulls.is_empty() && pulls_and_pushes.is_empty() {
            return Ok(Self::EMPTY);
        }

        let mut kinds = BTreeMap::new();
        for kind in pulls {
            kinds.insert(kind, false);
        }
        for kind in pulls_and_pushes {
            kinds.insert(kind, true);
        }
        let kinds = kinds.into_iter().collect::<Vec<_>>();

        Ok(Self::pack(PluginKindSetUnpacked::Interned(
            PLUGIN_KIND_SET_INTERNER.intern(PluginKindSetData(kinds)),
        )))
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::EMPTY
    }

    pub fn get(&self, kind: &PluginKind) -> Option<bool> {
        match self.unpack() {
            PluginKindSetUnpacked::None => None,
            PluginKindSetUnpacked::All => Some(true),
            PluginKindSetUnpacked::Interned(i) => i
                .iter()
                .find_map(|(k, v)| if k == kind { Some(*v) } else { None }),
        }
    }

    fn unpack(self) -> PluginKindSetUnpacked {
        if self.0 as usize == 0 {
            PluginKindSetUnpacked::None
        } else if self.0 as usize == 1 {
            PluginKindSetUnpacked::All
        } else {
            // SAFETY: Instances of this type are only created by `pack`
            PluginKindSetUnpacked::Interned(unsafe { Intern::from_ptr(self.0.cast()) })
        }
    }

    const fn pack(unpacked: PluginKindSetUnpacked) -> Self {
        match unpacked {
            PluginKindSetUnpacked::None => PluginKindSet(std::ptr::null::<()>()),
            PluginKindSetUnpacked::All => PluginKindSet(std::ptr::dangling::<()>()),
            PluginKindSetUnpacked::Interned(i) => PluginKindSet(i.as_ptr().cast()),
        }
    }
}

impl std::fmt::Debug for PluginKindSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.unpack(), f)
    }
}

impl PartialEq<PluginKindSet> for PluginKindSet {
    fn eq(&self, other: &Self) -> bool {
        PartialEq::eq(&self.unpack(), &other.unpack())
    }
}

impl Eq for PluginKindSet {}

impl std::hash::Hash for PluginKindSet {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::hash::Hash::hash(&self.unpack(), state)
    }
}

impl strong_hash::StrongHash for PluginKindSet {
    fn strong_hash<H: std::hash::Hasher>(&self, state: &mut H) {
        strong_hash::StrongHash::strong_hash(&self.unpack(), state)
    }
}

impl PartialOrd for PluginKindSet {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PluginKindSet {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        Ord::cmp(&self.unpack(), &other.unpack())
    }
}

impl Allocative for PluginKindSet {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        Allocative::visit(&self.unpack(), visitor)
    }
}

// SAFETY: `PluginKindSet` is a trivial wrapper only changing the representation
unsafe impl Sync for PluginKindSet where PluginKindSetUnpacked: Sync {}
unsafe impl Send for PluginKindSet where PluginKindSetUnpacked: Send {}

/// Elements in the plugin list come in three kinds: Either they appear as direct `plugin_dep`s on
/// the rule, or they arrive indirectly and may or may not need to be propagated.
///
/// Note that the `Ord` impl on this type is semantically meaningful - larger values indicate
/// stronger kinds of membership, and when a plugin enters the plugin lists twice the larger of the
/// two values is preferred.
#[derive(
    Copy, Clone, Dupe, Debug, Display, Eq, PartialEq, Hash, Ord, PartialOrd, Allocative, Pagable
)]
pub enum PluginListElemKind {
    NoPropagate,
    Propagate,
    Direct,
}

/// The plugins a configured target carries, by kind.
///
/// Within a kind, plugins are in the order dependency gathering met them; that order is what the
/// `plugins` attribute and analysis's `ctx.plugins` show.
// TODO(JakobDegen): Representation with fewer allocations
#[derive(Clone, Debug, Default, Eq, PartialEq, Hash, Allocative, Pagable)]
pub struct PluginLists(
    BTreeMap<PluginKind, BuckFixedInsertionMap<TargetLabel, PluginListElemKind>>,
);

impl PluginLists {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&PluginKind, &TargetLabel, &PluginListElemKind)> {
        self.0
            .iter()
            .flat_map(|(k, v)| v.iter().map(move |t| (k, t.0, t.1)))
    }

    pub fn iter_by_kind(
        &self,
    ) -> impl Iterator<
        Item = (
            &PluginKind,
            impl Iterator<Item = (&TargetLabel, &PluginListElemKind)>,
        ),
    > {
        self.0.iter().map(|(k, v)| (k, v.iter()))
    }

    pub fn iter_for_kind(
        &self,
        kind: &PluginKind,
    ) -> impl Iterator<Item = (&TargetLabel, &PluginListElemKind)> + use<'_> {
        self.0.get(kind).into_iter().flatten()
    }
}

/// Accumulates a target's [`PluginLists`] while its dependencies are gathered.
#[derive(Default)]
pub struct PluginListsBuilder(
    BTreeMap<PluginKind, BuckMutInsertionMap<TargetLabel, PluginListElemKind>>,
);

impl PluginListsBuilder {
    pub fn new() -> Self {
        Default::default()
    }

    /// Adds a plugin; a plugin already present keeps the stronger of its two membership kinds.
    pub fn insert(&mut self, kind: PluginKind, target: TargetLabel, elem_kind: PluginListElemKind) {
        match self.0.entry(kind).or_default().entry(target) {
            Entry::Occupied(mut occupied) => {
                let current = occupied.get_mut();
                if *current < elem_kind {
                    *current = elem_kind;
                }
            }
            Entry::Vacant(vacant) => {
                vacant.insert(elem_kind);
            }
        }
    }

    pub fn freeze(self) -> PluginLists {
        PluginLists(self.0.into_iter().map(|(k, v)| (k, v.freeze())).collect())
    }
}

#[cfg(test)]
mod tests {
    use std::hash::DefaultHasher;
    use std::hash::Hash;
    use std::hash::Hasher;

    use allocative::Allocative;
    use buck2_hash::BuckHasher;
    use derive_more::Display;
    use pagable::Pagable;
    use pagable::PagableDeserialize;
    use pagable::PagableSerialize;
    use pagable::testing::TestingDeserializer;
    use pagable::testing::TestingSerializer;
    use static_interner::Intern;
    use static_interner::interner;
    use strong_hash::StrongHash;

    use super::PluginKind;
    use crate::cells::cell_path::CellPath;

    /// A `PluginKind` defined directly over its own interned `{ name, cell }`, which
    /// `PluginKind` must stay indistinguishable from.
    #[derive(
        Clone,
        Pagable,
        strong_hash::StrongHash,
        Debug,
        Display,
        Eq,
        PartialEq,
        Hash,
        Ord,
        PartialOrd,
        Allocative
    )]
    #[display("{name}")]
    struct DirectInner {
        name: String,
        cell: CellPath,
    }

    impl<'a> From<&'a DirectInner> for DirectInner {
        fn from(x: &'a DirectInner) -> Self {
            x.clone()
        }
    }

    interner!(DIRECT_INTERNER, BuckHasher, DirectInner);

    fn direct(name: &str, cell: &str) -> Intern<DirectInner> {
        DIRECT_INTERNER.intern(DirectInner {
            name: name.to_owned(),
            cell: CellPath::testing_new(cell),
        })
    }

    fn kind(name: &str, cell: &str) -> PluginKind {
        PluginKind::new(name.to_owned(), CellPath::testing_new(cell))
    }

    fn std_hash(v: &impl Hash) -> u64 {
        let mut h = DefaultHasher::new();
        v.hash(&mut h);
        h.finish()
    }

    fn strong_hash(v: &impl StrongHash) -> u64 {
        let mut h = DefaultHasher::new();
        v.strong_hash(&mut h);
        h.finish()
    }

    fn serialize(v: &impl PagableSerialize) -> Vec<u8> {
        let mut ser = TestingSerializer::new();
        v.pagable_serialize(&mut ser).unwrap();
        ser.finish()
    }

    #[test]
    fn test_plugin_kind_matches_direct_interned_identity() {
        let k = kind("python", "root//defs.bzl");
        let d = direct("python", "root//defs.bzl");

        assert_eq!(k.to_string(), d.to_string(), "Display is the name");
        assert_eq!(k.as_str(), "python");
        assert_eq!(std_hash(&k), std_hash(&d), "Hash is the interned pre-hash");
        assert_eq!(
            strong_hash(&k),
            strong_hash(&d),
            "StrongHash covers name then cell"
        );

        let from_direct =
            PluginKind::pagable_deserialize(&mut TestingDeserializer::new(&serialize(&d))).unwrap();
        assert_eq!(
            from_direct, k,
            "the direct form deserializes as the same kind"
        );
        let to_direct = Intern::<DirectInner>::pagable_deserialize(&mut TestingDeserializer::new(
            &serialize(&k),
        ))
        .unwrap();
        assert_eq!(to_direct, d, "a kind deserializes as the same direct value");
    }

    #[test]
    fn test_plugin_kind_identity_and_order() {
        let a = kind("a", "root//x.bzl");
        let b_x = kind("b", "root//x.bzl");
        let b_y = kind("b", "root//y.bzl");

        assert_eq!(a, kind("a", "root//x.bzl"));
        assert_ne!(b_x, b_y, "the defining file is part of the identity");
        assert_eq!(
            [&a, &b_x, &b_y].map(|k| k.cmp(&b_x)),
            [
                direct("a", "root//x.bzl"),
                direct("b", "root//x.bzl"),
                direct("b", "root//y.bzl")
            ]
            .map(|d| d.cmp(&direct("b", "root//x.bzl"))),
            "Ord compares the name, then the cell"
        );
    }
}
