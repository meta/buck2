/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::hash::Hash;
use std::hash::Hasher;
use std::sync::Arc;

use allocative::Allocative;
use buck2_hash::BuckMutMap;
use buck2_util::strong_hasher::StrongHash128;
use dupe::Dupe;
use pagable::Pagable;

use crate::attrs::arena::builder::CoercedAttrArenaBuilder;
use crate::attrs::arena::storage::AttrToken;
use crate::attrs::arena::storage::AttrWord;
use crate::attrs::arena::storage::CoercedAttrArena;
use crate::attrs::arena::view::CoercedAttrRef;

/// The owned form of one attribute value, for holders that outlive any
/// package (rule defaults): arena-encoded in its own single-value arena,
/// so it needs no package. Equality and hashing are
/// content-based, so two `OwnedCoercedAttr`s encoding equal values compare
/// equal regardless of which arena holds them.
#[derive(Clone, Dupe, Debug, Allocative, Pagable)]
pub struct OwnedCoercedAttr {
    pub(super) arena: Arc<CoercedAttrArena>,
    pub(super) word: AttrWord,
}

impl OwnedCoercedAttr {
    /// A value `f` encodes into an arena of its own.
    pub fn new_with(
        f: impl FnOnce(&mut CoercedAttrArenaBuilder) -> buck2_error::Result<AttrToken>,
    ) -> buck2_error::Result<OwnedCoercedAttr> {
        let mut builder = CoercedAttrArenaBuilder::default();
        let value = f(&mut builder)?;
        Self::seal_from_builder(builder, value)
    }

    /// Seal `builder` into this value's own arena; `token` must have been
    /// minted by `builder`.
    pub fn seal_from_builder(
        mut builder: CoercedAttrArenaBuilder,
        token: AttrToken,
    ) -> buck2_error::Result<OwnedCoercedAttr> {
        let word = builder.root(token);
        Ok(OwnedCoercedAttr {
            arena: Arc::new(builder.finish()),
            word,
        })
    }

    /// A handle to a value of `arena`, without re-encoding.
    pub(crate) fn from_parts(arena: Arc<CoercedAttrArena>, word: AttrWord) -> OwnedCoercedAttr {
        OwnedCoercedAttr { arena, word }
    }

    /// A handle to `child`, which must be a view into this value's arena.
    pub fn sub(&self, child: CoercedAttrRef<'_>) -> OwnedCoercedAttr {
        debug_assert!(
            std::ptr::eq(&*self.arena, child.arena),
            "child view is not backed by this value's arena"
        );
        OwnedCoercedAttr {
            arena: self.arena.dupe(),
            word: child.r,
        }
    }

    pub fn as_ref(&self) -> CoercedAttrRef<'_> {
        self.arena.view(self.word)
    }

    /// The identity-grade strong hash of the value. Roots read the retained
    /// hash; interior values (`sub` handles) compute it on demand.
    pub fn strong_hash(&self) -> StrongHash128 {
        self.arena
            .root_strong_hash(self.word)
            .unwrap_or_else(|| self.arena.strong_of(self.word, &mut BuckMutMap::default()))
    }
}

impl PartialEq for OwnedCoercedAttr {
    fn eq(&self, other: &OwnedCoercedAttr) -> bool {
        // The 128-bit blake3 content hash IS equality (`CoercedAttrArena::
        // strong_of`); debug builds keep verifying that structurally.
        let eq = self.strong_hash() == other.strong_hash();
        if eq {
            debug_assert!(
                self.as_ref().content_eq(&other.as_ref()),
                "identity hashes matched but content differs"
            );
        }
        eq
    }
}

impl Eq for OwnedCoercedAttr {}

impl Hash for OwnedCoercedAttr {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.strong_hash().hash(state);
    }
}
