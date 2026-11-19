/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::alloc::Layout;
use std::cmp::Ordering;
use std::fmt;
use std::fmt::Debug;
use std::hash::Hash;
use std::hash::Hasher;
use std::mem;
use std::ptr;
use std::ptr::NonNull;
use std::slice;
use std::str;

use allocative::Allocative;
use allocative::Key;
use allocative::Visitor;
use buck2_data::ToProtoMessage;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_hash::BuckHasher;
use buck2_util::pointer_identity::StaticPointerIdentity;
use dupe::Dupe;
use lock_free_hashtable::atomic_value::AtomicValue;
use pagable::PagableDeserialize;
use pagable::PagableDeserializer;
use pagable::PagableSerialize;
use pagable::PagableSerializer;
use pagable::arc_erase::ArcErase;
use pagable::arc_erase::ArcEraseType;
use pagable::arc_erase::ArcSerializeOutcome;
use pagable::arc_erase::StdArcEraseType;
use pagable::arc_erase::deserialize_arc;
use serde::Serialize;
use serde::Serializer;
use strong_hash::StrongHash;

use crate::cells::CellAliasResolver;
use crate::cells::CellResolver;
use crate::cells::name::CellName;
use crate::cells::paths::CellRelativePath;
use crate::configuration::data::ConfigurationData;
use crate::configuration::pair::Configuration;
use crate::configuration::pair::ConfigurationNoExec;
use crate::package::PackageLabel;
use crate::pattern::pattern::ParsedPattern;
use crate::pattern::pattern::lex_target_pattern;
use crate::pattern::pattern_type::TargetPatternExtra;
use crate::target::configured_target_label::ConfiguredTargetLabel;
use crate::target::label::interner::global_intern;
use crate::target::name::TargetNameRef;

/// Header of a packed target label allocation. The target name bytes are
/// stored inline immediately after this header, `name_len` bytes long.
///
/// Labels are immortal and deduplicated, so no refcount or capacity is
/// stored: the whole allocation is `NAME_OFFSET + name_len` bytes (with a
/// 16-byte minimum), the name occupying the tail padding and beyond.
// `repr(C)` is load-bearing, not documentation: the name bytes are stored at
// `NAME_OFFSET`, immediately after `name_len`, which is only free space if
// `name_len` is the *last* field in memory. `repr(C)` guarantees declaration
// order; without it the compiler could reorder fields and `NAME_OFFSET`
// would land inside one. (The const asserts below turn any layout drift into
// a compile error rather than corruption.)
#[repr(C)]
struct LabelData {
    pkg: PackageLabel,
    /// Hash of target label (not package, not name). Stored because it must
    /// stay bit-stable — it feeds target hashes (`TargetNode::target_hash`,
    /// surfaced by `targets --show-target-hash`) — and it doubles as the
    /// precomputed `Hash` for label-keyed maps. Equality does not read it:
    /// labels are interned, so pointer comparison suffices.
    hash: u32,
    /// Length of the name bytes stored inline at `NAME_OFFSET`, which is
    /// inside this struct's tail padding: the fields end at byte 14 and
    /// `pkg`'s 8-byte alignment pads the struct size to 16.
    /// `TargetName::verify` enforces the `u16` bound.
    name_len: u16,
}

// The manual offset arithmetic below is sound only for exactly this layout.
const _: () = assert!(mem::size_of::<LabelData>() == 16);
const _: () = assert!(mem::align_of::<LabelData>() == 8);
const _: () = assert!(mem::offset_of!(LabelData, pkg) == 0);
const _: () = assert!(mem::offset_of!(LabelData, hash) == 8);
const _: () = assert!(mem::offset_of!(LabelData, name_len) == 12);
const _: () = assert!(LabelData::NAME_OFFSET == 14);
const _: () = assert!(mem::size_of::<TargetLabel>() == 8);
const _: () = assert!(mem::size_of::<Option<TargetLabel>>() == 8);

// Nothing ever runs drop glue for `LabelData`: entries live forever in the
// arena, and even a racy-insert loser is abandoned in place. Fields must
// therefore be plain data that is safe to leak.
const _: () = assert!(!std::mem::needs_drop::<LabelData>());

impl LabelData {
    /// Name bytes start in the struct's tail padding, two bytes before
    /// `size_of::<LabelData>()`.
    const NAME_OFFSET: usize = mem::offset_of!(LabelData, name_len) + mem::size_of::<u16>();

