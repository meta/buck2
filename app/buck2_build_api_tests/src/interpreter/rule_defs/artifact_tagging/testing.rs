/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_build_api::interpreter::rule_defs::artifact_tagging::ArtifactTag;
use starlark::environment::GlobalsBuilder;
use starlark::starlark_module;
use starlark::values::Heap;
use starlark::values::ValueOf;

#[starlark_module]
pub(crate) fn artifact_tag_factory(builder: &mut GlobalsBuilder) {
    fn make_tag<'v>(heap: Heap<'v>) -> starlark::Result<ValueOf<'v, ArtifactTag>> {
        Ok(ValueOf::alloc(heap, ArtifactTag::testing_new()))
    }
}
