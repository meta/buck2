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

pub(crate) mod string;

use std::convert::Infallible;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Formatter;
use std::marker;
use std::mem;
use std::ops::Deref;
use std::ptr::NonNull;
use std::sync::atomic::AtomicPtr;
use std::sync::atomic::Ordering;

use allocative::Allocative;
use dupe::Clone_;
use dupe::Copy_;
use dupe::Dupe_;
use serde::Serialize;
use starlark_map::Hashed;

use crate as starlark;
use crate::any::AnyLifetime;
use crate::any::IsStaticType;
use crate::any::ProvidesStaticType;
use crate::any::ReinfectStatic;
use crate::typing::Ty;
use crate::values::AllocFrozenValue;
use crate::values::AllocValue;
use crate::values::Freeze;
use crate::values::FreezeResult;
use crate::values::Freezer;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::HeapEdge;
use crate::values::StarlarkType;
use crate::values::StarlarkTypeExpr;
use crate::values::StarlarkValue;
use crate::values::StringValue;
use crate::values::Trace;
use crate::values::Tracer;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::alloc_value::AllocStringValue;
use crate::values::layout::avalue::AValue;
use crate::values::layout::avalue::AValueImpl;
use crate::values::layout::heap::repr::AValueRepr;
use crate::values::string::str_type::StarlarkStr;
use crate::values::type_repr::StarlarkTypeRepr;

/// [`Value`] wrapper which asserts contained value is of type `<T>`.
#[derive(Copy_, Clone_, Dupe_, ProvidesStaticType, Allocative)]
#[allocative(skip)] // Heap owns the value.
#[repr(transparent)]
pub struct ValueTyped<'v, T: StarlarkValue<'v>>(Value<'v>, marker::PhantomData<T>);
/// [`Value`] wrapper which asserts contained value is of type `<T>` and is frozen.
///
/// At the brand of a frozen heap (`'fh`, `'fv`, `'fm`: no mutable heap has such a brand) every
/// value is frozen, so there a [`ValueTyped`] says as much. This type is for the brand where
/// frozen and unfrozen values mix, a module's value heap `'v`: it records that a value handed out
/// there is frozen by construction, such as a provider collection, a transitive-set definition,
/// or a compiler product in the module's frozen heap. The frozen bit of the pointer is set; every
/// constructor establishes that.
///
/// Freezing a `FrozenValueTyped` re-types it at the new brand and copies nothing, so a type that
/// holds one can be frozen whether or not `T` itself can be: a `T` that only ever lives in
/// frozen heaps needs no [`Freeze`] impl of its own.
#[derive(Copy_, Clone_, Dupe_, ProvidesStaticType, Allocative)]
#[allocative(skip)] // Heap owns the value.
#[repr(transparent)]
pub struct FrozenValueTyped<'v, T: StarlarkValue<'v>>(Value<'v>, marker::PhantomData<T>);

unsafe impl<'v, 'f, T: StarlarkValue<'f>> Trace<'v> for FrozenValueTyped<'f, T> {
    fn trace(&mut self, _tracer: &Tracer<'v>) {}
}

impl<'v, T: StarlarkValue<'v>> Debug for ValueTyped<'v, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ValueTyped").field(&self.0).finish()
    }
}

impl<'v, T: StarlarkValue<'v>> Debug for FrozenValueTyped<'v, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("FrozenValueTyped").field(&self.0).finish()
    }
}

impl<'v, T: StarlarkValue<'v>> Display for ValueTyped<'v, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl<'v, T: StarlarkValue<'v>> Display for FrozenValueTyped<'v, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl<'v, T: StarlarkValue<'v>> Serialize for ValueTyped<'v, T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'v, T: StarlarkValue<'v>> Serialize for FrozenValueTyped<'v, T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'v, T: StarlarkValue<'v>> PartialEq for ValueTyped<'v, T> {
    fn eq(&self, other: &Self) -> bool {
        // Poor man specialization.
        if T::static_type_id() == StarlarkStr::static_type_id() {
            // SAFETY: just checked type ids.
            let (this, other) = unsafe {
                (
                    StringValue::new_unchecked(self.0),
                    StringValue::new_unchecked(other.0),
                )
            };
            this.0.ptr_eq(other.0) || StarlarkStr::eq(this.as_ref(), other.as_ref())
        } else {
            // Slow comparison with virtual call.
            self.0 == other.0
        }
    }
}

