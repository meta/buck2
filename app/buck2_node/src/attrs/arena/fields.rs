/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Field types: what a head field, a run of refs or the text tail of a
//! record means, and how each is read, encoded, hashed and compared.
//!
//! Items are `pub` in this crate-private module for the reason given on
//! [`Kind`](crate::attrs::arena::schema::Kind).

use std::hash::Hash;
use std::hash::Hasher;

use buck2_core::provider::label::ProvidersLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_hash::BuckHasher;
use buck2_hash::BuckMutMap;
use buck2_util::strong_hasher::Blake3StrongHasher;
use buck2_util::strong_hasher::StrongHash128;
use dupe::Dupe;
use strong_hash::StrongHash;

use crate::attrs::arena::builder::CoercedAttrArenaBuilder;
use crate::attrs::arena::builder::EncodeScratchFields;
use crate::attrs::arena::builder::EncodeScratchRefs;
use crate::attrs::arena::schema::Kind;
use crate::attrs::arena::schema::View;
use crate::attrs::arena::storage::ABSENT_REF;
use crate::attrs::arena::storage::AttrToken;
use crate::attrs::arena::storage::AttrWord;
use crate::attrs::arena::storage::CoercedAttrArena;
use crate::attrs::arena::storage::INLINE_PAYLOAD_BITS;
use crate::attrs::arena::storage::INLINE_PAYLOAD_MASK;
use crate::attrs::arena::storage::INT_INLINE_MAX;
use crate::attrs::arena::storage::INT_INLINE_MIN;
use crate::attrs::arena::storage::RefRun;
use crate::attrs::arena::storage::Tail;
use crate::attrs::arena::storage::unzigzag;
use crate::attrs::arena::storage::zigzag;
use crate::attrs::arena::view::CoercedAttrRef;
use crate::attrs::arena::view::Pairs;
use crate::attrs::arena::view::RefItem;
use crate::attrs::arena::view::Refs;
use crate::attrs::arena::view::Values;
use crate::attrs::coerced::kinds::CoercedAttrKind;
use crate::attrs::coerced_path::CoercedPath;
use crate::metadata::map::MetadataMap;
use crate::modifiers::CfgModifiers;
use crate::visibility::VisibilitySpecification;
use crate::visibility::WithinViewSpecification;

/// The blake3 identity hasher plus the per-arena memo child hashes use.
pub struct IdHasher<'m> {
    pub(crate) h: Blake3StrongHasher,
    pub(super) memo: &'m mut BuckMutMap<u32, StrongHash128>,
}

impl IdHasher<'_> {
    /// A child's whole-value identity hash, memoized per record.
    pub(crate) fn child(&mut self, c: CoercedAttrRef<'_>) {
        c.arena.strong_of(c.r, self.memo).strong_hash(&mut self.h);
    }
}

/// Reads one value's fields in order: the inline payload, or a record's
/// head fields and tail.
pub struct Cursor<'a> {
    pub(crate) arena: &'a CoercedAttrArena,
    pub(crate) r: AttrWord,
    /// Head fields of a record; `None` for an inline word.
    pub(super) head: Option<RefRun<'a>>,
    pub(super) tail: Tail<'a>,
    pub(super) head_i: u32,
    pub(super) ref_i: u32,
}

impl<'a> Cursor<'a> {
    /// Whether the value is an inline word rather than a record.
    pub(crate) fn is_inline(&self) -> bool {
        self.head.is_none()
    }

    pub(crate) fn new(arena: &'a CoercedAttrArena, r: AttrWord) -> Cursor<'a> {
        let (head, tail) = if r.is_inline() {
            (None, Tail::None)
        } else {
            let rec = arena.open(r);
            (Some(rec.head), rec.tail)
        };
        Cursor {
            arena,
            r,
            head,
            tail,
            head_i: 0,
            ref_i: 0,
        }
    }

    /// The next head field, or the inline payload.
    pub(crate) fn raw(&mut self) -> u32 {
        match self.head {
            None => {
                self.r
                    .inline_parts()
                    .expect("should be inline: no record head")
                    .1
            }
            Some(head) => {
                let v = head.raw(self.head_i);
                self.head_i += 1;
                v
            }
        }
    }

    pub(crate) fn text(&self) -> &'a str {
        self.arena.str_at(self.r)
    }

