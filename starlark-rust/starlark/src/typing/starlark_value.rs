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
use std::hash::Hash;
use std::hash::Hasher;
use std::marker::PhantomData;

use allocative::Allocative;
use dupe::Dupe;
#[cfg(feature = "pagable")]
use pagable::StaticValue;
use starlark_syntax::codemap::Span;

use crate::typing::Ty;
use crate::typing::TyBasic;
use crate::typing::TypingBinOp;
use crate::typing::TypingOracleCtx;
use crate::typing::TypingUnOp;
use crate::typing::error::TypingError;
use crate::typing::error::TypingNoContextError;
use crate::typing::ty::TypeRenderConfig;
use crate::values::StarlarkValue;
use crate::values::bool::StarlarkBool;
use crate::values::dict::value::FrozenDict;
use crate::values::float::StarlarkFloat;
use crate::values::list::value::FrozenList;
use crate::values::none::NoneType;
use crate::values::set::value::FrozenSet;
use crate::values::starlark_type_id::StarlarkTypeId;
use crate::values::string::str_type::StarlarkStr;
use crate::values::traits::StarlarkTypeVTable;
use crate::values::traits::StarlarkTypeVTableGet;
use crate::values::tuple::value::Tuple;
use crate::values::types::bigint::StarlarkBigInt;
use crate::values::typing::type_compiled::alloc::TypeMatcherAlloc;
use crate::values::typing::type_compiled::matchers::StarlarkTypeIdMatcher;

#[derive(Debug, thiserror::Error)]
enum TyStarlarkValueError {
    #[error("Type `{0}` is not callable")]
    NotCallable(TyStarlarkValue),
}

// This is a bit suboptimal for binary size:
// we have two vtable instances for each type: this one, and the one within `AValue` vtable.
#[doc(hidden)]
pub struct TyStarlarkValueVTable {
    type_name: &'static str,
    vtable: StarlarkTypeVTable,
    starlark_type_id: StarlarkTypeId,
    /// `starlark_type_id` is `TypeId` of `T::Canonical`.
    /// This is `TypeId` of `T::Canonical::Canonical`.
    starlark_type_id_check: StarlarkTypeId,
}

// `StarlarkTypeVTable` contains function pointers which don't implement `Ord`,
// so we derive identity from `starlark_type_id`.
impl PartialEq for TyStarlarkValueVTable {
    fn eq(&self, other: &Self) -> bool {
        self.starlark_type_id == other.starlark_type_id
    }
}
impl Eq for TyStarlarkValueVTable {}
impl PartialOrd for TyStarlarkValueVTable {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for TyStarlarkValueVTable {
    fn cmp(&self, other: &Self) -> Ordering {
        self.starlark_type_id.cmp(&other.starlark_type_id)
    }
}

// Declare static TyStarlarkValueVTable for pagable serialization.
pagable::declare_static_value_type!(TyStarlarkValueVTable, TyStarlarkValueVTableStaticEntry);

/// Trait linking a type to its registered `StaticValue` of
/// `TyStarlarkValueVTable`, used for pagable round-trip by stable index.
///
/// `#[starlark_value]` implements this automatically for:
/// 1. plain types (e.g. `Foo`)
/// 2. lifetime-only types (e.g. `Foo<'v>`)
///
/// For any other shape — type or const parameters — call
/// [`register_ty_starlark_value!`][crate::register_ty_starlark_value]
/// once per concrete instantiation.
///
/// When the `pagable` feature is disabled this is just an empty marker trait
/// with a blanket impl.
#[cfg(feature = "pagable")]
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not registered for TyVTable lookup",
    label = "missing `starlark::register_ty_starlark_value!({Self})`",
    note = "call `starlark::register_ty_starlark_value!({Self})` once per concrete instantiation. \
            `#[starlark_value]` does this automatically for plain and lifetime-only types — \
            otherwise register manually."
)]
pub trait HasTyVTable {
    /// Handle to the type's `TyStarlarkValueVTable` entry, used for pagable
    /// round-trip of [`TyStarlarkValue`] by stable index.
    const TY_VTABLE_STATIC: StaticValue<TyStarlarkValueVTable>;
}

/// Empty marker trait when the `pagable` feature is disabled
#[cfg(not(feature = "pagable"))]
pub trait HasTyVTable {}