impl<'v, T: StarlarkValue<'v>> Eq for ValueTyped<'v, T> {}

impl<'v, T: StarlarkValue<'v>> PartialEq for FrozenValueTyped<'v, T> {
    fn eq(&self, other: &Self) -> bool {
        self.to_value_typed() == other.to_value_typed()
    }
}

impl<'v, T: StarlarkValue<'v>> Eq for FrozenValueTyped<'v, T> {}

impl<'v, T: StarlarkValue<'v>> ValueTyped<'v, T> {
    /// Downcast.
    #[inline]
    pub fn new(value: Value<'v>) -> Option<ValueTyped<'v, T>> {
        value.downcast_ref::<T>()?;
        Some(ValueTyped(value, marker::PhantomData))
    }

    /// Downcast.
    #[inline]
    pub fn new_err(value: Value<'v>) -> crate::Result<ValueTyped<'v, T>> {
        value.downcast_ref_err::<T>()?;
        Ok(ValueTyped(value, marker::PhantomData))
    }

    /// Construct typed value without checking the value is of type `<T>`.
    ///
    /// # Safety
    ///
    /// `value` must be of type `T`.
    #[inline]
    pub unsafe fn new_unchecked(value: Value<'v>) -> ValueTyped<'v, T> {
        debug_assert!(value.downcast_ref::<T>().is_some());
        ValueTyped(value, marker::PhantomData)
    }

    /// # Safety
    ///
    /// As for [`Value::new_repr`].
    #[inline]
    pub(crate) unsafe fn new_repr<A: AValue<'v, StarlarkValue = T>>(
        repr: NonNull<AValueRepr<AValueImpl<'v, A>>>,
    ) -> ValueTyped<'v, T> {
        ValueTyped(unsafe { Value::new_repr(repr) }, marker::PhantomData)
    }

    /// A value allocated in a frozen heap: the pointer carries the frozen tag.
    ///
    /// # Safety
    ///
    /// As for [`Value::new_frozen_ptr`].
    #[inline]
    pub(crate) unsafe fn new_frozen_repr<A: AValue<'v, StarlarkValue = T>>(
        repr: NonNull<AValueRepr<AValueImpl<'v, A>>>,
    ) -> ValueTyped<'v, T> {
        ValueTyped(
            unsafe { Value::new_frozen_ptr(repr.cast(), A::IS_STR) },
            marker::PhantomData,
        )
    }

    /// Construct a typed wrapper around a value that may not be initialized yet.
    ///
    /// Unlike [`new_unchecked`](Self::new_unchecked), this omits even the `debug_assert`
    /// type-check. This is useful in pagable deserialization.
    ///
    /// # Safety
    ///
    /// As for [`new_unchecked`](Self::new_unchecked), once the value is initialized.
    #[inline]
    pub(crate) unsafe fn new_allow_uninitialized(value: Value<'v>) -> ValueTyped<'v, T> {
        ValueTyped(value, marker::PhantomData)
    }

    /// A statically allocated value, which is usable at any brand because a `&'static` to it
    /// outlives every heap.
    #[inline]
    pub(crate) fn new_static_repr<A: AValue<'static, StarlarkValue = T>>(
        repr: &'static AValueRepr<AValueImpl<'static, A>>,
    ) -> ValueTyped<'v, T> {
        // Statics carry the frozen tag, like every value not allocated in an unfrozen heap.
        // SAFETY: a static is immortal, and the reference covers the whole object.
        ValueTyped(
            unsafe { Value::new_frozen_ptr(NonNull::from(repr).cast(), A::IS_STR) },
            marker::PhantomData,
        )
    }

    /// Erase the type.
    #[inline]
    pub fn to_value(self) -> Value<'v> {
        self.0
    }

    /// Get the reference to the pointed value.
    #[inline]
    pub fn as_ref(self) -> &'v T {
        // SAFETY: type is checked in constructor.
        unsafe { self.0.downcast_ref_unchecked() }
    }

    /// Compute the hash value.
    pub fn hashed(self) -> crate::Result<Hashed<Self>> {
        let hash = if let Some(s) = self.to_value().unpack_starlark_str() {
            s.get_hash()
        } else {
            self.to_value().get_hash()?
        };
        Ok(Hashed::new_unchecked(hash, self))
    }
}

