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
use std::sync::Arc;

use allocative::Allocative;
use dupe::Dupe;
use pagable::Pagable;
use pagable::PagableBoxDeserialize;
use pagable::PagableDeserialize;
use pagable::PagableDeserializer;
use pagable::PagableSerialize;

use crate::values::typing::type_compiled::alloc::TypeMatcherAlloc;
use crate::values::typing::type_compiled::compiled::TypeCompiled;
use crate::values::typing::type_compiled::factory::TypeCompiledFactory;
use crate::values::typing::type_compiled::matcher::TypeMatcher;
use crate::values::typing::type_compiled::matcher::TypeMatcherBox;
use crate::values::typing::type_compiled::matcher::TypeMatcherBoxAlloc;

#[derive(Allocative, Debug, Pagable)]
struct TypeMatcherFactoryImpl<M: TypeMatcher> {
    matcher: M,
}

pub(crate) trait TypeMatcherFactoryDyn: Allocative + Debug + Send + Sync + 'static {
    fn matcher_box(&self) -> TypeMatcherBox;
    fn type_compiled<'v>(&self, factory: TypeCompiledFactory<'_, 'v>) -> TypeCompiled<'v>;
}

impl<M: TypeMatcher> TypeMatcherFactoryDyn for TypeMatcherFactoryImpl<M> {
    fn matcher_box(&self) -> TypeMatcherBox {
        TypeMatcherBoxAlloc.alloc(self.matcher.clone())
    }

    fn type_compiled<'v>(&self, factory: TypeCompiledFactory<'_, 'v>) -> TypeCompiled<'v> {
        factory.alloc(self.matcher.clone())
    }
}

/// Boxed `TypeMatcher`.
#[derive(Dupe, Clone, Allocative, Debug, Pagable)]
pub struct TypeMatcherFactory {
    pub(crate) factory: Arc<dyn TypeMatcherFactoryDyn>,
}

impl TypeMatcherFactory {
    /// Create a new `TypeMatcherFactory` from a `TypeMatcher`.
    pub fn new(matcher: impl TypeMatcher) -> TypeMatcherFactory {
        TypeMatcherFactory {
            factory: Arc::new(TypeMatcherFactoryImpl { matcher }),
        }
    }
}

// Pagable via `TypeMatcherBox` to reuse the existing `dyn TypeMatcherDyn`
// typetag registry — avoids standing up a parallel registry for every
// concrete `M`. The concrete `M` is lost on round-trip (factory becomes
// `TypeMatcherFactoryImpl<TypeMatcherBox>`), but `TypeMatcherBox: TypeMatcher`
// keeps behavior intact.
impl PagableSerialize for dyn TypeMatcherFactoryDyn {
    fn pagable_serialize(
        &self,
        serializer: &mut dyn pagable::PagableSerializer,
    ) -> pagable::Result<()> {
        let matcher_box = self.matcher_box();
        matcher_box.pagable_serialize(serializer)
    }
}

impl<'de> PagableBoxDeserialize<'de> for dyn TypeMatcherFactoryDyn {
    fn deserialize_box<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Box<Self>> {
        let matcher_box = TypeMatcherBox::pagable_deserialize(deserializer)?;
        let factory = TypeMatcherFactoryImpl {
            matcher: matcher_box,
        };
        Ok(Box::new(factory))
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
    use crate::values::typing::type_compiled::matchers::IsAnyOfTwo;
    use crate::values::typing::type_compiled::matchers::IsInt;
    use crate::values::typing::type_compiled::matchers::IsNone;
    use crate::values::typing::type_compiled::matchers::IsStr;

    fn round_trip(f: &TypeMatcherFactory) -> TypeMatcherFactory {
        let mut ser = TestingSerializer::new();
        f.pagable_serialize(&mut ser).unwrap();
        let bytes = ser.finish();
        let mut de = TestingDeserializer::new(&bytes);
        TypeMatcherFactory::pagable_deserialize(&mut de).unwrap()
    }

    #[test]
    fn test_round_trip_non_generic() {
        // Factory wrapping a non-generic matcher.
        let f = TypeMatcherFactory::new(IsStr);
        let restored = round_trip(&f);
        let m = restored.factory.matcher_box();
        assert!(m.0.matches_dyn(const_frozen_string!("hi").at().to_value()));
        assert!(!m.0.matches_dyn(Value::new_bool(true)));
    }

    // Generic monomorphizations register through program constructors, which
    // wasm does not support, so generic tags cannot be deserialized there.
    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn test_round_trip_generic_2_inner() {
        // Factory wrapping a 2-generic matcher.
        let f = TypeMatcherFactory::new(IsAnyOfTwo(IsNone, IsStr));
        let restored = round_trip(&f);
        let m = restored.factory.matcher_box();
        assert!(m.0.matches_dyn(Value::new_none()));
        assert!(m.0.matches_dyn(const_frozen_string!("x").at().to_value()));
        assert!(!m.0.matches_dyn(Value::new_bool(false)));
    }

    #[test]
    fn test_arc_dedup() {
        // Two clones of one `TypeMatcherFactory` share an `Arc`. Serialize
        // both — pagable's `serialize_arc` should dedup and emit the
        // factory payload just once (the second write is just the identity
        // reference).
        let f = TypeMatcherFactory::new(IsInt);
        let f2 = f.clone();

        let mut ser = TestingSerializer::new();
        f.pagable_serialize(&mut ser).unwrap();
        let bytes_one = ser.finish();

        let mut ser = TestingSerializer::new();
        f.pagable_serialize(&mut ser).unwrap();
        f2.pagable_serialize(&mut ser).unwrap();
        let bytes_two = ser.finish();

        // Two serializations of the same Arc are cheaper than one-then-one
        // of distinct payloads: the 2nd write only emits the shared arc
        // identity, not the factory body again.
        assert!(
            bytes_two.len() < 2 * bytes_one.len(),
            "serialize_arc should dedup: one={}, two={}",
            bytes_one.len(),
            bytes_two.len()
        );

        // Both deserialize back equivalently.
        let mut de = TestingDeserializer::new(&bytes_two);
        let a = TypeMatcherFactory::pagable_deserialize(&mut de).unwrap();
        let b = TypeMatcherFactory::pagable_deserialize(&mut de).unwrap();
        Heap::temp(|heap| {
            let int_val = heap.alloc(42i32).to_value();
            assert!(a.factory.matcher_box().0.matches_dyn(int_val));
            assert!(b.factory.matcher_box().0.matches_dyn(int_val));
        });
    }
}
