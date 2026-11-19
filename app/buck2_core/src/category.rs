/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Categories and identifiers for actions run by buck2.
//!
//! A category is a snake case identifier that identifies a family of actions that are related in some way but differ
//! in their inputs. The canonical example of this is the category `cxx_compile`; conceptually, this represents the
//! category of all actions that invoke a C++ compiler, of which there are potentially many in a single C++ rule
//! implementation.

use std::sync::LazyLock;

use allocative::Allocative;
use buck2_hash::BuckHasher;
use dupe::Dupe;
use pagable::Pagable;
use pagable::PagableDeserialize;
use pagable::PagableDeserializer;
use pagable::PagableSerialize;
use pagable::PagableSerializer;
use regex::Regex;
use static_interner::Intern;
use static_interner::interner;
use strong_hash::StrongHash;

/// Interned category data, representing a family of actions.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Allocative,
    StrongHash,
    derive_more::Display,
    Pagable
)]
struct CategoryData(String);

interner!(CATEGORY_INTERNER, BuckHasher, CategoryData, String);

/// A category, representing a family of actions.
#[derive(
    Clone,
    Copy,
    Dupe,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Allocative,
    StrongHash,
    derive_more::Display
)]
pub struct Category(Intern<CategoryData>);

// TODO(dcssiva): remove CategoryRef because Category has a cheap Copy
#[derive(Debug, Clone, Copy, Dupe, PartialEq, Eq, Hash, derive_more::Display)]
pub struct CategoryRef<'a>(&'a str);

impl Category {
    pub fn new(s: String) -> buck2_error::Result<Self> {
        CategoryRef::new(&s)?;
        Ok(Category(CATEGORY_INTERNER.intern(s)))
    }

    /// Returns a string representation of this category.
    pub fn as_str(&self) -> &str {
        self.0.0.as_str()
    }

    pub fn as_ref(&self) -> CategoryRef<'_> {
        CategoryRef(self.0.0.as_str())
    }
}

impl PagableSerialize for Category {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> pagable::Result<()> {
        self.as_str().pagable_serialize(serializer)
    }
}

impl<'de> PagableDeserialize<'de> for Category {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        let s = String::pagable_deserialize(deserializer)?;
        Ok(Category(CATEGORY_INTERNER.intern(s)))
    }
}

impl<'a> CategoryRef<'a> {
    pub fn unchecked_new(s: &'a str) -> Self {
        CategoryRef(s)
    }

    pub fn as_str(self) -> &'a str {
        self.0
    }

    pub fn new(s: &'a str) -> buck2_error::Result<Self> {
        static CATEGORY_REGEX: LazyLock<Regex> =
            LazyLock::new(|| Regex::new("^[a-z][a-z0-9]*(_[a-z][a-z0-9]*)*$").unwrap());

        if !CATEGORY_REGEX.is_match(s) {
            Err(CategoryParseError::NotSnakeCase(s.to_owned()).into())
        } else {
            Ok(CategoryRef(s))
        }
    }

    pub fn to_owned(self) -> Category {
        Category(CATEGORY_INTERNER.intern(self.0.to_owned()))
    }
}

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum CategoryParseError {
    #[error(
        "Invalid category `{0}`. Must be a snake_cased identifier consisting of lowercase alphanumeric characters, e.g. `cxx_compile`. Each section of the snake_cased identifier must begin with a lowercase letter (not a number)."
    )]
    NotSnakeCase(String),
}

#[cfg(test)]
mod tests {
    use super::CategoryRef;

    #[test]
    fn valid_categories() {
        CategoryRef::new("valid_category").unwrap();
        CategoryRef::new("valid_category_with_numbers10").unwrap();
        CategoryRef::new("singleword").unwrap();
    }

    #[test]
    fn invalid_categories() {
        CategoryRef::new("_leading_underscore").unwrap_err();
        CategoryRef::new("NotSnakeCase").unwrap_err();
        CategoryRef::new("Not_Snake_Case").unwrap_err();
        CategoryRef::new("contains_4_number").unwrap_err();
        CategoryRef::new("trailing_underscore_").unwrap_err();
    }
}