#[cfg(not(feature = "pagable"))]
impl<T: ?Sized> HasTyVTable for T {}

#[doc(hidden)]
pub struct TyStarlarkValueVTableGet<'v, T: StarlarkValue<'v>>(PhantomData<&'v T>);

impl<'v, T: StarlarkValue<'v>> TyStarlarkValueVTableGet<'v, T> {
    #[doc(hidden)]
    pub const VTABLE: TyStarlarkValueVTable = TyStarlarkValueVTable {
        type_name: T::TYPE,
        vtable: StarlarkTypeVTableGet::<T::Type>::VTABLE,
        starlark_type_id: StarlarkTypeId::of_canonical::<T>(),
        starlark_type_id_check: StarlarkTypeId::of_canonical::<T::Canonical>(),
    };
}

/// Internal: inside a `const _: () = { ... };` block, emit a `VTABLE` static
/// containing `TyStarlarkValueVTableGet::<$frozen_ty::Canonical>::VTABLE` and
/// a pagable `VTABLE_STATIC` entry pointing at it.
#[doc(hidden)]
#[macro_export]
macro_rules! __declare_ty_vtable_static {
    ($frozen_ty:ty) => {
        static VTABLE: $crate::__derive_refs::TyStarlarkValueVTable =
            $crate::__derive_refs::TyStarlarkValueVTableGet::<
                <$frozen_ty as $crate::values::StarlarkValue<'_>>::Canonical,
            >::VTABLE;
        $crate::__derive_refs::static_value!(
            pub VTABLE_STATIC: $crate::__derive_refs::TyStarlarkValueVTable = &VTABLE,
            $crate::__derive_refs::TyStarlarkValueVTableStaticEntry
        );
    };
}

/// Internal: emits its body only when `pagable` feature is enabled
#[cfg(feature = "pagable")]
#[doc(hidden)]
#[macro_export]
macro_rules! __starlark_pagable_only {
    ($($body:tt)*) => { $($body)* };
}

#[cfg(not(feature = "pagable"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __starlark_pagable_only {
    ($($body:tt)*) => {};
}

/// Register a `TyStarlarkValueVTable` for pagable round-trip of
/// [`TyStarlarkValue`][crate::typing::starlark_value::TyStarlarkValue].
///
/// `#[starlark_value]` invokes this automatically for plain and lifetime-only
/// types. Otherwise call it manually, once per concrete instantiation. See
/// [`HasTyVTable`] for the full rule.
///
/// When the `pagable` feature is disabled this macro expands to nothing
#[macro_export]
macro_rules! register_ty_starlark_value {
    // Register a type `$ty`. `$ty` can be a simple type (e.g. `NoneType`) or
    // a lifetime-parameterized type written with `'_` (e.g.
    // `StarlarkPromise<'_>`).
    ($ty:ty) => {
        $crate::__starlark_pagable_only! {
            const _: () = {
                $crate::__declare_ty_vtable_static!($ty);
                impl $crate::__derive_refs::HasTyVTable for $ty {
                    const TY_VTABLE_STATIC: $crate::__derive_refs::StaticValue<
                        $crate::__derive_refs::TyStarlarkValueVTable,
                    > = VTABLE_STATIC;
                }
            };
        }
    };
    // V-parameterized unfrozen variant. `$elided_ty` supplies the VTABLE
    // content, which is `'static` and doesn't depend on the original
    // lifetime; `$impl_ty` is the `impl Self` type with explicit `$lt`
    // lifetimes, needed when multi-lifetime unfrozen types (e.g.
    // `StructGen<'v, Value<'v>>`) must tie both args to `'v`.
    (generic = < $($lt:lifetime),* >, elided_ty = $elided_ty:ty, impl_ty = $impl_ty:ty) => {
        $crate::__starlark_pagable_only! {
            const _: () = {
                $crate::__declare_ty_vtable_static!($elided_ty);
                impl< $($lt),* > $crate::__derive_refs::HasTyVTable for $impl_ty {
                    const TY_VTABLE_STATIC: $crate::__derive_refs::StaticValue<
                        $crate::__derive_refs::TyStarlarkValueVTable,
                    > = VTABLE_STATIC;
                }
            };
        }
    };
}

