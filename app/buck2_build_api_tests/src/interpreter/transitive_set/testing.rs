/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::Cell;

use buck2_build_api::artifact_groups::deferred::TransitiveSetIndex;
use buck2_build_api::artifact_groups::deferred::TransitiveSetKey;
use buck2_build_api::interpreter::rule_defs::transitive_set::TransitiveSet;
use buck2_build_api::interpreter::rule_defs::transitive_set::TransitiveSetDefinitionView;
use buck2_build_api::interpreter::rule_defs::transitive_set::TransitiveSetOrdering;
use buck2_build_api::interpreter::rule_defs::transitive_set::transitive_set_definition::register_transitive_set;
use buck2_core::deferred::key::DeferredHolderKey;
use buck2_error::BuckErrorOptionContext;
use buck2_interpreter::from_freeze::from_freeze_error;
use buck2_interpreter::testing::Buck2TestHeapName;
use indoc::indoc;
use starlark::environment::FrozenModule;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Module;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::FreezeErrorContext;
use starlark::values::OwnedFrozen;
use starlark::values::Value;
use starlark::values::ValueTyped;

use crate::interpreter::rule_defs::artifact::testing::artifactory;

#[starlark_module]
pub(crate) fn tset_factory(builder: &mut GlobalsBuilder) {
    fn make_tset<'v>(
        definition: TransitiveSetDefinitionView<'v>,
        value: Option<Value<'v>>,
        children: Option<Value<'v>>, // An iterable.
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<ValueTyped<'v, TransitiveSet<'v>>> {
        let definition = definition.frozen()?;
        // Per thread, so a test's indices start at zero whatever other tests
        // run alongside it: `libtest` runs each test on its own thread, and
        // the test harnesses built on `make_tset` index their sets by
        // position.
        thread_local! {
            static LAST_ID: Cell<u32> = const { Cell::new(0) };
        }

        let tset_id = TransitiveSetIndex::testing_new(LAST_ID.with(|id| {
            let next = id.get();
            id.set(next + 1);
            next
        }));

        let set = TransitiveSet::new_from_values(
            TransitiveSetKey::new(DeferredHolderKey::testing_new("cell//more:tsets"), tset_id),
            definition,
            value,
            children,
            eval,
        )?;

        Ok(eval.heap().alloc_representation(set))
    }
}

pub(crate) fn new_transitive_set(
    code: &str,
) -> buck2_error::Result<OwnedFrozen<ValueTyped<'static, TransitiveSet<'static>>>> {
    Module::with_temp_heap(|env| {
        let globals = GlobalsBuilder::standard()
            .with(register_transitive_set)
            .with(tset_factory)
            .with(artifactory)
            .build();

        buck2_interpreter_for_build::attrs::coerce::testing::to_value(&env, &globals, code);

        let frozen = env
            .freeze_named(Buck2TestHeapName::frozen_heap_name())
            .freeze_error_context("Freeze failed")
            .map_err(from_freeze_error)?;

        let make = frozen.get("make").expect("`make` was not found");

        Module::with_temp_heap(|env2| {
            let ret = Evaluator::new(&env2).eval_function(
                make.as_ref().add_to_heap(env2.heap()),
                &[],
                &[],
            )?;

            env2.set_extra_value(ret);

            let frozen = env2
                .freeze_named(Buck2TestHeapName::frozen_heap_name())
                .map_err(from_freeze_error)?;

            frozen
                .extra_value()
                .internal_error("Frozen value must be in extra value")?
                .downcast_starlark::<TransitiveSet<'static>>()
                .map_err(buck2_error::Error::from)
        })
    })
}

/// Freeze `code` as `root//:defs.bzl`. Two calls produce two allocations of every definition in it,
/// which is what a page-out/page-in of one module heap can leave behind in a live daemon.
fn freeze_defs_module(code: &str) -> buck2_error::Result<FrozenModule> {
    Module::with_temp_heap(|env| {
        let globals = GlobalsBuilder::standard()
            .with(register_transitive_set)
            .with(tset_factory)
            .with(artifactory)
            .build();

        buck2_interpreter_for_build::attrs::coerce::testing::to_value(&env, &globals, code);

        env.freeze_named(Buck2TestHeapName::frozen_heap_name())
            .freeze_error_context("Freeze failed")
            .map_err(from_freeze_error)
    })
}

/// A child whose definition is a different allocation of the same logical `FooSet` must still be
/// accepted. Comparing definitions by pointer rejects it with an error whose `expected` and `got`
/// render identically.
#[test]
fn test_child_definition_from_other_incarnation_is_accepted() -> buck2_error::Result<()> {
    let defs = "FooSet = transitive_set()";

    let first = freeze_defs_module(defs)?;
    let second = freeze_defs_module(defs)?;

    let first_foo_set = first.get("FooSet").expect("`FooSet` was not found");
    let second_foo_set = second.get("FooSet").expect("`FooSet` was not found");

    Module::with_temp_heap(|env| {
        let globals = GlobalsBuilder::standard()
            .with(register_transitive_set)
            .with(tset_factory)
            .with(artifactory)
            .build();

        buck2_interpreter_for_build::attrs::coerce::testing::to_value(
            &env,
            &globals,
            indoc!(
                r#"
                def make(first_foo_set, second_foo_set):
                    child = make_tset(first_foo_set, value = 1)
                    return make_tset(second_foo_set, value = 2, children = [child])
                "#
            ),
        );

        let make = env.get("make").expect("`make` was not found");
        let args = [
            first_foo_set.as_ref().add_to_heap(env.heap()),
            second_foo_set.as_ref().add_to_heap(env.heap()),
        ];
        Evaluator::new(&env).eval_function(make, &args, &[])?;

        buck2_error::Ok(())
    })
}

#[test]
fn test_new_transitive_set() -> buck2_error::Result<()> {
    let set = new_transitive_set(indoc!(
        r#"
        FooSet = transitive_set()

        def make():
            s1 = make_tset(FooSet, value = "foo")
            return make_tset(FooSet, value = "bar", children = [s1])
        "#
    ))?;

    assert_eq!(
        set.by_ref(|s| s.iter(TransitiveSetOrdering::Preorder).unwrap().count()),
        2
    );

    Ok(())
}
