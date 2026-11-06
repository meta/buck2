/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::convert::Infallible;
use std::fmt::Display;

use allocative::Allocative;
use dupe::Dupe;
use serde::Serializer;
use starlark::__derive_refs::serde::Serialize;
use starlark::any::ProvidesStaticType;
use starlark::typing::Ty;
use starlark::values::Freeze;
use starlark::values::FreezeResult;
use starlark::values::Freezer;
use starlark::values::StarlarkPagable;
use starlark::values::Trace;
use starlark::values::UnpackValue;
use starlark::values::UnpackValueStable;
use starlark::values::Value;
use starlark::values::ValueTyped;
use starlark::values::type_repr::StarlarkTypeRepr;

use crate::interpreter::rule_defs::cmd_args::CommandLineArgLike;
use crate::interpreter::rule_defs::cmd_args::StarlarkCmdArgs;
use crate::interpreter::rule_defs::cmd_args::value_as::ValueAsCommandLineLike;

fn serialize_as_display<V: Display, S>(v: &V, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    s.collect_str(v)
}

/// A tiny wrapper around `Value` that proxies `CommandLineArgLike` calls.
///
/// This should be unnecessary, however I'm not smart enough to figure out how to get
/// things to live long enough, in `ValueAsCommandLineArgLike`, so I'm moving on with my life
/// for now. All values contained in here are guaranteed to implement `CommandLineArgLike`.
#[derive(
    Debug,
    Clone,
    Copy,
    Dupe,
    Trace,
    derive_more::Display,
    Serialize,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[serde(transparent)]
#[repr(transparent)]
pub struct CommandLineArg<'v>(#[serde(serialize_with = "serialize_as_display")] Value<'v>);

impl<'v> PartialEq for CommandLineArg<'v> {
    fn eq(&self, other: &Self) -> bool {
        self.0.ptr_eq(other.0)
    }
}

impl<'v> Eq for CommandLineArg<'v> {}

impl<'v> Freeze<'v> for CommandLineArg<'v> {
    type Frozen<'fv> = CommandLineArg<'fv>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<CommandLineArg<'fv>> {
        // Freezing does not change a value's type, so the constructor check carries over.
        Ok(CommandLineArg(self.0.freeze(freezer)?))
    }
}

impl<'v> StarlarkTypeRepr for CommandLineArg<'v> {
    type Canonical = <ValueAsCommandLineLike<'v> as StarlarkTypeRepr>::Canonical;

    fn starlark_type_repr() -> Ty {
        ValueAsCommandLineLike::starlark_type_repr()
    }
}

impl<'v> UnpackValueStable<'v> for CommandLineArg<'v> {}

impl<'v> UnpackValue<'v> for CommandLineArg<'v> {
    type Error = Infallible;

    fn unpack_value_impl(value: Value<'v>) -> Result<Option<Self>, Self::Error> {
        if ValueAsCommandLineLike::unpack_value_opt(value).is_some() {
            Ok(Some(CommandLineArg(value)))
        } else {
            Ok(None)
        }
    }
}

impl<'v> CommandLineArg<'v> {
    pub fn new(value: Value<'v>) -> buck2_error::Result<CommandLineArg<'v>> {
        ValueAsCommandLineLike::unpack_value_err(value)?;
        Ok(CommandLineArg(value))
    }

    pub fn from_cmd_args(cmd_args: ValueTyped<'v, StarlarkCmdArgs<'v>>) -> Self {
        let _no_check_needed: &dyn CommandLineArgLike<'v> = cmd_args.as_ref();
        CommandLineArg(cmd_args.to_value())
    }

    pub fn as_command_line_arg(self) -> &'v dyn CommandLineArgLike<'v> {
        ValueAsCommandLineLike::unpack_value_err(self.0)
            .expect("checked type in constructor")
            .0
    }

    pub fn to_value(self) -> Value<'v> {
        self.0
    }

    /// View a `FrozenStarlarkCmdArgs`' element storage, whose elements were checked when the
    /// unfrozen form was built.
    pub fn slice_from_values_unchecked<'a>(values: &'a [Value<'v>]) -> &'a [CommandLineArg<'v>] {
        // SAFETY: `#[repr(transparent)]` over `Value` at the same lifetime. All this asserts is
        // the `CommandLineArgLike` invariant, which is what makes it `_unchecked`.
        unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), values.len()) }
    }
}
