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

use std::fmt::Debug;

use allocative::Allocative;
use pagable::Pagable;
use pagable::pagable_typetag;
use pagable::typetag::PagableStableName;
use pagable::typetag::PagableTagged;
use starlark_derive::type_matcher;

use crate as starlark;
use crate::typing::custom::TyCustom;
use crate::values::Value;
use crate::values::typing::type_compiled::alloc::TypeMatcherAlloc;
use crate::values::typing::type_compiled::type_matcher_factory::TypeMatcherFactory;

/// Marker trait for type matchers which are registered.
///
/// This trait is automatically implemented by the `#[type_matcher]` proc macro.
///
/// # Safety
///
/// This trait must only be implemented by the `#[type_matcher]` proc macro,
/// which ensures the type is properly registered in the vtable registry.
/// Manual implementations may break deserialization.
#[cfg_attr(not(feature = "pagable"), allow(dead_code))]
pub unsafe trait TypeMatcherRegistered {}

/// Base trait for type matchers
///
///  When `pagable` is enabled, matchers must also implement `TypeMatcherRegistered`
/// to ensure they are registered.
#[cfg(feature = "pagable")]
pub trait TypeMatcherBase:
    TypeMatcherRegistered + Allocative + Debug + Clone + Sized + Send + Sync + 'static
{
}

#[cfg(feature = "pagable")]
impl<T> TypeMatcherBase for T where
    T: TypeMatcherRegistered + Allocative + Debug + Clone + Sized + Send + Sync + 'static
{
}

/// Base trait for type matchers
#[cfg(not(feature = "pagable"))]
pub trait TypeMatcherBase: Allocative + Debug + Clone + Sized + Send + Sync + 'static {}

#[cfg(not(feature = "pagable"))]
impl<T> TypeMatcherBase for T where T: Allocative + Debug + Clone + Sized + Send + Sync + 'static {}

/// Runtime type matcher. E.g. when `isinstance(1, int)` is called,
/// implementation of `TypeMatcher` for `int` is used.
/// `PagableStableName` is a supertrait so generic matcher wrappers can
/// compose their persisted typetags from their argument matchers' stable
/// names; any matcher deriving `Pagable` gets it automatically.
pub trait TypeMatcher: TypeMatcherBase + Pagable + PagableTagged + PagableStableName {
    /// Check if the value matches the type.
    fn matches(&self, value: Value) -> bool;
    /// True if this matcher matches any value.
    fn is_wildcard(&self) -> bool {
        false
    }
}

#[pagable_typetag]
#[doc(hidden)]
pub trait TypeMatcherDyn: Debug + Allocative + PagableTagged + Send + Sync + 'static {
    fn matches_dyn(&self, value: Value) -> bool;
    fn is_wildcard_dyn(&self) -> bool;

    fn to_box(&self) -> TypeMatcherBox;
}

impl<T: TypeMatcher> TypeMatcherDyn for T {
    fn matches_dyn(&self, value: Value) -> bool {
        TypeMatcher::matches(self, value)
    }

    fn is_wildcard_dyn(&self) -> bool {
        TypeMatcher::is_wildcard(self)
    }

    fn to_box(&self) -> TypeMatcherBox {
        TypeMatcherBox::new(self.clone())
    }
}

#[pagable_typetag(TypeMatcherDyn)]
#[doc(hidden)]
#[derive(Debug, Allocative, Pagable)]
pub struct TypeMatcherBox(pub(crate) Box<dyn TypeMatcherDyn>);

impl TypeMatcherBox {
    pub(crate) fn new<T: TypeMatcher>(matcher: T) -> TypeMatcherBox {
        TypeMatcherBox(Box::new(matcher))
    }
}

impl Clone for TypeMatcherBox {
    fn clone(&self) -> Self {
        self.0.to_box()
    }
}

#[type_matcher]
impl TypeMatcher for TypeMatcherBox {
    fn matches(&self, value: Value) -> bool {
        self.0.matches_dyn(value)
    }

    fn is_wildcard(&self) -> bool {
        self.0.is_wildcard_dyn()
    }
}

/// Type allocator which allocates `TypeMatcher` into `TypeMatcherBox`.
pub(crate) struct TypeMatcherBoxAlloc;

impl TypeMatcherAlloc for TypeMatcherBoxAlloc {
    type Result = TypeMatcherBox;

