/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The record buffer and the words that address it.
//!
//! An attribute value is either an inline leaf, held entirely in its
//! [`AttrWord`], or a tagged record in the buffer, whose word is the byte
//! offset of the record's tag:
//!
//! ```text
//! AttrWord: [ 1 | kind:5 | payload:26 ]      inline leaf: None, bools,
//!                                            small ints, table indices
//!           [ 0 | record offset:31 ]         everything else
//!
//! record:   tag:u8 = [ width:2 | kind:6 ]   head fields...   tail
//!
//!   head    the record's immediate content (counts, table indices, split
//!           ints) as `width + 1`-byte little-endian fields; how many, and
//!           what follows, is the kind's field list (its row in the kinds
//!           table);
//!   tail    text bytes (length in the first head field), or child refs,
//!           `width + 1` bytes each: bit 0 set = an inline leaf, kind in the
//!           next five bits and payload above; bit 0 clear = a record ref,
//!           the backward delta from the record's own offset above; a zero
//!           field marks an absent value. The width is chosen per record
//!           from its largest head field or ref.
//! ```
//!
//! A value's kind costs at most one byte read: inline leaves carry it in
//! the word, records in their tag, so a traversal over deps opens only
//! containers. Worked example: `{"srcs": ["a.c"]}`, a dict of one pair
//! whose value is a one-element list of a source file:
//!
//! ```text
//! bytes:  [String  4 "srcs"]   [List  1  SourceFile#0]   [Dict  1  ->"srcs" ->list]
//!          ^ S                  ^ L                      ^ D = the value's own ref
//!
//! the list's one ref is the inline source leaf (path table index 0); the
//! dict's refs are deltas D - S and D - L, all one byte wide here.
//! ```
//!
//! Offsets are indices, not addresses: the buffer can grow while the
//! package coerces, and the sealed form is position-independent. Refs only
//! ever point at already-appended records (children are encoded before
//! their parents), so deltas are never negative and a ref cannot be
//! observed half-built.

use std::hash::Hash;
use std::hash::Hasher;

use allocative::Allocative;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::soft_error;
use buck2_core::target::label::label::TargetLabel;
use buck2_error::buck2_error;
use buck2_hash::BuckMutMap;
use buck2_util::strong_hasher::Blake3StrongHasher;
use buck2_util::strong_hasher::StrongHash128;
use dupe::Dupe;
use either::Either;
use pagable::Pagable;
use pagable::PagableDeserialize;
use pagable::PagableDeserializer;
use pagable::PagableSerialize;
use pagable::PagableSerializer;
use strong_hash::StrongHash;

use crate::attrs::arena::builder::CoercedAttrArenaBuilder;
use crate::attrs::arena::fields::IdHasher;
use crate::attrs::arena::schema::HashParts;
use crate::attrs::arena::schema::Shape;
use crate::attrs::arena::view::CoercedAttrRef;
use crate::attrs::coerced::kinds::CoercedAttrKind;
use crate::attrs::coerced::kinds::dispatch;
use crate::attrs::coerced_path::CoercedPath;
use crate::metadata::map::MetadataMap;
use crate::modifiers::CfgModifiers;
use crate::visibility::VisibilitySpecification;
use crate::visibility::WithinViewSpecification;

/// Tag byte of a record: `[ width:2 | kind:6 ]`. `width` is the byte width
/// of every head field and ref in the record, minus one.
pub(super) const TAG_KIND_MASK: u8 = 0x3F;
pub(super) const TAG_WIDTH_SHIFT: u8 = 6;

/// The tail of a record body, sized.
#[derive(Copy, Clone)]
enum TailLen {
    None,
    Text(u32),
    Refs(u32),
}

/// Head field count and tail of a `kind` record whose head field `i` reads
/// as `field(i)`; `None` when a field the layout depends on is missing.
#[inline]
fn shape(kind: CoercedAttrKind, field: impl Fn(u32) -> Option<u32>) -> Option<(u32, TailLen)> {
    let e = dispatch(kind, Shape { peek: &field })?;
    let tail = match (e.text, e.refs) {
        (Some(len), _) => TailLen::Text(len),
        (None, Some(n)) => TailLen::Refs(n),
        (None, None) => TailLen::None,
    };
    Some((e.head, tail))
}

