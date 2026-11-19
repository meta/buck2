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
use dupe::Dupe;
use pagable::Pagable;
use pagable::pagable_typetag;

use crate::deferred::base_deferred_key::BaseDeferredKey;
use crate::deferred::dyn_eval_kind_key::DynEvalKindKey;
use crate::deferred::key::DeferredHolderKey;
use crate::fs::dynamic_actions_action_key::DynamicActionsActionKey;

/// The base key. We can actually get rid of this and just use 'DeferredKey' if rule analysis is an
/// 'Deferred' itself. This is used to construct the composed 'DeferredKey::Deferred' or
/// 'DeferredKey::Base' type.
#[derive(
    Hash,
    Eq,
    PartialEq,
    Clone,
    Dupe,
    derive_more::Display,
    Debug,
    Allocative,
    strong_hash::StrongHash,
    Pagable
)]
#[display("{_0}_{_1}")]
#[pagable_typetag(DynEvalKindKey)]
pub struct DynamicLambdaResultsKey(DeferredHolderKey, DynamicLambdaIndex);

impl DynamicLambdaResultsKey {
    pub fn new(key: DeferredHolderKey, idx: DynamicLambdaIndex) -> Self {
        Self(key, idx)
    }

    pub fn owner(&self) -> &BaseDeferredKey {
        self.0.owner()
    }

    pub fn holder_key(&self) -> &DeferredHolderKey {
        &self.0
    }

    pub fn dynamic_actions_index(&self) -> DynamicLambdaIndex {
        self.1
    }

    pub fn action_key(&self) -> DynamicActionsActionKey {
        let mut v = match self.0.action_key() {
            Some(v) => v.as_str().to_owned(),
            None => String::new(),
        };
        write!(&mut v, "_{}", self.1).unwrap();
        DynamicActionsActionKey::new(&v).unwrap()
    }
}

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
    Pagable
)]
pub struct DynamicLambdaIndex(u32);

impl DynamicLambdaIndex {
    pub fn new(v: u32) -> Self {
        Self(v)
    }

    pub fn as_u32(&self) -> u32 {
        self.0
    }
}