    /// The next `n` head fields as a run.
    pub(crate) fn raws(&mut self, n: u32) -> RefRun<'a> {
        let head = self
            .head
            .expect("should be a record: inline words have one payload");
        let out = RefRun {
            len: n,
            ..head.skip(self.head_i)
        };
        self.head_i += n;
        out
    }

    /// The next `n` refs of the tail.
    pub(crate) fn refs(&mut self, n: u32) -> RefRun<'a> {
        let Tail::Refs(run) = self.tail else {
            unreachable!("should have refs: the kind's fields say so")
        };
        let out = RefRun {
            len: n,
            ..run.skip(self.ref_i)
        };
        self.ref_i += n;
        out
    }

    /// The next ref of the tail; `None` for an absent slot.
    pub(crate) fn child(&mut self) -> Option<AttrWord> {
        self.refs(1).get_opt(0)
    }
}

/// Head fields, child refs and weak content hash of one value being
/// encoded, gathered field by field.
pub struct RecordWriter<'s> {
    pub(super) kind: CoercedAttrKind,
    pub(super) head: EncodeScratchFields,
    pub(super) refs: EncodeScratchRefs,
    pub(super) text: Option<&'s str>,
    pub(super) h: BuckHasher,
}

impl<'s> RecordWriter<'s> {
    pub(super) fn new(kind: CoercedAttrKind) -> RecordWriter<'s> {
        let mut h = BuckHasher::default();
        Hash::hash(&(kind as u8), &mut h);
        RecordWriter {
            kind,
            head: EncodeScratchFields::new(),
            refs: EncodeScratchRefs::new(),
            text: None,
            h,
        }
    }

    pub(crate) fn head(&mut self, v: u32) {
        self.head.push(v);
    }

    pub(crate) fn r(&mut self, w: AttrWord) {
        self.refs.push(w);
    }

    pub(crate) fn hash<T: Hash + ?Sized>(&mut self, t: &T) {
        t.hash(&mut self.h);
    }
}

/// What a field, or a whole record, occupies: head fields, then a tail of
/// refs or of text.
pub struct Extent {
    pub(crate) head: u32,
    pub(crate) refs: Option<u32>,
    pub(crate) text: Option<u32>,
}

impl Extent {
    pub(crate) const fn head(n: u32) -> Extent {
        Extent {
            head: n,
            refs: None,
            text: None,
        }
    }
}

/// One typed component of a record: what a head field, a run of refs or
/// the text tail means. Each field knows how to read itself, how to encode
/// its input form, how to re-encode its decoded form into another arena,
/// and how it hashes and compares.
pub trait Field {
    /// Whether the field can hold a dep: a label, a path or a child.
    const DEPS: bool;
    /// Whether the field is at most one head value and nothing else, so a
    /// value whose only field this is can live in the word's payload.
    const INLINE: bool;
    /// Whether the field is the text tail.
    const HAS_TEXT: bool;
    /// Decoded form, borrowed from the arena.
    type View<'a>: Copy;
    /// Input form for encoding: leaf values by reference, children as
    /// tokens.
    type Spec<'s>: Copy;
    /// What the field occupies, reading head fields through `peek`; `at` is
    /// the field's first head index.
    fn shape(peek: &dyn Fn(u32) -> Option<u32>, at: u32) -> Option<Extent>;
    fn decode<'a>(c: &mut Cursor<'a>) -> Self::View<'a>;
    fn encode<'s>(
        spec: Self::Spec<'s>,
        b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'s>,
    ) -> buck2_error::Result<()>;
    fn reemit<'a>(
        view: Self::View<'a>,
        b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'a>,
    ) -> buck2_error::Result<()>;
    fn identity(view: Self::View<'_>, h: &mut IdHasher<'_>);
    fn same(a: Self::View<'_>, b: Self::View<'_>) -> bool;
}

/// No fields.
impl Field for () {
    const DEPS: bool = false;
    const INLINE: bool = true;
    const HAS_TEXT: bool = false;
    type View<'a> = ();
    type Spec<'s> = ();

    fn shape(_peek: &dyn Fn(u32) -> Option<u32>, _at: u32) -> Option<Extent> {
        Some(Extent::head(0))
    }