/// Type implementation where typing is handled by the `StarlarkValue` trait implementation.
#[derive(Allocative, Clone, Copy, Dupe)]
pub struct TyStarlarkValue {
    #[allocative(skip)]
    #[cfg(feature = "pagable")]
    vtable: StaticValue<TyStarlarkValueVTable>,
    #[allocative(skip)]
    #[cfg(not(feature = "pagable"))]
    vtable: &'static TyStarlarkValueVTable,
}

impl Debug for TyStarlarkValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("TyStarlarkValue")
            .field("type_name", &self.vtable.type_name)
            .finish_non_exhaustive()
    }
}

impl Display for TyStarlarkValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.fmt_with_config(f, &TypeRenderConfig::Default)
    }
}

impl PartialEq for TyStarlarkValue {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.vtable.starlark_type_id == other.vtable.starlark_type_id
    }
}

impl Eq for TyStarlarkValue {}

impl Hash for TyStarlarkValue {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Hash type name because type id is not stable.
        // TODO(nga): store hash in vtable.
        self.vtable.type_name.hash(state);
    }
}

impl PartialOrd for TyStarlarkValue {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TyStarlarkValue {
    fn cmp(&self, other: &Self) -> Ordering {
        self.vtable.type_name.cmp(other.vtable.type_name)
    }
}

// Wrappers derive `Pagable` unconditionally, so these impls must exist in
// both modes; the body is unreachable when `pagable` is off.
impl pagable::PagableSerialize for TyStarlarkValue {
    fn pagable_serialize(
        &self,
        serializer: &mut dyn pagable::PagableSerializer,
    ) -> pagable::Result<()> {
        #[cfg(feature = "pagable")]
        {
            self.vtable.pagable_serialize(serializer)
        }
        #[cfg(not(feature = "pagable"))]
        {
            let _ = serializer;
            unreachable!("pagable serialization requires the `pagable` feature")
        }
    }
}

impl<'de> pagable::PagableDeserialize<'de> for TyStarlarkValue {
    fn pagable_deserialize<D: pagable::PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        #[cfg(feature = "pagable")]
        {
            Ok(TyStarlarkValue {
                vtable: pagable::StaticValue::<TyStarlarkValueVTable>::pagable_deserialize(
                    deserializer,
                )?,
            })
        }
        #[cfg(not(feature = "pagable"))]
        {
            let _ = deserializer;
            unreachable!("pagable deserialization requires the `pagable` feature")
        }
    }
}

impl TyStarlarkValue {
    /// Create a type instance from an implementation of `StarlarkValue`.
    /// When `pagable` is enabled, requires `T::Canonical` to have been
    /// registered via [`register_ty_starlark_value!`]; otherwise the bound is
    /// trivially satisfied by the blanket impl on [`HasTyVTable`].
    pub const fn new<'v, T: StarlarkValue<'v>>() -> TyStarlarkValue
    where
        T::Canonical: HasTyVTable,
    {
        TyStarlarkValue {
            #[cfg(feature = "pagable")]
            vtable: <T::Canonical as HasTyVTable>::TY_VTABLE_STATIC,
            #[cfg(not(feature = "pagable"))]
            vtable: &TyStarlarkValueVTableGet::<T::Canonical>::VTABLE,
        }
    }

    #[inline]
    pub(crate) fn starlark_type_id(self) -> StarlarkTypeId {
        self.vtable.starlark_type_id
    }

    // Cannot have this check in constructor where it belongs because `new` is `const`.
    #[inline]
    fn self_check(self) {
        debug_assert_eq!(
            self.vtable.starlark_type_id, self.vtable.starlark_type_id_check,
            "`Canonical` for `{}` is not canonical",
            self.vtable.type_name
        );
    }

