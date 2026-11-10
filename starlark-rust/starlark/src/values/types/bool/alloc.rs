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
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::types::bool::StarlarkBool;

impl<'v> AllocValue<'v> for bool {
    type Type = StarlarkBool;

    fn alloc_value(self, _heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        ValueOf::new_unchecked(Value::new_bool(self))
    }
}

impl<'fv> AllocFrozenValue<'fv> for bool {
    type Type = StarlarkBool;

    fn alloc_frozen_value(self, _heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        ValueOf::new_unchecked(Value::new_bool(self))
    }
}
