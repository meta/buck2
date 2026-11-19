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
use buck2_common::dice::cells::HasCellResolver;
use buck2_core::cells::CellResolver;
use buck2_interpreter::dice::starlark_types::GetStarlarkTypes;
use dice::DiceComputations;
use dice::EqualityBehavior;
use dice::Key;
use dice::OkPagableValueSerialize;
use dice::ValueSerialize;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use dupe::ResultDupedErrExt;
use pagable::Pagable;
use pagable::pagable_typetag;
use starlark::environment::GlobalFrozenHeapName;
use starlark::environment::Globals;

use crate::interpreter::configuror::BuildInterpreterConfiguror;
use crate::interpreter::context::HasInterpreterContext;
use crate::interpreter::globals::base_globals;

pagable::static_str!(GLOBAL_ENV_HEAP_NAME = concat!(module_path!(), "::global_env"));

/// Information shared across interpreters. Contains no cell-specific
/// information.
#[derive(Allocative, pagable::Pagable)]
pub struct GlobalInterpreterState {
    pub cell_resolver: CellResolver,

    /// The GlobalEnvironment contains all the globally available symbols
    /// (primarily starlark stdlib and Buck-provided functions).
    pub global_env: Globals,

    /// Interpreter Configurer
    pub configuror: Arc<BuildInterpreterConfiguror>,

    /// Check types in Starlark (or just parse and ignore).
    pub disable_starlark_types: bool,

    /// Static typechecking for bzl and bxl files.
    pub unstable_typecheck: bool,
}

impl GlobalInterpreterState {
    pub fn new(
        cell_resolver: CellResolver,
        interpreter_configuror: Arc<BuildInterpreterConfiguror>,
        disable_starlark_types: bool,
        unstable_typecheck: bool,
    ) -> buck2_error::Result<Self> {
        let global_env = base_globals()
            .with(|g| {
                if let Some(additional_globals) = interpreter_configuror.additional_globals() {
                    additional_globals.0.apply(g);
                }
            })
            .build_named(GlobalFrozenHeapName {
                name: GLOBAL_ENV_HEAP_NAME,
            });

        Ok(Self {
            cell_resolver,
            global_env,
            configuror: interpreter_configuror,
            disable_starlark_types,
            unstable_typecheck,
        })
    }

    pub fn configuror(&self) -> &Arc<BuildInterpreterConfiguror> {
        &self.configuror
    }

    pub fn globals(&self) -> &Globals {
        &self.global_env
    }
}

#[async_trait]
pub trait HasGlobalInterpreterState<'d> {
    async fn get_global_interpreter_state(
        &mut self,
    ) -> buck2_error::Result<&'d Arc<GlobalInterpreterState>>;
}

#[async_trait]
impl<'d> HasGlobalInterpreterState<'d> for DiceComputations<'d> {
    async fn get_global_interpreter_state(
        &mut self,
    ) -> buck2_error::Result<&'d Arc<GlobalInterpreterState>> {
        #[derive(
            Clone,
            derive_more::Display,
            Dupe,
            Debug,
            Eq,
            Hash,
            PartialEq,
            Allocative,
            Pagable
        )]
        #[display("{:?}", self)]
        #[pagable_typetag(dice::DiceKeyDyn)]
        struct GisKey();

        #[async_trait]
        impl Key for GisKey {
            type Value = buck2_error::Result<Arc<GlobalInterpreterState>>;
            async fn compute(
                &self,
                ctx: &mut DiceComputations,
                _cancellation: &CancellationContext,
            ) -> Self::Value {
                let interpreter_configuror = ctx.get_interpreter_configuror().await?.dupe();
                let cell_resolver = ctx.get_cell_resolver().await?.dupe();
                let disable_starlark_types = ctx.get_disable_starlark_types().await?;
                let unstable_typecheck = ctx.get_unstable_typecheck().await?;

                Ok(Arc::new(GlobalInterpreterState::new(
                    cell_resolver,
                    interpreter_configuror,
                    disable_starlark_types,
                    unstable_typecheck,
                )?))
            }

            fn equality_behavior() -> EqualityBehavior<Self::Value> {
                EqualityBehavior::AlwaysUnequal
            }

            fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
                OkPagableValueSerialize::<Self::Value>::new()
            }
        }

        self.compute(&GisKey()).await?.as_ref().duped_err()
    }
}
