/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_hash::BuckMutInsertionMap;
use buck2_util::late_binding::LateBinding;
use starlark::values::StringValue;
use starlark::values::Value;

/// `rule()`, `anon_rule()`, `bxl.anon_rule()` value `impl` field.
pub static FROZEN_RULE_GET_IMPL: LateBinding<
    for<'v> fn(Value<'v>) -> buck2_error::Result<Value<'v>>,
> = LateBinding::new("FROZEN_RULE_GET_IMPL");

pub static FROZEN_PROMISE_ARTIFACT_MAPPINGS_GET_IMPL: LateBinding<
    for<'v> fn(Value<'v>) -> buck2_error::Result<BuckMutInsertionMap<StringValue<'v>, Value<'v>>>,
> = LateBinding::new("FROZEN_PROMISE_ARTIFACT_MAPPINGS_GET_IMPL");
