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

use std::cmp::Ordering;
use std::fmt;
use std::fmt::Display;
use std::hash::Hasher;

use allocative::Allocative;
use starlark_derive::ProvidesStaticType;
use starlark_derive::StarlarkPagable;
use starlark_derive::starlark_value;
use starlark_map::StarlarkHashValue;
use starlark_map::StarlarkHasher;

use crate as starlark;
use crate::__derive_refs::ViewOfRepresentations;
use crate::__derive_refs::serde::Serialize;
use crate::private::Private;
use crate::static_starlark_value;
use crate::values::AllocStaticSimple;
use crate::values::StarlarkType;
use crate::values::StarlarkValue;
use crate::values::Value;
use crate::values::ValueError;
use crate::values::ValueTyped;

/// The result of calling `type()` on booleans.
pub const BOOL_TYPE: &str = "bool";

/// The representation of `bool`. The type is the Rust `bool`, which is what a
/// [`ValueOf`](crate::values::ValueOf) names and is read as. The only two values of this are
/// the statics below.
#[derive(ProvidesStaticType, Debug, Serialize, Allocative, StarlarkPagable)]
#[serde(transparent)]
pub struct StarlarkBool(pub(crate) bool);

impl Display for StarlarkBool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 {
            write!(f, "True")
        } else {
            write!(f, "False")
        }
    }
}

// Define individual boolean values with registration (multiple values of same type)
static_starlark_value!(StarlarkBool {
    pub(crate) VALUE_FALSE = StarlarkBool(false);
    pub(crate) VALUE_TRUE = StarlarkBool(true);
});

// Array of references for efficient boolean lookup by index
pub(crate) static VALUE_FALSE_TRUE: [&AllocStaticSimple<StarlarkBool>; 2] =
    [&VALUE_FALSE, &VALUE_TRUE];

#[starlark_value(type = BOOL_TYPE)]
impl<'v> StarlarkType<'v> for bool {
    fn collect_repr(&self, s: &mut String) {
        // The default writes the receiver's `Display`, which for a Rust `bool` is `true`.
        // repr() for bool is also quite hot, so optimise it
        if *self {
            s.push_str("True")
        } else {
            s.push_str("False")
        }
    }

    fn to_bool(&self) -> bool {
        *self
    }

    fn write_hash(&self, hasher: &mut StarlarkHasher) -> crate::Result<()> {
        hasher.write_u8(if *self { 1 } else { 0 });
        Ok(())
    }

    fn get_hash(&self, _private: Private) -> crate::Result<StarlarkHashValue> {
        // These constants are just two random numbers.
        Ok(StarlarkHashValue::new_unchecked(if *self {
            0xa4acba08
        } else {
            0x71e8ba71
        }))
    }

    fn compare(&self, other: Value) -> crate::Result<Ordering> {
        if let Some(other) = other.unpack_bool() {
            Ok(self.cmp(&other))
        } else {
            ValueError::unsupported_with(self, "<>", other)
        }
    }
}

impl<'v> ViewOfRepresentations<'v> for bool {
    type Canonical = StarlarkBool;

    #[inline]
    fn from_value(value: Value<'v>) -> Option<bool> {
        value.unpack_bool()
    }
}

#[starlark_value]
impl<'v> StarlarkValue<'v> for StarlarkBool {
    type Type = bool;

    #[inline]
    fn with_type<R>(this: ValueTyped<'v, Self>, f: impl FnOnce(&bool) -> R) -> R {
        f(&this.as_ref().0)
    }

    fn is_special(_: Private) -> bool
    where
        Self: Sized,
    {
        true
    }
}
