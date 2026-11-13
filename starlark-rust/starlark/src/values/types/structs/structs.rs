/*
 * Copyright 2018 The Starlark in Rust Authors.
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

//! Implementation of `struct` function.

use allocative::Allocative;
use dupe::Dupe;
use mini_map::MiniFixedSortedMap;
use pagable::Pagable;
use starlark_derive::starlark_module;

use crate as starlark;
use crate::codemap::Span;
use crate::environment::GlobalsBuilder;
use crate::eval::Arguments;
use crate::typing::ParamSpec;
use crate::typing::Ty;
use crate::typing::call_args::TyCallArgs;
use crate::typing::callable::TyCallable;
use crate::typing::error::TypingOrInternalError;
use crate::typing::function::TyCustomFunctionImpl;
use crate::typing::oracle::ctx::TypingOracleCtx;
use crate::typing::structs::TyStruct;
use crate::util::arc_str::ArcStr;
use crate::values::Heap;
use crate::values::ValueOf;
use crate::values::structs::value::Struct;

#[derive(
    Allocative, Clone, Copy, Dupe, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Pagable
)]
struct StructType;

impl TyCustomFunctionImpl for StructType {
    fn is_type(&self) -> bool {
        // `struct` is declared `as_type = Struct`, so at runtime it has a
        // `.type` attribute and can be used in a type expression (`struct | None`).
        true
    }

    fn as_callable(&self) -> TyCallable {
        // TODO(nga): this should be obtained from function signature from function definition.
        TyCallable::new(ParamSpec::kwargs(Ty::any()), Ty::any_struct())
    }

    fn validate_call(
        &self,
        _span: Span,
        args: &TyCallArgs,
        oracle: TypingOracleCtx,
    ) -> Result<Ty, TypingOrInternalError> {
        if let [pos, ..] = args.pos.as_slice() {
            return Err(oracle.msg_error(pos.span, "Positional arguments not allowed"));
        }
        let mut fields = Vec::new();
        for named in &args.named {
            let (name, ty) = &named.node;
            fields.push((ArcStr::from(*name), ty.clone()));
        }
        let extra = args.kwargs.is_some();
        Ok(Ty::custom(TyStruct {
            fields: MiniFixedSortedMap::from_iter(fields),
            extra,
        }))
    }
}

/// Register `struct` builtin.
#[starlark_module]
pub(crate) fn register_struct(builder: &mut GlobalsBuilder) {
    #[starlark(
        ty_custom_function = StructType,
        as_type = Struct<'static>,
    )]
    fn r#struct<'v>(
        args: &Arguments<'v, '_>,
        heap: Heap<'v>,
    ) -> starlark::Result<ValueOf<'v, Struct<'v>>> {
        args.no_positional_args(heap)?;
        // TODO(nga): missing optimization: practically most `struct` invocations are
        //   performed with fixed named arguments, e.g. `struct(a = 1, b = 2)`.
        //   In this case we can avoid allocating the map, but instead
        //   allocate field index once at compilation time and store field values in a vector.
        Ok(heap.alloc(Struct::new(args.names_map()?)))
    }
}
