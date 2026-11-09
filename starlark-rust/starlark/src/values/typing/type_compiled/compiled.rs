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

use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Formatter;
use std::hash::Hash;
use std::hash::Hasher;

use allocative::Allocative;
use dupe::Dupe;
use pagable::Pagable;
use pagable::pagable_typetag;
use starlark_derive::StarlarkPagable;
use starlark_derive::starlark_module;
use starlark_derive::starlark_value;
use starlark_derive::type_matcher;
use starlark_map::StarlarkHasher;
use starlark_syntax::slice_vec_ext::SliceExt;
use starlark_syntax::slice_vec_ext::VecExt;
use thiserror::Error;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::environment::Methods;
use crate::environment::MethodsBuilder;
use crate::pagable::static_value::TypeCompiledStaticRegistered;
use crate::pagable::static_value::static_type_compiled;
use crate::private::Private;
use crate::typing::Ty;
use crate::values::AllocStaticSimple;
use crate::values::AllocValue;
use crate::values::Demand;
use crate::values::Freeze;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::NoSerialize;
use crate::values::StarlarkType;
use crate::values::StarlarkValue;
use crate::values::StaticValueRegistered;
use crate::values::StringValue;
use crate::values::Trace;
use crate::values::Value;
use crate::values::ValueTyped;
use crate::values::dict::DictRef;
use crate::values::list::ListRef;
use crate::values::none::NoneType;
use crate::values::type_repr::StarlarkTypeRepr;
use crate::values::types::tuple::value::Tuple;
use crate::values::typing::type_compiled::factory::TYPE_COMPILED_BOOL;
use crate::values::typing::type_compiled::factory::TYPE_COMPILED_INT;
use crate::values::typing::type_compiled::factory::TYPE_COMPILED_NONE;
use crate::values::typing::type_compiled::factory::TYPE_COMPILED_STRING;
use crate::values::typing::type_compiled::factory::TypeCompiledFactory;
use crate::values::typing::type_compiled::matcher::TypeMatcher;
use crate::values::typing::type_compiled::matcher::TypeMatcherDyn;

// Static type-compiled value for `typing.Any`.
static_type_compiled!(TYPE_COMPILED_ANY: IsAny, Ty::any());

#[derive(Debug, Error)]
enum TypingError {
    /// The value does not have the specified type
    #[error("Value `{0}` of type `{1}` does not match the type annotation `{2}` for {3}")]
    TypeAnnotationMismatch(String, String, String, String),
    /// The given type annotation does not represent a type
    #[error("Type `{0}` is not a valid type annotation")]
    InvalidTypeAnnotation(String),
    #[error("`{{A: B}}` cannot be used as type, perhaps you meant `dict[A, B]`")]
    Dict,
    #[error("`[X]` cannot be used as type, perhaps you meant `list[X]`")]
    List,
    /// The given type annotation does not exist, but the user might have forgotten quotes around
    /// it
    #[error(r#"Found `{0}` instead of a valid type annotation. Perhaps you meant `"{1}"`?"#)]
    PerhapsYouMeant(String, String),
    #[error("Value of type `{1}` does not match type `{2}`: {0}")]
    ValueDoesNotMatchType(String, &'static str, String),
    #[error("String literals are not allowed in type expressions: `{0}`")]
    StringLiteralNotAllowed(String),
}

pub(crate) trait TypeCompiledDyn: Debug + Allocative + Send + Sync + 'static {
    fn as_ty_dyn(&self) -> &Ty;
    fn is_runtime_wildcard_dyn(&self) -> bool;
    fn to_frozen_dyn<'f>(&self, heap: FrozenHeap<'f>) -> TypeCompiled<'f>;
}

// TODO(nga): derive.
unsafe impl<'v> ProvidesStaticType<'v> for &'v dyn TypeCompiledDyn {
    type StaticType = &'static dyn TypeCompiledDyn;
}

