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

//! Handle special "unbound" globals: methods or attributes.

use std::fmt;
use std::fmt::Debug;
use std::fmt::Formatter;

use dupe::Dupe;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::eval::Arguments;
use crate::eval::Evaluator;
use crate::eval::runtime::frame_span::FrameSpan;
use crate::values::FrozenValueTyped;
use crate::values::Heap;
use crate::values::HeapEdge;
use crate::values::Value;
use crate::values::function::BoundMethod;
use crate::values::function::NativeAttribute;
use crate::values::function::NativeMethod;

/// A method or attribute of a type, before it is bound to an instance. Lives in a
/// [`Methods`](crate::environment::Methods) table.
#[derive(Clone, Copy, Dupe, ProvidesStaticType, crate::StarlarkPagable)]
pub(crate) enum UnboundValue<'v> {
    /// A method with `this` unbound.
    Method(FrozenValueTyped<'v, NativeMethod<'v>>),
    /// An attribute with `this` unbound.
    Attr(FrozenValueTyped<'v, NativeAttribute<'v>>),
}

impl<'v> Debug for UnboundValue<'v> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("MaybeUnboundValue").finish_non_exhaustive()
    }
}

impl UnboundValue<'static> {
    /// The member, for use with any heap.
    ///
    /// Methods tables are only ever reached as `&'static Methods`, which is what makes their
    /// members immortal and the `'static` brand honest, see
    /// [`HeapEdge::immortal`](HeapEdge::immortal).
    #[inline]
    pub(crate) fn at<'v>(&'static self) -> UnboundValue<'v> {
        HeapEdge::immortal().rebrand(*self)
    }
}

impl<'v> UnboundValue<'v> {
    #[inline]
    pub(crate) fn to_value(self) -> Value<'v> {
        match self {
            UnboundValue::Method(m) => m.to_value(),
            UnboundValue::Attr(a) => a.to_value(),
        }
    }

    /// Bind this object to given `this` value.
    #[inline]
    pub(crate) fn bind(self, this: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        match self {
            UnboundValue::Method(m) => Ok(heap.alloc_complex(BoundMethod::new(this, m))),
            UnboundValue::Attr(a) => a.invoke(this, heap),
        }
    }

    #[inline]
    pub(crate) fn invoke_method(
        self,
        this: Value<'v>,
        span: &'v FrameSpan<'v>,
        args: &Arguments<'v, '_>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> crate::Result<Value<'v>> {
        match self {
            UnboundValue::Method(m) => eval.with_call_stack(m.to_value(), Some(span), |eval| {
                m.function.invoke(eval, this, args)
            }),
            UnboundValue::Attr(a) => {
                let value = a.invoke(this, eval.heap())?;
                value.invoke_with_loc(Some(span), args, eval)
            }
        }
    }
}
