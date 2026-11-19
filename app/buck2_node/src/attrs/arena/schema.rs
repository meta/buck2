/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The kinds table and the per-kind structure it derives: record shape,
//! decoding, copying, identity hashing and equality.

use crate::attrs::arena::builder::CoercedAttrArenaBuilder;
use crate::attrs::arena::fields::Cursor;
use crate::attrs::arena::fields::Extent;
use crate::attrs::arena::fields::Field;
use crate::attrs::arena::fields::IdHasher;
use crate::attrs::arena::fields::RecordWriter;
use crate::attrs::arena::storage::AttrToken;
use crate::attrs::arena::storage::AttrWord;
use crate::attrs::arena::storage::CoercedAttrArena;
use crate::attrs::arena::storage::INLINE_KIND_MASK;
use crate::attrs::arena::view::CoercedAttrRef;
use crate::attrs::coerced::kinds::CoercedAttrKind;
use crate::attrs::coerced::kinds::dispatch;

/// Defines everything that follows from the list of kinds as a whole. The
/// invocation names each item it defines, and the per-kind trait the value
/// visitor is bounded on; `unit` kinds have no view,
/// `value` kinds' view-enum cases hold the kind's [`Kind::View`], and
/// `internal` kinds are records inside other records, never attribute
/// values.
macro_rules! arena_kinds {
    (
        $(#[$tag_doc:meta])* enum $tag:ident;
        $(#[$view_doc:meta])* enum $view:ident<$lt:lifetime>;
        impl $kind_tag:ident;
        fn $dispatch:ident;
        fn $dispatch_value:ident;
        fn $decode:ident;
        trait $visitor:ident: $semantics:ident;
        unit { $( $(#[$udoc:meta])* $u:ident = $utag:literal, )* }
        value { $( $(#[$vdoc:meta])* $v:ident = $vtag:literal, )* }
        internal { $( $(#[$idoc:meta])* $i:ident = $itag:literal, )* }
    ) => {
        $(#[$tag_doc])*
        #[derive(Copy, Clone, PartialEq, Eq, Debug, ::strum::FromRepr)]
        #[repr(u8)]
        pub enum $tag {
            $( $(#[$udoc])* $u = $utag, )*
            $( $(#[$vdoc])* $v = $vtag, )*
            $( $(#[$idoc])* $i = $itag, )*
        }

        $( pub struct $u; )*
        $( pub struct $v; )*
        $( pub struct $i; )*

        $( impl $kind_tag for $u { const KIND: $tag = $tag::$u; } )*
        $( impl $kind_tag for $v { const KIND: $tag = $tag::$v; } )*
        $( impl $kind_tag for $i { const KIND: $tag = $tag::$i; } )*

        $(#[$view_doc])*
        pub enum $view<$lt> {
            $( $u, )*
            $( $v($crate::attrs::arena::schema::View<$lt, $v>), )*
        }

        /// Runs `v` for the [`Kind`] type of `kind`.
        pub(crate) fn $dispatch<V: KindVisitor>(kind: $tag, v: V) -> V::Out {
            match kind {
                $( $tag::$u => v.visit::<$u>(), )*
                $( $tag::$v => v.visit::<$v>(), )*
                $( $tag::$i => v.visit::<$i>(), )*
            }
        }

        /// A visitor over the attribute value kinds, each of which implements
        /// `$semantics`; `visit` receives the value's decoded view.
        pub(crate) trait $visitor<$lt> {
            type Out;
            fn visit<K: $semantics + 'static>(
                self,
                view: $crate::attrs::arena::schema::View<$lt, K>,
            ) -> Self::Out;
        }

        /// Decodes `v` and runs `op` for its kind, which must be an attribute
        /// value kind.
        pub(crate) fn $dispatch_value<$lt, V: $visitor<$lt>>(
            v: $crate::attrs::arena::CoercedAttrRef<$lt>,
            op: V,
        ) -> V::Out {
            match v.arena.kind_of(v.r) {
                $( $tag::$u => op.visit::<$u>(<$u as $crate::attrs::arena::schema::Kind>::decode(v.arena, v.r)), )*
                $( $tag::$v => op.visit::<$v>(<$v as $crate::attrs::arena::schema::Kind>::decode(v.arena, v.r)), )*
                $( $tag::$i => unreachable!("not an attribute value: {:?}", $tag::$i), )*
            }
        }

        /// The decoded view of the value at `r`.
        pub(crate) fn $decode<$lt>(
            arena: &$lt $crate::attrs::arena::CoercedAttrArena,
            r: $crate::attrs::arena::AttrWord,
        ) -> $view<$lt> {
            match arena.kind_of(r) {
                $( $tag::$u => $view::$u, )*
                $( $tag::$v => $view::$v(<$v as $crate::attrs::arena::schema::Kind>::decode(arena, r)), )*
                $( $tag::$i => unreachable!("not an attribute value: {:?}", $tag::$i), )*
            }
        }
    };
}
pub(crate) use arena_kinds;

/// The tag of a kind, from its row in the kinds list.
pub trait KindTag {
    const KIND: CoercedAttrKind;
}

impl CoercedAttrKind {
    /// Kinds whose values hold no deps, inputs, or labels: a traversal can
    /// pass over them without decoding. Select outcomes are scalar for the
    /// unconfigured walk only; the configured walk must report them.
    pub(crate) fn is_scalar(self) -> bool {
        dispatch(self, IsScalar)
    }
}

/// How a kind's values are stored.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum Storage {
    /// The word carries the kind and the single field's value whenever that
    /// value fits the payload; a record of that one field otherwise.
    Inline,
    /// A record of head fields and child refs, deduped by content.
    Record,
    /// A text record, deduped by bytes.
    Text,
}

pub(crate) const fn storage_class(kind: CoercedAttrKind, has_text: bool, inline: bool) -> Storage {
    if has_text {
        Storage::Text
    } else if inline && (kind as u32) <= INLINE_KIND_MASK {
        Storage::Inline
    } else {
        Storage::Record
    }
}

/// One record kind as a type: a tag from the kinds list and the [`Field`]
/// its values are, one field or a tuple of them in sequence. Everything
/// else about the kind derives from that field. [`dispatch`] selects the
/// type from a value's tag and monomorphizes a visitor per kind, so every
/// operation that is uniform over kinds is written once.
///
/// This trait, and everything a kind's [`View`] is spelled through, is
/// `pub` inside crate-private modules: the public
/// [`CoercedAttrRef::downcast`] returns the view, and an associated type
/// may only be given in terms of nominally public items. None of it is
/// reachable from outside the crate.
pub trait Kind: KindTag + 'static {
    type Fields: Field;

    /// How values are stored follows from the field and the tag: a text
    /// tail makes a text record; a single head value fits the word, if the
    /// tag fits the word's kind bits; anything else is a record.
    const STORAGE: Storage = storage_class(
        Self::KIND,
        <Self::Fields as Field>::HAS_TEXT,
        <Self::Fields as Field>::INLINE,
    );

    /// Whether values hold no deps: no labels, paths or children, so
    /// traversals have nothing to visit.
    const SCALAR: bool = !<Self::Fields as Field>::DEPS;

    /// What a record occupies, reading head fields through `peek`; `None`
    /// when a field the layout depends on is missing.
    fn shape(peek: &dyn Fn(u32) -> Option<u32>) -> Option<Extent> {
        Self::Fields::shape(peek, 0)
    }

    fn decode<'a>(arena: &'a CoercedAttrArena, r: AttrWord) -> View<'a, Self> {
        let mut c = Cursor::new(arena, r);
        Self::Fields::decode(&mut c)
    }

    /// Re-encode into `b`, children first.
    fn copy_into(
        parts: View<'_, Self>,
        b: &mut CoercedAttrArenaBuilder,
    ) -> buck2_error::Result<AttrToken> {
        let mut w = RecordWriter::new(Self::KIND);
        Self::Fields::reemit(parts, b, &mut w)?;
        b.emit(Self::STORAGE, w)
    }

    /// Identity hash of the parts; the kind byte is already written.
    fn identity(parts: View<'_, Self>, h: &mut IdHasher<'_>) {
        Self::Fields::identity(parts, h)
    }

    fn same(a: View<'_, Self>, b: View<'_, Self>) -> bool {
        Self::Fields::same(a, b)
    }
}

/// The decoded value of a kind: leaves as borrowed side-table values or
/// immediates, children as refs.
pub type View<'a, K> = <<K as Kind>::Fields as Field>::View<'a>;

/// Encode a kind's value from its field specs.
pub(crate) fn encode_kind<K: Kind>(
    b: &mut CoercedAttrArenaBuilder,
    spec: <K::Fields as Field>::Spec<'_>,
) -> buck2_error::Result<AttrToken> {
    let mut w = RecordWriter::new(K::KIND);
    K::Fields::encode(spec, b, &mut w)?;
    b.emit(K::STORAGE, w)
}

/// Selects the [`Kind`] type for a tag at runtime, monomorphizing the
/// visitor per kind.
pub(crate) trait KindVisitor {
    type Out;
    fn visit<K: Kind + 'static>(self) -> Self::Out;
}

/// [`CoercedAttrKind::is_scalar`].
pub(super) struct IsScalar;

impl KindVisitor for IsScalar {
    type Out = bool;
    fn visit<K: Kind + 'static>(self) -> bool {
        K::SCALAR
    }
}

/// Record shape from a kind's fields: [`shape`].
pub(super) struct Shape<'f> {
    pub(super) peek: &'f dyn Fn(u32) -> Option<u32>,
}

impl KindVisitor for Shape<'_> {
    type Out = Option<Extent>;
    fn visit<K: Kind + 'static>(self) -> Self::Out {
        K::shape(self.peek)
    }
}

/// Copies a value into a builder: [`CoercedAttrArenaBuilder::encode_view`].
pub(super) struct CopyParts<'b, 'a> {
    pub(super) v: CoercedAttrRef<'a>,
    pub(super) b: &'b mut CoercedAttrArenaBuilder,
}

impl KindVisitor for CopyParts<'_, '_> {
    type Out = buck2_error::Result<AttrToken>;
    fn visit<K: Kind + 'static>(self) -> Self::Out {
        K::copy_into(K::decode(self.v.arena, self.v.r), self.b)
    }
}

/// Folds a value's parts into the identity hasher: [`CoercedAttrArena::strong_of`].
pub(super) struct HashParts<'h, 'm, 'a> {
    pub(super) v: CoercedAttrRef<'a>,
    pub(super) h: &'h mut IdHasher<'m>,
}

impl KindVisitor for HashParts<'_, '_, '_> {
    type Out = ();
    fn visit<K: Kind + 'static>(self) {
        K::identity(K::decode(self.v.arena, self.v.r), self.h)
    }
}

/// Compares two values of the same kind: [`CoercedAttrRef::content_eq`].
pub(super) struct EqParts<'a, 'b> {
    pub(super) a: CoercedAttrRef<'a>,
    pub(super) b: CoercedAttrRef<'b>,
}

impl KindVisitor for EqParts<'_, '_> {
    type Out = bool;
    fn visit<K: Kind + 'static>(self) -> bool {
        K::same(
            K::decode(self.a.arena, self.a.r),
            K::decode(self.b.arena, self.b.r),
        )
    }
}
