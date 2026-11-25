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

use std::sync::OnceLock;

use crate::eval::ParametersSpec;
use crate::eval::ParametersSpecParam;
use crate::values::HeapEdge;
use crate::values::Value;

/// The signature of a native method: a `static` next to the method's glue, built the first
/// time a call needs it. A method whose parameters are all positional-only binds without one
/// and has none.
///
/// The defaults in it are values that exist without a heap, which is what lets it be a
/// `static` and be handed out at any heap's brand.
pub struct NativeSignature {
    spec: OnceLock<ParametersSpec<Value<'static>>>,
    build: fn() -> ParametersSpec<Value<'static>>,
}

impl NativeSignature {
    pub const fn new(build: fn() -> ParametersSpec<Value<'static>>) -> NativeSignature {
        NativeSignature {
            spec: OnceLock::new(),
            build,
        }
    }

    #[inline]
    pub fn get<'v>(&'static self) -> &'v ParametersSpec<Value<'v>> {
        let spec = match self.spec.get() {
            Some(spec) => spec,
            None => self.spec.get_or_init(self.build),
        };
        HeapEdge::immortal().rebrand_ref(spec)
    }
}

pub enum NativeSigArg<'v> {
    Required(&'static str),
    Optional(&'static str),
    Defaulted(&'static str, Value<'v>),
}

impl<'v> NativeSigArg<'v> {
    fn param(&self) -> (&str, ParametersSpecParam<Value<'v>>) {
        match self {
            NativeSigArg::Required(name) => (name, ParametersSpecParam::Required),
            NativeSigArg::Optional(name) => (name, ParametersSpecParam::Optional),
            NativeSigArg::Defaulted(name, value) => (name, ParametersSpecParam::Defaulted(*value)),
        }
    }
}

pub fn parameter_spec<'v>(
    name: &'static str,
    pos_only: &[NativeSigArg<'v>],
    pos_or_named: &[NativeSigArg<'v>],
    args: bool,
    named_only: &[NativeSigArg<'v>],
    kwargs: bool,
) -> ParametersSpec<Value<'v>> {
    ParametersSpec::new_parts(
        name,
        pos_only.iter().map(NativeSigArg::param),
        pos_or_named.iter().map(NativeSigArg::param),
        args,
        named_only.iter().map(NativeSigArg::param),
        kwargs,
    )
}

/// `ParametersSpec` for a function which accepts `&Arguments`.
pub fn parameter_spec_for_arguments<'v>(name: &'static str) -> ParametersSpec<Value<'v>> {
    parameter_spec(name, &[], &[], true, &[], true)
}
