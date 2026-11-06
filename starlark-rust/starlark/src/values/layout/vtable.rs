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

use std::cmp::Ordering;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Formatter;
use std::marker::PhantomData;
use std::mem;
use std::ops::Deref;
use std::ptr;

use allocative::Allocative;
use allocative::Visitor;
use dupe::Dupe;

use crate::any::AnyLifetime;
use crate::any::ProvidesStaticType;
use crate::cast::transmute;
use crate::collections::Hashed;
use crate::collections::StarlarkHashValue;
use crate::collections::StarlarkHasher;
use crate::docs::DocItem;
use crate::environment::Methods;
use crate::eval::Arguments;
use crate::eval::Evaluator;
use crate::pagable::DeserTypeId;
use crate::pagable::starlark_deserialize::StarlarkDeserializeContext;
use crate::pagable::starlark_serialize::StarlarkSerializeContext;
use crate::private::Private;
use crate::typing::Ty;
use crate::values::FreezeResult;
use crate::values::Freezer;
use crate::values::Heap;
use crate::values::StarlarkValue;
use crate::values::StringValue;
use crate::values::Tracer;
use crate::values::Value;
use crate::values::demand::Demand;
use crate::values::int::pointer_i32::PointerI32;
use crate::values::layout::avalue::AValue;
use crate::values::layout::const_type_id::ConstTypeId;
use crate::values::layout::heap::repr::AValueHeader;
use crate::values::layout::heap::repr::AValueRepr;
use crate::values::layout::value_alloc_size::ValueAllocSize;
use crate::values::starlark_type_id::StarlarkTypeId;
use crate::values::traits::StarlarkValueVTable;
use crate::values::traits::StarlarkValueVTableGet;

/// Untyped raw pointer to `StarlarkValue` without vtable.
///
/// * `'v` lifetime is heap lifetime
/// * `'a` lifetime is value lifetime
#[repr(C)]
#[derive(Copy, Clone, Dupe)]
pub(crate) struct StarlarkValueRawPtr {
    /// Points to the end of `AValueHeader`.
    /// May not be equal to the start of `StarlarkValue` due to alignment.
    pub(crate) ptr: *const (),
}

impl StarlarkValueRawPtr {
    /// The payload that follows the header at `ptr`. Only the address is used,
    /// so the header need not be initialized yet.
    #[inline]
    pub(crate) fn new_header_ptr(ptr: *const AValueHeader) -> Self {
        debug_assert!((ptr as usize).is_multiple_of(AValueHeader::ALIGN));
        StarlarkValueRawPtr {
            ptr: ptr.wrapping_add(1) as *const (),
        }
    }

    #[inline]
    pub(crate) fn new_header(ptr: &AValueHeader) -> Self {
        Self::new_header_ptr(ptr)
    }

    #[inline]
    pub(crate) fn new_pointer_i32(ptr: &'static PointerI32) -> Self {
        let ptr = ptr as *const PointerI32 as *const ();
        StarlarkValueRawPtr { ptr }
    }

    #[inline]
    pub(crate) unsafe fn value_ptr<T>(self) -> *mut T {
        assert!(
            AValueRepr::<PointerI32>::padding_after_header() == 0,
            "There is no header for `PointerI32`, but following code should work"
        );

        // `self.ptr` is a pointer to the end of `AValueHeader`.
        // Align it up to `T` by adding padding between `AValueRepr` fields.
        let ptr = self
            .ptr
            .wrapping_byte_add(AValueRepr::<T>::padding_after_header());
        debug_assert!(ptr.addr().is_multiple_of(mem::align_of::<T>()));
        ptr.cast::<T>().cast_mut()
    }

    #[inline]
    pub(crate) unsafe fn value_ref<'v, T: StarlarkValue<'v>>(self) -> &'v T {
        unsafe { &*self.value_ptr() }
    }
}