    pub(crate) fn as_name(self) -> &'static str {
        self.self_check();
        self.vtable.type_name
    }

    pub(crate) const fn int() -> TyStarlarkValue {
        TyStarlarkValue::new::<StarlarkBigInt>()
    }

    pub(crate) const fn float() -> TyStarlarkValue {
        TyStarlarkValue::new::<StarlarkFloat>()
    }

    pub(crate) const fn tuple() -> TyStarlarkValue {
        TyStarlarkValue::new::<Tuple>()
    }

    pub(crate) fn is_str(self) -> bool {
        self.self_check();
        self == TyStarlarkValue::new::<StarlarkStr>()
    }

    pub(crate) fn is_int(self) -> bool {
        self.self_check();
        self == TyStarlarkValue::new::<StarlarkBigInt>()
    }

    pub(crate) fn is_list(self) -> bool {
        self.self_check();
        self == TyStarlarkValue::new::<FrozenList>()
    }

    pub(crate) fn is_dict(self) -> bool {
        self.self_check();
        self == TyStarlarkValue::new::<FrozenDict>()
    }

    pub(crate) fn is_tuple(self) -> bool {
        self.self_check();
        self == TyStarlarkValue::new::<Tuple>()
    }

    #[allow(dead_code)]
    pub(crate) fn is_set(self) -> bool {
        self.self_check();
        self == TyStarlarkValue::new::<FrozenSet>()
    }

    /// Result of applying unary operator to this type.
    pub(crate) fn un_op(self, un_op: TypingUnOp) -> Result<TyStarlarkValue, TypingNoContextError> {
        let has = match un_op {
            TypingUnOp::Plus => self.vtable.vtable.HAS_plus,
            TypingUnOp::Minus => self.vtable.vtable.HAS_minus,
            TypingUnOp::BitNot => self.vtable.vtable.HAS_bit_not,
        };
        if has {
            Ok(self)
        } else {
            Err(TypingNoContextError)
        }
    }

    pub(crate) fn bin_op(self, op: TypingBinOp, rhs: &TyBasic) -> Result<Ty, TypingNoContextError> {
        match (self.vtable.vtable.bin_op_ty)(op, rhs) {
            Some(ty) => Ok(ty),
            None => Err(TypingNoContextError),
        }
    }

    pub(crate) fn rbin_op(
        self,
        op: TypingBinOp,
        lhs: &TyBasic,
    ) -> Result<Ty, TypingNoContextError> {
        match (self.vtable.vtable.rbin_op_ty)(lhs, op) {
            Some(ty) => Ok(ty),
            None => Err(TypingNoContextError),
        }
    }

    pub(crate) fn index(self, _index: &TyBasic) -> Result<Ty, TypingNoContextError> {
        if self.vtable.vtable.HAS_at {
            Ok(Ty::any())
        } else {
            Err(TypingNoContextError)
        }
    }

    /// If this type can be slice, return the result type of slicing.
    pub(crate) fn slice(self) -> Result<Ty, TypingNoContextError> {
        if self.vtable.vtable.HAS_slice {
            // All known implementations of slice return self type.
            Ok(Ty::basic(TyBasic::StarlarkValue(self)))
        } else {
            Err(TypingNoContextError)
        }
    }

    pub(crate) fn is_indexable(self) -> bool {
        self.vtable.vtable.HAS_at
    }

    pub(crate) fn attr_from_methods(self, name: &str) -> Result<Ty, TypingNoContextError> {
        if let Some(methods) = (self.vtable.vtable.get_methods)() {
            if let Some(ty) = methods.get_ty(name) {
                return Ok(ty);
            }
        }
        Err(TypingNoContextError)
    }

    pub(crate) fn attr(self, name: &str) -> Result<Ty, TypingNoContextError> {
        if let Ok(ty) = self.attr_from_methods(name) {
            return Ok(ty);
        }
        if let Some(ty) = (self.vtable.vtable.attr_ty)(name) {
            return Ok(ty);
        }
        Err(TypingNoContextError)
    }

    pub(crate) fn is_callable(self) -> bool {
        self.vtable.vtable.HAS_invoke
    }

    pub(crate) fn validate_call(
        self,
        span: Span,
        oracle: TypingOracleCtx,
    ) -> Result<Ty, TypingError> {
        if self.is_callable() {
            Ok(Ty::any())
        } else {
            Err(oracle.mk_error(span, TyStarlarkValueError::NotCallable(self)))
        }
    }

    #[inline]
    pub(crate) fn is_iterable(vtable: &StarlarkTypeVTable) -> bool {
        vtable.HAS_iterate || vtable.HAS_iterate_collect
    }

    /// Instance of this type can be evaluated as a type.
    #[inline]
    pub(crate) fn is_type_from_vtable(vtable: &StarlarkTypeVTable) -> bool {
        vtable.HAS_eval_type
    }

    pub(crate) fn is_type(self) -> bool {
        self.self_check();
        Self::is_type_from_vtable(&self.vtable.vtable)
    }

    pub(crate) fn iter_item(self) -> Result<Ty, TypingNoContextError> {
        if Self::is_iterable(&self.vtable.vtable) {
            Ok(Ty::any())
        } else {
            Err(TypingNoContextError)
        }
    }

    /// Convert to runtime type matcher.
    pub(crate) fn matcher<T: TypeMatcherAlloc>(self, matcher: T) -> T::Result {
        self.self_check();

        // First handle special cases that can match faster than default matcher.
        // These are optimizations.
        if self.vtable.starlark_type_id == StarlarkTypeId::of::<StarlarkBigInt>() {
            matcher.int()
        } else if self.vtable.starlark_type_id == StarlarkTypeId::of::<StarlarkBool>() {
            matcher.bool()
        } else if self.vtable.starlark_type_id == StarlarkTypeId::of::<NoneType>() {
            matcher.none()
        } else if self.vtable.starlark_type_id == StarlarkTypeId::of::<StarlarkStr>() {
            matcher.str()
        } else {
            matcher.alloc(StarlarkTypeIdMatcher::new(self))
        }
    }

    pub(crate) fn fmt_with_config(
        &self,
        f: &mut fmt::Formatter<'_>,
        config: &TypeRenderConfig,
    ) -> fmt::Result {
        let type_name = match self.vtable.type_name {
            "string" => "str",
            "NoneType" => "None",
            name => name,
        };
        match config {
            TypeRenderConfig::Default => write!(f, "{type_name}"),
            TypeRenderConfig::LinkedType {
                render_linked_ty_starlark_value,
            } => {
                write!(f, "{}", render_linked_ty_starlark_value(self))
            }
        }
    }
}