/// A `width`-byte little-endian field at `at`, if in bounds.
#[inline]
pub(super) fn field_at(bytes: &[u8], at: usize, width: u8) -> Option<u32> {
    let b = bytes.get(at..at + width as usize)?;
    Some(match width {
        1 => b[0] as u32,
        2 => u16::from_le_bytes([b[0], b[1]]) as u32,
        3 => (b[0] as u32) | ((b[1] as u32) << 8) | ((b[2] as u32) << 16),
        _ => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
    })
}

#[inline]
pub(crate) fn zigzag(i: i64) -> u64 {
    ((i << 1) ^ (i >> 63)) as u64
}

#[inline]
pub(crate) fn unzigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

/// Reference to an attribute value in a [`CoercedAttrArena`]: the byte
/// offset of the value's record. Only meaningful together with the arena
/// that minted it.
#[derive(
    Copy, Clone, Dupe, PartialEq, Eq, Hash, StrongHash, Debug, Allocative, Pagable
)]
pub struct AttrWord(pub(super) u32);

/// Content hash of an encoded attribute value, as computed by
/// [`CoercedAttrArenaBuilder`]: kind-tagged, folded over child hashes, and stable
/// across arenas. Travels alongside an [`AttrWord`] so consumers can hash
/// and prefilter equality without decoding.
#[derive(
    Copy, Clone, Dupe, PartialEq, Eq, Hash, StrongHash, Debug, Allocative, Pagable
)]
pub struct CoercedAttrHash(pub u64);

/// A value encoded into an arena: its ref, and the content hash a parent
/// folds into its own. This is what coercion hands around and what the
/// per-kind encoders return.
#[derive(Copy, Clone, Dupe, PartialEq, Eq, Debug, Allocative)]
pub struct AttrToken(pub AttrWord, pub CoercedAttrHash);

impl AttrToken {
    pub fn word(self) -> AttrWord {
        self.0
    }
}

/// Word layout: the inline flag, then for an inline leaf the kind and the
/// payload, for a record ref the offset.
pub(super) const INLINE_BIT: u32 = 1 << 31;
pub(super) const OFFSET_MASK: u32 = INLINE_BIT - 1;
pub(super) const INLINE_KIND_BITS: u32 = 5;
pub(super) const INLINE_KIND_MASK: u32 = (1 << INLINE_KIND_BITS) - 1;
pub(crate) const INLINE_PAYLOAD_BITS: u32 = 26;
pub(super) const INLINE_KIND_SHIFT: u32 = INLINE_PAYLOAD_BITS;
pub(crate) const INLINE_PAYLOAD_MASK: u32 = (1 << INLINE_PAYLOAD_BITS) - 1;

/// Ref field layout, inside a record's tail: the inline flag is the low
/// bit, so a small inline leaf fits a one-byte field; above it an inline
/// leaf carries its kind then its payload, a record ref its backward delta.
const FIELD_INLINE_FLAG: u32 = 1;
const FIELD_KIND_SHIFT: u32 = 1;
const FIELD_PAYLOAD_SHIFT: u32 = FIELD_KIND_SHIFT + INLINE_KIND_BITS;
const FIELD_DELTA_SHIFT: u32 = 1;

/// Small ints are sign-extended inline payloads.
pub(crate) const INT_INLINE_MAX: i64 = (1 << (INLINE_PAYLOAD_BITS - 1)) - 1;
pub(crate) const INT_INLINE_MIN: i64 = -(1 << (INLINE_PAYLOAD_BITS - 1));