/// VTable for `AValue` operations.
///
/// This struct contains function pointers for all operations on a Starlark value,
/// allowing dynamic dispatch.
pub struct AValueVTable {
    // Common `AValue` fields.
    pub(crate) static_type_of_value: ConstTypeId,
    pub(crate) starlark_type_id: StarlarkTypeId,
    pub(crate) type_name: &'static str,
    /// Cache `type_name` here to avoid computing hash.
    pub(crate) type_as_allocative_key: allocative::Key,

    // `StarlarkValue`
    pub(crate) starlark_value: StarlarkValueVTable,

    /// Deserialization type identifier
    /// Used for vtable lookup during deserialization.
    pub(crate) deser_type_id: DeserTypeId,

    // `Drop`
    drop_in_place: fn(StarlarkValueRawPtr),

    // `AValue`
    pub(crate) is_str: bool,
    memory_size: fn(StarlarkValueRawPtr) -> ValueAllocSize,
    /// Generic over the brands the value is frozen out of and into, like `heap_copy` over the
    /// `Tracer`'s: the vtable is the erased instantiation's, the brands are the caller's.
    heap_freeze:
        for<'v, 'fv> fn(StarlarkValueRawPtr, &Freezer<'v, 'fv>) -> FreezeResult<Value<'fv>>,
    heap_copy: for<'v> fn(StarlarkValueRawPtr, &Tracer<'v>) -> Value<'v>,
    starlark_serialize:
        fn(StarlarkValueRawPtr, &mut dyn StarlarkSerializeContext) -> crate::Result<()>,
    /// Generic over the brand the value is deserialized at, like `heap_freeze` over the brand
    /// it freezes into: the vtable is the erased instantiation's, the brand is the caller's.
    pub(crate) starlark_deserialize: for<'de, 'fv> fn(
        StarlarkValueRawPtr,
        &mut dyn StarlarkDeserializeContext<'de, 'fv>,
    ) -> crate::Result<()>,

    // `StarlarkValue` supertraits.
    display: unsafe fn(StarlarkValueRawPtr) -> *const dyn Display,
    debug: unsafe fn(StarlarkValueRawPtr) -> *const dyn Debug,
    erased_serde_serialize: unsafe fn(StarlarkValueRawPtr) -> *const dyn erased_serde::Serialize,
    allocative: unsafe fn(StarlarkValueRawPtr) -> *const dyn Allocative,
    total_memory_for_profile: unsafe fn(StarlarkValueRawPtr) -> usize,
    visit_extra_allocative: for<'a, 'b> unsafe fn(StarlarkValueRawPtr, &'a mut Visitor<'b>),
}

struct GetTypeId<'v, T: StarlarkValue<'v>>(PhantomData<&'v T>);

impl<'v, T: StarlarkValue<'v>> GetTypeId<'v, T> {
    const TYPE_ID: ConstTypeId = ConstTypeId::of::<<T as ProvidesStaticType>::StaticType>();
    const STARLARK_TYPE_ID: StarlarkTypeId = StarlarkTypeId::of::<T>();
}

struct GetAllocativeKey<'v, T: StarlarkValue<'v>>(PhantomData<&'v T>);

impl<'v, T: StarlarkValue<'v>> GetAllocativeKey<'v, T> {
    const ALLOCATIVE_KEY: allocative::Key = allocative::Key::new(T::TYPE);
}

/// Helper struct to compute [`DeserTypeId`] as an associated const
/// for a generic `StarlarkValue` type parameter.
/// Used in `AValueVTable::new()` which is a `const fn`.
struct GetDeserTypeId<'v, T: StarlarkValue<'v>>(PhantomData<&'v T>);

impl<'v, T: StarlarkValue<'v>> GetDeserTypeId<'v, T> {
    const DESER_TYPE_ID: DeserTypeId = DeserTypeId::of::<T>();
}

/// Marker type for uninitialized deserialized values.
/// All methods panic with a descriptive message.
#[derive(Allocative)]
struct UninitializedValue;

impl Display for UninitializedValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "<uninitialized>")
    }
}