    fn decode<'a>(_c: &mut Cursor<'a>) {}

    fn encode<'s>(
        (): (),
        _b: &mut CoercedAttrArenaBuilder,
        _w: &mut RecordWriter<'s>,
    ) -> buck2_error::Result<()> {
        Ok(())
    }

    fn reemit<'a>(
        (): (),
        _b: &mut CoercedAttrArenaBuilder,
        _w: &mut RecordWriter<'a>,
    ) -> buck2_error::Result<()> {
        Ok(())
    }

    fn identity((): (), _h: &mut IdHasher<'_>) {}

    fn same((): (), (): ()) -> bool {
        true
    }
}

/// Two fields in sequence.
impl<A: Field, B: Field> Field for (A, B) {
    const DEPS: bool = A::DEPS || B::DEPS;
    const INLINE: bool = false;
    const HAS_TEXT: bool = A::HAS_TEXT || B::HAS_TEXT;
    type View<'a> = (A::View<'a>, B::View<'a>);
    type Spec<'s> = (A::Spec<'s>, B::Spec<'s>);

    fn shape(peek: &dyn Fn(u32) -> Option<u32>, at: u32) -> Option<Extent> {
        let a = A::shape(peek, at)?;
        let b = B::shape(peek, at.checked_add(a.head)?)?;
        let refs = match (a.refs, b.refs) {
            (None, None) => None,
            (x, y) => Some(x.unwrap_or(0).checked_add(y.unwrap_or(0))?),
        };
        Some(Extent {
            head: a.head.checked_add(b.head)?,
            refs,
            text: a.text.or(b.text),
        })
    }

    fn decode<'a>(c: &mut Cursor<'a>) -> Self::View<'a> {
        (A::decode(c), B::decode(c))
    }

    fn encode<'s>(
        (a, b): Self::Spec<'s>,
        bld: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'s>,
    ) -> buck2_error::Result<()> {
        A::encode(a, bld, w)?;
        B::encode(b, bld, w)
    }

    fn reemit<'a>(
        (a, b): Self::View<'a>,
        bld: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'a>,
    ) -> buck2_error::Result<()> {
        A::reemit(a, bld, w)?;
        B::reemit(b, bld, w)
    }

    fn identity((a, b): Self::View<'_>, h: &mut IdHasher<'_>) {
        A::identity(a, h);
        B::identity(b, h);
    }

    fn same((a, b): Self::View<'_>, (a2, b2): Self::View<'_>) -> bool {
        A::same(a, a2) && B::same(b, b2)
    }
}

/// A value interned into one of the arena's side tables: the field is the
/// table index, and the value is borrowed from the table on decode.
pub(crate) trait Interned: StrongHash + PartialEq + Sized + 'static {
    /// Whether the value is a dep (a label or path) rather than a whole
    /// value with no deps of its own.
    const DEPS: bool;
    fn table(arena: &CoercedAttrArena) -> &Vec<Self>;
    /// Intern into the builder's table: the index and the value's content
    /// hash.
    fn intern(b: &mut CoercedAttrArenaBuilder, v: &Self) -> (u32, u64);
}

impl Interned for ProvidersLabel {
    const DEPS: bool = true;
    fn table(arena: &CoercedAttrArena) -> &Vec<Self> {
        &arena.providers_labels
    }
    fn intern(b: &mut CoercedAttrArenaBuilder, v: &Self) -> (u32, u64) {
        b.providers_label_idx_and_hash(v)
    }
}

impl Interned for TargetLabel {
    const DEPS: bool = true;
    fn table(arena: &CoercedAttrArena) -> &Vec<Self> {
        &arena.target_labels
    }
    fn intern(b: &mut CoercedAttrArenaBuilder, v: &Self) -> (u32, u64) {
        let mut h = BuckHasher::default();
        v.hash(&mut h);
        (b.target_label_idx(v.dupe()), h.finish())
    }
}

impl Interned for CoercedPath {
    const DEPS: bool = true;
    fn table(arena: &CoercedAttrArena) -> &Vec<Self> {
        &arena.paths
    }
    fn intern(b: &mut CoercedAttrArenaBuilder, v: &Self) -> (u32, u64) {
        b.source_path_idx_and_hash(v)
    }
}

impl Interned for VisibilitySpecification {
    const DEPS: bool = false;
    fn table(arena: &CoercedAttrArena) -> &Vec<Self> {
        &arena.visibility
    }
    fn intern(b: &mut CoercedAttrArenaBuilder, v: &Self) -> (u32, u64) {
        intern_by_content(&mut b.visibility_dedup, &mut b.arena.visibility, v)
    }
}

