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

//! Convert a value implementing [`StarlarkValue`] into a type usable in type expression.

use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Formatter;
use std::marker::PhantomData;

use allocative::Allocative;
use starlark_derive::NoSerialize;
use starlark_derive::StarlarkPagablePanic;
use starlark_derive::starlark_value;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::docs::DocItem;
use crate::docs::DocMember;
use crate::docs::DocProperty;
use crate::docs::DocType;
use crate::typing::Ty;
use crate::values::AllocFrozenValue;
use crate::values::AllocStaticSimple;
use crate::values::AllocValue;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::StarlarkType;
use crate::values::StarlarkValue;
use crate::values::StaticValueRegistered;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::type_repr::StarlarkTypeRepr;
use crate::values::typing::TypeType;
use crate::values::typing::ty::AbstractType;

/// Marker trait indicating that `StarlarkValueAsType<T>` has been registered
/// as a static value for pagable.
///
/// # Safety
///
/// This trait should only be implemented by the `#[starlark_module]` proc macro
/// when it generates the static registration for a `StarlarkValueAsType` constant.
/// Manual implementations may lead to missing registrations or duplicate impl trait.
pub unsafe trait AsTypeStaticRegistered {}

/// When pagable is disabled, this blanket impl ensures that
/// `StarlarkValueAsType::new()` and `new_no_docs()` work without requiring
/// explicit `AsTypeStaticRegistered` impls from downstream consumers.
#[cfg(not(feature = "pagable"))]
unsafe impl<T> AsTypeStaticRegistered for T {}

#[derive(
    Debug,
    NoSerialize,
    Allocative,
    ProvidesStaticType,
// Instances are allocated via `AllocStaticSimple` and pagable round-trip is
// handled by the static-value registry, so a real `StarlarkPagable` impl is
// not needed.
    StarlarkPagablePanic
)]
struct StarlarkValueAsTypeStarlarkValue(fn() -> Ty, fn() -> DocItem);

// SAFETY: This type is only used in static contexts via StarlarkValueAsType::new()
// which creates a static value that is properly registered for pagable serialization.
unsafe impl StaticValueRegistered for StarlarkValueAsTypeStarlarkValue {}

#[starlark_value(type = "type")]
impl<'v> StarlarkValue<'v> for StarlarkValueAsTypeStarlarkValue {
    fn get_type_starlark_repr() -> Ty {
        <AbstractType as StarlarkType>::get_type_starlark_repr()
    }

    fn eval_type(&self) -> Option<Ty> {
        Some((self.0)())
    }

    fn at(&self, _index: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        let base_ty = (self.0)();
        Err(crate::Error::new_other(anyhow::anyhow!(
            "Type `{}` does not support type parameters",
            base_ty,
        )))
    }

    fn documentation(&self) -> DocItem {
        (self.1)()
    }
}

impl Display for StarlarkValueAsTypeStarlarkValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&(self.0)(), f)
    }
}

/// Utility to declare a value usable in type expression.
///
/// # Example
///
/// ```
/// use allocative::Allocative;
/// use starlark::any::ProvidesStaticType;
/// use starlark::environment::GlobalsBuilder;
/// use starlark::values::NoSerialize;
/// use starlark::values::StarlarkPagable;
/// use starlark::values::StarlarkValue;
/// use starlark::values::starlark_value;
/// use starlark::values::starlark_value_as_type::StarlarkValueAsType;
/// use starlark_derive::starlark_module;
///
/// #[derive(
///     Debug,
///     derive_more::Display,
///     Allocative,
///     ProvidesStaticType,
///     NoSerialize,
///     StarlarkPagable
/// )]
/// struct StarlarkTemperature;
///
/// #[starlark_value(type = "temperature")]
/// impl<'v> StarlarkValue<'v> for StarlarkTemperature {}
///
/// // Use with #[starlark_module]:
/// // #[starlark_module]
/// // #[starlark_types(StarlarkTemperature as Temperature)]
/// // fn my_type_globals(globals: &mut GlobalsBuilder) {}
///
/// // Or standalone:
/// // starlark::declare_starlark_value_as_type!(TEMPERATURE, StarlarkTemperature);
/// ```
pub struct StarlarkValueAsType<T: StarlarkTypeRepr>(
    &'static AllocStaticSimple<StarlarkValueAsTypeStarlarkValue>,
    PhantomData<fn(&T)>,
);

