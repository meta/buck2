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

/// Reduce boilerplate when defining a Starlark value that contains other values.
///
/// The type is written once, as `X<'v>`, and freezes to `X<'fv>` — see
/// [`Freeze`](crate::values::Freeze). The macro registers `X<'fv>` as the frozen form and
/// defines an instance of [`AllocFrozenRepresentation`](crate::values::AllocFrozenRepresentation)
/// for it, plus a `X::from_value` downcast that matches frozen and unfrozen instances alike. A
/// value is
/// allocated with [`Heap::alloc_representation`](crate::values::Heap::alloc_representation).
///
/// Hand-expand it when a piece needs customizing.
#[macro_export]
macro_rules! starlark_complex_value {
    ($v:vis $x:ident) => {
        const _: () = {
            $crate::register_simple_vtable_entry!($x<'static>);

            // SAFETY: The vtable entry is registered above. The deser type id
            // is lifetime-erased, so registering the `'static` instantiation
            // covers the frozen form at any heap lifetime.
            unsafe impl<'v> $crate::__derive_refs::VtableRegistered for $x<'v> {}

            impl<'fv> $crate::values::AllocFrozenRepresentation<'fv> for $x<'fv> {
                type Repr = Self;

                #[inline]
                fn alloc_frozen(
                    self,
                    allocator: $crate::values::FrozenAllocator<'fv>,
                ) -> $crate::values::ValueTyped<'fv, Self> {
                    allocator.alloc_simple(self)
                }
            }

            impl<'v> $x<'v> {
                /// Downcast the value.
                #[inline]
                #[allow(dead_code)] // A type private to a module may not use it.
                pub fn from_value(x: $crate::values::Value<'v>) -> Option<&'v Self> {
                    x.downcast_ref::<$x<'v>>()
                }
            }
        };
    };
}

/// A macro reducing boilerplace defining Starlark values which are simple - they
/// aren't mutable and can't contain references to other Starlark values.
///
/// Let's define a simple object, where `+x` makes the string uppercase:
///
/// ```
/// use allocative::Allocative;
/// use derive_more::Display;
/// use starlark::starlark_simple_value;
/// use starlark::values::Heap;
/// use starlark::values::NoSerialize;
/// use starlark::values::ProvidesStaticType;
/// use starlark::values::StarlarkPagable;
/// use starlark::values::StarlarkValue;
/// use starlark::values::Value;
/// use starlark_derive::starlark_value;
///
/// #[derive(
///     Debug,
///     Display,
///     ProvidesStaticType,
///     NoSerialize,
///     StarlarkPagable,
///     Allocative
/// )]
/// struct MyObject(String);
/// starlark_simple_value!(MyObject);
///
/// #[starlark_value(type = "my_object")]
/// impl<'v> StarlarkValue<'v> for MyObject {
///     // We can choose to implement whichever methods we want.
///     // All other operations will result in runtime errors.
///     fn plus(&self, heap: Heap<'v>) -> starlark::Result<Value<'v>> {
///         Ok(heap
///             .alloc_representation(MyObject(self.0.to_uppercase()))
///             .to_value())
///     }
/// }
/// ```
///
/// The `starlark_simple_value!` macro defines an instance of
/// [`ProvidesStaticType`](crate::values::ProvidesStaticType); the allocation impls come from
/// `#[starlark_value]`. It also defines a method:
///
/// ```
/// # use crate::starlark::values::*;
/// # struct MyObject;
/// impl MyObject {
///     pub fn from_value<'v>(x: Value<'v>) -> Option<&'v MyObject> {
/// # unimplemented!(
/// # r#"
///         ...
/// # "#);
///     }
/// }
/// ```
#[macro_export]
macro_rules! starlark_simple_value {
    ($x:ident) => {
        const _: () = {
            impl $x {
                /// Downcast a value to self type.
                #[inline]
                #[allow(dead_code)] // A type private to a module may not use it.
                pub fn from_value<'v>(x: $crate::values::Value<'v>) -> Option<&'v Self> {
                    x.downcast_ref::<$x>()
                }
            }
        };
    };
}
