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

use std::any::TypeId;
use std::marker::PhantomData;

use crate::any::AnyLifetime;
use crate::values::StarlarkTypeExpr;
use crate::values::Value;
use crate::values::predicate::StarlarkPredicate;

/// What a request for the handle of the predicate type `P` is keyed by. Nothing outside this
/// module can name it, so no value provided by type can be taken for a handle.
struct PredicateKey<P>(PhantomData<P>);

/// Taken by [`StarlarkValue::provide`](crate::values::StarlarkValue::provide)
/// to provide different data depending on the type.
pub struct Demand<'a, 'v> {
    type_id_of_t: TypeId,
    /// `&'a mut Option<T>`.
    option: *mut (),
    _marker: PhantomData<&'a mut &'v ()>,
}

impl<'a, 'v> Demand<'a, 'v> {
    fn new<T: AnyLifetime<'v>>(option: &mut Option<T>) -> Demand<'a, 'v> {
        Demand {
            type_id_of_t: T::static_type_id(),
            option: option as *mut _ as *mut (),
            _marker: PhantomData,
        }
    }

    /// Provide a value of given type.
    ///
    /// If type matches the type requested from [`Value::request_value`], the value is stored
    /// inside the [`Demand`] and later returned, otherwise the value is discarded.
    pub fn provide_value<T: AnyLifetime<'v>>(&mut self, value: T) {
        if self.type_id_of_t == T::static_type_id() {
            // SAFETY: check checked type.
            unsafe { *(self.option as *mut Option<T>) = Some(value) };
        }
    }

    /// A demand for the handle of the predicate type `P`.
    fn for_predicate<P: StarlarkPredicate>(
        option: &mut Option<<P as StarlarkTypeExpr<'v>>::Get>,
    ) -> Demand<'a, 'v> {
        Demand {
            type_id_of_t: TypeId::of::<PredicateKey<P>>(),
            option: option as *mut _ as *mut (),
            _marker: PhantomData,
        }
    }

    /// Provide the handle of the predicate type `P`, made by `handle`, if that is what is
    /// demanded.
    ///
    /// [`#[starlark_value]`](crate::values::starlark_value) writes the calls of this, from the
    /// memberships in the block.
    #[inline]
    pub fn provide_handle<P: StarlarkPredicate>(
        &mut self,
        handle: impl FnOnce() -> <P as StarlarkTypeExpr<'v>>::Get,
    ) {
        if self.type_id_of_t == TypeId::of::<PredicateKey<P>>() {
            // SAFETY: the key says the option is the one `for_predicate::<P>` was given.
            unsafe {
                *(self.option as *mut Option<<P as StarlarkTypeExpr<'v>>::Get>) = Some(handle())
            };
        }
    }
}

pub(crate) fn request_value_impl<'v, T: AnyLifetime<'v>>(value: Value<'v>) -> Option<T> {
    let mut option = None;
    value.get_ref().provide(&mut Demand::new(&mut option));
    option
}

/// The handle of the predicate type `P` to `value`, if the type of `value` says in its
/// `#[starlark_value]` block that it is a member.
#[inline]
pub(crate) fn request_predicate_handle<'v, P: StarlarkPredicate>(
    value: Value<'v>,
) -> Option<<P as StarlarkTypeExpr<'v>>::Get> {
    let value = value.get_ref();
    if !value.has_predicate_handles() {
        return None;
    }
    let mut option = None;
    value.provide_predicate_handle(&mut Demand::for_predicate::<P>(&mut option));
    option
}

#[cfg(test)]
mod tests {
    use allocative::Allocative;
    use starlark_derive::NoSerialize;
    use starlark_derive::StarlarkPagable;
    use starlark_derive::starlark_value;

    use crate as starlark;
    use crate::any::ProvidesStaticType;
    use crate::starlark_simple_value;
    use crate::values::Heap;
    use crate::values::StarlarkValue;
    use crate::values::demand::Demand;

    trait SomeTrait {
        fn payload(&self) -> u32;
    }

    // TODO(nga): implement derive for this.
    unsafe impl<'v> ProvidesStaticType<'v> for &'v dyn SomeTrait {
        type StaticType = &'static dyn SomeTrait;
    }

    #[derive(
        ProvidesStaticType,
        derive_more::Display,
        Debug,
        NoSerialize,
        Allocative,
        StarlarkPagable
    )]
    #[display("SomeType")]
    struct MyValue {
        payload: u32,
    }

    impl SomeTrait for MyValue {
        fn payload(&self) -> u32 {
            self.payload
        }
    }

    starlark_simple_value!(MyValue);

    #[starlark_value(type = "MyValue")]
    impl<'v> StarlarkValue<'v> for MyValue {
        fn provide(&'v self, demand: &mut Demand<'_, 'v>) {
            demand.provide_value::<&dyn SomeTrait>(self);
        }
    }

    #[test]
    fn test_trait_downcast() {
        Heap::temp(|heap| {
            let value = heap.alloc_simple(MyValue { payload: 17 });

            assert!(value.request_value::<String>().is_none());

            let some_trait = value.request_value::<&dyn SomeTrait>().unwrap();
            assert_eq!(17, some_trait.payload());
        });
    }
}