impl<T> TypeCompiledDyn for TypeCompiledImplAsStarlarkValue<T>
where
    T: TypeMatcher,
{
    fn as_ty_dyn(&self) -> &Ty {
        &self.ty
    }
    fn is_runtime_wildcard_dyn(&self) -> bool {
        self.type_compiled_impl.is_wildcard()
    }
    fn to_frozen_dyn<'f>(&self, heap: FrozenHeap<'f>) -> TypeCompiled<'f> {
        TypeCompiled(heap.alloc_simple::<TypeCompiledImplAsStarlarkValue<T>>(Self::clone(self)))
    }
}

#[derive(
    Clone,
    Eq,
    PartialEq,
    Debug,
    Allocative,
    ProvidesStaticType,
    NoSerialize,
    StarlarkPagable
)]
#[starlark_pagable(bound = "T: TypeMatcher")]
/// A compiled type expression wrapped as a Starlark value with a type matcher.
pub struct TypeCompiledImplAsStarlarkValue<T: 'static> {
    #[starlark_pagable(pagable)]
    type_compiled_impl: T,
    #[starlark_pagable(pagable)]
    ty: Ty,
}

// SAFETY: TypeMatcherRegistered is only implemented via #[type_matcher], which
// registers concrete matcher vtables by inventory and relies on the load-time
// constructors emitted by `TypeCompiled::alloc` for generic monomorphizations.
unsafe impl<T: crate::values::typing::type_compiled::matcher::TypeMatcherRegistered>
    crate::pagable::vtable_register::VtableRegistered for TypeCompiledImplAsStarlarkValue<T>
{
}

// SAFETY: TypeCompiledImplAsStarlarkValue<T> is only statically allocated when T is
// registered via static_type_compiled!, which ensures proper pagable registration.
unsafe impl<T: TypeCompiledStaticRegistered> StaticValueRegistered
    for TypeCompiledImplAsStarlarkValue<T>
{
}

#[cfg(all(test, feature = "pagable"))]
impl<T> TypeCompiledImplAsStarlarkValue<T> {
    pub(crate) fn new_for_test(
        type_compiled_impl: T,
        ty: Ty,
    ) -> TypeCompiledImplAsStarlarkValue<T> {
        TypeCompiledImplAsStarlarkValue {
            type_compiled_impl,
            ty,
        }
    }

    pub(crate) fn ty_for_test(&self) -> &Ty {
        &self.ty
    }

    pub(crate) fn impl_for_test(&self) -> &T {
        &self.type_compiled_impl
    }
}

impl<T> TypeCompiledImplAsStarlarkValue<T>
where
    TypeCompiledImplAsStarlarkValue<T>: StarlarkValue<'static>,
{
    pub(crate) const fn alloc_static(
        imp: T,
        ty: Ty,
    ) -> AllocStaticSimple<TypeCompiledImplAsStarlarkValue<T>>
    where
        T: TypeCompiledStaticRegistered,
    {
        AllocStaticSimple::alloc(TypeCompiledImplAsStarlarkValue {
            type_compiled_impl: imp,
            ty,
        })
    }
}

#[doc(hidden)]
#[derive(Hash, Eq, PartialEq, Debug, Clone, Allocative, Pagable)]
#[pagable_typetag(TypeMatcherDyn)]
pub struct DummyTypeMatcher;

#[type_matcher]
impl TypeMatcher for DummyTypeMatcher {
    fn matches(&self, _value: Value) -> bool {
        unreachable!()
    }
}

// Register the canonical (`DummyTypeMatcher`) variant. Other matchers'
// `Canonical` points here.
crate::register_ty_starlark_value!(TypeCompiledImplAsStarlarkValue<DummyTypeMatcher>);

starlark::methods_static!(TYPE_COMPILED_METHODS = type_compiled_methods);