// Manual Clone/Copy implementations
impl<T: StarlarkTypeRepr> Clone for StarlarkValueAsType<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: StarlarkTypeRepr> Copy for StarlarkValueAsType<T> {}

impl<T: StarlarkTypeRepr> Debug for StarlarkValueAsType<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("StarlarkValueAsType")
            .field(&T::starlark_type_repr())
            .finish()
    }
}

impl<T: StarlarkTypeRepr> Display for StarlarkValueAsType<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&T::starlark_type_repr(), f)
    }
}

impl<T: StarlarkTypeRepr> StarlarkValueAsType<T> {
    /// Constructor.
    ///
    /// Use [`new_no_docs`](Self::new_no_docs) if `T` is not a `StarlarkValue`.
    pub const fn new() -> Self
    where
        T: StarlarkValue<'static> + AsTypeStaticRegistered,
    {
        StarlarkValueAsType(
            &const {
                AllocStaticSimple::alloc(StarlarkValueAsTypeStarlarkValue(
                    T::starlark_type_repr,
                    || DocItem::Type(DocType::from_starlark_value::<T>()),
                ))
            },
            PhantomData,
        )
    }

    /// Constructor.
    pub const fn new_no_docs() -> Self
    where
        T: AsTypeStaticRegistered,
    {
        StarlarkValueAsType(
            &const {
                AllocStaticSimple::alloc(StarlarkValueAsTypeStarlarkValue(
                    T::starlark_type_repr,
                    || {
                        DocItem::Member(DocMember::Property(DocProperty {
                            docs: None,
                            typ: AbstractType::starlark_type_repr(),
                        }))
                    },
                ))
            },
            PhantomData,
        )
    }

    /// The value for this type.
    ///
    /// A static is immortal, which is what the `'static` brand means.
    pub fn to_value(&self) -> Value<'static> {
        self.0.unpack().to_value()
    }
}

impl<T: StarlarkTypeRepr> StarlarkTypeRepr for StarlarkValueAsType<T> {
    type Canonical = <TypeType as StarlarkTypeRepr>::Canonical;

    fn starlark_type_repr() -> Ty {
        <Self::Canonical as StarlarkTypeRepr>::starlark_type_repr()
    }
}

impl<'v, T: StarlarkTypeRepr> AllocValue<'v> for StarlarkValueAsType<T> {
    type Type = TypeType;

    fn alloc_value(self, _heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        // A `StarlarkValueAsType` is a type object.
        ValueOf::new_unchecked(self.0.at().to_value())
    }
}

impl<'fv, T: StarlarkTypeRepr> AllocFrozenValue<'fv> for StarlarkValueAsType<T> {
    type Type = TypeType;

    fn alloc_frozen_value(self, _heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        // A `StarlarkValueAsType` is a type object.
        ValueOf::new_unchecked(self.0.at().to_value())
    }
}

