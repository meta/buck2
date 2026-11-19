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

use std::any::TypeId;

use crate::any::ProvidesStaticType;

/// Like [`Any`](std::any::Any), but while [`Any`](std::any::Any) requires `'static`, this version
/// allows a lifetime parameter.
///
/// Code using this trait is _unsafe_ if your implementation of the inner methods do not meet the
/// invariants listed. Therefore, it is recommended you use one of the helper macros.
///
/// You cannot implement this trait directly. You should instead implement `ProvidesStaticType`,
/// usually via the derive macro:
///
/// ```
/// use starlark::any::ProvidesStaticType;
/// #[derive(ProvidesStaticType)]
/// struct Foo1();
/// #[derive(ProvidesStaticType)]
/// struct Foo2<'a>(&'a ());
/// ```
///
/// If your data type is not of the form `Foo` or `Foo<'v>` you may need to implement
/// `ProvidesStaticType` directly.
///
/// ```
/// use starlark::any::ProvidesStaticType;
/// # fn main() {
/// # use std::fmt::Display;
/// struct Baz<T: Display>(T);
/// # // TODO: `#[derive(ProvidesStaticType)]` should learn to handle this case too.
/// unsafe impl<'a, T> ProvidesStaticType<'a> for Baz<T>
/// where
///     T: ProvidesStaticType<'a> + Display,
///     T::StaticType: Display + Sized,
/// {
///     type StaticType = Baz<T::StaticType>;
/// }
/// # }
/// ```
pub trait AnyLifetime<'a>: seal::ProvidesStaticTypeSealed<'a> + 'a {
    /// Must return the `TypeId` of `Self` but where the lifetimes are changed
    /// to `'static`. Must be consistent with `static_type_of`.
    fn static_type_id() -> TypeId
    where
        Self: Sized;

    /// Must return the `TypeId` of `Self` but where the lifetimes are changed
    /// to `'static`. Must be consistent with `static_type_id`. Must not
    /// consult the `self` parameter in any way.
    fn static_type_of(&self) -> TypeId;
    // Required so we can have a `dyn AnyLifetime`.
}

mod seal {
    /// A bound required by `AnyLifetime<'a>` for sealing it
    pub trait ProvidesStaticTypeSealed<'a> {}
    impl<'a, T: crate::any::ProvidesStaticType<'a> + ?Sized> ProvidesStaticTypeSealed<'a> for T {}
}

/// Any `ProvidesStaticType` can implement `AnyLifetime`.
///
/// Note `ProvidesStaticType` and `AnyLifetime` cannot be the same type,
/// because `AnyLifetime` need to be object safe,
/// and `ProvidesStaticType` has type member.
impl<'a, T: ProvidesStaticType<'a> + 'a + ?Sized> AnyLifetime<'a> for T {
    fn static_type_id() -> TypeId
    where
        Self: Sized,
    {
        TypeId::of::<T::StaticType>()
    }

    fn static_type_of(&self) -> TypeId {
        TypeId::of::<T::StaticType>()
    }
}

impl<'a> dyn AnyLifetime<'a> {
    /// Is the value of type `T`.
    pub fn is<T: AnyLifetime<'a>>(&self) -> bool {
        self.static_type_of() == T::static_type_id()
    }

    /// Downcast a reference to type `T`, or return [`None`] if it is not the
    /// right type.
    pub fn downcast_ref<T: AnyLifetime<'a>>(&self) -> Option<&T> {
        if self.is::<T>() {
            // SAFETY: just checked whether we are pointing to the correct type.
            unsafe { Some(&*(self as *const Self as *const T)) }
        } else {
            None
        }
    }

    /// Downcast a mutable reference to type `T`, or return [`None`] if it is not
    /// the right type.
    pub fn downcast_mut<T: AnyLifetime<'a>>(&mut self) -> Option<&mut T> {
        if self.is::<T>() {
            // SAFETY: just checked whether we are pointing to the correct type.
            unsafe { Some(&mut *(self as *mut Self as *mut T)) }
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate as starlark;

    #[test]
    fn test_can_convert() {
        #[derive(Debug, PartialEq, ProvidesStaticType)]
        struct Value<'a>(&'a str);

        #[derive(ProvidesStaticType)]
        #[allow(dead_code)] // field `0` is never read
        struct Value2<'a>(&'a str);

        // Changing the return type too `Value<'static>` causes a compile error.
        fn convert_value<'a>(x: &'a Value<'a>) -> Option<&'a Value<'a>> {
            <dyn AnyLifetime>::downcast_ref(x)
        }

        fn convert_any<'p, 'a>(x: &'p dyn AnyLifetime<'a>) -> Option<&'p Value<'a>> {
            x.downcast_ref()
        }

        let v = Value("test");
        let v2 = Value2("test");
        assert_eq!(convert_value(&v), Some(&v));
        assert_eq!(convert_any(&v), Some(&v));
        assert_eq!(convert_any(&v2), None);
    }

    #[test]
    fn test_any_lifetime() {
        fn test<'a, A: AnyLifetime<'a>>(expected: TypeId) {
            assert_eq!(expected, A::static_type_id());
        }

        test::<&str>(TypeId::of::<&str>());
        test::<&String>(TypeId::of::<&String>());
        test::<Box<str>>(TypeId::of::<Box<str>>());
    }
}
