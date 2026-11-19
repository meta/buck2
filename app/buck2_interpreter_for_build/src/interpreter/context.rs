/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use allocative::Allocative;
use async_trait::async_trait;
use derive_more::Display;
use dice::DiceComputations;
use dice::DiceTransactionUpdater;
use dice::EqualityBehavior;
use dice::InjectedKey;
use dice::PagableValueSerialize;
use dice::ValueSerialize;
use dupe::Dupe;
use pagable::Pagable;
use pagable::pagable_typetag;

use crate::interpreter::configuror::BuildInterpreterConfiguror;

#[derive(Clone, Dupe, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("{:?}", self)]
#[pagable_typetag(dice::DiceKeyDyn)]
struct BuildContextKey();

impl InjectedKey for BuildContextKey {
    type Value = Arc<BuildInterpreterConfiguror>;

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        PagableValueSerialize::<Self::Value>::new()
    }
}

#[async_trait]
pub trait HasInterpreterContext<'d> {
    async fn get_interpreter_configuror(
        &mut self,
    ) -> buck2_error::Result<&'d Arc<BuildInterpreterConfiguror>>;
}

#[async_trait]
impl<'d> HasInterpreterContext<'d> for DiceComputations<'d> {
    async fn get_interpreter_configuror(
        &mut self,
    ) -> buck2_error::Result<&'d Arc<BuildInterpreterConfiguror>> {
        Ok(self.compute(&BuildContextKey()).await?)
    }
}

pub trait SetInterpreterContext {
    fn set_interpreter_context(
        &mut self,
        interpreter_configuror: Arc<BuildInterpreterConfiguror>,
    ) -> buck2_error::Result<()>;
}

impl SetInterpreterContext for DiceTransactionUpdater {
    fn set_interpreter_context(
        &mut self,
        interpreter_configuror: Arc<BuildInterpreterConfiguror>,
    ) -> buck2_error::Result<()> {
        Ok(self.changed_to(vec![(BuildContextKey(), interpreter_configuror)])?)
    }
}
