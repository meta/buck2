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

use std::marker::PhantomData;
use std::mem;

use crate::private::Private;
use crate::values::ComplexValue;
use crate::values::FreezePreservesType;
use crate::values::FreezeResult;
use crate::values::Freezer;
use crate::values::Heap;
use crate::values::HeapSendable;
use crate::values::Trace;
use crate::values::Tracer;
use crate::values::Value;
use crate::values::ValueTyped;
use crate::values::freeze_dynamic::freeze_dynamic;
use crate::values::layout::avalue::AValue;
use crate::values::layout::avalue::AValueImpl;
use crate::values::layout::avalue::heap_copy_impl;
use crate::values::layout::heap::repr::AValueRepr;

pub(crate) struct AValueComplex<T>(PhantomData<T>);

impl<'v, T> AValue<'v> for AValueComplex<T>
where
    T: ComplexValue<'v>,
{
    type StarlarkValue = T;

    type ExtraElem = ();

    fn extra_len(_value: &T) -> usize {
        0
    }

    fn offset_of_extra() -> usize {
        mem::size_of::<Self>()
    }

    unsafe fn heap_freeze<'fv>(
        me: *mut AValueRepr<Self::StarlarkValue>,
        freezer: &Freezer<'v, 'fv>,
    ) -> FreezeResult<Value<'fv>> {
        unsafe {
            let fv = freeze_dynamic(me, freezer)?;
            if let Some(frozen_def) = ValueTyped::new(fv) {
                freezer.frozen_defs.borrow_mut().push(frozen_def);
            }
            Ok(fv)
        }
    }

    unsafe fn heap_copy(
        me: *mut AValueRepr<Self::StarlarkValue>,
        tracer: &Tracer<'v>,
    ) -> Value<'v> {
        unsafe { heap_copy_impl::<Self>(me, tracer, Trace::trace) }
    }
}