impl Interned for WithinViewSpecification {
    const DEPS: bool = false;
    fn table(arena: &CoercedAttrArena) -> &Vec<Self> {
        &arena.within_view
    }
    fn intern(b: &mut CoercedAttrArenaBuilder, v: &Self) -> (u32, u64) {
        intern_by_content(&mut b.within_view_dedup, &mut b.arena.within_view, v)
    }
}

impl Interned for CfgModifiers {
    const DEPS: bool = false;
    fn table(arena: &CoercedAttrArena) -> &Vec<Self> {
        &arena.modifiers
    }
    fn intern(b: &mut CoercedAttrArenaBuilder, v: &Self) -> (u32, u64) {
        intern_by_content(&mut b.modifiers_dedup, &mut b.arena.modifiers, v)
    }
}

impl Interned for MetadataMap {
    const DEPS: bool = false;
    fn table(arena: &CoercedAttrArena) -> &Vec<Self> {
        &arena.metadata
    }
    fn intern(b: &mut CoercedAttrArenaBuilder, v: &Self) -> (u32, u64) {
        intern_by_content(&mut b.metadata_dedup, &mut b.arena.metadata, v)
    }
}

/// Interns a whole value by content, one table entry per distinct value:
/// the index and the value's content hash.
pub(crate) fn intern_by_content<T: Hash + PartialEq + Clone>(
    dedup: &mut hashbrown::HashTable<(u64, u32)>,
    table: &mut Vec<T>,
    v: &T,
) -> (u32, u64) {
    let mut h = BuckHasher::default();
    v.hash(&mut h);
    let hash = h.finish();
    if let Some(&(_, i)) = dedup.find(hash, |&(h, i)| h == hash && table[i as usize] == *v) {
        return (i, hash);
    }
    let i = table.len() as u32;
    table.push(v.clone());
    dedup.insert_unique(hash, (hash, i), |&(h, _)| h);
    (i, hash)
}

/// A table index field for an [`Interned`] value.
pub struct Idx<T>(std::marker::PhantomData<T>);

/// Providers-label, target-label and path table indices.
pub type Label = Idx<ProvidersLabel>;
pub type Target = Idx<TargetLabel>;
pub type Path = Idx<CoercedPath>;

impl<T: Interned> Field for Idx<T> {
    const DEPS: bool = T::DEPS;
    const INLINE: bool = true;
    const HAS_TEXT: bool = false;
    type View<'a> = &'a T;
    type Spec<'s> = &'s T;

    fn shape(_peek: &dyn Fn(u32) -> Option<u32>, _at: u32) -> Option<Extent> {
        Some(Extent::head(1))
    }

    fn decode<'a>(c: &mut Cursor<'a>) -> &'a T {
        &T::table(c.arena)[c.raw() as usize]
    }

    fn encode<'s>(
        v: &'s T,
        b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'s>,
    ) -> buck2_error::Result<()> {
        let (idx, hash) = T::intern(b, v);
        w.head(idx);
        w.hash(&hash);
        Ok(())
    }

    fn reemit<'a>(
        view: &'a T,
        b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'a>,
    ) -> buck2_error::Result<()> {
        <Self as Field>::encode(view, b, w)
    }

    fn identity(view: &T, h: &mut IdHasher<'_>) {
        view.strong_hash(&mut h.h);
    }

    fn same(a: &T, b: &T) -> bool {
        a == b
    }
}

/// A providers-label table index plus one, or zero for absent.
pub struct OptLabel;

impl Field for OptLabel {
    const DEPS: bool = true;
    const INLINE: bool = true;
    const HAS_TEXT: bool = false;
    type View<'a> = Option<&'a ProvidersLabel>;
    type Spec<'s> = Option<&'s ProvidersLabel>;

    fn shape(_peek: &dyn Fn(u32) -> Option<u32>, _at: u32) -> Option<Extent> {
        Some(Extent::head(1))
    }