    /// The header is written as a whole 16-byte struct, so the allocation
    /// must never be smaller than the struct, even when `NAME_OFFSET +
    /// name_len` is (names shorter than two bytes). The arena additionally
    /// rounds carves up to its bump step; `arena_size` is the
    /// accounting-accurate figure.
    fn layout(name_len: usize) -> Layout {
        let size = std::cmp::max(
            mem::size_of::<LabelData>(),
            Self::NAME_OFFSET
                .checked_add(name_len)
                .expect("name length overflows a Layout"),
        );
        Layout::from_size_align(size, mem::align_of::<LabelData>())
            .expect("label allocation overflows a Layout")
    }

    /// Bytes actually consumed in the arena for a label with this name
    /// length (the layout size rounded to the arena's bump step).
    fn arena_size(name_len: usize) -> usize {
        Self::layout(name_len)
            .size()
            .next_multiple_of(crate::target::label::arena::BUMP_STEP)
    }
}

/// The owning form of a target label allocation.
///
/// Exactly one of these exists per unique label, and it lives forever inside
/// the global interner's table (the only exception: the losing candidate of a
/// racy insert of the same label, which is abandoned in place before any
/// handle to it escapes). All public `TargetLabel` values are non-owning
/// copies of these entries.
///
/// Visibility is deliberately restricted to this module tree: a `TargetLabel`
/// handle is canonical only if it came from the global interner, so letting
/// other code mint or store owning entries would break the pointer-equality
/// invariant every label comparison relies on.
pub(in crate::target::label) struct OwnedTargetLabel(NonNull<LabelData>);

// SAFETY: the pointee is immutable after construction and never freed;
// this type only ever reads it.
unsafe impl Send for OwnedTargetLabel {}
// SAFETY: as above; shared access only reads immutable data.
unsafe impl Sync for OwnedTargetLabel {}

impl OwnedTargetLabel {
    /// This computation must not change: the truncated `u32` feeds target
    /// hashes (`TargetNode::target_hash`), which are compared across daemons.
    pub(crate) fn label_hash(pkg: PackageLabel, name: &TargetNameRef) -> u64 {
        let key = &(pkg.dupe(), &name);
        let mut hasher = BuckHasher::default();
        key.hash(&mut hasher);
        hasher.finish()
    }

    pub(crate) fn alloc(pkg: PackageLabel, name: &TargetNameRef, hash: u64) -> OwnedTargetLabel {
        let name = name.as_str().as_bytes();
        // `TargetName::verify` enforces the bound at construction; the stored
        // length bounds every later read of the name bytes, so truncation
        // here would read garbage, not just shorten a name.
        let name_len: u16 = name
            .len()
            .try_into()
            .expect("verified target names fit the u16 length limit");
        let layout = LabelData::layout(name.len());
        let raw = crate::target::label::interner::label_arenas().alloc(hash, layout);
        // SAFETY: the arena returned at least `layout.size()` >=
        // `size_of::<LabelData>()` 8-aligned bytes, so the whole-struct
        // header write is in-bounds and aligned. The name copy then
        // initializes bytes `NAME_OFFSET..NAME_OFFSET + name_len` (starting
        // in the struct's tail padding); any allocation bytes past that
        // remain uninitialized and are never read, since every reader is
        // bounded by `name_len`.
        unsafe {
            let data = raw.as_ptr() as *mut LabelData;
            data.write(LabelData {
                pkg,
                hash: hash as u32,
                name_len,
            });
            ptr::copy_nonoverlapping(
                name.as_ptr(),
                (data as *mut u8).add(LabelData::NAME_OFFSET),
                name.len(),
            );
            OwnedTargetLabel(NonNull::new_unchecked(data))
        }
    }

    /// Bytes `LabelArenas::alloc` accounted for this label; what `abandon`
    /// must hand back for a racy-insert loser.
    pub(in crate::target::label) fn arena_size(&self) -> usize {
        // SAFETY: the pointee is a live, immutable `LabelData`.
        LabelData::arena_size(unsafe { self.0.as_ref() }.name_len as usize)
    }
}

// No `Drop`: label storage comes from the immortal arenas, so even the
// losing candidate of a racy insert is simply abandoned in place (a small,
// rare hole counted in `label_arena_slack` once the interner reports it
// abandoned).

