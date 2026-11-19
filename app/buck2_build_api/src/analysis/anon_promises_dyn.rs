/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::OnceCell;
use std::rc::Rc;

use async_trait::async_trait;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_interpreter::factory::ReentrantStarlarkEvaluator;
use dice::DiceComputations;
use starlark::eval::Evaluator;

#[async_trait(?Send)]
pub trait AnonPromisesDyn<'v>: 'v {
    /// Resolves the pending promises. When `record_requested` is set, returns the
    /// anon target keys that were resolved (as the allocations DICE shares between
    /// requesters); otherwise returns nothing.
    async fn run_promises<'a, 'e: 'a>(
        self: Box<Self>,
        accessor: &mut dyn RunAnonPromisesAccessor<'v, 'a, 'e>,
        record_requested: bool,
    ) -> buck2_error::Result<Vec<BaseDeferredKey>>
    where
        'v: 'a;
}

pub trait RunAnonPromisesAccessor<'v: 'a, 'a, 'e> {
    fn with_evaluator(
        &mut self,
        closure: &mut dyn FnMut(&mut Evaluator<'v, 'a, 'e>) -> buck2_error::Result<()>,
    ) -> buck2_error::Result<()>;

    fn via_dice_impl<'s: 'b, 'b>(
        &'s mut self,
        f: Box<dyn for<'d> FnOnce(&'s mut DiceComputations<'d>) + 'b>,
    );
}

pub struct RunAnonPromisesAccessorPair<'me, 'v, 'a, 'e, 'd>(
    pub &'me mut ReentrantStarlarkEvaluator<'v, 'a, 'e>,
    pub &'me mut DiceComputations<'d>,
);

impl<'me, 'v, 'a, 'e, 'd> RunAnonPromisesAccessor<'v, 'a, 'e>
    for RunAnonPromisesAccessorPair<'me, 'v, 'a, 'e, 'd>
{
    fn with_evaluator(
        &mut self,
        closure: &mut dyn FnMut(&mut Evaluator<'v, 'a, 'e>) -> buck2_error::Result<()>,
    ) -> buck2_error::Result<()> {
        self.0.with_evaluator(closure)
    }

    fn via_dice_impl<'s: 'b, 'b>(
        &'s mut self,
        f: Box<dyn for<'d2> FnOnce(&'s mut DiceComputations<'d2>) + 'b>,
    ) {
        f(self.1)
    }
}

impl<'v: 'a, 'a, 'e> dyn RunAnonPromisesAccessor<'v, 'a, 'e> + '_ {
    pub fn with_dice<'s, T: 's>(
        &'s mut self,
        f: impl for<'d> FnOnce(&'s mut DiceComputations<'d>) -> T,
    ) -> T {
        // We can't capture a &mut res here in the closure unfortunately, so we need to do this little dance to get values out.
        let res: Rc<OnceCell<T>> = Rc::new(OnceCell::new());
        let res2 = res.clone();
        self.via_dice_impl(Box::new(move |dice| {
            res2.set(f(dice)).ok().unwrap();
        }));
        Rc::try_unwrap(res).ok().unwrap().take().unwrap()
    }
}