impl Debug for UninitializedValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "<uninitialized>")
    }
}

impl AValueVTable {
    /// Sentinel vtable for values that have been allocated but not yet deserialized.
    /// Every method panics with "accessing uninitialized deserialized value".
    pub(crate) fn uninitialized_sentinel() -> &'static AValueVTable {
        const UNINIT_ALLOCATIVE_KEY: allocative::Key = allocative::Key::new("UninitializedValue");
        const UNINIT_TYPE_ID: ConstTypeId = ConstTypeId::of::<UninitializedValue>();
        const UNINIT_STARLARK_TYPE_ID: StarlarkTypeId =
            StarlarkTypeId::from_type_id(UNINIT_TYPE_ID);
        const UNINIT_DESER_TYPE_ID: DeserTypeId = DeserTypeId::of::<UninitializedValue>();

        const PANIC_MSG: &str = "accessing a frozen value that has not been deserialized yet";

        // A `static`: walks recognize the sentinel by address (`walk_state`),
        // and a promoted `&const` can be duplicated per codegen unit.
        static SENTINEL: AValueVTable = AValueVTable {
            drop_in_place: |_| {},
            is_str: false,
            memory_size: |_| panic!("{}", PANIC_MSG),
            static_type_of_value: UNINIT_TYPE_ID,
            starlark_type_id: UNINIT_STARLARK_TYPE_ID,
            heap_freeze: |_, _| panic!("{}", PANIC_MSG),
            heap_copy: |_, _| panic!("{}", PANIC_MSG),
            starlark_serialize: |_, _| panic!("{}", PANIC_MSG),
            starlark_deserialize: |_, _| panic!("{}", PANIC_MSG),
            type_name: "UninitializedValue",
            type_as_allocative_key: UNINIT_ALLOCATIVE_KEY,
            deser_type_id: UNINIT_DESER_TYPE_ID,
            display: |_| panic!("{}", PANIC_MSG),
            debug: |_| panic!("{}", PANIC_MSG),
            erased_serde_serialize: |_| panic!("{}", PANIC_MSG),
            allocative: |_| panic!("{}", PANIC_MSG),
            total_memory_for_profile: |_| panic!("{}", PANIC_MSG),
            visit_extra_allocative: |_, _| panic!("{}", PANIC_MSG),
            starlark_value: StarlarkValueVTable::UNINITIALIZED_SENTINEL,
        };
        &SENTINEL
    }

    /// Public for use by `register_avalue_simple_frozen!` macro in doctests.
    /// Hidden from docs and uses private `AValue` bound to prevent direct external use.
    #[doc(hidden)]
    #[allow(private_bounds)]
    pub const fn new<'v, T: AValue<'v>>() -> &'static AValueVTable {
        &AValueVTable {
            drop_in_place: |p| unsafe {
                ptr::drop_in_place(p.value_ptr::<T::StarlarkValue>());
            },
            is_str: T::IS_STR,
            memory_size: |p| unsafe {
                let p = &*p.value_ptr::<T::StarlarkValue>();
                T::alloc_size_for_extra_len(T::extra_len(p))
            },
            // SAFETY (for the brand transmutes here and in `heap_copy`): This vtable serves the
            // type in every heap it is allocated in, so its entries are instantiated at one brand
            // `'v` and called at another. `p` points into the heap being frozen or copied, which
            // is the caller's, so the freezer's source brand and the tracer's brand name the heap
            // that `T`'s `'v` stands for.
            heap_freeze: |p, freezer| unsafe {
                let p = &mut *AValueRepr::from_payload_ptr_mut(p.value_ptr::<T::StarlarkValue>());
                T::heap_freeze(p, transmute!(&Freezer, &Freezer, freezer))
            },
            heap_copy: |p, tracer| unsafe {
                let p = &mut *AValueRepr::from_payload_ptr_mut(p.value_ptr::<T::StarlarkValue>());
                let value = T::heap_copy(p, transmute!(&Tracer, &Tracer, tracer));
                transmute!(Value, Value, value)
            },
            starlark_serialize: |p, ctx| unsafe {
                let p = AValueRepr::from_payload_ptr_mut(p.value_ptr::<T::StarlarkValue>())
                    as *const AValueRepr<T::StarlarkValue>;
                T::starlark_serialize(p, ctx)
            },
            starlark_deserialize: |p, ctx| unsafe {
                let p = AValueRepr::from_payload_ptr_mut(p.value_ptr::<T::StarlarkValue>());
                T::starlark_deserialize(p, ctx)
            },
            static_type_of_value: GetTypeId::<T::StarlarkValue>::TYPE_ID,
            starlark_type_id: GetTypeId::<T::StarlarkValue>::STARLARK_TYPE_ID,
            type_name: T::StarlarkValue::TYPE,
            type_as_allocative_key: GetAllocativeKey::<T::StarlarkValue>::ALLOCATIVE_KEY,
            deser_type_id: GetDeserTypeId::<T::StarlarkValue>::DESER_TYPE_ID,
            display: |this| unsafe {
                let this = this.value_ptr::<T::StarlarkValue>();
                let display = this as *const dyn Display;
                // Drop lifetime.
                mem::transmute(display)
            },
            debug: |this| unsafe {
                let this = this.value_ptr::<T::StarlarkValue>();
                let debug = this as *const dyn Debug;
                // Drop lifetime.
                mem::transmute(debug)
            },
            erased_serde_serialize: |this| unsafe {
                let this = this.value_ptr::<T::StarlarkValue>();
                let serialize = this as *const dyn erased_serde::Serialize;
                // Drop lifetime.
                mem::transmute(serialize)
            },
            allocative: |this| unsafe {
                let this = this.value_ptr::<T::StarlarkValue>();
                let allocative = this as *const dyn Allocative;
                // Drop lifetime.
                mem::transmute(allocative)
            },
            total_memory_for_profile: |this| unsafe {
                let p = &*this.value_ptr::<T::StarlarkValue>();
                T::total_memory_for_profile(p)
            },
            visit_extra_allocative: |this, visitor| unsafe {
                let p = &*this.value_ptr::<T::StarlarkValue>();
                T::visit_extra_allocative(p, visitor)
            },
            starlark_value: StarlarkValueVTableGet::<'v, T::StarlarkValue>::VTABLE,
        }
    }

    #[inline]
    pub(crate) fn type_value(&'static self) -> StringValue<'static> {
        (self.starlark_value.ty.get_type_value_static)()
    }

    pub(crate) fn type_starlark_repr(&'static self) -> Ty {
        (self.starlark_value.ty.get_type_starlark_repr)()
    }

    #[inline]
    pub(crate) fn methods(&'static self) -> Option<&'static Methods> {
        (self.starlark_value.ty.get_methods)()
    }

    pub(crate) fn drop_in_place(&'static self, value: StarlarkValueRawPtr) {
        (self.drop_in_place)(value)
    }
}

#[derive(Copy, Clone, Dupe)]
#[repr(C)]
pub(crate) struct AValueDyn<'v> {
    pub(crate) value: StarlarkValueRawPtr,
    vtable: &'static AValueVTable,
    // Invariant in the brand, like every carrier of one.
    _marker: PhantomData<fn(&'v ()) -> &'v ()>,
}

impl<'v> Debug for AValueDyn<'v> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("AValueDyn").finish_non_exhaustive()
    }
}

