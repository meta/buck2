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

//! The brand lifetime the pagable derives implement `StarlarkDeserialize` at.

use syn::DeriveInput;
use syn::Generics;
use syn::Lifetime;
use syn::LifetimeParam;
use syn::parse_quote;

/// The brand a type is deserialized at, and the generics of the `StarlarkDeserialize` impl.
///
/// A type's one lifetime parameter is its brand: `impl<'v> StarlarkDeserialize<'v> for Foo<'v>`.
/// A type without lifetime parameters holds no branded value and is deserializable at every
/// brand: `impl<'fv> StarlarkDeserialize<'fv> for Bar`, with `'fv` added to the impl generics.
/// A type with several lifetime parameters is rejected: the derive cannot tell which one is the
/// heap's, so it implements the trait by hand.
pub(crate) fn deserialize_brand(input: &DeriveInput) -> syn::Result<(Lifetime, Generics)> {
    let lifetimes: Vec<&LifetimeParam> = input.generics.lifetimes().collect();
    match lifetimes.as_slice() {
        [] => {
            let lifetime: Lifetime = parse_quote!('fv);
            let mut generics = input.generics.clone();
            generics.params.insert(0, parse_quote!(#lifetime));
            Ok((lifetime, generics))
        }
        [param] => Ok((param.lifetime.clone(), input.generics.clone())),
        _ => Err(syn::Error::new_spanned(
            &input.generics,
            "the derive cannot tell which lifetime parameter is the heap's brand; implement \
             `StarlarkDeserialize` by hand",
        )),
    }
}