/// Account the shared label allocation. Used by both the interner's owning
/// entry and every `TargetLabel` handle; `enter_shared` deduplicates by
/// pointer, so whichever is visited first accounts it, identically.
fn visit_label_allocation<'a, 'b: 'a>(data: NonNull<LabelData>, visitor: &'a mut Visitor<'b>) {
    if let Some(mut visitor) = visitor.enter_shared(
        Key::new("data"),
        mem::size_of::<*const ()>(),
        data.as_ptr() as *const (),
    ) {
        // SAFETY: the pointee is a live, immutable `LabelData`.
        let name_len = unsafe { data.as_ref().name_len as usize };
        // `arena_size` includes the 16-byte floor and the arena's rounding.
        visitor.visit_simple(Key::new("label_data"), LabelData::arena_size(name_len));
        visitor.exit();
    }
}

impl Allocative for OwnedTargetLabel {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        visit_label_allocation(self.0, &mut visitor);
        visitor.exit();
    }
}

/// This impl hands out `'static` `TargetLabel`s untied to the table's
/// lifetime, which is sound only for the immortal global interner. Inside
/// `insert`, the eq callback can also receive a handle to a not-yet-inserted
/// candidate that is abandoned if it loses the insert race; such handles
/// must never escape the callback — the memory stays valid (nothing is ever
/// freed), but an escaped candidate would compare unequal to the canonical
/// entry under pointer equality.
impl AtomicValue for OwnedTargetLabel {
    type Ref<'a> = TargetLabel;

    fn into_raw(this: Self) -> NonNull<()> {
        this.0.cast()
    }

    unsafe fn from_raw(raw: NonNull<()>) -> Self {
        OwnedTargetLabel(raw.cast())
    }

    unsafe fn deref<'a>(raw: NonNull<()>) -> Self::Ref<'a> {
        TargetLabel(raw.cast())
    }
}

/// 'TargetLabel' that uniquely maps to a 'target'
/// It contains a 'Package' which is the 'Package' defined by the build file
/// that contains this 'target', and a 'name' which is a 'TargetName'
/// representing the target name given to the particular target.
///
/// Every distinct label is interned exactly once in a global, immortal
/// interner, so this type is a `Copy` handle: cloning and dropping it is free,
/// and equality is pointer equality. The backing allocation is never freed.
#[derive(Copy, Clone, derive_more::Display)]
#[display("{}", self.as_ref())]
pub struct TargetLabel(NonNull<LabelData>);

// SAFETY: `TargetLabel` points to an allocation owned by the global interner,
// which is immutable after construction and is never freed, so sharing the
// pointer across threads is sound. The pointee's fields are themselves
// `Send + Sync` (`u32`, `u16`, and `PackageLabel`, an interned handle).
unsafe impl Send for TargetLabel {}
// SAFETY: as above; all access is to immutable data.
unsafe impl Sync for TargetLabel {}

// The `Send`/`Sync` arguments above lean on `PackageLabel` being freely
// shareable; make a change to that a compile error here rather than a
// latent soundness hole.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<PackageLabel>();
};

impl StrongHash for TargetLabel {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        self.pkg().strong_hash(state);
        self.name().strong_hash(state);
    }
}

impl Debug for TargetLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TargetLabel")
            .field("pkg", &self.pkg())
            .field("name", &self.name())
            .finish()
    }
}

impl Dupe for TargetLabel {}

impl PartialEq for TargetLabel {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        // Sound because every `TargetLabel` is created by the global interner:
        // one allocation exists per distinct (package, name).
        ptr::eq(self.as_raw(), other.as_raw())
    }
}

impl Eq for TargetLabel {}

#[allow(clippy::derived_hash_with_manual_eq)]
impl Hash for TargetLabel {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.header().hash.hash(state);
    }
}

impl Ord for TargetLabel {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_ref().cmp(&other.as_ref())
    }
}

impl PartialOrd for TargetLabel {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl TargetLabel {
    pub fn new(pkg: PackageLabel, name: &TargetNameRef) -> Self {
        global_intern(pkg, name)
    }

    #[inline]
    fn header(&self) -> &'static LabelData {
        // SAFETY: every label allocation comes from the immortal arenas and
        // is never freed or mutated — including a racy-insert loser, which
        // is abandoned in place — so extending the borrow to `'static` is
        // always sound. (Candidate handles inside the interner's insert eq
        // callback still must not escape, for canonicity, not liveness; see
        // the `AtomicValue` impl.)
        unsafe { &*self.0.as_ptr() }
    }