impl AttrWord {
    /// An inline leaf; `payload` must fit [`INLINE_PAYLOAD_MASK`].
    pub(super) fn inline(kind: CoercedAttrKind, payload: u32) -> AttrWord {
        debug_assert!(payload <= INLINE_PAYLOAD_MASK);
        AttrWord(INLINE_BIT | ((kind as u32) << INLINE_KIND_SHIFT) | payload)
    }

    /// Whether this is an inline leaf rather than a record ref.
    #[inline]
    pub(super) fn is_inline(self) -> bool {
        self.0 & INLINE_BIT != 0
    }

    /// The kind and payload of an inline leaf, or `None` for a record ref.
    #[inline]
    pub(super) fn inline_parts(self) -> Option<(CoercedAttrKind, u32)> {
        if !self.is_inline() {
            return None;
        }
        let kind =
            CoercedAttrKind::from_repr(((self.0 >> INLINE_KIND_SHIFT) & INLINE_KIND_MASK) as u8)
                .expect("should be valid: words are only minted with known kinds");
        Some((kind, self.0 & INLINE_PAYLOAD_MASK))
    }

    /// A record ref to the record at `off`.
    #[inline]
    pub(super) fn record(off: u32) -> AttrWord {
        AttrWord(off)
    }

    /// Byte offset of this record ref's tag.
    #[inline]
    pub(super) fn off(self) -> usize {
        debug_assert!(!self.is_inline(), "inline leaves have no record");
        (self.0 & OFFSET_MASK) as usize
    }

    /// The field value that encodes this word inside the record at `rec`.
    #[inline]
    pub(super) fn to_field(self, rec: u32) -> u32 {
        if self == ABSENT_REF {
            0
        } else if let Some((kind, payload)) = self.inline_parts() {
            (payload << FIELD_PAYLOAD_SHIFT)
                | ((kind as u32) << FIELD_KIND_SHIFT)
                | FIELD_INLINE_FLAG
        } else {
            (rec - self.0) << FIELD_DELTA_SHIFT
        }
    }

    /// Inverse of [`Self::to_field`]; `None` for an absent value.
    #[inline]
    pub(super) fn from_field(field: u32, rec: u32) -> Option<AttrWord> {
        if field == 0 {
            None
        } else if field & FIELD_INLINE_FLAG != 0 {
            let kind = (field >> FIELD_KIND_SHIFT) & INLINE_KIND_MASK;
            let payload = field >> FIELD_PAYLOAD_SHIFT;
            Some(AttrWord(INLINE_BIT | (kind << INLINE_KIND_SHIFT) | payload))
        } else {
            Some(AttrWord(rec - (field >> FIELD_DELTA_SHIFT)))
        }
    }
}

/// A ref slot with nothing in it (a selector without a default); written
/// as a zero field.
pub(super) const ABSENT_REF: AttrWord = AttrWord(u32::MAX);

/// The record buffer. Reads take its text records as UTF-8 on trust: the
/// encoder only writes `&str` content, and a buffer from anywhere else is
/// validated once, as it pages in.
#[derive(Debug, Default, Allocative)]
pub(super) struct RecordBytes(pub(super) Vec<u8>);

impl std::ops::Deref for RecordBytes {
    type Target = Vec<u8>;

    fn deref(&self) -> &Vec<u8> {
        &self.0
    }
}

impl std::ops::DerefMut for RecordBytes {
    fn deref_mut(&mut self) -> &mut Vec<u8> {
        &mut self.0
    }
}

impl PagableSerialize for RecordBytes {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> pagable::Result<()> {
        self.0.pagable_serialize(serializer)
    }
}

impl<'de> PagableDeserialize<'de> for RecordBytes {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        let bytes = Vec::<u8>::pagable_deserialize(deserializer)?;
        validate_records(&bytes).map_err(|e| pagable::anyhow!("Corrupt attr arena: {e}"))?;
        Ok(RecordBytes(bytes))
    }
}

