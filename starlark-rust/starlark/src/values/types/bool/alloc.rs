/*
 * Copyright 2019 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use crate::values::AllocFrozenValue;
use crate::values::AllocValue;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::ValueOf;
use crate::values::bool::value::VALUE_FALSE_TRUE;

impl<'v> ValueOf<'v, bool> {
    /// `True` or `False`, which are statics.
    #[inline]
    pub(crate) fn new_bool(x: bool) -> Self {
        // Indexing into a static keeps this branchless, as for `Value::new_bool`.
        ValueOf::from_representation(VALUE_FALSE_TRUE[x as usize].at())
    }
}

impl<'v> AllocValue<'v> for bool {
    type Type = bool;

    #[inline]
    fn alloc_value(self, _heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        ValueOf::new_bool(self)
    }
}

impl<'fv> AllocFrozenValue<'fv> for bool {
    type Type = bool;

    #[inline]
    fn alloc_frozen_value(self, _heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        ValueOf::new_bool(self)
    }
}