/// The behaviour of a compiled type, whose representations are one per matcher.
#[starlark_value(type = "type")]
impl<'v> StarlarkType<'v> for TypeCompiled<'v> {
    type Get = Self;

    fn get(value: Value<'v>) -> Option<Self> {
        value
            .request_value::<&dyn TypeCompiledDyn>()
            .is_some()
            .then_some(TypeCompiled(value))
    }

    fn write_hash(&self, hasher: &mut StarlarkHasher) -> crate::Result<()> {
        Hash::hash(self.as_ty(), hasher);
        Ok(())
    }

    fn equals(&self, other: Value<'v>) -> crate::Result<bool> {
        let Some(other) = other.request_value::<&dyn TypeCompiledDyn>() else {
            return Ok(false);
        };
        Ok(self.as_ty() == other.as_ty_dyn())
    }

    fn eval_type(&self) -> Option<Ty> {
        // `TypeCompiled::new` handles this type explicitly,
        // but implement this function to make proc-macro generate `bit_or`.
        // Also safer to be explicit here.
        Some(self.as_ty().clone())
    }

    fn get_methods() -> Option<&'static Methods>
    where
        Self: Sized,
    {
        Some(TYPE_COMPILED_METHODS.methods())
    }
}

#[starlark_value]
impl<'v, T: 'static> StarlarkValue<'v> for TypeCompiledImplAsStarlarkValue<T>
where
    T: TypeMatcher,
{
    type Type = TypeCompiled<'v>;

    fn with_type<R>(this: ValueTyped<'v, Self>, f: impl FnOnce(&Self::Type) -> R) -> R {
        f(&TypeCompiled(this.to_value()))
    }

    fn type_matches_value(&self, value: Value<'v>, _private: Private) -> bool {
        self.type_compiled_impl.matches(value)
    }

    fn provide(&'v self, demand: &mut Demand<'_, 'v>) {
        demand.provide_ref_static::<dyn TypeCompiledDyn>(self);
    }
}

impl<T: TypeMatcher> Display for TypeCompiledImplAsStarlarkValue<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.ty)
    }
}

#[starlark_module]
fn type_compiled_methods(methods: &mut MethodsBuilder) {
    /// True iff the value matches this type.
    fn matches<'v>(this: Value<'v>, value: Value<'v>) -> anyhow::Result<bool> {
        Ok(this.get_ref().type_matches_value(value))
    }

    /// Error if the value does not match this type.
    fn check_matches<'v>(this: Value<'v>, value: Value<'v>) -> anyhow::Result<NoneType> {
        if !this.get_ref().type_matches_value(value) {
            return Err(TypingError::ValueDoesNotMatchType(
                value.to_repr(),
                value.get_type(),
                TypeCompiled(this).to_string(),
            )
            .into());
        }
        Ok(NoneType)
    }
}

/// Wrapper for a [`Value`] that acts like a runtime type matcher.
#[derive(
    Debug,
    Allocative,
    Freeze,
    Trace,
    Clone,
    Copy,
    Dupe,
    ProvidesStaticType,
    StarlarkPagable
)]
pub struct TypeCompiled<'v>(
    /// A `TypeCompiledImplAsStarlarkValue`.
    Value<'v>,
);

impl<'v> Display for TypeCompiled<'v> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.downcast() {
            Ok(t) => Display::fmt(&t.as_ty_dyn(), f),
            Err(_) => {
                // This is unreachable, but we should not panic in `Display`.
                Display::fmt(&self.0, f)
            }
        }
    }
}

impl<'v> StarlarkTypeRepr for TypeCompiled<'v> {
    type Canonical = TypeCompiledImplAsStarlarkValue<DummyTypeMatcher>;

    fn starlark_type_repr() -> Ty {
        TypeCompiledImplAsStarlarkValue::<DummyTypeMatcher>::starlark_type_repr()
    }
}