/// Declare a static `StarlarkValueAsType<T>` with automatic pagable serialization registration.
///
/// This macro creates a module-level static and registers it via `inventory::submit!`
/// using `file!()` and `line!()` for deterministic ID generation.
///
/// # Usage
///
/// Standalone (outside `#[starlark_module]`):
/// ```ignore
/// declare_starlark_value_as_type!(pub MY_INT, StarlarkInt);
/// declare_starlark_value_as_type!(pub MY_PROVIDER, AbstractProvider, no_docs);
/// ```
///
/// The `#[starlark_module]` proc macro can also generate calls to this macro
/// via the `#[starlark_types]` attribute.
#[macro_export]
macro_rules! declare_starlark_value_as_type {
    ($vis:vis $name:ident, $T:ty) => {
        $crate::declare_starlark_value_as_type!($vis $name, $T, new);
    };
    ($vis:vis $name:ident, $T:ty, no_docs) => {
        $crate::declare_starlark_value_as_type!($vis $name, $T, new_no_docs);
    };
    ($vis:vis $name:ident, $T:ty, skip_type_registration) => {
        $crate::declare_starlark_value_as_type!(@skip_type_registration $vis $name, $T, new);
    };
    ($vis:vis $name:ident, $T:ty, skip_type_registration_no_docs) => {
        $crate::declare_starlark_value_as_type!(@skip_type_registration $vis $name, $T, new_no_docs);
    };
    ($vis:vis $name:ident, $T:ty, $constructor:ident) => {
        // Register the AsTypeStaticRegistered marker trait for T
        // (cfg-gated via __register_starlark_value_as_type, no-op when pagable is disabled).
        $crate::__register_starlark_value_as_type!(impl_trait = $T);

        $crate::declare_starlark_value_as_type!(@skip_type_registration $vis $name, $T, $constructor);
    };
    (@skip_type_registration $vis:vis $name:ident, $T:ty, $constructor:ident) => {
        $vis static $name: $crate::__derive_refs::StarlarkValueAsType<$T> =
            $crate::__derive_refs::StarlarkValueAsType::$constructor();

        $crate::__derive_refs::inventory::submit! {
            $crate::__derive_refs::StaticValueEntry::new(
                file!(),
                line!(),
                || $name.to_value()
            )
        }
    };
}

#[cfg(test)]
mod tests {
    use allocative::Allocative;
    use starlark_derive::NoSerialize;
    use starlark_derive::ProvidesStaticType;
    use starlark_derive::StarlarkPagable;
    use starlark_derive::starlark_module;
    use starlark_derive::starlark_value;

    use crate as starlark;
    use crate::assert::Assert;
    use crate::environment::GlobalsBuilder;
    use crate::values::AllocValue;
    use crate::values::Heap;
    use crate::values::StarlarkValue;
    use crate::values::ValueOf;
    use crate::values::types::starlark_value_as_type::tests;

    #[derive(
        derive_more::Display,
        Debug,
        NoSerialize,
        Allocative,
        ProvidesStaticType,
        StarlarkPagable
    )]
    struct StarlarkCompilerArgs(#[starlark_pagable(pagable)] String);

    #[starlark_value(type = "compiler_args")]
    impl<'v> StarlarkValue<'v> for StarlarkCompilerArgs {}

    impl<'v> AllocValue<'v> for StarlarkCompilerArgs {
        type Type = Self;

        fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
            ValueOf::alloc(heap, self)
        }
    }

    #[starlark_module]
    #[starlark_types(StarlarkCompilerArgs as CompilerArgs)]
    fn compiler_args_globals(globals: &mut GlobalsBuilder) {
        fn compiler_args(x: String) -> anyhow::Result<StarlarkCompilerArgs> {
            Ok(tests::StarlarkCompilerArgs(x))
        }
    }

    #[test]
    fn test_pass() {
        let mut a = Assert::new();
        a.globals_add(compiler_args_globals);
        a.pass(
            r#"
def f(x: CompilerArgs): pass

f(compiler_args("hello"))
        "#,
        );
    }

    #[test]
    fn test_fail_compile_time() {
        let mut a = Assert::new();
        a.globals_add(compiler_args_globals);
        a.fail(
            r#"
def g(x: CompilerArgs): pass

def h():
    g([])
"#,
            r#"Expected type `compiler_args` but got"#,
        );
    }

    #[test]
    fn test_fail_runtime() {
        let mut a = Assert::new();
        a.globals_add(compiler_args_globals);
        a.fail(
            r#"
def h(x: CompilerArgs): pass

noop(h)(1)
            "#,
            r#"Value `1` of type `int` does not match the type annotation"#,
        );
    }

    #[test]
    fn test_unsupported_param_on_custom_type() {
        let mut a = Assert::new();
        a.globals_add(compiler_args_globals);
        a.fail(
            r#"
def f(x: CompilerArgs[int]): pass
"#,
            "does not support type parameters",
        );
    }

    #[test]
    fn test_supported_param_list() {
        let a = Assert::new();
        a.pass(
            r#"
def f(x: list[str]): pass
"#,
        );
    }

    #[test]
    fn test_supported_param_dict() {
        let a = Assert::new();
        a.pass(
            r#"
def f(x: dict[str, int]): pass
"#,
        );
    }
}