/// Checks that `bytes` is a well-formed sequence of records whose text
/// records hold UTF-8 and whose refs land on earlier records. Records are
/// self-delimiting, so this is one pass in storage order.
pub(super) fn validate_records(bytes: &[u8]) -> Result<(), String> {
    let mut starts: Vec<u32> = Vec::new();
    let mut pos = 0usize;
    while pos < bytes.len() {
        let rec = pos;
        let t = bytes[pos];
        let kind = CoercedAttrKind::from_repr(t & TAG_KIND_MASK)
            .ok_or_else(|| format!("record at {rec} has invalid tag {t:#x}"))?;
        let width = (t >> TAG_WIDTH_SHIFT) + 1;
        let start = pos + 1;
        let (nfields, tail) = shape(kind, |i| {
            field_at(bytes, start + i as usize * width as usize, width)
        })
        .ok_or_else(|| format!("record at {rec} is truncated"))?;
        let tail_start = start + nfields as usize * width as usize;
        let end = match tail {
            TailLen::None => tail_start,
            TailLen::Text(len) => {
                let text = bytes
                    .get(tail_start..tail_start + len as usize)
                    .ok_or_else(|| format!("record at {rec} is truncated"))?;
                std::str::from_utf8(text)
                    .map_err(|_| format!("text record at {rec} is not UTF-8"))?;
                tail_start + len as usize
            }
            TailLen::Refs(n) => {
                for i in 0..n as usize {
                    let field = field_at(bytes, tail_start + i * width as usize, width)
                        .ok_or_else(|| format!("record at {rec} is truncated"))?;
                    // Zero is an absent ref and inline leaves need no
                    // target; the rest are backward deltas.
                    if field != 0 && field & FIELD_INLINE_FLAG == 0 {
                        (rec as u32)
                            .checked_sub(field >> FIELD_DELTA_SHIFT)
                            .filter(|target| starts.binary_search(target).is_ok())
                            .ok_or_else(|| {
                                format!("record at {rec} has a ref that is not an earlier record")
                            })?;
                    }
                }
                tail_start + n as usize * width as usize
            }
        };
        if end > bytes.len() {
            return Err(format!("record at {rec} is truncated"));
        }
        starts.push(rec as u32);
        pos = end;
    }
    Ok(())
}

/// Sealed arena of coerced attribute values for one package.
#[derive(Debug, Default, Allocative, Pagable)]
pub struct CoercedAttrArena {
    /// The buffer every [`AttrWord`] points into: one tagged record per
    /// value.
    pub(super) bytes: RecordBytes,
    /// Globally interned `ProvidersLabel`s, indexed by `Dep`, `Label`,
    /// `SourceLabel`, `ConfigurationDep`, `SplitTransitionDep`, and
    /// `SelectorKeys` records.
    pub(crate) providers_labels: Vec<ProvidersLabel>,
    /// Globally interned `TargetLabel`s, indexed by `PluginDep` records.
    pub(super) target_labels: Vec<TargetLabel>,
    /// Source paths, indexed by `SourceFile` records.
    pub(crate) paths: Vec<CoercedPath>,
    /// Whole-value leaves by table index, one entry per distinct value in
    /// the package.
    pub(super) visibility: Vec<VisibilitySpecification>,
    pub(super) within_view: Vec<WithinViewSpecification>,
    pub(super) metadata: Vec<MetadataMap>,
    /// `modifiers` values, indexed by `Modifiers` records; each shared with
    /// every holder that outlives the arena.
    pub(super) modifiers: Vec<CfgModifiers>,
    /// Identity-grade content hashes for root values (the values referenced
    /// from outside the arena: one per unique top-level attribute value),
    /// sorted by word for binary search. Read on the value-identity paths:
    /// attr equality for early cutoff when a package re-evaluates, and
    /// target hashing. Interior values carry no retained strong hash;
    /// 128-bit blake3 over full content, so equal hash implies equal value.
    // TODO(jtbraun): fold these parallel vecs into a `starlark_map` `Vec2`
    // once `pagable` has an impl for it.
    pub(super) root_words: Vec<u32>,
    pub(super) root_strong: Vec<StrongHash128>,
}

