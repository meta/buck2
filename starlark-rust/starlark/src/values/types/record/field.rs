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

use std::fmt;
use std::fmt::Display;
use std::hash::Hash;

use allocative::Allocative;
use dupe::Dupe;
use starlark_derive::NoSerialize;
use starlark_derive::StarlarkPagable;
use starlark_derive::Trace;
use starlark_derive::starlark_value;
use starlark_map::StarlarkHasher;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::starlark_complex_value;
use crate::typing::Ty;
use crate::values::Freeze;
use crate::values::StarlarkValue;
use crate::values::Value;
use crate::values::typing::type_compiled::compiled::TypeCompiled;

/// The result of `field()`.
#[derive(
    Clone,
    Debug,
    Dupe,
    Trace,
    Freeze,
    NoSerialize,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
pub struct Field<'v> {
    /// The expected type of the field.
    pub typ: TypeCompiled<'v>,
    /// The default value (if provided).
    pub default: Option<Value<'v>>,
}

impl<'v> Display for Field<'v> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "field(")?;
        Display::fmt(&self.typ, f)?;
        if let Some(d) = &self.default {
            write!(f, ", ")?;
            Display::fmt(d, f)?;
        }
        write!(f, ")")
    }
}

starlark_complex_value!(pub Field);

impl<'v> Field<'v> {
    /// Creates a new `Field`.
    pub fn new(typ: TypeCompiled<'v>, default: Option<Value<'v>>) -> Self {
        Self { typ, default }
    }
}

impl<'v> Field<'v> {
    pub(super) fn ty(&self) -> Ty {
        self.typ.as_ty().clone()
    }
}

#[starlark_value(type = "field")]
impl<'v> StarlarkValue<'v> for Field<'v> {
    fn write_hash(&self, hasher: &mut StarlarkHasher) -> crate::Result<()> {
        self.typ.write_hash(hasher)?;
        self.default.is_some().hash(hasher);
        if let Some(d) = self.default {
            d.write_hash(hasher)?;
        }
        Ok(())
    }
}