impl<'v> AValueDyn<'v> {
    #[inline]
    pub(crate) unsafe fn new(
        value: StarlarkValueRawPtr,
        vtable: &'static AValueVTable,
    ) -> AValueDyn<'v> {
        AValueDyn {
            value,
            vtable,
            _marker: PhantomData,
        }
    }

    #[inline]
    pub(crate) fn vtable(self) -> &'static AValueVTable {
        self.vtable
    }

    #[inline]
    pub(crate) fn memory_size(self) -> ValueAllocSize {
        (self.vtable.memory_size)(self.value)
    }

    pub(crate) fn as_allocative(self) -> &'v dyn Allocative {
        unsafe { &*(self.vtable.allocative)(self.value) }
    }

    pub(crate) fn total_memory_for_profile(self) -> usize {
        unsafe { (self.vtable.total_memory_for_profile)(self.value) }
    }

    pub(crate) fn visit_extra_allocative<'a, 'b: 'a>(self, visitor: &'a mut Visitor<'b>) {
        unsafe { (self.vtable.visit_extra_allocative)(self.value, visitor) }
    }

    #[inline]
    pub(crate) unsafe fn heap_freeze<'fv>(
        self,
        freezer: &Freezer<'v, 'fv>,
    ) -> FreezeResult<Value<'fv>> {
        (self.vtable.heap_freeze)(self.value, freezer)
    }

    #[inline]
    pub(crate) unsafe fn heap_copy(self, tracer: &Tracer<'v>) -> Value<'v> {
        (self.vtable.heap_copy)(self.value, tracer)
    }

    pub(crate) fn starlark_serialize(
        self,
        ctx: &mut dyn StarlarkSerializeContext,
    ) -> crate::Result<()> {
        (self.vtable.starlark_serialize)(self.value, ctx)
    }

    #[inline]
    pub(crate) fn downcast_ref<T: StarlarkValue<'v>>(self) -> Option<&'v T> {
        if self.vtable.static_type_of_value.get() == T::static_type_id() {
            // SAFETY: just checked whether we are pointing to the correct type.
            unsafe { Some(self.value.value_ref()) }
        } else {
            None
        }
    }

    #[inline]
    pub(crate) fn as_display(self) -> &'v dyn Display {
        unsafe { &*(self.vtable.display)(self.value) }
    }

    #[inline]
    pub(crate) fn as_debug(self) -> &'v dyn Debug {
        unsafe { &*(self.vtable.debug)(self.value) }
    }

    #[inline]
    pub(crate) fn as_serialize(self) -> &'v dyn erased_serde::Serialize {
        unsafe { &*(self.vtable.erased_serde_serialize)(self.value) }
    }
}