impl<'v> Heap<'v> {
    /// Allocate a value which can be traced (garbage collected) and frozen on the [`Heap`].
    pub(crate) fn alloc_complex<T>(self, x: T) -> Value<'v>
    where
        T: ComplexValue<'v> + FreezePreservesType<'v>,
        T: HeapSendable<'v>,
    {
        assert!(!T::is_special(Private));
        self.alloc_raw(AValueImpl::<AValueComplex<T>>::new(x))
            .to_value()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use allocative::Allocative;
    use derive_more::Display;
    use starlark_derive::NoSerialize;
    use starlark_derive::StarlarkPagable;
    use starlark_derive::StarlarkPagablePanic;
    use starlark_derive::StarlarkRepresentations;
    use starlark_derive::starlark_value;

    use crate as starlark;
    use crate::any::ProvidesStaticType;
    use crate::values::FreezeDynamic;
    use crate::values::FreezeError;
    use crate::values::FreezePlan;
    use crate::values::FreezePreservesType;
    use crate::values::FreezeResult;
    use crate::values::FreezeSlot;
    use crate::values::FreezeTarget;
    use crate::values::Freezer;
    use crate::values::StarlarkType;
    use crate::values::StarlarkValue;
    use crate::values::Trace;
    use crate::values::Value;
    use crate::values::ValueTyped;

    #[derive(
        Debug,
        Display,
        ProvidesStaticType,
        NoSerialize,
        Allocative,
        StarlarkPagable
    )]
    #[display("small dynamic freeze target")]
    struct SmallTarget(u32);

    /// The targets a dynamic freeze may select, and the value that is frozen into them.
    #[derive(Debug, Clone, Copy, StarlarkRepresentations)]
    enum DynamicFreezeTargetView<'v> {
        Small(ValueTyped<'v, SmallTarget>),
        Large(ValueTyped<'v, LargeTarget>),
        Drop(ValueTyped<'v, DropTarget>),
        Source(ValueTyped<'v, FreezeDynamicValue<'v>>),
    }

    #[starlark_value(type = "dynamic_freeze_target")]
    impl<'v> StarlarkType<'v> for DynamicFreezeTargetView<'v> {}

    #[starlark_value]
    impl<'v> StarlarkValue<'v> for SmallTarget {
        type Type = DynamicFreezeTargetView<'v>;
    }

    #[derive(
        Debug,
        Display,
        ProvidesStaticType,
        NoSerialize,
        Allocative,
        StarlarkPagable
    )]
    #[display("large dynamic freeze target")]
    struct LargeTarget([u64; 4]);

    #[starlark_value]
    impl<'v> StarlarkValue<'v> for LargeTarget {
        type Type = DynamicFreezeTargetView<'v>;
    }

    /// The size of `SmallTarget`, but with a destructor.
    #[derive(
        Debug,
        Display,
        ProvidesStaticType,
        NoSerialize,
        Allocative,
        StarlarkPagablePanic
    )]
    #[display("dropping dynamic freeze target")]
    struct DropTarget(Box<u32>);

    #[starlark_value]
    impl<'v> StarlarkValue<'v> for DropTarget {
        type Type = DynamicFreezeTargetView<'v>;
    }

    #[derive(Debug, Display, ProvidesStaticType, NoSerialize, Allocative, Trace)]
    #[display("dynamic freeze source")]
    struct FreezeDynamicValue<'v> {
        plan: DynamicPlan,
        /// The value this one is allocated as, for the plan that hands back the slot
        /// without writing it.
        this: RefCell<Option<Value<'v>>>,
    }

    impl<'v> FreezeDynamicValue<'v> {
        fn with_plan(plan: DynamicPlan) -> FreezeDynamicValue<'v> {
            FreezeDynamicValue {
                plan,
                this: RefCell::new(None),
            }
        }

        fn small() -> FreezeDynamicValue<'v> {
            Self::with_plan(DynamicPlan::Small)
        }

        fn large() -> FreezeDynamicValue<'v> {
            Self::with_plan(DynamicPlan::Large)
        }

        fn failing() -> FreezeDynamicValue<'v> {
            Self::with_plan(DynamicPlan::Fail)
        }

        // Only referenced by the `cfg(panic = "unwind")` test below.
        #[cfg(panic = "unwind")]
        fn panicking() -> FreezeDynamicValue<'v> {
            Self::with_plan(DynamicPlan::Panic)
        }
    }

    #[starlark_value(skip_vtable)]
    impl<'v> StarlarkValue<'v> for FreezeDynamicValue<'v> {
        type Type = DynamicFreezeTargetView<'v>;
    }

    #[derive(Copy, Clone, Debug, Allocative)]
    enum DynamicPlan {
        Small,
        Large,
        Fail,
        /// Constructed only by the `cfg(panic = "unwind")` test.
        #[cfg_attr(not(panic = "unwind"), allow(dead_code))]
        Panic,
        /// Selects `SmallTarget` and writes a `LargeTarget`.
        WrongTarget,
        /// Selects `SmallTarget` and writes a `DropTarget`, which has its size but a
        /// destructor.
        WrongDrop,
        /// Returns a frozen value that is not the slot.
        Impostor,
        /// Returns the slot's own value, obtained through the forward, without writing
        /// the slot.
        Unpublished,
    }

    /// Every target of `DynamicPlan` is a `DynamicFreezeTargetView`, as the source is.
    impl<'v> FreezePreservesType<'v> for FreezeDynamicValue<'v> {}

    impl<'v> FreezeDynamic<'v> for FreezeDynamicValue<'v> {
        type Plan<'fv> = DynamicPlan;

        fn prepare_freeze<'fv>(
            &self,
            _freezer: &Freezer<'v, 'fv>,
        ) -> FreezeResult<Self::Plan<'fv>> {
            Ok(self.plan)
        }
    }

    impl<'v, 'fv> FreezePlan<'v, 'fv, FreezeDynamicValue<'v>> for DynamicPlan {
        fn target(&self) -> FreezeTarget<'fv> {
            match self {
                Self::Large => FreezeTarget::simple::<LargeTarget>(),
                _ => FreezeTarget::simple::<SmallTarget>(),
            }
        }

        fn freeze_into(
            self,
            value: FreezeDynamicValue<'v>,
            freezer: &Freezer<'v, 'fv>,
            slot: FreezeSlot<'fv>,
        ) -> FreezeResult<Value<'fv>> {
            match self {
                Self::Small => slot.write(SmallTarget(1)),
                Self::Large => slot.write(LargeTarget([2, 3, 5, 7])),
                Self::Fail => Err(FreezeError::new(
                    "intentional dynamic freeze failure".to_owned(),
                )),
                Self::Panic => panic!("intentional dynamic freeze panic"),
                Self::WrongTarget => slot.write(LargeTarget([11, 13, 17, 19])),
                Self::WrongDrop => slot.write(DropTarget(Box::new(23))),
                // The last two leave `slot` unwritten.
                Self::Impostor => Ok(freezer.frozen_heap().alloc_simple(SmallTarget(29))),
                Self::Unpublished => {
                    // The source forwards to the slot, so freezing the source's own
                    // value hands back the slot's address.
                    let this = value.this.into_inner().expect("set by the test");
                    freezer.freeze(this)
                }
            }
        }
    }

    #[test]
    fn selects_different_runtime_frozen_types() {
        Freezer::testing_temp(|heap, freezer| {
            let small = heap.alloc_complex(FreezeDynamicValue::small());
            let large = heap.alloc_complex(FreezeDynamicValue::large());

            let small = freezer.freeze(small).expect("small value should freeze");
            let large = freezer.freeze(large).expect("large value should freeze");
            assert_eq!(
                1,
                small.downcast_ref::<SmallTarget>().expect("small target").0
            );
            assert_eq!(
                [2, 3, 5, 7],
                large.downcast_ref::<LargeTarget>().expect("large target").0,
            );
        });
    }

    #[test]
    fn failed_dynamic_freeze_leaves_frozen_heap_traversable() {
        Freezer::testing_temp(|heap, freezer| {
            let value = heap.alloc_complex(FreezeDynamicValue::failing());

            freezer
                .freeze(value)
                .expect_err("the dynamic freeze plan should fail");
            let following = freezer
                .frozen_heap()
                .alloc_str("after failed dynamic freeze");
            assert_eq!("after failed dynamic freeze", following.as_str());
        });
    }

    // Buck builds tests with panic=abort, where unwinding does not exist;
    // this test is meaningful only in unwind builds such as Cargo's.
    #[cfg(panic = "unwind")]
    #[test]
    fn panicking_dynamic_freeze_leaves_heaps_droppable() {
        Freezer::testing_temp(|heap, freezer| {
            let value = heap.alloc_complex(FreezeDynamicValue::panicking());

            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| freezer.freeze(value)))
                .expect_err("the dynamic freeze plan should panic");
            // The abandoned slot leaves a reservation header behind; the frozen
            // heap must remain allocatable, walkable, and droppable.
            let following = freezer
                .frozen_heap()
                .alloc_str("after panicking dynamic freeze");
            assert_eq!("after panicking dynamic freeze", following.as_str());
        });
    }

    // Buck builds tests with panic=abort, where unwinding does not exist;
    // this test is meaningful only in unwind builds such as Cargo's.
    #[cfg(all(debug_assertions, panic = "unwind"))]
    #[test]
    fn freezing_after_a_freeze_error_panics() {
        Freezer::testing_temp(|heap, freezer| {
            let value = heap.alloc_complex(FreezeDynamicValue::failing());
            freezer
                .freeze(value)
                .expect_err("the dynamic freeze plan should fail");

            let following = heap.alloc_str("after the failed freeze").to_value();
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| freezer.freeze(following)))
                .expect_err("freezing through an abandoned freezer should panic");
        });
    }

    /// Freezes a value with `plan`, expects an error mentioning `message`, and checks
    /// the frozen heap is still usable.
    fn expect_freeze_error(plan: DynamicPlan, message: &str) {
        Freezer::testing_temp(|heap, freezer| {
            let value = heap.alloc_complex(FreezeDynamicValue::with_plan(plan));
            if let Some(source) = value.downcast_ref::<FreezeDynamicValue>() {
                source.this.replace(Some(value));
            }

            let error = freezer.freeze(value).expect_err("the plan should fail");
            assert!(
                error.err_msg.contains(message),
                "unexpected error: {}",
                error.err_msg
            );
            let following = freezer.frozen_heap().alloc_str("after the failed freeze");
            assert_eq!("after the failed freeze", following.as_str());
        });
    }

    #[test]
    fn mismatched_freeze_target_is_an_error_not_a_panic() {
        expect_freeze_error(
            DynamicPlan::WrongTarget,
            "different from its selected target",
        );
    }

    #[test]
    fn same_size_different_drop_region_is_an_error() {
        expect_freeze_error(DynamicPlan::WrongDrop, "different from its selected target");
    }

    #[test]
    fn returning_another_value_is_an_error() {
        expect_freeze_error(DynamicPlan::Impostor, "did not initialize and publish");
    }

    #[test]
    fn returning_the_unwritten_slot_is_an_error() {
        expect_freeze_error(DynamicPlan::Unpublished, "did not initialize and publish");
    }
}