impl<'v> AllocValue<'v> for TypeCompiled<'v> {
    fn alloc_value(self, _heap: Heap<'v>) -> Value<'v> {
        self.0
    }
}

impl<'v> TypeCompiled<'v> {
    pub(crate) fn unchecked_new(value: Value<'v>) -> Self {
        TypeCompiled(value)
    }

    fn downcast(self) -> anyhow::Result<&'v dyn TypeCompiledDyn> {
        self.0
            .request_value::<&dyn TypeCompiledDyn>()
            .ok_or_else(|| anyhow::anyhow!("Not TypeCompiledImpl (internal error)"))
    }

    /// Check if given value matches this type.
    pub fn matches(&self, value: Value<'v>) -> bool {
        self.0.get_ref().type_matches_value(value)
    }

    /// Get the typechecker type for this runtime type.
    pub fn as_ty(&self) -> &'v Ty {
        self.downcast().unwrap().as_ty_dyn()
    }

    /// True if `TypeCompiled` matches any type at runtime.
    /// However, compile-time/lint typechecker may still check the type.
    pub(crate) fn is_runtime_wildcard(self) -> bool {
        self.downcast().unwrap().is_runtime_wildcard_dyn()
    }

    #[cold]
    #[inline(never)]
    fn check_type_error(self, value: Value<'v>, arg_name: Option<&str>) -> crate::Result<()> {
        Err(crate::Error::new_runtime_type(
            crate::RuntimeTypeErrorKind::Annotation,
            TypingError::TypeAnnotationMismatch(
                value.to_str(),
                value.get_type().to_owned(),
                self.to_string(),
                match arg_name {
                    None => "return type".to_owned(),
                    Some(x) => format!("argument `{x}`"),
                },
            ),
        ))
    }

    pub(crate) fn check_type(self, value: Value<'v>, arg_name: Option<&str>) -> crate::Result<()> {
        if self.matches(value) {
            Ok(())
        } else {
            self.check_type_error(value, arg_name)
        }
    }

    pub(crate) fn to_inner(self) -> Value<'v> {
        self.0
    }

    pub(crate) fn write_hash(self, hasher: &mut StarlarkHasher) -> crate::Result<()> {
        self.0.write_hash(hasher)
    }

    // Dead code, but may become useful in the future.
    pub(crate) fn _equals(self, other: Self) -> crate::Result<bool> {
        self.0.equals(other.0)
    }

    /// Copy the type into a frozen heap.
    ///
    /// For the compiler, which builds types at the value heap and keeps them in the IR. The type
    /// is copied even when it is already frozen, since nothing says which heap it was frozen in;
    /// the statics (`typing.Any`, `None`, `bool`, `int`, `str`) are immortal and are returned as
    /// they are.
    pub fn to_frozen<'f>(self, heap: FrozenHeap<'f>) -> TypeCompiled<'f> {
        if let Some(s) = self.as_static() {
            return s;
        }
        self.downcast().unwrap().to_frozen_dyn(heap)
    }

    /// This type at any brand, if it is one of the statics: those are immortal, so they are the
    /// same value at every brand. The compiler reaches `to_frozen` with a `None`, `bool`, `int`
    /// or `str` annotation far more often than with `typing.Any`, and copying a static into the
    /// frozen heap per annotation adds up.
    fn as_static<'f>(self) -> Option<TypeCompiled<'f>> {
        macro_rules! try_static {
            ($s:expr) => {
                if self.0.ptr_eq($s.at().to_value()) {
                    return Some(TypeCompiled($s.at().to_value()));
                }
            };
        }
        try_static!(TYPE_COMPILED_ANY);
        try_static!(TYPE_COMPILED_NONE);
        try_static!(TYPE_COMPILED_BOOL);
        try_static!(TYPE_COMPILED_INT);
        try_static!(TYPE_COMPILED_STRING);
        None
    }
}

