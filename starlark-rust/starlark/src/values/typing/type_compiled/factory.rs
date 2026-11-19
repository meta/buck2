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

use crate::pagable::static_value::static_type_compiled;
use crate::typing::Ty;
use crate::typing::custom::TyCustom;
use crate::values::Heap;
use crate::values::typing::type_compiled::alloc::TypeMatcherAlloc;
use crate::values::typing::type_compiled::compiled::TypeCompiled;
use crate::values::typing::type_compiled::matcher::TypeMatcher;
use crate::values::typing::type_compiled::matchers::IsAny;
use crate::values::typing::type_compiled::matchers::IsBool;
use crate::values::typing::type_compiled::matchers::IsInt;
use crate::values::typing::type_compiled::matchers::IsNone;
use crate::values::typing::type_compiled::matchers::IsStr;
use crate::values::typing::type_compiled::type_matcher_factory::TypeMatcherFactory;

// Static type-compiled values for common types.
static_type_compiled!(pub(crate) TYPE_COMPILED_NONE: IsNone, Ty::none());
static_type_compiled!(pub(crate) TYPE_COMPILED_BOOL: IsBool, Ty::bool());
static_type_compiled!(pub(crate) TYPE_COMPILED_INT: IsInt, Ty::int());
static_type_compiled!(pub(crate) TYPE_COMPILED_STRING: IsStr, Ty::string());

/// Allocate a `Ty` with a `TypeMatcher` in starlark heap as `TypeCompiled`.
pub struct TypeCompiledFactory<'a, 'v> {
    heap: Heap<'v>,
    ty: &'a Ty,
}

impl<'a, 'v> TypeMatcherAlloc for TypeCompiledFactory<'a, 'v> {
    type Result = TypeCompiled<'v>;

    fn alloc<T: TypeMatcher>(self, matcher: T) -> Self::Result {
        TypeCompiled::alloc(matcher, self.ty.clone(), self.heap)
    }

    fn custom(self, custom: &TyCustom) -> Self::Result {
        custom.matcher_with_type_compiled_factory(self)
    }

    fn from_type_matcher_factory(self, factory: &TypeMatcherFactory) -> Self::Result {
        factory.factory.type_compiled(self)
    }

    fn any(self) -> TypeCompiled<'v> {
        if self.ty == &Ty::any() {
            TypeCompiled::any()
        } else {
            self.alloc(IsAny)
        }
    }

    fn none(self) -> TypeCompiled<'v> {
        if self.ty == &Ty::none() {
            TypeCompiled::unchecked_new(TYPE_COMPILED_NONE.at().to_value())
        } else {
            self.alloc(IsNone)
        }
    }

    fn bool(self) -> TypeCompiled<'v> {
        if self.ty == &Ty::bool() {
            TypeCompiled::unchecked_new(TYPE_COMPILED_BOOL.at().to_value())
        } else {
            self.alloc(IsBool)
        }
    }

    fn int(self) -> TypeCompiled<'v> {
        if self.ty == &Ty::int() {
            TypeCompiled::unchecked_new(TYPE_COMPILED_INT.at().to_value())
        } else {
            self.alloc(IsInt)
        }
    }

    fn str(self) -> TypeCompiled<'v> {
        if self.ty == &Ty::string() {
            TypeCompiled::unchecked_new(TYPE_COMPILED_STRING.at().to_value())
        } else {
            self.alloc(IsStr)
        }
    }
}

impl<'a, 'v> TypeCompiledFactory<'a, 'v> {
    pub(crate) fn alloc_ty(ty: &'a Ty, heap: Heap<'v>) -> TypeCompiled<'v> {
        TypeCompiledFactory { heap, ty }.ty(ty)
    }
}