impl CoercedAttrArena {
    pub fn builder() -> CoercedAttrArenaBuilder {
        CoercedAttrArenaBuilder::default()
    }

    /// Total retained bytes of the record buffer.
    pub fn byte_len(&self) -> usize {
        self.bytes.len()
    }

    pub fn view(&self, r: AttrWord) -> CoercedAttrRef<'_> {
        CoercedAttrRef { arena: self, r }
    }

    /// The identity-grade content hash of a root value, if `r` was interned
    /// as a root (`CoercedAttrArenaBuilder::root`).
    pub fn root_strong_hash(&self, r: AttrWord) -> Option<StrongHash128> {
        self.root_words
            .binary_search(&r.0)
            .ok()
            .map(|i| self.root_strong[i])
    }

    /// The record's kind and the byte width of its refs. Record refs only.
    #[inline]
    pub(super) fn tag(&self, r: AttrWord) -> (CoercedAttrKind, u8) {
        let t = self.bytes[r.off()];
        let kind = CoercedAttrKind::from_repr(t & TAG_KIND_MASK)
            .expect("should be valid: records are only written by the encoder");
        (kind, (t >> TAG_WIDTH_SHIFT) + 1)
    }

    #[inline]
    pub(crate) fn kind_of(&self, r: AttrWord) -> CoercedAttrKind {
        match r.inline_parts() {
            Some((kind, _)) => kind,
            None => self.tag(r).0,
        }
    }

    /// The record at `r`, located per its kind's layout. Record refs only.
    #[inline]
    pub(super) fn open(&self, r: AttrWord) -> Rec<'_> {
        let (kind, width) = self.tag(r);
        let rec = r.off() as u32;
        let start = r.off() + 1;
        let (nfields, tail) = shape(kind, |i| {
            field_at(&self.bytes, start + i as usize * width as usize, width)
        })
        .expect("should be in bounds: records are only written by the encoder");
        let run = |start: usize, len: u32| RefRun {
            arena: self,
            rec,
            start,
            len,
            width,
        };
        let tail_start = start + nfields as usize * width as usize;
        let tail = match tail {
            TailLen::None => Tail::None,
            TailLen::Text(len) => Tail::Text(&self.bytes[tail_start..tail_start + len as usize]),
            TailLen::Refs(n) => Tail::Refs(run(tail_start, n)),
        };
        Rec {
            kind,
            head: run(start, nfields),
            tail,
        }
    }

    /// Identity-grade content hash of one value: blake3 over the value's kind
    /// and its parts, children by their own identity hashes. Equal content
    /// yields equal hashes across arenas and processes.
    pub(super) fn strong_of(
        &self,
        r: AttrWord,
        memo: &mut BuckMutMap<u32, StrongHash128>,
    ) -> StrongHash128 {
        if let Some(&h) = memo.get(&r.0) {
            return h;
        }
        let kind = self.kind_of(r);
        let mut h = IdHasher {
            h: Blake3StrongHasher::new(),
            memo,
        };
        h.h.write_u8(kind as u8);
        dispatch(
            kind,
            HashParts {
                v: self.view(r),
                h: &mut h,
            },
        );
        let hash = h.h.finalize128();
        memo.insert(r.0, hash);
        hash
    }

    /// Raw bytes of a text record, without UTF-8 validation: the string-dedup
    /// equality probe needs only byte equality.
    pub(super) fn bytes_at(&self, r: AttrWord) -> &[u8] {
        match self.open(r).tail {
            Tail::Text(bytes) => bytes,
            _ => unreachable!("should be text: only text records are read as bytes"),
        }
    }

    pub(crate) fn str_at(&self, r: AttrWord) -> &str {
        let bytes = self.bytes_at(r);
        debug_assert!(std::str::from_utf8(bytes).is_ok());
        // SAFETY: text records are written from `&str`, and a buffer from
        // outside this process is validated when it pages in (`RecordBytes`).
        unsafe { std::str::from_utf8_unchecked(bytes) }
    }
}

