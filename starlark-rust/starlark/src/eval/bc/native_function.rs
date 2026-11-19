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

use allocative::Allocative;
use dupe::Dupe;

use crate::eval::Arguments;
use crate::eval::Evaluator;
use crate::pagable::StarlarkDeserialize;
use crate::pagable::StarlarkDeserializeContext;
use crate::pagable::StarlarkSerialize;
use crate::pagable::StarlarkSerializeContext;
use crate::pagable::starlark_deserialize_context::deserialize_for_inspection;
use crate::values::Value;
use crate::values::ValueTyped;
use crate::values::function::NativeFunc;
use crate::values::function::NativeFunction;

/// Pointer to a native function optimized for bytecode execution, at the brand of the bytecode.
#[derive(Copy, Clone, Dupe, Allocative)]
pub(crate) struct BcNativeFunction<'v> {
    fun: ValueTyped<'v, NativeFunction<'v>>,
    /// Copy function here from `fun` to avoid extra dereference when calling.
    #[allocative(skip)]
    imp: &'v NativeFunc<'v>,
}

// Only the value is on the wire; `imp` is recomputed from `fun` on
// deserialize via `BcNativeFunction::new`.
impl<'v> StarlarkSerialize for BcNativeFunction<'v> {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        self.fun.starlark_serialize(ctx)
    }
}

impl<'v> StarlarkDeserialize<'v> for BcNativeFunction<'v> {
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'v>,
    ) -> crate::Result<Self> {
        let fun = deserialize_for_inspection(ctx)?;
        Ok(BcNativeFunction::new(fun))
    }
}

impl<'v> BcNativeFunction<'v> {
    pub(crate) fn new(fun: ValueTyped<'v, NativeFunction<'v>>) -> BcNativeFunction<'v> {
        BcNativeFunction {
            fun,
            imp: &fun.as_ref().function,
        }
    }

    #[inline]
    pub(crate) fn fun(&self) -> ValueTyped<'v, NativeFunction<'v>> {
        self.fun
    }

    #[inline]
    pub(crate) fn to_value(&self) -> Value<'v> {
        self.fun.to_value()
    }

    #[inline]
    pub(crate) fn invoke(
        &self,
        args: &Arguments<'v, '_>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> crate::Result<Value<'v>> {
        self.imp.invoke(eval, args)
    }
}
