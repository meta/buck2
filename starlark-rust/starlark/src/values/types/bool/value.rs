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
    /// [bool](
    /// https://github.com/bazelbuild/starlark/blob/master/spec.md#bool
    /// ): returns the truth value of any starlark value.
    ///
    /// ```
    /// # starlark::assert::all_true(r#"
    /// bool() == False
    /// bool([]) == False
    /// bool([1]) == True
    /// bool(True) == True
    /// bool(False) == False
    /// bool(None) == False
    /// bool(bool) == True
    /// bool(1) == True
    /// bool(0) == False
    /// bool({}) == False
    /// bool({1:2}) == True
    /// bool(()) == False
    /// bool((1,)) == True
    /// bool("") == False
    /// bool("1") == True
    /// # "#);
    /// ```
    #[starlark(constructor, speculative_exec_safe)]
    fn bool(#[starlark(require = pos, default = false)] x: Value) -> anyhow::Result<bool> {
        Ok(x.to_bool())
    }

    fn collect_repr(this: Self, s: &mut String) {
        // The default writes the receiver's `Display`, which for a Rust `bool` is `true`.
        // repr() for bool is also quite hot, so optimise it
        if this {
            s.push_str("True")
        } else {
            s.push_str("False")
        }
    }

    fn to_bool(this: Self) -> bool {
        this
    }

    fn write_hash(this: Self, hasher: &mut StarlarkHasher) {
        hasher.write_u8(if this { 1 } else { 0 });
    }

    fn get_hash(this: Self, _private: Private) -> crate::Result<StarlarkHashValue> {
        // These constants are just two random numbers.
        Ok(StarlarkHashValue::new_unchecked(if this {
            0xa4acba08
        } else {
            0x71e8ba71
        }))
    }

    fn compare(this: Self, other: bool) -> Ordering {
        this.cmp(&other)
    }
}

impl<'v> From<bool> for Value<'v> {
    #[inline]
    fn from(x: bool) -> Value<'v> {
        Value::new_bool(x)
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
    fn get_typed(this: ValueTyped<'v, Self>) -> bool {
        this.as_ref().0
    }

    fn is_special(_: Private) -> bool
    where
        Self: Sized,
    {
        true
    }
}