impl<T: StarlarkValue<'static>> ValueTyped<'static, T> {
    /// The value, for use with any heap.
    ///
    /// Data at the `'static` brand is immortal, so it can be used at every brand; see
    /// [`HeapEdge::immortal`]. The type follows the brand: a `ValueTyped<'static, Tuple<'static>>`
    /// becomes a `ValueTyped<'v, Tuple<'v>>`.
    #[inline]
    pub fn at<'v>(self) -> ValueTyped<'v, ReinfectStatic<'v, T>>
    where
        T::StaticType: StarlarkValue<'static> + IsStaticType + Sized,
        for<'lt> ReinfectStatic<'lt, T>: StarlarkValue<'lt> + Sized,
    {
        HeapEdge::immortal().rebrand(self)
    }
}

#[derive(thiserror::Error, Debug)]
#[error("Expected frozen value of type `{expected}`, got unfrozen: `{value}`")]
struct NotFrozenError {
    expected: Ty,
    value: String,
}

impl<'v, T: StarlarkValue<'v>> FrozenValueTyped<'v, T> {
    /// Downcast a value known to be frozen.
    #[inline]
    fn new_frozen(value: Value<'v>) -> Option<FrozenValueTyped<'v, T>> {
        debug_assert!(value.is_frozen());
        value.downcast_ref::<T>()?;
        Some(FrozenValueTyped(value, marker::PhantomData))
    }

    /// Downcast; `None` if the value is not of type `T` or not frozen.
    #[inline]
    pub fn new(value: Value<'v>) -> Option<FrozenValueTyped<'v, T>> {
        if !value.is_frozen() {
            return None;
        }
        Self::new_frozen(value)
    }

    /// Downcast; an error if the value is not of type `T` or not frozen.
    #[inline]
    pub fn new_err(value: Value<'v>) -> crate::Result<FrozenValueTyped<'v, T>> {
        value.downcast_ref_err::<T>()?;
        if !value.is_frozen() {
            return Err(crate::Error::new_value(NotFrozenError {
                expected: T::starlark_type_repr(),
                value: value.to_string_for_type_error(),
            }));
        }
        Ok(FrozenValueTyped(value, marker::PhantomData))
    }

    /// The typed value, if it is frozen; always the case for a value at a frozen heap's brand.
    #[inline]
    pub fn from_typed(value: ValueTyped<'v, T>) -> Option<FrozenValueTyped<'v, T>> {
        if value.to_value().is_frozen() {
            Some(FrozenValueTyped(value.to_value(), marker::PhantomData))
        } else {
            None
        }
    }

    /// Erase the type.
    #[inline]
    pub fn to_value(self) -> Value<'v> {
        self.0
    }

    /// Convert to the value.
    #[inline]
    pub fn to_value_typed(self) -> ValueTyped<'v, T> {
        // SAFETY: Type checked in the constructors.
        unsafe { ValueTyped::new_unchecked(self.0) }
    }

    /// Get the reference to the pointed value.
    #[inline]
    pub fn as_ref(self) -> &'v T {
        self.to_value_typed().as_ref()
    }
}

impl<'v> ValueTyped<'v, StarlarkStr> {
    /// Get the Rust string reference.
    #[inline]
    pub fn as_str(self) -> &'v str {
        self.as_ref().as_str()
    }
}

unsafe impl<'v, T: StarlarkValue<'v>> Trace<'v> for ValueTyped<'v, T> {
    fn trace(&mut self, tracer: &Tracer<'v>) {
        tracer.trace(&mut self.0);
        // If type of value changed, dereference will produce the wrong object type.
        debug_assert!(self.0.downcast_ref::<T>().is_some());
    }
}

impl<'v, T: StarlarkValue<'v>> Deref for FrozenValueTyped<'v, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        self.as_ref()
    }
}

impl<'v, T: StarlarkValue<'v>> Deref for ValueTyped<'v, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        self.as_ref()
    }
}

impl<'v, T: StarlarkValue<'v>> StarlarkTypeRepr for ValueTyped<'v, T> {
    type Canonical = <T as StarlarkTypeRepr>::Canonical;

    fn starlark_type_repr() -> Ty {
        T::starlark_type_repr()
    }
}

