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

use dice::UserComputationData;
use dupe::Dupe;

#[derive(Clone, Default, Dupe)]
pub struct Mergebase(pub Arc<Option<String>>); // Base revision

pub trait SetMergebase {
    fn set_mergebase(&mut self, mergebase: Mergebase);
}

pub trait GetMergebase {
    fn get_mergebase(&self) -> &Mergebase;
}

impl SetMergebase for UserComputationData {
    fn set_mergebase(&mut self, mergebase: Mergebase) {
        self.data.set(mergebase);
    }
}

impl GetMergebase for UserComputationData {
    fn get_mergebase(&self) -> &Mergebase {
        self.data
            .get::<Mergebase>()
            .expect("mergebase should be set")
    }
}