    #[inline]
    fn name_bytes(&self) -> &'static [u8] {
        let header = self.header();
        // SAFETY: `name_len` bytes were written immediately after the header
        // at construction, and the allocation is immortal and immutable.
        unsafe {
            slice::from_raw_parts(
                (self.0.as_ptr() as *const u8).add(LabelData::NAME_OFFSET),
                header.name_len as usize,
            )
        }
    }

    #[inline]
    pub fn pkg(&self) -> PackageLabel {
        self.header().pkg.dupe()
    }

    #[inline]
    pub fn name(&self) -> &'static TargetNameRef {
        // SAFETY(utf8): the bytes were copied from a valid `str` at
        // construction and are immutable.
        let name = unsafe { str::from_utf8_unchecked(self.name_bytes()) };
        TargetNameRef::unchecked_new(name)
    }

    /// Creates a 'ConfiguredTargetLabel' from ['Self'] based on the provided
    /// configuration.
    #[inline]
    pub fn configure(&self, cfg: ConfigurationData) -> ConfiguredTargetLabel {
        self.configure_pair(Configuration::new(cfg, None))
    }

    /// Like `configure`, but forces the execution configuration too.
    #[inline]
    pub fn configure_with_exec(
        &self,
        cfg: ConfigurationData,
        exec_cfg: ConfigurationData,
    ) -> ConfiguredTargetLabel {
        self.configure_pair(Configuration::new(cfg, Some(exec_cfg)))
    }

    #[inline]
    pub fn configure_pair(&self, cfg_pair: Configuration) -> ConfiguredTargetLabel {
        ConfiguredTargetLabel {
            target: self.dupe(),
            cfg_pair,
        }
    }

    #[inline]
    pub fn configure_pair_no_exec(&self, cfg: ConfigurationNoExec) -> ConfiguredTargetLabel {
        self.configure_pair(cfg.cfg_pair().dupe())
    }

    #[inline]
    pub fn as_ref(&self) -> TargetLabelRef<'_> {
        TargetLabelRef::new(self.pkg(), self.name())
    }

    pub fn parse(
        label: &str,
        cell_name: CellName,
        cell_resolver: &CellResolver,
        cell_alias_resolver: &CellAliasResolver,
    ) -> buck2_error::Result<TargetLabel> {
        let (target_label, TargetPatternExtra) =
            ParsedPattern::<TargetPatternExtra>::parse_precise(
                label,
                cell_name,
                cell_resolver,
                cell_alias_resolver,
            )?
            .as_literal(label)?;
        Ok(target_label)
    }

    pub(crate) fn as_raw(&self) -> *const () {
        self.0.as_ptr() as *const ()
    }

    /// TargetLabels are statically interned and never evicted; paging
    /// serializes a content copy, and page-in re-interns to the same
    /// allocation. Pointer equality therefore holds exactly for equal
    /// labels. The label's identity beyond lifetime of this daemon process
    /// should be a StrongHash of its content; this one is only cheaper,
    /// never a substitute.
    pub fn pointer_identity(&self) -> StaticPointerIdentity<TargetLabel> {
        StaticPointerIdentity::new(self.header())
    }

    /// Simple and incorrect target label parser which can be used in tests.
    pub fn testing_parse(target_label: &str) -> TargetLabel {
        let parts = lex_target_pattern(target_label, false).expect("failed to parse");
        let cell_name = CellName::testing_new(parts.cell_alias.expect("must have cell name"));

        let pattern_data = parts
            .pattern
            .reject_ambiguity()
            .expect("target label must be unambiguous");
        let (target_name, TargetPatternExtra) =
            pattern_data.target().expect("target label must be precise");

        TargetLabel::new(
            PackageLabel::new(
                cell_name,
                CellRelativePath::new(
                    <&ForwardRelativePath>::try_from(pattern_data.package_path())
                        .expect("must be valid path"),
                ),
            )
            .unwrap(),
            target_name,
        )
    }
}

impl Allocative for TargetLabel {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        visit_label_allocation(self.0, &mut visitor);
        visitor.exit();
    }
}

impl ArcErase for TargetLabel {
    type Weak = ();

    fn dupe_strong(&self) -> Self {
        *self
    }