    fn decode<'a>(c: &mut Cursor<'a>) -> Option<&'a ProvidersLabel> {
        match c.raw() {
            0 => None,
            i => Some(&c.arena.providers_labels[(i - 1) as usize]),
        }
    }

    fn encode<'s>(
        l: Option<&'s ProvidersLabel>,
        b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'s>,
    ) -> buck2_error::Result<()> {
        let (field, hash) = match l {
            Some(l) => {
                let (idx, hash) = b.providers_label_idx_and_hash(l);
                (idx + 1, Some(hash))
            }
            None => (0, None),
        };
        w.head(field);
        w.hash(&hash);
        Ok(())
    }

    fn reemit<'a>(
        view: Option<&'a ProvidersLabel>,
        b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'a>,
    ) -> buck2_error::Result<()> {
        <Self as Field>::encode(view, b, w)
    }

    fn identity(view: Option<&ProvidersLabel>, h: &mut IdHasher<'_>) {
        view.strong_hash(&mut h.h);
    }

    fn same(a: Option<&ProvidersLabel>, b: Option<&ProvidersLabel>) -> bool {
        a == b
    }
}

/// A boolean head field: the payload of an inline leaf that is either
/// true or false.
pub struct Flag;

impl Field for Flag {
    const DEPS: bool = false;
    const INLINE: bool = true;
    const HAS_TEXT: bool = false;
    type View<'a> = bool;
    type Spec<'s> = bool;

    fn shape(_peek: &dyn Fn(u32) -> Option<u32>, _at: u32) -> Option<Extent> {
        Some(Extent::head(1))
    }

    fn decode<'a>(c: &mut Cursor<'a>) -> bool {
        c.raw() != 0
    }

    fn encode<'s>(
        v: bool,
        _b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'s>,
    ) -> buck2_error::Result<()> {
        w.head(v as u32);
        w.hash(&v);
        Ok(())
    }

    fn reemit<'a>(
        view: bool,
        b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'a>,
    ) -> buck2_error::Result<()> {
        <Self as Field>::encode(view, b, w)
    }

    fn identity(view: bool, h: &mut IdHasher<'_>) {
        h.h.write_u8(view as u8);
    }

    fn same(a: bool, b: bool) -> bool {
        a == b
    }
}

/// An immediate head field.
pub struct U32;

impl Field for U32 {
    const DEPS: bool = false;
    const INLINE: bool = true;
    const HAS_TEXT: bool = false;
    type View<'a> = u32;
    type Spec<'s> = u32;

    fn shape(_peek: &dyn Fn(u32) -> Option<u32>, _at: u32) -> Option<Extent> {
        Some(Extent::head(1))
    }

    fn decode<'a>(c: &mut Cursor<'a>) -> u32 {
        c.raw()
    }

    fn encode<'s>(
        v: u32,
        _b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'s>,
    ) -> buck2_error::Result<()> {
        w.head(v);
        w.hash(&v);
        Ok(())
    }

    fn reemit<'a>(
        view: u32,
        b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'a>,
    ) -> buck2_error::Result<()> {
        <Self as Field>::encode(view, b, w)
    }

    fn identity(view: u32, h: &mut IdHasher<'_>) {
        h.h.write_u32(view);
    }

    fn same(a: u32, b: u32) -> bool {
        a == b
    }
}

/// A signed 64-bit value: one head value when it fits the word's payload,
/// sign-extended; otherwise two, the zigzag encoding in 32-bit halves. The
/// hash is the value's, whichever form it takes.
pub struct I64;

impl Field for I64 {
    const DEPS: bool = false;
    const INLINE: bool = true;
    const HAS_TEXT: bool = false;
    type View<'a> = i64;
    type Spec<'s> = i64;

    fn shape(_peek: &dyn Fn(u32) -> Option<u32>, _at: u32) -> Option<Extent> {
        Some(Extent::head(2))
    }

    fn decode<'a>(c: &mut Cursor<'a>) -> i64 {
        if c.is_inline() {
            let shifted = (c.raw() << (32 - INLINE_PAYLOAD_BITS)) as i32;
            (shifted >> (32 - INLINE_PAYLOAD_BITS)) as i64
        } else {
            let lo = c.raw() as u64;
            let hi = c.raw() as u64;
            unzigzag(lo | (hi << 32))
        }
    }

    fn encode<'s>(
        i: i64,
        _b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'s>,
    ) -> buck2_error::Result<()> {
        if (INT_INLINE_MIN..=INT_INLINE_MAX).contains(&i) {
            w.head((i as u32) & INLINE_PAYLOAD_MASK);
        } else {
            let z = zigzag(i);
            w.head(z as u32);
            w.head((z >> 32) as u32);
        }
        w.hash(&i);
        Ok(())
    }

    fn reemit<'a>(
        i: i64,
        b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'a>,
    ) -> buck2_error::Result<()> {
        <Self as Field>::encode(i, b, w)
    }

    fn identity(i: i64, h: &mut IdHasher<'_>) {
        h.h.write_i64(i);
    }

    fn same(a: i64, b: i64) -> bool {
        a == b
    }
}