/// Smallest width (1-4 bytes) holding `v`.
#[inline]
fn width_for(v: u32) -> u8 {
    if v < 1 << 8 {
        1
    } else if v < 1 << 16 {
        2
    } else if v < 1 << 24 {
        3
    } else {
        4
    }
}

/// Writes. The arena has no dedup of its own: [`CoercedAttrArenaBuilder`]
/// decides what to append and the arena appends it.
impl CoercedAttrArena {
    /// Append one record: tag, head fields, text, then refs, every field
    /// and ref in the smallest width that fits the largest of them.
    pub(super) fn append_record(
        &mut self,
        kind: CoercedAttrKind,
        fields: &[u32],
        text: &[u8],
        refs: &[AttrWord],
    ) -> buck2_error::Result<AttrWord> {
        let off = self.bytes.len();
        let rec = off as u32;
        let max_field = fields
            .iter()
            .copied()
            .chain(refs.iter().map(|&r| r.to_field(rec)))
            .max()
            .unwrap_or(0);
        let width = width_for(max_field);
        let end = off + 1 + (fields.len() + refs.len()) * width as usize + text.len();
        // Record offsets are 31 bits: an encoder-bug backstop at 2 GiB per
        // package, deliberately not a user-input limit.
        const CAPACITY: usize = INLINE_BIT as usize;
        if end > CAPACITY {
            return Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "Package attribute storage exceeds 2GiB of encoded attribute content; split the package or reduce attribute sizes"
            ));
        }
        // One warning per arena, on the crossing allocation: offsets are
        // monotone, so the threshold is crossed exactly once.
        const CAPACITY_WARN: usize = CAPACITY / 2;
        if off < CAPACITY_WARN && end >= CAPACITY_WARN {
            soft_error!(
                "attr_arena_near_capacity",
                buck2_error!(
                    buck2_error::ErrorTag::Environment,
                    "Attr arena buffer past 50% of its 2GiB capacity ({end} bytes)"
                )
            );
        }
        self.bytes
            .push(kind as u8 | ((width - 1) << TAG_WIDTH_SHIFT));
        let w = width as usize;
        for &f in fields {
            self.bytes.extend_from_slice(&f.to_le_bytes()[..w]);
        }
        self.bytes.extend_from_slice(text);
        for &r in refs {
            self.bytes
                .extend_from_slice(&r.to_field(rec).to_le_bytes()[..w]);
        }
        Ok(AttrWord::record(rec))
    }

    /// Whether the record at `r` is `kind` with head `fields`, text `text`,
    /// and refs that resolve to `refs`. Refs are compared by target since
    /// their widths and deltas depend on placement.
    pub(super) fn record_matches(
        &self,
        r: AttrWord,
        kind: CoercedAttrKind,
        fields: &[u32],
        text: &[u8],
        refs: &[AttrWord],
    ) -> bool {
        let rec = self.open(r);
        if rec.kind != kind || rec.head.len() as usize != fields.len() {
            return false;
        }
        if !rec.head.iter_raw().eq(fields.iter().copied()) {
            return false;
        }
        match rec.tail {
            Tail::None => refs.is_empty(),
            Tail::Text(got) => got == text,
            Tail::Refs(run) => {
                run.len() as usize == refs.len()
                    && refs
                        .iter()
                        .enumerate()
                        .all(|(i, &want)| match run.get_opt(i as u32) {
                            Some(got) => got == want,
                            None => want == ABSENT_REF,
                        })
            }
        }
    }

    /// Side-table appends; each returns the new entry's index.
    pub(super) fn push_providers_label(&mut self, l: &ProvidersLabel) -> u32 {
        self.providers_labels.push(l.dupe());
        self.providers_labels.len() as u32 - 1
    }

    pub(super) fn push_target_label(&mut self, l: TargetLabel) -> u32 {
        self.target_labels.push(l);
        self.target_labels.len() as u32 - 1
    }

    pub(super) fn push_path(&mut self, p: CoercedPath) -> u32 {
        self.paths.push(p);
        self.paths.len() as u32 - 1
    }

    /// Release the growth slack of every table; the sealed arena is retained
    /// for the life of the package.
    pub(super) fn shrink_to_fit(&mut self) {
        let CoercedAttrArena {
            bytes,
            providers_labels,
            target_labels,
            paths,
            visibility,
            within_view,
            metadata,
            modifiers,
            root_words,
            root_strong,
        } = self;
        bytes.shrink_to_fit();
        providers_labels.shrink_to_fit();
        target_labels.shrink_to_fit();
        paths.shrink_to_fit();
        visibility.shrink_to_fit();
        within_view.shrink_to_fit();
        metadata.shrink_to_fit();
        modifiers.shrink_to_fit();
        root_words.shrink_to_fit();
        root_strong.shrink_to_fit();
    }
}

