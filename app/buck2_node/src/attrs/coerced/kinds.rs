/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The list of record kinds. Everything derived from the list as a whole
//! is generated here: the tag enum, the decoded-view enum, each kind as a
//! type, and the dispatchers. What one kind holds and means is declared
//! beside its attribute type.

use crate::attrs::arena::schema::KindTag;
use crate::attrs::arena::schema::KindVisitor;
use crate::attrs::arena::schema::arena_kinds;
use crate::attrs::coerced::semantics::Semantics;

arena_kinds! {
    /// Record kinds, by tag.
    enum CoercedAttrKind;
    /// Shallow decode of a [`crate::attrs::arena::CoercedAttrRef`]: one case
    /// per value kind, holding the kind's view, with children left as refs.
    enum CoercedAttrRefKind<'a>;
    impl KindTag;
    fn dispatch;
    fn dispatch_value;
    fn decode;
    trait ValueVisitor: Semantics;
    // Kinds whose values carry nothing beyond the kind.
    unit {
        None = 0,
    }
    // Attribute value kinds.
    value {
        Bool = 1,
        /// Inline as a 26-bit sign-extended payload when it fits; otherwise a
        /// record with head `[lo, hi]`: the zigzag-encoded value in 32-bit halves.
        Int = 2,
        WithinView = 3,
        String = 4,
        EnumVariant = 5,
        List = 6,
        Tuple = 7,
        Dict = 8,
        Selector = 9,
        Concat = 10,
        OneOf = 11,
        Dep = 12,
        Label = 13,
        SourceLabel = 14,
        ConfigurationDep = 15,
        SplitTransitionDep = 16,
        PluginDep = 17,
        SourceFile = 18,
        Visibility = 19,
        /// `Arg` with no macros (`StringWithMacros::StringPart`) -- most `Arg`
        /// values in real graphs contain no macros.
        ArgSimple = 20,
        /// Parts are `ArgSimple` or `ArgMacro` records.
        Arg = 22,
        /// The parsed form of an `attrs.query()` value.
        Query = 25,
        /// A `select_fail()` / `select_incompatible()` leaf holding the message.
        SelectFail = 26,
        SelectIncompatible = 27,
        /// An `attrs.configured_dep()` value.
        ExplicitConfiguredDep = 28,
        /// The dynamic transition target is absent when the attr type fixes the
        /// transition.
        TransitionDep = 29,
        Modifiers = 30,
        Metadata = 31,
    }
    // Record kinds inside other records; never attribute values.
    internal {
        /// Head: `[n, idx...]`: `n` providers-label table indices, a selector's
        /// key set. Never appears as an attribute value ref.
        SelectorKeys = 21,
        /// Head: `[header, payload index, n]`; refs: `n` per the header's
        /// variant tag. Never appears as an attribute value ref.
        ArgMacro = 23,
        /// Head: `[subtag, nv, nv values, n]`; refs: `n` per `Q_EXPR_*`. Encodes
        /// the parsed form of a query macro's expression; target literals are
        /// label table indices, text is shared text records. Never appears as
        /// an attribute value ref.
        QueryExpr = 24,
    }
}