/// A value's vtable with the `Value` itself: what dispatching a [`StarlarkType`] method needs,
/// since the receiver handed to the method carries the `Value`. Derefs to the raw
/// [`AValueDyn`] for the operations that only need the payload.
#[derive(Copy, Clone, Dupe)]
pub(crate) struct AValueDynFull<'v> {
    avalue: AValueDyn<'v>,
    value: Value<'v>,
}

impl<'v> Deref for AValueDynFull<'v> {
    type Target = AValueDyn<'v>;

    #[inline]
    fn deref(&self) -> &AValueDyn<'v> {
        &self.avalue
    }
}

impl<'v> AValueDynFull<'v> {
    /// # Safety
    ///
    /// `avalue` must be `value`'s.
    #[inline]
    pub(crate) unsafe fn new(avalue: AValueDyn<'v>, value: Value<'v>) -> AValueDynFull<'v> {
        AValueDynFull { avalue, value }
    }

    #[inline]
    pub(crate) fn invoke(
        self,
        args: &Arguments<'v, '_>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> crate::Result<Value<'v>> {
        (self.avalue.vtable.starlark_value.invoke)(self.value, self.value, args, eval)
    }

    #[inline]
    pub(crate) fn documentation(self) -> DocItem {
        (self.avalue.vtable.starlark_value.documentation)(self.value)
    }

    pub(crate) fn typechecker_ty(self) -> Option<Ty> {
        (self.avalue.vtable.starlark_value.typechecker_ty)(self.value)
    }

    pub(crate) fn eval_type(self) -> Option<Ty> {
        (self.avalue.vtable.starlark_value.eval_type)(self.value)
    }

    #[inline]
    pub(crate) fn at(self, index: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        (self.avalue.vtable.starlark_value.at)(self.value, index, heap)
    }

    pub(crate) fn at2(
        self,
        index0: Value<'v>,
        index1: Value<'v>,
        heap: Heap<'v>,
    ) -> crate::Result<Value<'v>> {
        (self.avalue.vtable.starlark_value.at2)(self.value, index0, index1, heap, Private)
    }

    #[inline]
    pub(crate) fn is_in(self, collection: Value<'v>) -> crate::Result<bool> {
        (self.avalue.vtable.starlark_value.is_in)(self.value, collection)
    }

    #[inline]
    pub(crate) fn slice(
        self,
        start: Option<Value<'v>>,
        stop: Option<Value<'v>>,
        step: Option<Value<'v>>,
        heap: Heap<'v>,
    ) -> crate::Result<Value<'v>> {
        (self.avalue.vtable.starlark_value.slice)(self.value, start, stop, step, heap)
    }

    #[inline]
    pub(crate) fn get_attr(self, name: &str, heap: Heap<'v>) -> Option<Value<'v>> {
        (self.avalue.vtable.starlark_value.get_attr)(self.value, name, heap)
    }

    #[inline]
    pub(crate) fn get_attr_hashed(self, name: Hashed<&str>, heap: Heap<'v>) -> Option<Value<'v>> {
        (self.avalue.vtable.starlark_value.get_attr_hashed)(self.value, name, heap)
    }

    #[inline]
    pub(crate) fn has_attr(self, name: &str, heap: Heap<'v>) -> bool {
        (self.avalue.vtable.starlark_value.has_attr)(self.value, name, heap)
    }

    #[inline]
    pub(crate) fn dir_attr(self) -> Vec<String> {
        (self.avalue.vtable.starlark_value.dir_attr)(self.value)
    }

    #[inline]
    pub(crate) fn bit_and(self, other: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        (self.avalue.vtable.starlark_value.bit_and)(self.value, other, heap)
    }

    #[inline]
    pub(crate) fn bit_or(self, other: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        (self.avalue.vtable.starlark_value.bit_or)(self.value, other, heap)
    }

    #[inline]
    pub(crate) fn bit_xor(self, other: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        (self.avalue.vtable.starlark_value.bit_xor)(self.value, other, heap)
    }

    #[inline]
    pub(crate) fn bit_not(self, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        (self.avalue.vtable.starlark_value.bit_not)(self.value, heap)
    }

    #[inline]
    pub(crate) fn to_bool(self) -> bool {
        (self.avalue.vtable.starlark_value.to_bool)(self.value)
    }

    #[inline]
    pub(crate) fn length(self) -> crate::Result<i32> {
        (self.avalue.vtable.starlark_value.length)(self.value)
    }

    #[inline]
    pub(crate) fn iterate(self, me: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        (self.avalue.vtable.starlark_value.iterate)(self.value, me, heap)
    }

    #[inline]
    pub(crate) fn iter_next(self, index: usize, heap: Heap<'v>) -> Option<Value<'v>> {
        (self.avalue.vtable.starlark_value.iter_next)(self.value, index, heap)
    }

    #[inline]
    pub(crate) fn iter_size_hint(self, index: usize) -> (usize, Option<usize>) {
        (self.avalue.vtable.starlark_value.iter_size_hint)(self.value, index)
    }

    #[inline]
    pub(crate) fn iter_stop(self) {
        (self.avalue.vtable.starlark_value.iter_stop)(self.value)
    }

    #[inline]
    pub(crate) fn get_hash(self) -> crate::Result<StarlarkHashValue> {
        (self.avalue.vtable.starlark_value.get_hash)(self.value, Private)
    }

    #[inline]
    pub(crate) fn plus(self, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        (self.avalue.vtable.starlark_value.plus)(self.value, heap)
    }

    #[inline]
    pub(crate) fn minus(self, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        (self.avalue.vtable.starlark_value.minus)(self.value, heap)
    }

    #[inline]
    pub(crate) fn add(self, other: Value<'v>, heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
        (self.avalue.vtable.starlark_value.add)(self.value, other, heap)
    }

    #[inline]
    pub(crate) fn radd(self, other: Value<'v>, heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
        (self.avalue.vtable.starlark_value.radd)(self.value, other, heap)
    }

    #[inline]
    pub(crate) fn sub(self, other: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        (self.avalue.vtable.starlark_value.sub)(self.value, other, heap)
    }

    #[inline]
    pub(crate) fn mul(self, other: Value<'v>, heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
        (self.avalue.vtable.starlark_value.mul)(self.value, other, heap)
    }

    #[inline]
    pub(crate) fn rmul(self, other: Value<'v>, heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
        (self.avalue.vtable.starlark_value.rmul)(self.value, other, heap)
    }

    #[inline]
    pub(crate) fn div(self, other: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        (self.avalue.vtable.starlark_value.div)(self.value, other, heap)
    }

    #[inline]
    pub(crate) fn floor_div(self, other: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        (self.avalue.vtable.starlark_value.floor_div)(self.value, other, heap)
    }

    #[inline]
    pub(crate) fn percent(self, other: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        (self.avalue.vtable.starlark_value.percent)(self.value, other, heap)
    }

    #[inline]
    pub(crate) fn left_shift(self, other: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        (self.avalue.vtable.starlark_value.left_shift)(self.value, other, heap)
    }

    #[inline]
    pub(crate) fn right_shift(self, other: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        (self.avalue.vtable.starlark_value.right_shift)(self.value, other, heap)
    }

    #[inline]
    pub(crate) fn collect_repr(self, collector: &mut String) {
        (self.avalue.vtable.starlark_value.collect_repr)(self.value, collector)
    }

    #[inline]
    pub(crate) fn collect_repr_cycle(self, collector: &mut String) {
        (self.avalue.vtable.starlark_value.collect_repr_cycle)(self.value, collector)
    }

    #[inline]
    pub(crate) fn collect_str(self, collector: &mut String) {
        (self.avalue.vtable.starlark_value.collect_str)(self.value, collector)
    }

    #[inline]
    pub(crate) fn equals(self, other: Value<'v>) -> crate::Result<bool> {
        (self.avalue.vtable.starlark_value.equals)(self.value, other)
    }

    #[inline]
    pub(crate) fn compare(self, other: Value<'v>) -> crate::Result<Ordering> {
        (self.avalue.vtable.starlark_value.compare)(self.value, other)
    }

    #[inline]
    pub(crate) fn name_for_call_stack(self, me: Value<'v>) -> String {
        (self.avalue.vtable.starlark_value.name_for_call_stack)(self.value, me)
    }

    #[inline]
    pub(crate) fn export_as(
        self,
        variable_name: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> crate::Result<()> {
        (self.avalue.vtable.starlark_value.export_as)(self.value, variable_name, eval)
    }

    #[inline]
    pub(crate) fn set_at(self, index: Value<'v>, new_value: Value<'v>) -> crate::Result<()> {
        (self.avalue.vtable.starlark_value.set_at)(self.value, index, new_value)
    }

    #[inline]
    pub(crate) fn set_attr(self, attribute: &str, new_value: Value<'v>) -> crate::Result<()> {
        (self.avalue.vtable.starlark_value.set_attr)(self.value, attribute, new_value)
    }

    #[inline]
    pub(crate) fn write_hash(self, hasher: &mut StarlarkHasher) -> crate::Result<()> {
        (self.avalue.vtable.starlark_value.write_hash)(self.value, hasher)
    }

    #[inline]
    pub(crate) fn type_matches_value(self, value: Value<'v>) -> bool {
        (self.avalue.vtable.starlark_value.type_matches_value)(self.value, value, Private)
    }

    #[inline]
    pub(crate) fn provide(self, demand: &mut Demand<'_, 'v>) {
        (self.avalue.vtable.starlark_value.provide)(self.value, demand)
    }
}
