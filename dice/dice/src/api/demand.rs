/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::any::TypeId;
use std::marker::PhantomData;

pub(crate) trait DemandImpl {
    fn tag() -> TypeId;
}

pub(crate) struct DemandValue<T: 'static> {
    pub(crate) value: Option<T>,
}

pub(crate) struct DemandRef<'a, T: ?Sized + 'static> {
    pub(crate) value: Option<&'a T>,
}

impl<T: 'static> DemandImpl for DemandValue<T> {
    fn tag() -> TypeId {
        TypeId::of::<DemandValue<T>>()
    }
}

impl<T: ?Sized + 'static> DemandImpl for DemandRef<'_, T> {
    fn tag() -> TypeId {
        TypeId::of::<DemandRef<'static, T>>()
    }
}

/// Supplied value can be passed using this object.
pub struct Demand<'a> {
    /// What we want?
    demand_impl_tag: TypeId,
    // SAFETY: Must be a `&'a mut` to a `DemandRef<T>` or `DemandValue<T>`, matching the `TypeId` above
    demand_impl: *mut (),
    _phantom: PhantomData<&'a mut ()>,
}

impl<'a> Demand<'a> {
    pub(crate) fn new<I>(demand_impl: &'a mut I) -> Demand<'a>
    where
        I: DemandImpl,
    {
        Demand {
            demand_impl_tag: I::tag(),
            demand_impl: demand_impl as *mut _ as *mut (),
            _phantom: PhantomData,
        }
    }

    /// Provide a value. Discard the value if the type is not matched.
    pub fn provide_value<T: 'static>(&mut self, value: T) {
        self.provide_value_with(|| value);
    }

    /// Provide a value. Discard the value if the type is not matched.
    pub fn provide_value_with<T: 'static>(&mut self, f: impl FnOnce() -> T) {
        if self.demand_impl_tag == DemandValue::<T>::tag() {
            // SAFETY: We've checked that this is a pointer to a `DemandValue<T>`
            let demand_impl = unsafe { &mut *(self.demand_impl as *mut DemandValue<T>) };
            demand_impl.value = Some(f());
        }
    }

    /// Provide a reference. Discard the value if the type is not matched.
    pub fn provide_ref<T: ?Sized + 'static>(&mut self, value: &'a T) {
        self.provide_ref_with(|| value);
    }

    /// Provide a reference. Discard the value if the type is not matched.
    pub fn provide_ref_with<T: ?Sized + 'static>(&mut self, f: impl FnOnce() -> &'a T) {
        if self.demand_impl_tag == DemandRef::<T>::tag() {
            // SAFETY: We've checked that this is a pointer to a `DemandRef<T>`
            let demand_impl = unsafe { &mut *(self.demand_impl as *mut DemandRef<T>) };
            demand_impl.value = Some(f());
        }
    }
}

#[cfg(test)]
mod tests {
    use allocative::Allocative;
    use async_trait::async_trait;
    use dice_futures::cancellation::CancellationContext;
    use pagable::Pagable;
    use pagable::pagable_typetag;

    use crate::Demand;
    use crate::DiceComputations;
    use crate::DiceKeyDyn;
    use crate::DynKey;
    use crate::Key;
    use crate::api::key::NoValueSerialize;
    use crate::api::key::ValueSerialize;

    #[test]
    fn test_request() {
        #[derive(
            derive_more::Display,
            Eq,
            PartialEq,
            Hash,
            Allocative,
            Clone,
            Debug,
            Pagable
        )]
        #[pagable_typetag(DiceKeyDyn)]
        struct MKey(u32);

        #[async_trait]
        impl Key for MKey {
            type Value = ();

            async fn compute(
                &self,
                _ctx: &mut DiceComputations,
                _cancellations: &CancellationContext,
            ) -> Self::Value {
                panic!("not needed in tests")
            }

            fn equality_behavior() -> crate::EqualityBehavior<Self::Value> {
                crate::EqualityBehavior::Compare(|_x, _y| panic!("not needed in tests"))
            }

            fn provide<'a>(&'a self, demand: &mut Demand<'a>) {
                demand.provide_value(self.0);
                demand.provide_ref::<dyn Foo>(self);
            }

            fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
                NoValueSerialize::<Self::Value>::new()
            }
        }

        trait Foo {
            fn foo(&self) -> u32;
        }

        impl Foo for MKey {
            fn foo(&self) -> u32 {
                self.0
            }
        }

        let key = MKey(17);
        let key = DynKey::from_key(key);
        assert_eq!(Some(17), key.request_value::<u32>());
        assert_eq!(None, key.request_value::<i32>());
        assert_eq!(17, key.request_ref::<dyn Foo>().unwrap().foo());
        assert!(key.request_ref::<dyn std::io::Read>().is_none());
    }
}