/// The UTF-8 text tail, with its byte length as the head field. Text
/// storage writes the record; the field only carries the text.
pub struct Text;

impl Field for Text {
    const DEPS: bool = false;
    const INLINE: bool = false;
    const HAS_TEXT: bool = true;
    type View<'a> = &'a str;
    type Spec<'s> = &'s str;

    fn shape(peek: &dyn Fn(u32) -> Option<u32>, at: u32) -> Option<Extent> {
        Some(Extent {
            head: 1,
            refs: None,
            text: Some(peek(at)?),
        })
    }

    fn decode<'a>(c: &mut Cursor<'a>) -> &'a str {
        c.text()
    }

    fn encode<'s>(
        s: &'s str,
        _b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'s>,
    ) -> buck2_error::Result<()> {
        w.text = Some(s);
        Ok(())
    }

    fn reemit<'a>(
        view: &'a str,
        b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'a>,
    ) -> buck2_error::Result<()> {
        <Self as Field>::encode(view, b, w)
    }

    fn identity(view: &str, h: &mut IdHasher<'_>) {
        view.strong_hash(&mut h.h);
    }

    fn same(a: &str, b: &str) -> bool {
        a == b
    }
}

/// One child ref.
pub struct Child;

impl Field for Child {
    const DEPS: bool = true;
    const INLINE: bool = false;
    const HAS_TEXT: bool = false;
    type View<'a> = CoercedAttrRef<'a>;
    type Spec<'s> = AttrToken;

    fn shape(_peek: &dyn Fn(u32) -> Option<u32>, _at: u32) -> Option<Extent> {
        Some(Extent {
            head: 0,
            refs: Some(1),
            text: None,
        })
    }

    fn decode<'a>(c: &mut Cursor<'a>) -> CoercedAttrRef<'a> {
        c.arena.view(
            c.child()
                .expect("should be present: only a selector default may be absent"),
        )
    }

    fn encode<'s>(
        t: AttrToken,
        _b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'s>,
    ) -> buck2_error::Result<()> {
        w.r(t.0);
        w.hash(&t.1);
        Ok(())
    }

    fn reemit<'a>(
        view: CoercedAttrRef<'a>,
        b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'a>,
    ) -> buck2_error::Result<()> {
        <Self as Field>::encode(b.encode_view(view)?, b, w)
    }

    fn identity(view: CoercedAttrRef<'_>, h: &mut IdHasher<'_>) {
        h.child(view);
    }

    fn same(a: CoercedAttrRef<'_>, b: CoercedAttrRef<'_>) -> bool {
        a.content_eq(&b)
    }
}

/// One child ref that may be absent (a selector's default): the identity
/// hash writes a presence flag, then the child.
pub struct OptChild;

impl Field for OptChild {
    const DEPS: bool = true;
    const INLINE: bool = false;
    const HAS_TEXT: bool = false;
    type View<'a> = Option<CoercedAttrRef<'a>>;
    type Spec<'s> = Option<AttrToken>;

    fn shape(_peek: &dyn Fn(u32) -> Option<u32>, _at: u32) -> Option<Extent> {
        Some(Extent {
            head: 0,
            refs: Some(1),
            text: None,
        })
    }

    fn decode<'a>(c: &mut Cursor<'a>) -> Option<CoercedAttrRef<'a>> {
        c.child().map(|w| c.arena.view(w))
    }

    fn encode<'s>(
        t: Option<AttrToken>,
        _b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'s>,
    ) -> buck2_error::Result<()> {
        w.r(t.map_or(ABSENT_REF, |t| t.0));
        w.hash(&t.map(|t| t.1));
        Ok(())
    }

    fn reemit<'a>(
        view: Option<CoercedAttrRef<'a>>,
        b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'a>,
    ) -> buck2_error::Result<()> {
        let t = view.map(|v| b.encode_view(v)).transpose()?;
        <Self as Field>::encode(t, b, w)
    }

    fn identity(view: Option<CoercedAttrRef<'_>>, h: &mut IdHasher<'_>) {
        match view {
            Some(c) => {
                h.h.write_u8(1);
                h.child(c);
            }
            None => h.h.write_u8(0),
        }
    }

    fn same(a: Option<CoercedAttrRef<'_>>, b: Option<CoercedAttrRef<'_>>) -> bool {
        a == b
    }
}

