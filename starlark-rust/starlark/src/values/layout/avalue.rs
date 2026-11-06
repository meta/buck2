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

use std::cmp;
use std::marker::PhantomData;
use std::mem;
use std::mem::MaybeUninit;
use std::ptr;
use std::slice;

use allocative::Visitor;
use pagable::PagableDeserialize;
use pagable::PagableSerialize;

use crate::collections::maybe_uninit_backport::maybe_uninit_write_slice;
use crate::pagable::StarlarkSerialize;
use crate::pagable::starlark_deserialize::StarlarkDeserializeContext;
use crate::pagable::starlark_serialize::StarlarkSerializeContext;
use crate::values::FreezeResult;
use crate::values::Freezer;
use crate::values::HeapSendable;
use crate::values::StarlarkValue;
use crate::values::Tracer;
use crate::values::Value;
use crate::values::layout::aligned_size::AlignedSize;
use crate::values::layout::heap::arena::MIN_ALLOC;
use crate::values::layout::heap::repr::AValueForward;
use crate::values::layout::heap::repr::AValueHeader;
use crate::values::layout::heap::repr::AValueRepr;
use crate::values::layout::heap::send::HeapSyncable;
use crate::values::layout::value_alloc_size::ValueAllocSize;

/// Bound for the payload type `T` of `AValueSimple<T>` and `AValueFrozen<T>`.
///
/// Bundles `StarlarkValue` + send/sync (always required), plus, under the `pagable` feature,
/// `StarlarkSerialize`, `StarlarkDeserialize` at every brand (the type is deserialized at the
/// brand of the heap being paged in, whichever that is; see `StarlarkDeserializeAt`) and
/// `VtableRegistered`.
#[cfg(feature = "pagable")]
pub trait AValueSimpleBound<'v>:
    StarlarkValue<'v>
    + HeapSendable<'v>
    + HeapSyncable<'v>
    + crate::pagable::StarlarkSerialize
    + for<'fv> crate::pagable::StarlarkDeserializeAt<'v, 'fv>
    + crate::pagable::vtable_register::VtableRegistered
{
}
#[cfg(feature = "pagable")]
impl<'v, T> AValueSimpleBound<'v> for T where
    T: StarlarkValue<'v>
        + HeapSendable<'v>
        + HeapSyncable<'v>
        + crate::pagable::StarlarkSerialize
        + for<'fv> crate::pagable::StarlarkDeserializeAt<'v, 'fv>
        + crate::pagable::vtable_register::VtableRegistered
{
}

#[cfg(not(feature = "pagable"))]
pub trait AValueSimpleBound<'v>: StarlarkValue<'v> + HeapSendable<'v> + HeapSyncable<'v> {}
#[cfg(not(feature = "pagable"))]
impl<'v, T> AValueSimpleBound<'v> for T where
    T: StarlarkValue<'v> + HeapSendable<'v> + HeapSyncable<'v>
{
}

/// Extended vtable methods (those not covered by `StarlarkValue`).
pub(crate) trait AValue<'v>: Sized + 'v {
    /// Unwrapped type.
    type StarlarkValue: StarlarkValue<'v>;

    /// Certain types like `Tuple` or `StarlarkStr` have payload array
    /// placed in a heap after `Self`. This is the type of an element of that array.
    type ExtraElem: 'v;

    /// Whether this value must be allocated in the arena's drop region.
    ///
    /// Must be true whenever the payload or any trailing elements run
    /// destructors; values outside the drop region are never dropped.
    const NEEDS_DROP: bool =
        mem::needs_drop::<Self::StarlarkValue>() || mem::needs_drop::<Self::ExtraElem>();

    /// Payload array length.
    fn extra_len(value: &Self::StarlarkValue) -> usize;

    /// Offset of field holding content, in bytes.
    ///
    /// Return `mem::size_of::<Self>()` if there's no extra content.
    fn offset_of_extra() -> usize;

    /// Type is `StarlarkStr`.
    const IS_STR: bool = false;

    /// Memory size of starlark value including `AValueHeader`.
    fn alloc_size_for_extra_len(extra_len: usize) -> ValueAllocSize {
        assert!(
            Self::offset_of_extra() % mem::align_of::<Self::ExtraElem>() == 0,
            "extra must be aligned"
        );
        ValueAllocSize::new(cmp::max(
            cmp::max(
                AlignedSize::of::<AValueRepr<Self::StarlarkValue>>(),
                MIN_ALLOC,
            ),
            // Content is not necessarily aligned to end of `A`.
            AlignedSize::align_up(
                AValueRepr::<Self>::offset_of_extra()
                    + (mem::size_of::<Self::ExtraElem>() * extra_len),
            ),
        ))
    }