impl<'v> Hash for TypeCompiled<'v> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self.0.get_hash() {
            Ok(h) => h.hash(state),
            Err(_) => {
                // Unreachable, but we should not panic in `Hash`.
            }
        }
    }
}

impl<'v> PartialEq for TypeCompiled<'v> {
    #[allow(clippy::manual_unwrap_or)]
    fn eq(&self, other: &Self) -> bool {
        self.0.equals(other.0).unwrap_or_default()
    }
}

impl<'v> Eq for TypeCompiled<'v> {}

/// Registers the AValue vtable of `TypeCompiledImplAsStarlarkValue<T>` for
/// deserialization, once per monomorphization per image, when the image
/// loads. This is the vtable counterpart of the generic typetag registration
/// in `#[pagable_typetag]`: `__vtable_registration_anchor` emits one program
/// constructor record per monomorphization, so a matcher instantiation that
/// exists in the binary can page in without any hand-written
/// per-instantiation registration.
#[cfg(feature = "pagable")]
extern "C" fn __vtable_do_register<T: TypeMatcher>() {
    use crate::pagable::vtable_registry::GENERIC_VTABLE_ACCUMULATOR;
    use crate::pagable::vtable_registry::VTableRegistryEntry;
    GENERIC_VTABLE_ACCUMULATOR.push(VTableRegistryEntry {
        deser_type_id: crate::pagable::vtable_registry::DeserTypeId::of::<
            TypeCompiledImplAsStarlarkValue<T>,
        >(),
        vtable: crate::values::layout::vtable::AValueVTable::new::<
            crate::values::layout::avalues::simple::AValueFrozen<
                TypeCompiledImplAsStarlarkValue<T>,
            >,
        >(),
    });
}

#[cfg(feature = "pagable")]
#[inline(never)]
extern "C" fn __vtable_registration_anchor<T: TypeMatcher>() {
    pagable::__pagable_emit_generic_typetag_registration!(__vtable_do_register::<T>);
}

