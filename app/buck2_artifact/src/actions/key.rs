/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Write;

use allocative::Allocative;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_core::deferred::key::DeferredHolderKey;
use buck2_data::ToProtoMessage;
use buck2_util::size_assert;
use dupe::Dupe;
use pagable::Pagable;
use starlark::values::StarlarkPagableViaPagable;
use starlark_map::Hashed;

/// A key to look up an 'Action' from the 'ActionAnalysisResult'.
/// Since 'Action's are registered as 'Deferred's
#[derive(
    Debug,
    Eq,
    PartialEq,
    Hash,
    Clone,
    Dupe,
    derive_more::Display,
    starlark::values::Trace,
    Allocative,
    strong_hash::StrongHash,
    Pagable
)]
#[display("(target: `{parent}`, id: `{id}`)")]
pub struct ActionKey {
    parent: DeferredHolderKey,
    id: ActionIndex,
}

size_assert::words_of_type!(ActionKey, 4);

/// An unique identifier for different actions with the same parent.
#[derive(
    Debug,
    Eq,
    PartialEq,
    Hash,
    Clone,
    Dupe,
    Copy,
    derive_more::Display,
    Allocative,
    strong_hash::StrongHash,
    Pagable,
    StarlarkPagableViaPagable
)]
pub struct ActionIndex(pub u32);
impl ActionIndex {
    pub fn new(v: u32) -> ActionIndex {
        Self(v)
    }
}

impl<'fv> starlark::pagable::SmallMapKeyDeserialize<'fv> for ActionIndex {
    fn starlark_deserialize_hashed(
        ctx: &mut dyn starlark::pagable::StarlarkDeserializeContext<'_, 'fv>,
    ) -> starlark::Result<Hashed<Self>> {
        let k = <Self as starlark::pagable::StarlarkDeserialize<'fv>>::starlark_deserialize(ctx)?;
        Ok(Hashed::new(k))
    }
}

impl ActionKey {
    pub fn new(parent: DeferredHolderKey, id: ActionIndex) -> ActionKey {
        ActionKey { parent, id }
    }

    pub fn holder_key(&self) -> &DeferredHolderKey {
        &self.parent
    }

    pub fn action_index(&self) -> ActionIndex {
        self.id
    }

    pub fn owner(&self) -> &BaseDeferredKey {
        self.parent.owner()
    }

    fn action_key(&self) -> String {
        let mut v = match self.parent.action_key() {
            Some(v) => v.as_str().to_owned(),
            None => String::new(),
        };
        write!(&mut v, "_{}", self.id).unwrap();
        v
    }
}

impl ToProtoMessage for ActionKey {
    type Message = buck2_data::ActionKey;

    fn as_proto(&self) -> Self::Message {
        buck2_data::ActionKey {
            id: (self.id.0 as usize).to_ne_bytes().to_vec(),
            owner: Some(self.owner().to_proto().into()),
            key: self.action_key(),
        }
    }
}
