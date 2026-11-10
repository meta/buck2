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
use crate::values::FrozenHeap;
use crate::values::StarlarkValue;
use crate::values::ValueOf;
use crate::values::starlark_value;

#[derive(
    Debug,
    derive_more::Display,
    Allocative,
    ProvidesStaticType,
    NoSerialize,
    StarlarkPagable
)]
#[display("{}", Self::TYPE)]
pub(crate) struct TypingAny;

#[starlark_value(type = "typing.Any")]
impl<'v> StarlarkValue<'v> for TypingAny {
    fn documentation(&self) -> DocItem {
        DocItem::Type(DocType {
            docs: DocString::from_docstring(
                crate::docs::DocStringKind::Rust,
                "\
This type matches any type.

Any value can be assigned to a name or parameter of this type.
See also [`typing.Any` in the Python documentation][1].

[1]: https://docs.python.org/3/library/typing.html#typing.Any",
            ),
            ..DocType::from_starlark_value::<Self>()
        })
    }

    fn eval_type(&self) -> Option<Ty> {
        Some(Ty::any())
    }
}

static_starlark_value!(ANY: TypingAny = TypingAny);

impl<'fv> AllocFrozenValue<'fv> for TypingAny {
    type Type = Self;

    fn alloc_frozen_value(self, _heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        ANY.at().into()
    }
}

#[cfg(test)]
mod tests {
    use crate::assert;

    #[test]
    fn test_any_runtime() {
        assert::is_true("isinstance(1, typing.Any)");
    }

    #[test]
    fn test_any_compile_time() {
        assert::pass(
            r#"
def f(x: typing.Any):
    pass

f(1)
"#,
        );
    }
}