    /// The memory that should be charged to this value in a profile.
    ///
    /// Both the size of the value itself and anything it references.
    ///
    /// This existing is a bit of a hack to let statically allocated values set this to zero.
    fn total_memory_for_profile(value: &Self::StarlarkValue) -> usize {
        Self::alloc_size_for_extra_len(Self::extra_len(value)).bytes() as usize
            + allocative::size_of_unique_allocated_data(value)
    }

    /// Report inline extra payload that lives in the arena after the value.
    ///
    /// This is not visible to the value's normal `Allocative` implementation because it is not
    /// represented as a Rust field on the payload.
    fn visit_extra_allocative<'a, 'b: 'a>(
        _value: &Self::StarlarkValue,
        _visitor: &'a mut Visitor<'b>,
    ) {
    }

    unsafe fn heap_freeze<'fv>(
        me: *mut AValueRepr<Self::StarlarkValue>,
        freezer: &Freezer<'v, 'fv>,
    ) -> FreezeResult<Value<'fv>>;

    unsafe fn heap_copy(me: *mut AValueRepr<Self::StarlarkValue>, tracer: &Tracer<'v>)
    -> Value<'v>;

    /// Serialize this value using the provided context.
    /// Default implementation returns an error — override for types that support serialization.
    fn starlark_serialize(
        _me: *const AValueRepr<Self::StarlarkValue>,
        _ctx: &mut dyn StarlarkSerializeContext,
    ) -> crate::Result<()> {
        Err(crate::Error::new_kind(crate::ErrorKind::Other(
            anyhow::anyhow!(
                "Type `{}` does not support starlark serialization",
                Self::StarlarkValue::TYPE
            ),
        )))
    }

    /// Deserialize this value into pre-allocated memory using the provided context.
    /// Default implementation returns an error — override for types that support deserialization.
    ///
    /// `'fv` is the brand of the heap `me` points into, which is the brand the value is
    /// deserialized at; `me` itself is typed at the heap's erased brand `'v`, so implementations
    /// write a `'fv` value into `'v` memory. That is the framework erasing the brand of a value
    /// it puts into the heap, the arrangement that every owner of a frozen heap reads back at a
    /// brand it vouches for.
    fn starlark_deserialize<'fv>(
        _me: *mut AValueRepr<Self::StarlarkValue>,
        _ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<()> {
        Err(crate::Error::new_kind(crate::ErrorKind::Other(
            anyhow::anyhow!(
                "Type `{}` does not support starlark deserialization",
                Self::StarlarkValue::TYPE
            ),
        )))
    }
}

/// A value with extended (`AValue`) vtable methods.
#[repr(C)]
pub(crate) struct AValueImpl<'v, T: AValue<'v>>(PhantomData<T>, pub(crate) T::StarlarkValue);

impl<'v, T: AValue<'v>> AValueImpl<'v, T> {
    pub(crate) const fn new(value: T::StarlarkValue) -> Self {
        AValueImpl(PhantomData, value)
    }
}

/// Common `heap_copy` implementation for types without extra.
pub(super) unsafe fn heap_copy_impl<'v, A>(
    me: *mut AValueRepr<A::StarlarkValue>,
    tracer: &Tracer<'v>,
    trace: impl FnOnce(&mut A::StarlarkValue, &Tracer<'v>),
) -> Value<'v>
where
    A: AValue<'v, ExtraElem = ()>,
{
    unsafe {
        let r = tracer.reserve::<A>();
        let mut x = AValueHeader::overwrite_with_forward::<A::StarlarkValue>(me, r.forward_ptr());
        // We have to put the forwarding node in _before_ we trace in case there are cycles
        trace(&mut x, tracer);
        r.fill(x)
    }
}

/// A value whose `Value` elements trail its fixed payload in the arena.
///
/// `len` must come from the fixed payload alone: the heap sizes allocations
/// through it while walking arenas whose other values may already be forwarded
/// or not yet resident, so it can never follow a pointer to another value.
///
/// # Safety
///
/// `len` initialised elements must follow `content_ptr` whenever the value is
/// reachable.
pub(crate) unsafe trait TrailingValues<'v> {
    fn len(&self) -> usize;

    fn content_ptr(&self) -> *const Value<'v>;

    fn content_mut_ptr(&mut self) -> *mut Value<'v>;

    fn content(&self) -> &[Value<'v>] {
        unsafe { slice::from_raw_parts(self.content_ptr(), self.len()) }
    }

    fn content_mut(&mut self) -> &mut [Value<'v>] {
        let len = self.len();
        unsafe { slice::from_raw_parts_mut(self.content_mut_ptr(), len) }
    }
}

