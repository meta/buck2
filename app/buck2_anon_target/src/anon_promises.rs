/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use async_trait::async_trait;
use buck2_build_api::analysis::anon_promises_dyn::AnonPromisesDyn;
use buck2_build_api::analysis::anon_promises_dyn::RunAnonPromisesAccessor;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_interpreter::starlark_promise::StarlarkPromise;
use dupe::Dupe;
use either::Either;
use futures::FutureExt;
use starlark::eval::Evaluator;
use starlark::values::Trace;
use starlark::values::ValueTyped;
use starlark::values::list::AllocList;

use crate::anon_targets::AnonTargetKey;

#[derive(Default, Debug, Trace, Allocative)]
pub(crate) struct AnonPromises<'v> {
    // The actual data
    entries: Vec<(
        ValueTyped<'v, StarlarkPromise<'v>>,
        // Either a single entry, or a list that becomes a list of providers
        Either<AnonTargetKey, Vec<AnonTargetKey>>,
    )>,
}

impl<'v> AnonPromises<'v> {
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn push_one(
        &mut self,
        promise: ValueTyped<'v, StarlarkPromise<'v>>,
        one: AnonTargetKey,
    ) {
        self.entries.push((promise, Either::Left(one)));
    }
}

#[async_trait(?Send)]
impl<'v> AnonPromisesDyn<'v> for AnonPromises<'v> {
    async fn run_promises<'a, 'e: 'a>(
        self: Box<Self>,
        accessor: &mut dyn RunAnonPromisesAccessor<'v, 'a, 'e>,
        record_requested: bool,
    ) -> buck2_error::Result<Vec<BaseDeferredKey>>
    where
        'v: 'a,
    {
        // Resolve all the targets in parallel
        // We have vectors of vectors, so we create a "shape" which has the same shape but with indices
        let mut shape = Vec::new();
        let mut anon_target_keys = Vec::new();
        for (promise, xs) in self.entries {
            match xs {
                Either::Left(x) => {
                    shape.push((promise, Either::Left(shape.len())));
                    anon_target_keys.push(x);
                }
                Either::Right(xs) => {
                    shape.push((promise, Either::Right(shape.len()..shape.len() + xs.len())));
                    anon_target_keys.extend(xs);
                }
            }
        }

        let values = accessor
            .with_dice(|dice| {
                dice.try_compute_join(anon_target_keys.iter(), async |dice, anon_target_key| {
                    let (key, result) = anon_target_key.resolve_with_key(dice).await?;
                    buck2_error::Ok((key, result.dupe()))
                })
                .boxed_local()
            })
            .await?;

        accessor.with_evaluator(&mut |eval: &mut Evaluator| {
            // But must bind the promises sequentially
            for (promise, xs) in shape.iter() {
                match xs {
                    Either::Left(i) => {
                        let val = values[*i].1.providers()?.add_heap_ref(eval.heap());
                        promise.resolve(val.to_value(), eval)?
                    }
                    Either::Right(is) => {
                        let xs: Vec<_> = is
                            .clone()
                            .map(|i| Ok(values[i].1.providers()?.add_heap_ref(eval.heap())))
                            .collect::<buck2_error::Result<_>>()?;
                        let list = eval.heap().alloc(AllocList(xs)).to_value();
                        promise.resolve(list, eval)?
                    }
                }
            }
            Ok(())
        })?;

        if !record_requested {
            return Ok(Vec::new());
        }
        // Record the key allocations DICE holds, not this requester's copies:
        // every analysis requesting the same anon target then shares one.
        Ok(values
            .into_iter()
            .map(|(key, _)| BaseDeferredKey::AnonTarget(key.0.dupe()))
            .collect::<Vec<_>>())
    }
}