#[cfg(all(test, feature = "pagable"))]
mod tests {
    use allocative::Allocative;
    use pagable::PagableDeserialize;
    use pagable::PagableSerialize;
    use pagable::testing::TestingDeserializer;
    use pagable::testing::TestingSerializer;
    use starlark_derive::ProvidesStaticType;
    use starlark_derive::StarlarkPagable;

    use super::*;
    use crate as starlark;
    use crate::values::any::StarlarkAny;
    use crate::values::any_complex::StarlarkAnyComplex;
    use crate::values::dict::value::FrozenDict;
    use crate::values::list::value::FrozenList;
    use crate::values::none::NoneType;
    use crate::values::tuple::value::Tuple;
    use crate::values::types::any_array::AnyArray;

    #[derive(Allocative, ProvidesStaticType, Debug, StarlarkPagable)]
    struct TestPayloadA;

    #[derive(Allocative, ProvidesStaticType, Debug, StarlarkPagable)]
    struct TestPayloadB;

    crate::register_starlark_any_complex!(TestPayloadA);
    crate::register_starlark_any_complex!(TestPayloadB);

    fn round_trip(value: TyStarlarkValue) -> TyStarlarkValue {
        let mut ser = TestingSerializer::new();
        value.pagable_serialize(&mut ser).unwrap();
        let bytes = ser.finish();
        let mut de = TestingDeserializer::new(&bytes);
        TyStarlarkValue::pagable_deserialize(&mut de).unwrap()
    }

    #[test]
    fn test_round_trip_simple_type() {
        // Simple non-generic type.
        let ty = TyStarlarkValue::new::<NoneType>();
        let restored = round_trip(ty);
        assert_eq!(ty, restored);
        assert_eq!(ty.to_string(), restored.to_string());
    }

    #[test]
    fn test_round_trip_frozen_generic() {
        // V-parameterized frozen variant.
        let ty = TyStarlarkValue::new::<FrozenDict>();
        let restored = round_trip(ty);
        assert_eq!(ty, restored);
    }

    #[test]
    fn test_round_trip_live_generic_matches_frozen_canonical() {
        // `Tuple::Canonical = TupleGen<Value<'v>>` and
        // `Tuple::Canonical = Tuple` (which is `TupleGen<Value<'v>>`). Both go
        // through the same `HasTyVTable` impl and should be indistinguishable.
        let ty_frozen = TyStarlarkValue::new::<Tuple<'static>>();
        let ty_live = TyStarlarkValue::new::<Tuple>();
        assert_eq!(ty_frozen, ty_live);
        assert_eq!(round_trip(ty_frozen), ty_frozen);
        assert_eq!(round_trip(ty_live), ty_live);
    }

