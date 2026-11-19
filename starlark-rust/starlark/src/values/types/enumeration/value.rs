/*
 * Copyright 2018 The Starlark in Rust Authors.
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

use allocative::Allocative;
use derivative::Derivative;
use dupe::Dupe;
use starlark_derive::Freeze;
use starlark_derive::Trace;
use starlark_derive::starlark_module;
use starlark_derive::starlark_value;
use starlark_map::StarlarkHasher;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::environment::Methods;
use crate::environment::MethodsBuilder;
use crate::starlark_complex_value;
use crate::typing::Ty;
use crate::values::StarlarkPagable;
use crate::values::StarlarkValue;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::enumeration::enum_type::AnyEnumType;
use crate::values::enumeration::enum_type::EnumTypeView;
use crate::values::types::type_instance_id::TypeInstanceId;

/// A value from an enumeration.
#[derive(
    Clone,
    Derivative,
    Trace,
    Freeze,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[repr(C)]
#[derivative(Debug)]
pub struct EnumValue<'v> {
    // Must ignore value.typ or type.elements, since they are circular
    #[derivative(Debug = "ignore")]
    pub(super) typ: Value<'v>, // Must be EnumType it points back to (so it can get the type)
    pub(super) value: Value<'v>, // The value of this enumeration
    pub(super) index: i32,       // The index in the enumeration
    pub(super) id: TypeInstanceId,
}

impl<'v> Display for EnumValue<'v> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ty_enum_data = self.get_enum_type().ty_enum_data();
        match ty_enum_data {
            Some(ty_enum_data) => {
                {
                    write!(f, "{}", ty_enum_data.name)?;
                    write!(f, "(")?;
                    Display::fmt(&self.value, f)?;
                    write!(f, ")")?
                };
                Ok(())
            }
            None => {
                {
                    write!(f, "enum()(")?;
                    Display::fmt(&self.value, f)?;
                    write!(f, ")")?
                };
                Ok(())
            }
        }
    }
}

starlark_complex_value!(pub EnumValue);

impl<'v> EnumValue<'v> {
    /// The result of calling `type()` on an enum value.
    pub(super) const TYPE: &'static str = "enum";

    fn get_enum_type(&self) -> AnyEnumType<'v> {
        // Safe to unwrap because we always ensure typ is EnumType
        EnumTypeView::unpack_value_err(self.typ).unwrap().as_dyn()
    }
}

starlark::methods_static!(ENUM_VALUE_METHODS = enum_value_methods);

#[starlark_value(type = EnumValue::TYPE)]
impl<'v> StarlarkValue<'v> for EnumValue<'v> {
    fn write_hash(&self, hasher: &mut StarlarkHasher) -> crate::Result<()> {
        self.value.write_hash(hasher)
    }

    fn get_methods() -> Option<&'static Methods>
    where
        Self: Sized,
    {
        Some(ENUM_VALUE_METHODS.methods())
    }

    fn typechecker_ty(&self) -> Option<Ty> {
        Some(self.get_enum_type().ty_enum_data()?.ty_enum_value.dupe())
    }
}

impl<'v> serde::Serialize for EnumValue<'v> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.value.serialize(serializer)
    }
}

#[starlark_module]
fn enum_value_methods(methods: &mut MethodsBuilder) {
    #[starlark(attribute)]
    fn index(this: &EnumValue) -> starlark::Result<i32> {
        Ok(this.index)
    }

    #[starlark(attribute)]
    fn value<'v>(this: &EnumValue<'v>) -> starlark::Result<Value<'v>> {
        Ok(this.value.to_value())
    }
}