/// A parameter of a type with one representation, bound to that representation. A type with
/// several representations is bound as its view or as a [`ValueOf`]; a specific representation
/// of it is not a parameter binding.
impl<'v, T> UnpackValue<'v> for ValueTyped<'v, T>
where
    T: StarlarkValue<'v> + StarlarkType<'v, Get = ValueTyped<'v, T>>,
{
    type Type = T;
    type Error = Infallible;

    #[inline]
    fn unpack(_value: ValueOf<'v, T>, get: ValueTyped<'v, T>) -> Result<Self, Infallible> {
        Ok(get)
    }
}

#[derive(thiserror::Error, Debug)]
#[error(
    "Value `{value}` is a `{ty}`, but not in the representation `{representation}` the function needs"
)]
struct NotRepresentationError {
    ty: Ty,
    representation: &'static str,
    value: String,
}

#[cold]
fn not_representation_error<'v, T: StarlarkValue<'v>>(value: Value<'v>) -> crate::Error {
    crate::Error::new_value(NotRepresentationError {
        ty: T::starlark_type_repr(),
        representation: std::any::type_name::<T>(),
        value: value.to_string_for_type_error(),
    })
}

impl<'v, T: StarlarkValue<'v>> AllocValue<'v> for ValueTyped<'v, T>
where
    T::Type: StarlarkTypeExpr<'v>,
{
    type Type = T::Type;

    fn alloc_value(self, _heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        ValueOf::from_representation(self)
    }
}

impl<'v, T> Freeze<'v> for ValueTyped<'v, T>
where
    T: StarlarkValue<'v>,
    T: Freeze<'v>,
    for<'fv> <T as Freeze<'v>>::Frozen<'fv>: StarlarkValue<'fv>,
{
    type Frozen<'fv> = ValueTyped<'fv, <T as Freeze<'v>>::Frozen<'fv>>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        Ok(ValueTyped::new_err(self.0.freeze(freezer)?)
            .expect("Freezing a value is known to be well-behaved"))
    }
}

/// The value is frozen already, so freezing it copies nothing and its type does not change:
/// `Frozen<'fv>` is `T` at `'fv`. That takes no [`Freeze`] impl on `T`, see the type doc.
impl<'v, T> Freeze<'v> for FrozenValueTyped<'v, T>
where
    T: StarlarkValue<'v>,
    T::StaticType: IsStaticType,
    for<'fv> <T::StaticType as IsStaticType>::Reinfect<'fv>: StarlarkValue<'fv>,
{
    type Frozen<'fv> = FrozenValueTyped<'fv, <T::StaticType as IsStaticType>::Reinfect<'fv>>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        // The freezer hands a frozen value back at `'fv` through its `SealEdge`.
        Ok(FrozenValueTyped::new_err(self.0.freeze(freezer)?)
            .expect("a frozen value's type does not change across brands"))
    }
}

impl<'v> AllocStringValue<'v> for StringValue<'v> {
    fn alloc_string_value(self, _heap: Heap<'v>) -> StringValue<'v> {
        self
    }
}

impl<'v, T: StarlarkValue<'v>> StarlarkTypeRepr for FrozenValueTyped<'v, T> {
    type Canonical = <T as StarlarkTypeRepr>::Canonical;

    fn starlark_type_repr() -> Ty {
        T::starlark_type_repr()
    }
}

/// A parameter bound to this representation of the type, frozen: an unfrozen value of the type
/// is of the declared type and fails as a conversion.
impl<'v, T: StarlarkValue<'v>> UnpackValue<'v> for FrozenValueTyped<'v, T>
where
    T::Type: StarlarkTypeExpr<'v>,
{
    type Type = T::Type;
    type Error = crate::Error;

    fn unpack(
        value: ValueOf<'v, T::Type>,
        _get: <T::Type as StarlarkTypeExpr<'v>>::Get,
    ) -> crate::Result<Self> {
        let value = value.to_value();
        if !value.is_frozen() {
            return Err(crate::Error::new_value(NotFrozenError {
                expected: T::starlark_type_repr(),
                value: value.to_string_for_type_error(),
            }));
        }
        FrozenValueTyped::new_frozen(value).ok_or_else(|| not_representation_error::<T>(value))
    }
}

impl<'v, T: StarlarkValue<'v>> AllocValue<'v> for FrozenValueTyped<'v, T>
where
    T::Type: StarlarkTypeExpr<'v>,
{
    type Type = T::Type;

    fn alloc_value(self, _heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        ValueOf::from_representation(self.to_value_typed())
    }
}

impl<'v, T: StarlarkValue<'v>> AllocFrozenValue<'v> for FrozenValueTyped<'v, T>
where
    T::Type: StarlarkTypeExpr<'v>,
{
    type Type = T::Type;

    fn alloc_frozen_value(self, _heap: FrozenHeap<'v>) -> ValueOf<'v, Self::Type> {
        ValueOf::from_representation(self.to_value_typed())
    }
}

impl<'v, T: StarlarkValue<'v>> crate::pagable::StarlarkSerialize for FrozenValueTyped<'v, T> {
    fn starlark_serialize(
        &self,
        ctx: &mut dyn crate::pagable::starlark_serialize::StarlarkSerializeContext,
    ) -> crate::Result<()> {
        self.0.starlark_serialize(ctx)
    }
}

impl<'v, T: StarlarkValue<'v>> crate::pagable::StarlarkDeserialize<'v> for FrozenValueTyped<'v, T> {
    fn starlark_deserialize(
        ctx: &mut dyn crate::pagable::starlark_deserialize::StarlarkDeserializeContext<'_, 'v>,
    ) -> crate::Result<Self> {
        let v = ValueTyped::<T>::starlark_deserialize(ctx)?.to_value();
        // Pagable deserializes only frozen heaps, so the pointer carries the frozen tag even
        // before the value behind it is initialized.
        debug_assert!(v.is_frozen());
        Ok(FrozenValueTyped(v, marker::PhantomData))
    }
}

/// Only frozen heaps are serialized; frozen types store their contents as `ValueTyped<'fv, T>`.
impl<'v, T: StarlarkValue<'v>> crate::pagable::StarlarkSerialize for ValueTyped<'v, T> {
    fn starlark_serialize(
        &self,
        ctx: &mut dyn crate::pagable::starlark_serialize::StarlarkSerializeContext,
    ) -> crate::Result<()> {
        self.to_value().starlark_serialize(ctx)
    }
}

