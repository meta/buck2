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
/// [`Freeze`](crate::values::Freeze). The macro defines instances of
/// [`AllocValue`](crate::values::AllocValue),
/// [`AllocFrozenValue`](crate::values::AllocFrozenValue),
/// [`StarlarkTypeRepr`](crate::values::type_repr::StarlarkTypeRepr) and
/// [`UnpackValue`](crate::values::UnpackValue) for `&'v X<'v>`, plus a
/// `X::from_value` downcast that matches frozen and unfrozen instances alike.
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

            impl<'v> $crate::values::AllocValue<'v> for $x<'v> {
                #[inline]
                fn alloc_value(self, heap: $crate::values::Heap<'v>) -> $crate::values::Value<'v> {
                    heap.alloc_complex(self)
                }
            }

            impl<'fv> $crate::values::AllocFrozenValue<'fv> for $x<'fv> {
                #[inline]
                fn alloc_frozen_value(
                    self,
                    heap: $crate::values::FrozenHeap<'fv>,
                ) -> $crate::values::Value<'fv> {
                    heap.alloc_simple_typed(self).to_value()
                }
            }

            impl<'v> $x<'v> {
                /// Downcast the value.
                #[inline]
                pub fn from_value(x: $crate::values::Value<'v>) -> Option<&'v Self> {
                    x.downcast_ref::<$x<'v>>()
                }
            }

            impl<'v> $crate::values::type_repr::StarlarkTypeRepr for &'v $x<'v> {
                type Canonical = $x<'v>;

                #[inline]
                fn starlark_type_repr() -> $crate::typing::Ty {
                    <$x as $crate::values::StarlarkValue>::get_type_starlark_repr()
                }
            }

            impl<'v> $crate::values::UnpackValue<'v> for &'v $x<'v> {
                type Error = std::convert::Infallible;

                #[inline]
                fn unpack_value_impl(
                    x: $crate::values::Value<'v>,
                ) -> Result<Option<&'v $x<'v>>, Self::Error> {
                    Ok($x::from_value(x))
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
///         Ok(heap.alloc(MyObject(self.0.to_uppercase())))
///     }
/// }
/// ```
///
/// The `starlark_simple_value!` macro defines instances of
/// [`ProvidesStaticType`](crate::values::ProvidesStaticType),
/// [`AllocValue`](crate::values::AllocValue),
/// [`AllocFrozenValue`](crate::values::AllocFrozenValue) and
/// [`UnpackValue`](crate::values::UnpackValue). It also defines a method:
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
            impl<'v> $crate::values::AllocValue<'v> for $x {
                #[inline]
                fn alloc_value(self, heap: $crate::values::Heap<'v>) -> $crate::values::Value<'v> {
                    heap.alloc_simple(self)
                }
            }

            impl<'fv> $crate::values::AllocFrozenValue<'fv> for $x {
                #[inline]
                fn alloc_frozen_value(
                    self,
                    heap: $crate::values::FrozenHeap<'fv>,
                ) -> $crate::values::Value<'fv> {
                    heap.alloc_simple(self)
                }
            }

            impl $x {
                /// Downcast a value to self type.
                #[inline]
                pub fn from_value<'v>(x: $crate::values::Value<'v>) -> Option<&'v Self> {
                    x.downcast_ref::<$x>()
                }
            }

            impl<'v> $crate::values::type_repr::StarlarkTypeRepr for &'v $x {
                type Canonical = $x;

                fn starlark_type_repr() -> $crate::typing::Ty {
                    <$x as $crate::values::StarlarkValue>::get_type_starlark_repr()
                }
            }

            impl<'v> $crate::values::UnpackValue<'v> for &'v $x {
                type Error = std::convert::Infallible;

                #[inline]
                fn unpack_value_impl(
                    x: $crate::values::Value<'v>,
                ) -> std::result::Result<Option<&'v $x>, Self::Error> {
                    std::result::Result::Ok($x::from_value(x))
                }
            }
        };
    };
}
