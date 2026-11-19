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

/// Provides access to the same type as `Self` but with all lifetimes dropped to `'static`
/// (including lifetimes of parameters).
///
/// This type is usually implemented with `#[derive(ProvidesStaticType)]`.
///
/// # Safety
///
/// `Self` must have no lifetimes other than `'a` and `'static`, and `StaticType` must be `Self`
/// with `'a` replaced by `'static`. It follows that two types with the same `StaticType` are one
/// type at two lifetimes, and so have one layout; every brand change in the crate rests on that
/// (`rebrand_unchecked` in `values/layout/heap/branding.rs`).
pub unsafe trait ProvidesStaticType<'a> {
    /// Same type as `Self` but with lifetimes dropped to `'static`.
    type StaticType: 'static + ?Sized;
}

/// A `'static` type that can be "reinfected" with a lifetime to produce a related type.
///
/// This trait is the inverse of [`ProvidesStaticType`]: `T::Reinfect<'a>` is `T` with `'a` in
/// place of `'static`, and `T` is its own static type. Both are checked by the bounds, so the
/// trait is safe to implement and adds nothing to what [`ProvidesStaticType`]'s contract
/// promises. (The converse, that every static type implements this trait, cannot be a bound on
/// [`ProvidesStaticType`]: the two would be mutually recursive.)
///
/// This type is usually implemented with `#[derive(ProvidesStaticType)]`.
pub trait IsStaticType: 'static + ProvidesStaticType<'static, StaticType = Self> {
    /// The type with lifetime `'lt` injected.
    type Reinfect<'lt>: ?Sized + ProvidesStaticType<'lt, StaticType = Self>;
}

/// `T`, which is at the `'static` brand, with `'lt` injected in its place.
pub type ReinfectStatic<'lt, T> =
    <<T as ProvidesStaticType<'static>>::StaticType as IsStaticType>::Reinfect<'lt>;

#[cfg(test)]
mod tests {
    use crate as starlark;
    use crate::any::IsStaticType;
    use crate::any::ProvidesStaticType;

    #[test]
    fn test_provides_static_type_id() {
        // This test is just about checking that things compile. We wrap the test in this function
        // so that there's a non-'static lifetime in scope
        #[allow(dead_code)]
        // extra_unused_lifetime suppression doesn't work?
        fn some_lifetime<'a>(_r: &'a ()) {
            fn check_static_type_is<'a, T: ProvidesStaticType<'a, StaticType = U>, U>() {}
            fn check_reinfected_type_is<'a, T: IsStaticType<Reinfect<'a> = U>, U>() {}

            #[derive(ProvidesStaticType)]
            struct Aaa;
            check_static_type_is::<Aaa, Aaa>();
            check_reinfected_type_is::<Aaa, Aaa>();

            #[derive(ProvidesStaticType)]
            #[allow(dead_code)] // field `0` is never read
            struct Bbb<'a>(&'a str);
            check_static_type_is::<Bbb<'a>, Bbb<'static>>();
            check_reinfected_type_is::<Bbb<'static>, Bbb<'a>>();

            #[derive(ProvidesStaticType)]
            struct Ccc<X>(X);
            check_static_type_is::<Ccc<String>, Ccc<String>>();
            check_reinfected_type_is::<Ccc<String>, Ccc<String>>();

            #[derive(ProvidesStaticType)]
            struct LifetimeTypeConst<'a, T, const N: usize>([&'a T; N]);
            check_static_type_is::<LifetimeTypeConst<'a, i32, 3>, LifetimeTypeConst<'static, i32, 3>>(
            );
            check_reinfected_type_is::<
                LifetimeTypeConst<'static, i32, 3>,
                LifetimeTypeConst<'a, i32, 3>,
            >();

            #[derive(ProvidesStaticType)]
            struct TypeWithConstraint<T: std::fmt::Display>(T);
            check_static_type_is::<TypeWithConstraint<String>, TypeWithConstraint<String>>();
            check_reinfected_type_is::<TypeWithConstraint<String>, TypeWithConstraint<String>>();

            struct TypeWhichDoesNotImplementAnyLifetime;

            #[derive(ProvidesStaticType)]
            struct TypeWithStaticLifetime<T: 'static>(T);
            check_static_type_is::<
                TypeWithStaticLifetime<TypeWhichDoesNotImplementAnyLifetime>,
                TypeWithStaticLifetime<TypeWhichDoesNotImplementAnyLifetime>,
            >();
            check_reinfected_type_is::<
                TypeWithStaticLifetime<TypeWhichDoesNotImplementAnyLifetime>,
                TypeWithStaticLifetime<TypeWhichDoesNotImplementAnyLifetime>,
            >();
        }
    }

    #[test]
    fn test_provides_static_type_when_type_parameter_has_bound_with_lifetime() {
        trait My<'a> {}

        #[derive(ProvidesStaticType)]
        #[allow(dead_code)] // field `0` is never read
        struct FooBar<'x, P: My<'x>>(&'x P);
    }
}