/// One located record: its head fields and tail.
#[derive(Copy, Clone)]
pub(super) struct Rec<'a> {
    pub(super) kind: CoercedAttrKind,
    pub(super) head: RefRun<'a>,
    pub(super) tail: Tail<'a>,
}

#[derive(Copy, Clone)]
pub(super) enum Tail<'a> {
    None,
    Text(&'a [u8]),
    Refs(RefRun<'a>),
}

/// A run of fixed-width values inside one record: ref fields (see
/// [`AttrWord::to_field`]) or raw head values.
#[derive(Copy, Clone)]
pub(crate) struct RefRun<'a> {
    pub(crate) arena: &'a CoercedAttrArena,
    pub(super) rec: u32,
    pub(super) start: usize,
    pub(super) len: u32,
    pub(super) width: u8,
}

impl<'a> RefRun<'a> {
    /// The `i`th fixed-width value, little-endian.
    #[inline]
    pub(crate) fn raw(&self, i: u32) -> u32 {
        let w = self.width as usize;
        let b = &self.arena.bytes[self.start + i as usize * w..][..w];
        match w {
            1 => b[0] as u32,
            2 => u16::from_le_bytes([b[0], b[1]]) as u32,
            3 => (b[0] as u32) | ((b[1] as u32) << 8) | ((b[2] as u32) << 16),
            _ => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        }
    }

    #[inline]
    pub(crate) fn get(&self, i: u32) -> AttrWord {
        self.get_opt(i)
            .expect("should be present: only a selector default may be absent")
    }

    #[inline]
    pub(super) fn get_opt(&self, i: u32) -> Option<AttrWord> {
        AttrWord::from_field(self.raw(i), self.rec)
    }

    pub(crate) fn len(&self) -> u32 {
        self.len
    }

    /// The run without its first `n` refs.
    pub(crate) fn skip(&self, n: u32) -> RefRun<'a> {
        RefRun {
            start: self.start + n as usize * self.width as usize,
            len: self.len - n,
            ..*self
        }
    }

    /// The raw values in order. One bounds check for the whole run; the
    /// width is matched once so each arm is a fixed-stride loop.
    pub(crate) fn iter_raw(&self) -> impl ExactSizeIterator<Item = u32> + use<'a> {
        let w = self.width as usize;
        let bytes = &self.arena.bytes[self.start..self.start + self.len as usize * w];
        match w {
            1 => Either::Left(Either::Left(bytes.iter().map(|&b| b as u32))),
            2 => Either::Left(Either::Right(
                bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes([c[0], c[1]]) as u32),
            )),
            3 => {
                Either::Right(Either::Left(bytes.as_chunks::<3>().0.iter().map(|c| {
                    (c[0] as u32) | ((c[1] as u32) << 8) | ((c[2] as u32) << 16)
                })))
            }
            _ => Either::Right(Either::Right(
                bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])),
            )),
        }
    }

    pub(crate) fn iter(&self) -> impl ExactSizeIterator<Item = AttrWord> + use<'a> {
        let rec = self.rec;
        self.iter_raw().map(move |f| {
            AttrWord::from_field(f, rec)
                .expect("should be present: only a selector default may be absent")
        })
    }
}