// These functions are small, but are deliberately out-of-line so we get better
// information in profiling about the origin of these closures
impl<'v> TypeCompiled<'v> {
    pub(crate) fn alloc<M: TypeMatcher>(
        type_compiled_impl: M,
        ty: Ty,
        heap: Heap<'v>,
    ) -> TypeCompiled<'v> {
        // Keep the monomorphized anchor and its emitted constructor record
        // linked until `#[used(linker)]` is stable in the supported
        // toolchains.
        #[cfg(feature = "pagable")]
        core::hint::black_box(__vtable_registration_anchor::<M> as extern "C" fn());
        TypeCompiled(heap.alloc_simple(TypeCompiledImplAsStarlarkValue {
            type_compiled_impl,
            ty,
        }))
    }

    pub(crate) fn type_list_of(t: TypeCompiled<'v>, heap: Heap<'v>) -> TypeCompiled<'v> {
        TypeCompiledFactory::alloc_ty(&Ty::list(t.as_ty().clone()), heap)
    }

    pub(crate) fn type_set_of(t: TypeCompiled<'v>, heap: Heap<'v>) -> TypeCompiled<'v> {
        TypeCompiledFactory::alloc_ty(&Ty::set(t.as_ty().clone()), heap)
    }

    pub(crate) fn type_any_of_two(
        t0: TypeCompiled<'v>,
        t1: TypeCompiled<'v>,
        heap: Heap<'v>,
    ) -> TypeCompiled<'v> {
        let ty = Ty::union2(t0.as_ty().clone(), t1.as_ty().clone());
        TypeCompiledFactory::alloc_ty(&ty, heap)
    }

    pub(crate) fn type_any_of(ts: Vec<TypeCompiled<'v>>, heap: Heap<'v>) -> TypeCompiled<'v> {
        let ty = Ty::unions(ts.into_map(|t| t.as_ty().clone()));
        TypeCompiledFactory::alloc_ty(&ty, heap)
    }

    pub(crate) fn type_dict_of(
        kt: TypeCompiled<'v>,
        vt: TypeCompiled<'v>,
        heap: Heap<'v>,
    ) -> TypeCompiled<'v> {
        let ty = Ty::dict(kt.as_ty().clone(), vt.as_ty().clone());
        TypeCompiledFactory::alloc_ty(&ty, heap)
    }

    pub(crate) fn from_ty(ty: &Ty, heap: Heap<'v>) -> Self {
        TypeCompiledFactory::alloc_ty(ty, heap)
    }

    /// Evaluate type annotation at runtime.
    pub fn new(ty: Value<'v>, heap: Heap<'v>) -> anyhow::Result<Self> {
        if ty.request_value::<&dyn TypeCompiledDyn>().is_some() {
            // This branch is optimization: `TypeCompiledAsStarlarkValue` implements `eval_type`,
            // but this branch avoids copying the type.
            Ok(TypeCompiled(ty))
        } else {
            let ty = Self::ty_from_value(ty, &|ty| invalid_type_annotation(ty, heap))?;
            Ok(TypeCompiled::from_ty(&ty, heap))
        }
    }

    /// The type a type annotation value denotes; `invalid` describes a value that is not one.
    fn ty_from_value<'x>(
        ty: Value<'x>,
        invalid: &dyn Fn(Value<'x>) -> TypingError,
    ) -> anyhow::Result<Ty> {
        if let Some(s) = StringValue::new(ty) {
            Err(TypingError::StringLiteralNotAllowed(s.to_string()).into())
        } else if ty.is_none() {
            Ok(Ty::none())
        } else if let Some(t) = Tuple::from_value(ty) {
            let elems = t.content().try_map(|t| Self::ty_from_value(*t, invalid))?;
            Ok(Ty::tuple(elems))
        } else if let Some(t) = ListRef::from_value(ty) {
            match t.content() {
                [] | [_] => Err(TypingError::List.into()),
                // A union type, can match any
                ts @ [_, _, ..] => Ok(Ty::unions(
                    ts.try_map(|t| Self::ty_from_value(*t, invalid))?,
                )),
            }
        } else if let Some(t) = ty.request_value::<&dyn TypeCompiledDyn>() {
            Ok(t.as_ty_dyn().clone())
        } else {
            match ty.get_ref().eval_type() {
                Some(ty) => Ok(ty),
                _ => Err(invalid(ty).into()),
            }
        }
    }

    /// Evaluate a type annotation which is a constant of the frozen heap the compiler allocates
    /// on, into that heap. The error is not shown to the user, so it carries no hint.
    pub(crate) fn new_frozen(ty: Value<'v>, frozen_heap: FrozenHeap<'v>) -> anyhow::Result<Self> {
        let ty = Self::ty_from_value(ty, &|ty| TypingError::InvalidTypeAnnotation(ty.to_str()))?;
        // TODO(nga): trip to a heap is not free.
        Heap::temp(|heap| Ok(TypeCompiled::from_ty(&ty, heap).to_frozen(frozen_heap)))
    }
}

impl<'v> TypeCompiled<'v> {
    /// `typing.Any`.
    pub fn any() -> TypeCompiled<'v> {
        TypeCompiled(TYPE_COMPILED_ANY.at().to_value())
    }
}

fn invalid_type_annotation<'v>(ty: Value<'v>, heap: Heap<'v>) -> TypingError {
    if DictRef::from_value(ty).is_some() {
        TypingError::Dict
    } else if ListRef::from_value(ty).is_some() {
        TypingError::List
    } else if let Some(name) = ty
        .get_attr("type", heap)
        .ok()
        .flatten()
        .and_then(|v| v.unpack_str())
    {
        TypingError::PerhapsYouMeant(ty.to_str(), name.into())
    } else {
        TypingError::InvalidTypeAnnotation(ty.to_str())
    }
}