    fn alloc<T: TypeMatcher>(self, matcher: T) -> Self::Result {
        TypeMatcherBox::new(matcher)
    }

    fn custom(self, custom: &TyCustom) -> Self::Result {
        custom.matcher_with_box()
    }

    fn from_type_matcher_factory(self, factory: &TypeMatcherFactory) -> Self::Result {
        factory.factory.matcher_box()
    }
}

#[cfg(test)]
mod tests {
    use pagable::PagableDeserialize;
    use pagable::PagableSerialize;
    use pagable::testing::TestingDeserializer;
    use pagable::testing::TestingSerializer;

    use super::*;
    use crate::const_frozen_string;
    use crate::values::Heap;
    use crate::values::Value;
    use crate::values::list::AllocList;
    use crate::values::typing::type_compiled::matchers::IsAnyOfTwo;
    use crate::values::typing::type_compiled::matchers::IsInt;
    use crate::values::typing::type_compiled::matchers::IsListOf;
    use crate::values::typing::type_compiled::matchers::IsNone;
    use crate::values::typing::type_compiled::matchers::IsStr;

    fn round_trip(b: &TypeMatcherBox) -> TypeMatcherBox {
        let mut ser = TestingSerializer::new();
        b.pagable_serialize(&mut ser).unwrap();
        let bytes = ser.finish();
        let mut de = TestingDeserializer::new(&bytes);
        TypeMatcherBox::pagable_deserialize(&mut de).unwrap()
    }

    #[test]
    fn test_round_trip_non_generic() {
        // Non-generic matcher (registered via `#[pagable_typetag]` on struct).
        let b = TypeMatcherBox::new(IsStr);
        let restored = round_trip(&b);
        assert!(
            restored
                .0
                .matches_dyn(const_frozen_string!("hi").at().to_value())
        );
        assert!(!restored.0.matches_dyn(Value::new_bool(true)));
    }

    // Generic monomorphizations register through program constructors, which
    // wasm does not support, so generic tags cannot be deserialized there.
    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn test_round_trip_generic_1_inner() {
        // Generic wrapper with 1 inner; the monomorphization registers
        // itself at image load.
        let b = TypeMatcherBox::new(IsListOf(IsStr));
        let restored = round_trip(&b);
        // `list[str]` matches an empty list (vacuously).
        Heap::temp(|heap| {
            let list = heap.alloc(AllocList::EMPTY).to_value();
            assert!(restored.0.matches_dyn(list));
        });
    }

    // See `test_round_trip_generic_1_inner` for why this is not run on wasm.
    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn test_round_trip_generic_2_inner() {
        // 2-generic wrapper with DISTINCT inners; the monomorphization
        // registers itself at image load.
        let b = TypeMatcherBox::new(IsAnyOfTwo(IsNone, IsStr));
        let restored = round_trip(&b);
        assert!(restored.0.matches_dyn(Value::new_none()));
        assert!(
            restored
                .0
                .matches_dyn(const_frozen_string!("x").at().to_value())
        );
        assert!(!restored.0.matches_dyn(Value::new_bool(false)));
    }

    #[test]
    fn test_distinct_matchers_distinct_tags() {
        // Two different matchers round-trip to their own types, no confusion.
        let a = TypeMatcherBox::new(IsStr);
        let b = TypeMatcherBox::new(IsInt);

        let mut ser = TestingSerializer::new();
        a.pagable_serialize(&mut ser).unwrap();
        b.pagable_serialize(&mut ser).unwrap();
        let bytes = ser.finish();

        let mut de = TestingDeserializer::new(&bytes);
        let restored_a = TypeMatcherBox::pagable_deserialize(&mut de).unwrap();
        let restored_b = TypeMatcherBox::pagable_deserialize(&mut de).unwrap();

        // a matches string, not int; b matches int, not string.
        Heap::temp(|heap| {
            let str_val = const_frozen_string!("hi").at().to_value();
            let int_val = heap.alloc(42i32).to_value();
            assert!(restored_a.0.matches_dyn(str_val));
            assert!(!restored_a.0.matches_dyn(int_val));
            assert!(!restored_b.0.matches_dyn(str_val));
            assert!(restored_b.0.matches_dyn(int_val));
        });
    }
}