impl<'v, T: StarlarkValue<'v>> crate::pagable::StarlarkDeserialize<'v> for ValueTyped<'v, T> {
    fn starlark_deserialize(
        ctx: &mut dyn crate::pagable::starlark_deserialize::StarlarkDeserializeContext<'_, 'v>,
    ) -> crate::Result<Self> {
        let v = ctx.deserialize_value()?;
        // SAFETY: pagable deserializes this field through the same Rust type that serialized
        // it.
        Ok(unsafe { ValueTyped::new_allow_uninitialized(v) })
    }
}

/// `Atomic<Option<FrozenValueTyped<'v, T>>>`.
///
/// Holds a back reference that is filled in after its holder has been frozen and can no longer be
/// mutated: a [`Def`](crate::eval::compiler::def::Def)'s module. Because the value is frozen it is
/// not traced, and freezing the holder only re-types it at the new brand.
pub(crate) struct AtomicFrozenValueTypedOption<'v, T> {
    ptr: AtomicPtr<()>,
    /// The auto traits of the `FrozenValueTyped<'v, T>` held.
    _marker: marker::PhantomData<(Value<'v>, T)>,
}

// `encode` and `decode` transmute `Option<Value>` <-> `*mut ()`; the niche maps `None` to null.
const _: () = assert!(mem::size_of::<Option<Value<'static>>>() == mem::size_of::<*mut ()>());

impl<'v, T: StarlarkValue<'v>> AtomicFrozenValueTypedOption<'v, T> {
    fn encode(value: Option<FrozenValueTyped<'v, T>>) -> *mut () {
        let value: Option<Value<'v>> = value.map(FrozenValueTyped::to_value);
        // SAFETY: The sizes match (asserted above), and `Option<Value>` has no padding: `None`
        // is the null niche of the pointer.
        unsafe { mem::transmute(value) }
    }

    /// # Safety
    ///
    /// `raw` must come from `encode` on this type.
    unsafe fn decode(raw: *mut ()) -> Option<FrozenValueTyped<'v, T>> {
        // SAFETY: The caller's obligation.
        let value: Option<Value<'v>> = unsafe { mem::transmute(raw) };
        // `encode` took a `FrozenValueTyped<'v, T>`.
        value.map(|v| FrozenValueTyped(v, marker::PhantomData))
    }

    pub(crate) fn new(value: Option<FrozenValueTyped<'v, T>>) -> Self {
        Self {
            ptr: AtomicPtr::new(Self::encode(value)),
            _marker: marker::PhantomData,
        }
    }

    pub(crate) fn load_relaxed(&self) -> Option<FrozenValueTyped<'v, T>> {
        // SAFETY: Only `encode`d pointers are stored.
        unsafe { Self::decode(self.ptr.load(Ordering::Relaxed)) }
    }

    pub(crate) fn store_relaxed(&self, value: FrozenValueTyped<'v, T>) {
        self.ptr.store(Self::encode(Some(value)), Ordering::Relaxed);
    }
}

