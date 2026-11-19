/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use proc_macro::TokenStream;

mod derive_pagable;
mod derive_pagable_panic;
mod derive_pagable_tagged;
mod derive_pagable_unsupported;
mod typetag;

#[proc_macro_derive(Pagable, attributes(pagable))]
pub fn derive_pagable(input: TokenStream) -> TokenStream {
    derive_pagable::derive_pagable(input, true, true)
}

/// Derives `PagableSerialize` and `PagableDeserialize` implementations that panic
/// when called. Useful for types that need to satisfy trait bounds but
/// should never actually be serialized/deserialized.
#[proc_macro_derive(PagablePanic, attributes(pagable))]
pub fn derive_pagable_panic(input: TokenStream) -> TokenStream {
    derive_pagable_panic::derive_pagable_panic(input)
}

/// Derives `PagableSerialize` and `PagableDeserialize` implementations that
/// fail with an error naming the type. For types that must satisfy the bounds
/// but cannot be paged: unlike `PagablePanic`, reaching one during page-out
/// fails that page-out and leaves the value resident, rather than aborting the
/// process.
#[proc_macro_derive(PagableUnsupported, attributes(pagable))]
pub fn derive_pagable_unsupported(input: TokenStream) -> TokenStream {
    derive_pagable_unsupported::derive_pagable_unsupported(input)
}

#[proc_macro_derive(PagableSerialize, attributes(pagable))]
pub fn derive_pagable_serialize(input: TokenStream) -> TokenStream {
    derive_pagable::derive_pagable(input, true, false)
}

#[proc_macro_derive(PagableDeserialize, attributes(pagable))]
pub fn derive_pagable_deserialize(input: TokenStream) -> TokenStream {
    derive_pagable::derive_pagable(input, false, true)
}

/// Attribute macro for pagable trait object serialization.
///
/// Similar to the `typetag` crate, but for pagable serialization.
///
/// # Usage on traits
///
/// ```ignore
/// #[pagable_typetag]
/// trait MyTrait: pagable::typetag::PagableTagged + Send + Sync {}
/// ```
///
/// This generates:
/// - A static registry for the trait
/// - `PagableSerialize` impl for `dyn MyTrait`
/// - `PagableBoxDeserialize` impl for `dyn MyTrait`
///
/// # Usage on impl blocks
///
/// ```ignore
/// #[derive(Pagable, PagableTagged)]
/// struct MyImpl { value: i32 }
///
/// #[pagable_typetag]
/// impl MyTrait for MyImpl {}
/// ```
///
/// This generates automatic registration via `inventory`.
///
/// **Note:** The type must also implement `PagableTagged` (via `#[pagable_tagged]`
/// or `#[pagable_typetag(Trait)]`).
///
/// Generate a `PagableTagged` impl using `type_name`, with
/// `PagableRegisteredFor<dyn Trait, Self>` bounds on generic params to enforce
/// that concrete instantiations are registered via `register_typetag!`.
///
/// ```ignore
/// #[pagable_tagged(MyTrait)]
/// struct Wrapper<T: MyInnerTrait>(pub T);
/// ```
#[proc_macro_attribute]
pub fn pagable_tagged(attr: TokenStream, item: TokenStream) -> TokenStream {
    derive_pagable_tagged::pagable_tagged_impl(attr, item)
}

#[proc_macro_attribute]
pub fn pagable_typetag(attr: TokenStream, item: TokenStream) -> TokenStream {
    typetag::pagable_typetag_impl(attr, item)
}