/// A count head field, then that many items of `T`, each `T::STRIDE`
/// child refs.
pub struct ChildrenOf<T>(std::marker::PhantomData<T>);

/// Child values.
pub type Children = ChildrenOf<Values>;
/// (key, value) child pairs.
pub type ChildPairs = ChildrenOf<Pairs>;

impl<T: RefItem + 'static> Field for ChildrenOf<T> {
    const DEPS: bool = true;
    const INLINE: bool = false;
    const HAS_TEXT: bool = false;
    type View<'a> = Refs<'a, T>;
    type Spec<'s> = &'s T::Spec;

    fn shape(peek: &dyn Fn(u32) -> Option<u32>, at: u32) -> Option<Extent> {
        Some(Extent {
            head: 1,
            refs: Some(peek(at)?.checked_mul(T::STRIDE)?),
            text: None,
        })
    }

    fn decode<'a>(c: &mut Cursor<'a>) -> Refs<'a, T> {
        let n = c.raw();
        Refs {
            run: c.refs(n * T::STRIDE),
            _item: std::marker::PhantomData,
        }
    }

    fn encode<'s>(
        items: &'s T::Spec,
        _b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'s>,
    ) -> buck2_error::Result<()> {
        let mut n = 0;
        let head_at = w.head.len();
        w.head(0);
        for (i, t) in T::tokens(items).enumerate() {
            w.r(t.0);
            w.hash(&t.1);
            n = i + 1;
        }
        w.head[head_at] = (n as u32) / T::STRIDE;
        Ok(())
    }

    fn reemit<'a>(
        view: Refs<'a, T>,
        b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'a>,
    ) -> buck2_error::Result<()> {
        w.head(view.len() as u32);
        for c in view.children() {
            let t = b.encode_view(c)?;
            w.r(t.0);
            w.hash(&t.1);
        }
        Ok(())
    }

    fn identity(view: Refs<'_, T>, h: &mut IdHasher<'_>) {
        h.h.write_usize(view.len());
        for c in view.children() {
            h.child(c);
        }
    }

    fn same(a: Refs<'_, T>, b: Refs<'_, T>) -> bool {
        a.len() == b.len() && a.children().zip(b.children()).all(|(x, y)| x == y)
    }
}

/// One child ref to a record of kind `K`, decoded as that kind's view and
/// hashed and compared by that kind's rules.
pub struct ChildOf<K>(std::marker::PhantomData<K>);

impl<K: Kind> Field for ChildOf<K> {
    const DEPS: bool = !K::SCALAR;
    const INLINE: bool = false;
    const HAS_TEXT: bool = false;
    type View<'a> = View<'a, K>;
    type Spec<'s> = AttrToken;

    fn shape(_peek: &dyn Fn(u32) -> Option<u32>, _at: u32) -> Option<Extent> {
        Some(Extent {
            head: 0,
            refs: Some(1),
            text: None,
        })
    }

    fn decode<'a>(c: &mut Cursor<'a>) -> View<'a, K> {
        K::decode(
            c.arena,
            c.child()
                .expect("should be present: only a selector default may be absent"),
        )
    }

    fn encode<'s>(
        AttrToken(r, hash): AttrToken,
        _b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'s>,
    ) -> buck2_error::Result<()> {
        w.r(r);
        w.hash(&hash);
        Ok(())
    }

    fn reemit<'a>(
        view: View<'a, K>,
        b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'a>,
    ) -> buck2_error::Result<()> {
        <Self as Field>::encode(K::copy_into(view, b)?, b, w)
    }

    fn identity(view: View<'_, K>, h: &mut IdHasher<'_>) {
        K::identity(view, h);
    }

    fn same(a: View<'_, K>, b: View<'_, K>) -> bool {
        K::same(a, b)
    }
}