    #[test]
    fn test_distinct_types_are_distinct() {
        // Different types must produce distinct TyStarlarkValues and distinct
        // pagable indices (otherwise round-tripping would mix them up).
        let a = TyStarlarkValue::new::<NoneType>();
        let b = TyStarlarkValue::new::<FrozenDict>();
        let c = TyStarlarkValue::new::<FrozenList>();
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
        assert_eq!(round_trip(a), a);
        assert_eq!(round_trip(b), b);
        assert_eq!(round_trip(c), c);
    }

    #[test]
    fn test_starlark_any_per_t_distinct() {
        // `StarlarkAny<T>` uses the per-T marker trait; different T's must
        // each get their own typing vtable entry (never collapse via the
        // blanket `HasTyVTable for StarlarkAny<T>` impl).
        use starlark_syntax::codemap::CodeMap;

        use crate::environment::Globals;

        let any_codemap = TyStarlarkValue::new::<StarlarkAny<CodeMap>>();
        let any_globals = TyStarlarkValue::new::<StarlarkAny<Globals>>();
        assert_ne!(any_codemap, any_globals);
        assert_eq!(round_trip(any_codemap), any_codemap);
        assert_eq!(round_trip(any_globals), any_globals);

        // And neither collides with a non-wrapped registered type.
        assert_ne!(any_codemap, TyStarlarkValue::new::<NoneType>());
    }

    #[test]
    fn test_any_array_per_t_distinct() {
        // Same contract for `AnyArray<T>`: different T gets different entry.
        use crate::eval::bc::stack_ptr::BcSlotOut;
        use crate::eval::runtime::slots::LocalSlotId;

        let arr_slot = TyStarlarkValue::new::<AnyArray<LocalSlotId>>();
        let arr_bc = TyStarlarkValue::new::<AnyArray<BcSlotOut>>();
        assert_ne!(arr_slot, arr_bc);
        assert_eq!(round_trip(arr_slot), arr_slot);
        assert_eq!(round_trip(arr_bc), arr_bc);
    }

    #[test]
    fn test_starlark_any_complex_per_t_distinct() {
        // Per-T typing vtable registration for `StarlarkAnyComplex<T>`: each
        // concrete `T` gets its own entry, round-trip preserves identity.
        let ty_a = TyStarlarkValue::new::<StarlarkAnyComplex<TestPayloadA>>();
        let ty_b = TyStarlarkValue::new::<StarlarkAnyComplex<TestPayloadB>>();
        assert_ne!(ty_a, ty_b);
        assert_eq!(round_trip(ty_a), ty_a);
        assert_eq!(round_trip(ty_b), ty_b);
    }

    #[test]
    fn test_any_vs_any_array_same_t_distinct() {
        // `StarlarkAny<LocalSlotId>` and `AnyArray<LocalSlotId>` wrap the same T but are
        // different container types — their entries must not alias.
        use crate::eval::runtime::slots::LocalSlotId;

        let any_slot = TyStarlarkValue::new::<StarlarkAny<LocalSlotId>>();
        let arr_slot = TyStarlarkValue::new::<AnyArray<LocalSlotId>>();
        assert_ne!(any_slot, arr_slot);
        assert_eq!(round_trip(any_slot), any_slot);
        assert_eq!(round_trip(arr_slot), arr_slot);
    }

    #[test]
    fn test_multiple_values_in_one_stream() {
        // Two distinct types serialized/deserialized in one stream must each
        // round-trip to their own entry without cross-contamination.
        let none_ty = TyStarlarkValue::new::<NoneType>();
        let tuple_ty = TyStarlarkValue::new::<Tuple<'static>>();

        let mut ser = TestingSerializer::new();
        none_ty.pagable_serialize(&mut ser).unwrap();
        tuple_ty.pagable_serialize(&mut ser).unwrap();
        let bytes = ser.finish();

        let mut de = TestingDeserializer::new(&bytes);
        let restored_none = TyStarlarkValue::pagable_deserialize(&mut de).unwrap();
        let restored_tuple = TyStarlarkValue::pagable_deserialize(&mut de).unwrap();
        assert_eq!(restored_none, none_ty);
        assert_eq!(restored_tuple, tuple_ty);
        assert_ne!(restored_none, restored_tuple);
    }
}