/// Common `heap_copy` implementation for types with trailing `Value` elements.
///
/// `copy_header` receives the fixed payload once the forward is in place, so it
/// may trace fixed fields; the trailing elements are traced and copied here.
pub(super) unsafe fn heap_copy_with_extra<'v, A>(
    me: *mut AValueRepr<A::StarlarkValue>,
    tracer: &Tracer<'v>,
    copy_header: impl FnOnce(A::StarlarkValue, &Tracer<'v>) -> A::StarlarkValue,
) -> Value<'v>
where
    A: AValue<'v, ExtraElem = Value<'v>>,
    A::StarlarkValue: TrailingValues<'v>,
{
    unsafe {
        AValueForward::assert_does_not_overwrite_extra::<A>();
        // The elements live past the fixed payload, where the forward never reaches.
        let content = (*me).payload.content_mut();

        let (r, extra) = tracer.reserve_with_extra::<A>(content.len());
        let x = AValueHeader::overwrite_with_forward::<A::StarlarkValue>(me, r.forward_ptr());
        debug_assert_eq!(content.len(), x.len());

        // The forward is in place before anything is traced, so a cycle back to
        // this value through its fixed fields terminates here.
        let x = copy_header(x, tracer);
        for elem in content.iter_mut() {
            tracer.trace(elem);
        }
        let v = r.fill(x);
        maybe_uninit_write_slice(&mut *extra, content);
        v
    }
}

/// Common `heap_freeze` implementation for types with trailing `Value` elements,
/// freezing into the `F` representation.
///
/// `freeze_header` receives the fixed payload once the forward is in place and
/// the element count, and returns the frozen fixed payload.
pub(super) unsafe fn heap_freeze_with_extra<'v, 'fv, A, F>(
    me: *mut AValueRepr<A::StarlarkValue>,
    freezer: &Freezer<'v, 'fv>,
    freeze_header: impl FnOnce(
        A::StarlarkValue,
        &Freezer<'v, 'fv>,
        usize,
    ) -> FreezeResult<F::StarlarkValue>,
) -> FreezeResult<Value<'fv>>
where
    A: AValue<'v, ExtraElem = Value<'v>>,
    A::StarlarkValue: TrailingValues<'v>,
    F: AValue<'fv, ExtraElem = Value<'fv>>,
    F::StarlarkValue: HeapSendable<'fv> + HeapSyncable<'fv>,
{
    unsafe {
        AValueForward::assert_does_not_overwrite_extra::<A>();
        let content = (*me).payload.content();

        let (r, extra) = freezer.frozen_heap().reserve_with_extra::<F>(content.len());
        let x = AValueHeader::overwrite_with_forward::<A::StarlarkValue>(me, r.forward_ptr());
        let header = freeze_header(x, freezer, content.len())?;

        let extra = &mut *extra;
        assert_eq!(extra.len(), content.len());
        for (elem_place, elem) in extra.iter_mut().zip(content) {
            elem_place.write(freezer.freeze(*elem)?);
        }
        Ok(r.fill(header))
    }
}

/// Common `starlark_serialize` implementation for frozen types with trailing
/// `Value` elements: the element count, the fixed fields `serialize_header`
/// writes, then the elements.
pub(super) fn starlark_serialize_with_extra<'v, A>(
    me: *const AValueRepr<A::StarlarkValue>,
    ctx: &mut dyn StarlarkSerializeContext,
    serialize_header: impl FnOnce(
        &A::StarlarkValue,
        &mut dyn StarlarkSerializeContext,
    ) -> crate::Result<()>,
) -> crate::Result<()>
where
    A: AValue<'v>,
    A::StarlarkValue: TrailingValues<'v>,
{
    let value = unsafe { &(*me).payload };
    let content = value.content();
    content.len().pagable_serialize(ctx.pagable())?;
    serialize_header(value, ctx)?;
    for elem in content {
        StarlarkSerialize::starlark_serialize(elem, ctx)?;
    }
    Ok(())
}

