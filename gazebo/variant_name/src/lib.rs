/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The name of an enum's current variant, for debug output and metrics.
//!
//! ```
//! use variant_name::VariantName;
//!
//! #[derive(VariantName)]
//! enum Foo {
//!     Bar,
//!     Baz(usize),
//!     Qux { i: usize },
//! }
//!
//! assert_eq!(Foo::Bar.variant_name(), "Bar");
//! assert_eq!(Foo::Baz(1).variant_name(), "Baz");
//! assert_eq!(Foo::Qux { i: 1 }.variant_name(), "Qux");
//! ```

// The derive refers to this crate by name, which is also what makes it work in this crate's own
// tests.
#[cfg(test)]
extern crate self as variant_name;

pub use variant_name_derive::VariantName;

pub trait VariantName {
    /// The variant's name as written in the enum declaration.
    fn variant_name(&self) -> &'static str;

    /// The variant's name in snake case: `FooBar` becomes `foo_bar`.
    fn variant_name_lowercase(&self) -> &'static str;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_variant_names() {
        #[allow(unused)] // The fields aren't used, only the variant names
        #[derive(VariantName)]
        enum MyEnum {
            Foo,
            Bar(usize),
            FooBaz { field: usize },
        }

        let x = MyEnum::Foo;
        assert_eq!(x.variant_name(), "Foo");
        assert_eq!(x.variant_name_lowercase(), "foo");

        let x = MyEnum::Bar(1);
        assert_eq!(x.variant_name(), "Bar");
        assert_eq!(x.variant_name_lowercase(), "bar");

        let x = MyEnum::FooBaz { field: 1 };
        assert_eq!(x.variant_name(), "FooBaz");
        assert_eq!(x.variant_name_lowercase(), "foo_baz");
    }

    #[test]
    fn derive_on_empty_enum() {
        #[derive(VariantName)]
        enum Never {}

        fn name(x: Option<&Never>) -> Option<&'static str> {
            x.map(VariantName::variant_name)
        }
        assert_eq!(name(None), None);
    }
}
