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
use starlark_derive::NoSerialize;
use starlark_derive::ProvidesStaticType;
use starlark_derive::StarlarkPagable;

use crate as starlark;
use crate::docs::DocItem;
use crate::docs::DocString;
use crate::docs::DocType;
use crate::static_starlark_value;
use crate::typing::Ty;
use crate::values::AllocFrozenValue;
use crate::values::AllocValue;
use crate::values::FrozenHeap;
use crate::values::StarlarkValue;
use crate::values::ValueOf;
use crate::values::starlark_value;
use crate::values::type_repr::StarlarkTypeRepr;

#[derive(
    Debug,
    derive_more::Display,
    Allocative,
    ProvidesStaticType,
    NoSerialize,
    StarlarkPagable
)]
#[display("{}", Self::TYPE)]
pub(crate) struct TypingNever;

#[starlark_value(type = "typing.Never")]
impl<'v> StarlarkValue<'v> for TypingNever {
    fn documentation(&self) -> DocItem {
        DocItem::Type(DocType {
            docs: DocString::from_docstring(
                crate::docs::DocStringKind::Rust,
                "\
This type can never be constructed.

Equivalent to [Python's `typing.Never`][1], it is Starlark's representation
of the [bottom type][2]. A function returning `typing.Never` will never
return. A function taking an argument of `typing.Never` can never be called.

See also [`typing.Never` in the Python documentation][1].

[1]: https://docs.python.org/3/library/typing.html#typing.Never
[2]: https://en.wikipedia.org/wiki/Bottom_type",
            ),
            ..DocType::from_starlark_value::<Self>()
        })
    }

    fn eval_type(&self) -> Option<Ty> {
        Some(Ty::never())
    }
}

static_starlark_value!(NEVER: TypingNever = TypingNever);

impl<'fv> AllocFrozenValue<'fv> for TypingNever {
    type Type = Self;

    fn alloc_frozen_value(self, _heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        NEVER.at().into()
    }
}

/// Never type, can be used as native function return type.
pub enum StarlarkNever {}

impl StarlarkTypeRepr for StarlarkNever {
    type Canonical = Self;

    fn starlark_type_repr() -> Ty {
        Ty::never()
    }
}

impl<'v> AllocValue<'v> for StarlarkNever {
    type Type = Self;

    fn alloc_value(self, _heap: crate::values::Heap<'v>) -> ValueOf<'v, Self::Type> {
        match self {}
    }
}

#[cfg(test)]
mod tests {
    use crate::assert;

    #[test]
    fn test_never_runtime() {
        assert::is_true("not isinstance(1, typing.Never)");
    }

    #[test]
    fn test_never_compile_time() {
        assert::pass(
            r#"
def f() -> typing.Never:
    return fail()
"#,
        );
    }
}