    fn erase_type() -> impl ArcEraseType {
        StdArcEraseType::<Self>::new()
    }

    fn identity(&self) -> usize {
        // One allocation exists per distinct label, and it is never freed,
        // so the address is a stable unique identity.
        self.as_raw() as usize
    }

    fn downgrade(&self) -> Option<Self::Weak> {
        None
    }

    fn serialize_inner(
        &self,
        ser: &mut dyn PagableSerializer,
    ) -> pagable::Result<ArcSerializeOutcome> {
        self.pkg().pagable_serialize(ser)?;
        self.name().as_str().pagable_serialize(ser)?;
        Ok(ArcSerializeOutcome::Serialized)
    }

    fn deserialize_inner<'de, D: PagableDeserializer<'de> + ?Sized>(
        deser: &mut D,
    ) -> pagable::Result<Self> {
        let pkg = PackageLabel::pagable_deserialize(deser)?;
        let name = String::pagable_deserialize(deser)?;
        // Re-validate: a label minted here becomes a permanent entry in the
        // global interner, so corrupted storage must not be trusted.
        let name = TargetNameRef::new(&name)
            .map_err(|e| pagable::anyhow!("Invalid target name in paged data: {e:#}"))?;
        Ok(TargetLabel::new(pkg, name))
    }
}

impl PagableSerialize for TargetLabel {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> pagable::Result<()> {
        serializer.serialize_arc(self)
    }
}

impl<'de> PagableDeserialize<'de> for TargetLabel {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        deserialize_arc::<Self, _>(deserializer)
    }
}

impl Serialize for TargetLabel {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl ToProtoMessage for TargetLabel {
    type Message = buck2_data::TargetLabel;

    fn as_proto(&self) -> Self::Message {
        buck2_data::TargetLabel {
            package: self.pkg().to_string(),
            name: self.name().to_string(),
        }
    }
}

#[derive(
    Clone,
    Dupe,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Debug,
    derive_more::Display
)]
#[display("{}:{}", pkg, name)]
pub struct TargetLabelRef<'a> {
    pkg: PackageLabel,
    name: &'a TargetNameRef,
}

impl<'a> TargetLabelRef<'a> {
    #[inline]
    pub fn new(pkg: PackageLabel, name: &'a TargetNameRef) -> TargetLabelRef<'a> {
        TargetLabelRef { pkg, name }
    }
}

#[cfg(test)]
mod tests {
    use crate::target::label::label::TargetLabel;

    #[test]
    fn test_short_and_long_names_roundtrip() {
        // A one-byte name exercises the minimum-allocation path (the header
        // write is larger than `NAME_OFFSET + name_len`).
        let short = TargetLabel::testing_parse("foo//some/pkg:a");
        assert_eq!("a", short.name().as_str());
        // Exactly fills the header's tail padding: NAME_OFFSET + 2 == 16.
        let exact = TargetLabel::testing_parse("foo//some/pkg:ab");
        assert_eq!("ab", exact.name().as_str());
        let long_name = format!("t{}", "x".repeat(300));
        let long = TargetLabel::testing_parse(&format!("foo//some/pkg:{long_name}"));
        assert_eq!(long_name, long.name().as_str());
        assert_eq!(301, long.name().as_str().len());
    }

    #[test]
    fn test_max_length_name_roundtrip() {
        // The longest legal name: exercises `alloc`'s u16 boundary, a carve
        // larger than an arena chunk (which installs a dedicated oversize
        // chunk), and interning right at the cap.
        let max_name = format!("t{}", "x".repeat(u16::MAX as usize - 1));
        let label = TargetLabel::testing_parse(&format!("foo//some/pkg:{max_name}"));
        assert_eq!(max_name, label.name().as_str());
        assert_eq!(u16::MAX as usize, label.name().as_str().len());
        let again = TargetLabel::testing_parse(&format!("foo//some/pkg:{max_name}"));
        assert_eq!(
            label, again,
            "max-length labels canonicalize like any other"
        );
    }

    #[test]
    fn test_accessors_roundtrip() {
        let label = TargetLabel::testing_parse("foo//some/pkg:a_target_name");
        assert_eq!("a_target_name", label.name().as_str());
        assert_eq!("foo//some/pkg", label.pkg().to_string());
        assert_eq!("foo//some/pkg:a_target_name", label.to_string());
    }
}
