/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The documented type of a provider field that is routinely `None` has to say so.

use buck2_build_api::interpreter::rule_defs::provider::builtin::external_runner_test_info::ExternalRunnerTestInfoCallable;
use buck2_build_api::interpreter::rule_defs::provider::builtin::internal_runner_test_info::InternalRunnerTestInfoCallable;
use buck2_build_api::interpreter::rule_defs::provider::builtin::worker_run_info::WorkerRunInfoCallable;
use starlark::docs::DocItem;
use starlark::docs::DocMember;
use starlark::values::StarlarkType;

fn field_type(doc: DocItem, field: &str) -> String {
    let DocItem::Type(ty) = doc else {
        panic!("provider documentation should be a type");
    };
    let (_, member) = ty
        .members
        .iter()
        .find(|(name, _)| *name == field)
        .unwrap_or_else(|| panic!("no field `{field}`"));
    let DocMember::Property(property) = member else {
        panic!("field `{field}` should be a property");
    };
    property.typ.to_string()
}

#[test]
fn test_optional_fields_are_documented_as_optional() {
    let external = StarlarkType::documentation(&ExternalRunnerTestInfoCallable::new());
    assert_eq!(
        "CommandExecutorConfig | None",
        field_type(external.clone(), "default_executor")
    );
    assert_eq!("None | WorkerInfo", field_type(external, "worker"));

    let internal = StarlarkType::documentation(&InternalRunnerTestInfoCallable::new());
    assert_eq!(
        "CommandExecutorConfig | None",
        field_type(internal.clone(), "default_executor")
    );
    assert_eq!("None | WorkerInfo", field_type(internal, "worker"));

    let worker_run = StarlarkType::documentation(&WorkerRunInfoCallable::new());
    assert_eq!(
        "None | WorkerInfo",
        field_type(worker_run.clone(), "remote_worker")
    );
    assert_eq!("None | WorkerInfo", field_type(worker_run, "worker"));
}