unsafe impl<'v, T: StarlarkValue<'v>> Trace<'v> for AtomicFrozenValueTypedOption<'v, T> {
    fn trace(&mut self, _: &Tracer<'v>) {
        // The value is frozen.
    }
}

impl<'v, T> Freeze<'v> for AtomicFrozenValueTypedOption<'v, T>
where
    T: StarlarkValue<'v>,
    T::StaticType: IsStaticType,
    for<'fv> <T::StaticType as IsStaticType>::Reinfect<'fv>: StarlarkValue<'fv>,
{
    type Frozen<'fv> =
        AtomicFrozenValueTypedOption<'fv, <T::StaticType as IsStaticType>::Reinfect<'fv>>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        Ok(AtomicFrozenValueTypedOption::new(
            self.load_relaxed().map(|v| v.freeze(freezer)).transpose()?,
        ))
    }
}

impl<'v, T: StarlarkValue<'v>> crate::pagable::StarlarkSerialize
    for AtomicFrozenValueTypedOption<'v, T>
{
    fn starlark_serialize(
        &self,
        ctx: &mut dyn crate::pagable::starlark_serialize::StarlarkSerializeContext,
    ) -> crate::Result<()> {
        self.load_relaxed().starlark_serialize(ctx)
    }
}

impl<'v, T: StarlarkValue<'v>> crate::pagable::StarlarkDeserialize<'v>
    for AtomicFrozenValueTypedOption<'v, T>
{
    fn starlark_deserialize(
        ctx: &mut dyn crate::pagable::starlark_deserialize::StarlarkDeserializeContext<'_, 'v>,
    ) -> crate::Result<Self> {
        Ok(Self::new(
            <Option<FrozenValueTyped<'v, T>> as crate::pagable::StarlarkDeserialize>::starlark_deserialize(ctx)?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use starlark_derive::starlark_module;

    use crate as starlark;
    use crate::assert::Assert;
    use crate::environment::GlobalsBuilder;
    use crate::eval::Evaluator;
    use crate::tests::util::TestComplexValue;
    use crate::values::FrozenValueTyped;
    use crate::values::Heap;
    use crate::values::Value;
    use crate::values::ValueOf;
    use crate::values::int::pointer_i32::PointerI32;
    use crate::values::none::NoneType;

    #[test]
    fn int() {
        let v = FrozenValueTyped::<PointerI32>::new(Value::testing_new_int(17)).unwrap();
        assert_eq!(17, v.as_ref().get().to_i32());
    }

    #[test]
    fn test_unpack_value_for_frozen_value_typed() {
        #[starlark_module]
        fn module(globals: &mut GlobalsBuilder) {
            fn mutable<'v>(heap: Heap<'v>) -> anyhow::Result<ValueOf<'v, TestComplexValue<'v>>> {
                Ok(ValueOf::alloc(heap, TestComplexValue(Value::new_none())))
            }

            fn frozen<'v>(eval: &mut Evaluator<'v, '_, '_>) -> anyhow::Result<Value<'v>> {
                Ok(eval.frozen_heap(|fh, edge| {
                    edge.rebrand(
                        fh.alloc_representation(TestComplexValue(Value::new_none()))
                            .to_value(),
                    )
                }))
            }

            fn takes_frozen_value_typed<'v>(
                value: FrozenValueTyped<'v, TestComplexValue<'v>>,
            ) -> anyhow::Result<NoneType> {
                let _ = value;
                Ok(NoneType)
            }
        }

        let mut a = Assert::new();
        a.globals_add(module);

        a.pass("takes_frozen_value_typed(frozen())");
        a.fail("takes_frozen_value_typed(1)", "Type of parameter `value` doesn't match, expected `TestComplexValue`, actual `int (repr: 1)`");
        a.fail(
            "takes_frozen_value_typed(mutable())",
            "Expected frozen value",
        );
    }
}
