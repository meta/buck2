/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Debug;

use allocative::Allocative;
use buck2_build_api_derive::internal_provider;
use buck2_hash::BuckMutInsertionMap;
use either::Either;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::values::Freeze;
use starlark::values::StarlarkPagable;
use starlark::values::StringValue;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::dict::DictType;

use crate as buck2_build_api;
use crate::interpreter::rule_defs::cmd_args::value::CommandLineArg;

/// A provider that is used for expansions in string attribute templates
///
/// String attribute templates allow two types of user-defined placeholders, "unkeyed placeholders"
/// like `$(CXX)` or `$(aapt)` and "keyed placeholders"  that include a target key like
/// `$(cxxppflags //some:target)`. The expansion of each of these types is based on the
/// `TemplatePlaceholderInfo` providers.
///
/// "keyed placeholders" are used for the form `$(<key> <target>)` or `$(<key> <target> <arg>)`. In both cases
/// the lookup will expect a `TemplatePlaceholderInfo` in the providers of `<target>`. It will then lookup
/// `<key>` in the keyed_variables (call this the `value`). There are then four valid possibilities:
///  1. no-arg placeholder, an arg-like `value`: resolve to `value`
///  2. no-arg placeholder, a dictionary `value`: resolve to `value["DEFAULT"]`
///  3. arg placeholder, a non-dictionary `value`: this is an error
///  4. arg placeholder, a dictionary `value`: resolve to `value[<arg>]`
///
/// "unkeyed placeholders" are resolved by matching to any of the deps of the target. `$(CXX)` will resolve
/// to the "CXX" value in any dep's `TemplateProviderInfo.unkeyed_variables`
///
/// Fields:
///  - unkeyed_variables: A mapping of names to arg-like values. These are used for "unkeyed placeholder" expansion.
///  - keyed_variables: A mapping of names to arg-like values or dictionary of string to
///        arg-like values. These are used for "keyed placeholder" expansion.
#[internal_provider(template_placeholder_info_creator)]
#[derive(
    Clone,
    Debug,
    Trace,
    Freeze,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[repr(C)]
pub struct TemplatePlaceholderInfo<'v> {
    unkeyed_variables: ValueOf<'v, DictType<StringValue<'v>, CommandLineArg<'v>>>,
    keyed_variables: ValueOf<
        'v,
        DictType<
            StringValue<'v>,
            Either<CommandLineArg<'v>, DictType<StringValue<'v>, CommandLineArg<'v>>>,
        >,
    >,
}

fn collect_variables<'v>(
    variables: ValueOf<'v, DictType<StringValue<'v>, CommandLineArg<'v>>>,
) -> buck2_error::Result<BuckMutInsertionMap<StringValue<'v>, CommandLineArg<'v>>> {
    variables
        .iter()
        .map(|entry| {
            let (k, v) = entry?;
            Ok((k.unpack(), v.unpack()))
        })
        .collect()
}

impl<'v> TemplatePlaceholderInfo<'v> {
    pub fn unkeyed_variables(
        &self,
    ) -> buck2_error::Result<BuckMutInsertionMap<StringValue<'v>, CommandLineArg<'v>>> {
        collect_variables(self.unkeyed_variables)
    }

    pub fn keyed_variables(
        &self,
    ) -> buck2_error::Result<
        BuckMutInsertionMap<
            StringValue<'v>,
            Either<CommandLineArg<'v>, BuckMutInsertionMap<StringValue<'v>, CommandLineArg<'v>>>,
        >,
    > {
        self.keyed_variables
            .iter()
            .map(|entry| {
                let (k, v) = entry?;
                let v = match v.split()? {
                    Either::Left(arg) => Either::Left(arg.unpack()),
                    Either::Right(dict) => Either::Right(collect_variables(dict)?),
                };
                Ok((k.unpack(), v))
            })
            .collect()
    }
}

#[starlark_module]
fn template_placeholder_info_creator(builder: &mut GlobalsBuilder) {
    #[starlark(as_type = TemplatePlaceholderInfo<'static>)]
    fn TemplatePlaceholderInfo<'v>(
        #[starlark(default = ValueOf::new_unchecked(Value::new_empty_dict()))]
        unkeyed_variables: ValueOf<'v, DictType<StringValue<'v>, CommandLineArg<'v>>>,
        #[starlark(default = ValueOf::new_unchecked(Value::new_empty_dict()))]
        keyed_variables: ValueOf<
            'v,
            DictType<
                StringValue<'v>,
                Either<CommandLineArg<'v>, DictType<StringValue<'v>, CommandLineArg<'v>>>,
            >,
        >,
    ) -> starlark::Result<TemplatePlaceholderInfo<'v>> {
        Ok(TemplatePlaceholderInfo {
            unkeyed_variables,
            keyed_variables,
        })
    }
}
