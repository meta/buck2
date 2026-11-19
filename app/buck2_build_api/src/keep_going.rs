/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use dice::DiceComputations;
use dice::UserComputationData;
use futures::Future;
use futures::FutureExt;

pub struct KeepGoing;

impl KeepGoing {
    pub fn try_compute_join_all<'a, 'd, Items, Mapper, T, R, E>(
        ctx: &'a mut DiceComputations<'d>,
        items: Items,
        mapper: Mapper,
    ) -> impl Future<Output = Result<Vec<R>, E>> + Send + use<'a, 'd, Items, Mapper, T, R, E>
    where
        Items: IntoIterator<Item = T>,
        Items::IntoIter: ExactSizeIterator,
        Mapper: for<'x> AsyncFnOnce(&'x mut DiceComputations<'d>, T) -> Result<R, E>
            + Send
            + Sync
            + Copy,
        for<'x> <Mapper as AsyncFnOnce<(&'x mut DiceComputations<'d>, T)>>::CallOnceFuture: Send,
        T: Send,
        R: Send,
        E: Send,
    {
        let keep_going = ctx.per_transaction_data().get_keep_going();

        if keep_going {
            ctx.compute_join(items, mapper)
                .map(|v| v.into_iter().try_collect::<Vec<_>>())
                .left_future()
        } else {
            ctx.try_compute_join(items, mapper).right_future()
        }
    }
}

pub struct KeepGoingHolder(bool);

pub trait HasKeepGoing {
    fn set_keep_going(&mut self, keep_going: bool);

    fn get_keep_going(&self) -> bool;
}

impl HasKeepGoing for UserComputationData {
    fn set_keep_going(&mut self, keep_going: bool) {
        self.data.set(KeepGoingHolder(keep_going));
    }

    fn get_keep_going(&self) -> bool {
        self.data
            .get::<KeepGoingHolder>()
            .expect("KeepGoing should be set")
            .0
    }
}