/// Counterpart of [`starlark_serialize_with_extra`].
///
/// `make_header` reads the fixed fields and builds the fixed payload from them
/// and the element count. `H` is `A::StarlarkValue` at the brand `'fv` the heap
/// is being deserialized at, which the heap types at `'v`: the brand erasure
/// described on [`AValue::starlark_deserialize`].
pub(super) unsafe fn starlark_deserialize_with_extra<'a, 'v, 'fv, A, H>(
    me: *mut AValueRepr<A::StarlarkValue>,
    ctx: &mut dyn StarlarkDeserializeContext<'a, 'fv>,
    make_header: impl FnOnce(&mut dyn StarlarkDeserializeContext<'a, 'fv>, usize) -> crate::Result<H>,
) -> crate::Result<()>
where
    A: AValue<'v, ExtraElem = Value<'v>>,
{
    debug_assert_eq!(mem::size_of::<H>(), mem::size_of::<A::StarlarkValue>());
    debug_assert_eq!(mem::align_of::<H>(), mem::align_of::<A::StarlarkValue>());
    let len = usize::pagable_deserialize(ctx.pagable())?;
    let header = make_header(&mut *ctx, len)?;
    unsafe {
        let payload_offset = AValueRepr::<A::StarlarkValue>::offset_of_payload();
        ptr::write((me as *mut u8).add(payload_offset) as *mut H, header);
        let extra_ptr = (me as *mut u8).add(payload_offset + A::offset_of_extra())
            as *mut MaybeUninit<Value<'fv>>;
        for i in 0..len {
            (*extra_ptr.add(i)).write(ctx.deserialize_value()?);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fmt;

    use allocative::Allocative;
    use starlark_derive::StarlarkRepresentations;
    use starlark_derive::starlark_value;

    use crate as starlark;
    use crate::any::ProvidesStaticType;
    use crate::environment::Module;
    use crate::values::Freeze;
    use crate::values::Freezer;
    use crate::values::NoSerialize;
    use crate::values::StarlarkPagable;
    use crate::values::StarlarkType;
    use crate::values::StarlarkValue;
    use crate::values::Trace;
    use crate::values::UnpackValue;
    use crate::values::Value;
    use crate::values::ValueTyped;
    use crate::values::dict::AllocDict;
    use crate::values::freeze_error::FreezeResult;
    use crate::values::layout::heap::name::StarlarkTestHeapName;
    use crate::values::types::list::value::FrozenListData;
    use crate::values::types::list::value::ListData;
    use crate::values::types::list::value::ListGen;
    use crate::values::types::tuple::value::Tuple;

    #[derive(Debug, Trace, ProvidesStaticType, NoSerialize, Allocative)]
    struct ReentrantTupleFreeze<'v>(RefCell<Option<Value<'v>>>);

    impl fmt::Display for ReentrantTupleFreeze<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("ReentrantTupleFreeze")
        }
    }

    #[derive(Debug, Clone, Copy, StarlarkRepresentations)]
    enum ReentrantTupleFreezeView<'v> {
        Unfrozen(ValueTyped<'v, ReentrantTupleFreeze<'v>>),
        Frozen(ValueTyped<'v, FrozenReentrantTupleFreeze>),
    }

    #[starlark_value(type = "reentrant_tuple_freeze")]
    impl<'v> StarlarkType<'v> for ReentrantTupleFreezeView<'v> {}

    #[starlark_value]
    impl<'v> StarlarkValue<'v> for ReentrantTupleFreeze<'v> {
        type Type = ReentrantTupleFreezeView<'v>;
    }

    #[derive(Debug, ProvidesStaticType, NoSerialize, Allocative, StarlarkPagable)]
    struct FrozenReentrantTupleFreeze;

    impl fmt::Display for FrozenReentrantTupleFreeze {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("FrozenReentrantTupleFreeze")
        }
    }

    #[starlark_value]
    impl<'v> StarlarkValue<'v> for FrozenReentrantTupleFreeze {
        type Canonical = ReentrantTupleFreeze<'v>;
        type Type = ReentrantTupleFreezeView<'v>;
    }

    impl<'v> Freeze<'v> for ReentrantTupleFreeze<'v> {
        type Frozen<'fv> = FrozenReentrantTupleFreeze;

        fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
            let owner = self
                .0
                .into_inner()
                .expect("test value should point back to the tuple containing it");
            let owner = freezer.freeze(owner)?;
            assert!(
                owner.downcast_ref::<Tuple<'fv>>().is_none(),
                "tuple should not be observable until its inline elements are initialized",
            );
            Ok(FrozenReentrantTupleFreeze)
        }
    }

    #[derive(Debug, Trace, ProvidesStaticType, NoSerialize, Allocative)]
    struct ReentrantListFreeze<'v>(RefCell<Option<Value<'v>>>);

    impl fmt::Display for ReentrantListFreeze<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("ReentrantListFreeze")
        }
    }

    #[derive(Debug, Clone, Copy, StarlarkRepresentations)]
    enum ReentrantListFreezeView<'v> {
        Unfrozen(ValueTyped<'v, ReentrantListFreeze<'v>>),
        Frozen(ValueTyped<'v, FrozenReentrantListFreeze>),
    }

    #[starlark_value(type = "reentrant_list_freeze")]
    impl<'v> StarlarkType<'v> for ReentrantListFreezeView<'v> {}

    #[starlark_value]
    impl<'v> StarlarkValue<'v> for ReentrantListFreeze<'v> {
        type Type = ReentrantListFreezeView<'v>;
    }

    #[derive(Debug, ProvidesStaticType, NoSerialize, Allocative, StarlarkPagable)]
    struct FrozenReentrantListFreeze;

    impl fmt::Display for FrozenReentrantListFreeze {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("FrozenReentrantListFreeze")
        }
    }

    #[starlark_value]
    impl<'v> StarlarkValue<'v> for FrozenReentrantListFreeze {
        type Canonical = ReentrantListFreeze<'v>;
        type Type = ReentrantListFreezeView<'v>;
    }

    impl<'v> Freeze<'v> for ReentrantListFreeze<'v> {
        type Frozen<'fv> = FrozenReentrantListFreeze;

        fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
            let owner = self
                .0
                .into_inner()
                .expect("test value should point back to the list containing it");
            let owner = freezer.freeze(owner)?;
            assert!(
                owner
                    .downcast_ref::<ListGen<FrozenListData<'fv>>>()
                    .is_none(),
                "list should not be observable until its inline elements are initialized",
            );
            Ok(FrozenReentrantListFreeze)
        }
    }

    #[test]
    fn tuple_cycle_freeze() {
        Module::with_temp_heap(|module| {
            let list = module.heap().alloc_list(&[]);
            let tuple = module.heap().alloc_tuple(&[list]);
            ListData::from_value_mut(list)
                .unwrap()
                .push(tuple, module.heap());
            module.set("t", tuple);
            module.freeze_named(StarlarkTestHeapName::frozen_heap_name())?;
            crate::Result::Ok(())
        })
        .unwrap();
    }

    #[test]
    fn tuple_freeze_initializes_extra_before_payload_is_observable() {
        Module::with_temp_heap(|module| {
            let reentrant = module
                .heap()
                .alloc_complex(ReentrantTupleFreeze(RefCell::new(None)));
            let tuple = module.heap().alloc_tuple(&[reentrant]);
            reentrant
                .downcast_ref::<ReentrantTupleFreeze>()
                .unwrap()
                .0
                .replace(Some(tuple));
            module.set("t", tuple);
            module.freeze_named(StarlarkTestHeapName::frozen_heap_name())?;
            crate::Result::Ok(())
        })
        .unwrap();
    }

    #[test]
    fn list_freeze_initializes_extra_before_payload_is_observable() {
        Module::with_temp_heap(|module| {
            let reentrant = module
                .heap()
                .alloc_complex(ReentrantListFreeze(RefCell::new(None)));
            let list = module.heap().alloc_list(&[reentrant]);
            reentrant
                .downcast_ref::<ReentrantListFreeze>()
                .unwrap()
                .0
                .replace(Some(list));
            module.set("l", list);
            module.freeze_named(StarlarkTestHeapName::frozen_heap_name())?;
            crate::Result::Ok(())
        })
        .unwrap();
    }

    #[test]
    fn test_freeze_to_existing_value() {
        // Empty dictionaries freeze to one statically allocated value.

        Module::with_temp_heap(|module| {
            let d0 = module.heap().alloc(AllocDict::EMPTY);
            let d1 = module.heap().alloc(AllocDict::EMPTY);
            // Pointers are not equal.
            assert_ne!(d0.0.raw(), d1.0.raw());

            module.set_extra_value(module.heap().alloc((d0, d1)));

            let module = module.freeze_named(StarlarkTestHeapName::frozen_heap_name())?;
            module.extra_value().unwrap().by_ref(|extra| {
                let (d0, d1) = <(Value, Value)>::unpack_value_err(*extra).unwrap();
                // Pointers are equal.
                assert_eq!(d0.0.raw(), d1.0.raw());
            });
            crate::Result::Ok(())
        })
        .unwrap();
    }
}
