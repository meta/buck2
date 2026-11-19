/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use buck2_analysis::attrs::resolve::ctx::AnalysisQueryResult;
use buck2_analysis::attrs::resolve::ctx::AttrResolutionContext;
use buck2_build_api::interpreter::rule_defs::cmd_args::value::CommandLineArg;
use buck2_build_api::interpreter::rule_defs::provider::builtin::template_placeholder_info::TemplatePlaceholderInfo;
use buck2_build_api::interpreter::rule_defs::provider::callable::register_provider;
use buck2_build_api::interpreter::rule_defs::provider::collection::FrozenProviderCollectionValue;
use buck2_build_api::interpreter::rule_defs::provider::collection::ProviderCollection;
use buck2_build_api::interpreter::rule_defs::provider::registration::register_builtin_providers;
use buck2_core::configuration::data::ConfigurationData;
use buck2_core::execution_types::execution::ExecutionPlatformResolution;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_error::BuckErrorOptionContext;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::buck_mut_insertion_map;
use buck2_interpreter::testing::Buck2TestHeapName;
use buck2_interpreter::types::provider::callable::ValueAsProviderCallableLike;
use buck2_interpreter_for_build::attrs::coerce;
use buck2_interpreter_for_build::attrs::coerce::testing;
use buck2_node::attrs::coercion_context::AttrCoercionContext;
use buck2_node::provider_id_set::ProviderIdSet;
use dupe::Dupe;
use dupe::OptionDupedExt;
use indoc::indoc;
use starlark::environment::FrozenModule;
use starlark::environment::Globals;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Module;
use starlark::values::FrozenValueTyped;
use starlark::values::Value;
use starlark::values::dict::DictRef;

use crate::interpreter::rule_defs::artifact::testing::artifactory;

pub(crate) fn resolution_ctx<'v>(module: &Module<'v>) -> impl AttrResolutionContext<'v> {
    resolution_ctx_with_providers(module).0
}

pub(crate) fn resolution_ctx_with_providers<'v>(
    module: &Module<'v>,
) -> (impl AttrResolutionContext<'v>, ProviderIdSet) {
    struct Ctx<'a, 'v> {
        module: &'a Module<'v>,
        // The module the provider collections were frozen in.
        _deps_env: FrozenModule,
        deps: BuckMutInsertionMap<ConfiguredProvidersLabel, FrozenProviderCollectionValue>,
    }

    impl Ctx<'_, '_> {
        fn eval(env: &Module, globals: &Globals) {
            testing::to_value(
                env,
                globals,
                // Note that putting a list of providers into the subtargets of a DefaultInfo will
                // convert it into a provider collection (which we expect later on). This is why
                // we do that and don't use lists of providers directly.
                indoc!(
                    r#"
             foo_a = source_artifact("", "default.cpp")
             bar_a = source_artifact("", "bar.cpp")
             bar1_a = source_artifact("", "bar1.cpp")
             bar2_a = source_artifact("", "bar2.cpp")
             bar3_a = source_artifact("", "bar3.cpp")
             bar = DefaultInfo(default_outputs=[bar_a])
             zero = DefaultInfo(default_outputs=[])
             single = DefaultInfo(default_outputs=[bar1_a])
             multiple = DefaultInfo(default_outputs=[bar1_a, bar2_a, bar3_a])
             foo = DefaultInfo(
                 sub_targets={
                     "bar": [bar],
                     "zero": [zero],
                     "single": [single],
                     "multiple": [multiple],
                     "foo_only": [FooInfo(foo="f1")],
                     "foo_and_bar": [FooInfo(foo="f1"), BarInfo(bar="b1")],
                 },
                 default_outputs=[foo_a],
             )
             outer = DefaultInfo(sub_targets={
                 "foo": [foo],
                 "toolchain": [zero, TemplatePlaceholderInfo(unkeyed_variables = {"CXX": "clang++"})],
                 "keyed_placeholder": [zero, TemplatePlaceholderInfo(keyed_variables = {
                     "user_key": "hello",
                     "key_with_args": { "DEFAULT": "world", "big": "big world" },
                 })]
             })
             ret = {
                 "foo": outer.sub_targets["foo"],
                 "bar": foo.sub_targets["bar"],
                 "zero": foo.sub_targets["zero"],
                 "single": foo.sub_targets["single"],
                 "multiple": foo.sub_targets["multiple"],
                 "foo_only": foo.sub_targets["foo_only"],
                 "foo_and_bar": foo.sub_targets["foo_and_bar"],
                 "toolchain": outer.sub_targets["toolchain"],
                 "keyed_placeholder": outer.sub_targets["keyed_placeholder"],
             }
             "#
                ),
            );
        }

        /// Create a simple label -> impl result mapping
        fn simple_deps() -> (
            FrozenModule,
            BuckMutInsertionMap<ConfiguredProvidersLabel, FrozenProviderCollectionValue>,
            ProviderIdSet,
        ) {
            let globals = GlobalsBuilder::standard()
                .with(register_builtin_providers)
                .with(register_provider)
                .with(artifactory)
                .build();

            // Add some providers into the environment
            let frozen_provider_env = Module::with_temp_heap(|provider_env| {
                let provider_content = indoc!(
                    r#"
                 FooInfo = provider(fields=["foo"])
                 BarInfo = provider(fields=["bar"])
                 None
                 "#
                );
                testing::to_value(&provider_env, &globals, provider_content);
                provider_env.freeze_named(Buck2TestHeapName::frozen_heap_name())
            })
            .expect("provider should freeze successfully");
            let foo_info = frozen_provider_env.get("FooInfo").unwrap();
            let bar_info = frozen_provider_env.get("BarInfo").unwrap();

            let frozen = Module::with_temp_heap(|env| {
                env.set("FooInfo", foo_info.as_ref().add_to_heap(env.heap()));
                env.set("BarInfo", bar_info.as_ref().add_to_heap(env.heap()));
                Self::eval(&env, &globals);
                env.freeze_named(Buck2TestHeapName::frozen_heap_name())
            })
            .expect("should freeze successfully");
            let label_and_result = |label, var_name| {
                let configured_label = coerce::testing::coercion_ctx()
                    .coerce_providers_label(label)
                    .unwrap()
                    .configure(ConfigurationData::testing_new());
                let val = FrozenProviderCollectionValue::try_from_value(
                    frozen.get("ret").unwrap().map::<Value<'static>, _>(|x| {
                        DictRef::from_value(x).unwrap().get_str(var_name).unwrap()
                    }),
                )
                .unwrap();
                (configured_label, val)
            };

            let (foo_label, foo_result) = label_and_result("//sub/dir:foo", "foo");
            let (bar_label, bar_result) = label_and_result("//sub/dir:foo[bar]", "bar");
            let (zero_label, zero_result) = label_and_result("//sub/dir:foo[zero]", "zero");
            let (single_label, single_result) = label_and_result("//sub/dir:foo[single]", "single");
            let (multiple_label, multiple_result) =
                label_and_result("//sub/dir:foo[multiple]", "multiple");
            let (foo_only_label, foo_only_result) =
                label_and_result("//sub/dir:foo[foo_only]", "foo_only");
            let (foo_and_bar_label, foo_and_bar_result) =
                label_and_result("//sub/dir:foo[foo_and_bar]", "foo_and_bar");

            let (toolchain_label, toolchain_result) =
                label_and_result("//sub/dir:toolchain", "toolchain");
            let (keyed_placeholder_label, keyed_placeholder_result) =
                label_and_result("//sub/dir:keyed_placeholder", "keyed_placeholder");

            let res = buck_mut_insertion_map![
                foo_label => foo_result,
                bar_label => bar_result,
                zero_label => zero_result,
                single_label => single_result,
                multiple_label => multiple_result,
                foo_only_label => foo_only_result,
                foo_and_bar_label => foo_and_bar_result,
                toolchain_label => toolchain_result,
                keyed_placeholder_label => keyed_placeholder_result
            ];

            let provider_ids = ProviderIdSet::from(vec![
                foo_info
                    .as_ref()
                    .value()
                    .as_provider_callable()
                    .unwrap()
                    .id()
                    .unwrap()
                    .dupe(),
                bar_info
                    .as_ref()
                    .value()
                    .as_provider_callable()
                    .unwrap()
                    .id()
                    .unwrap()
                    .dupe(),
            ]);
            (frozen, res, provider_ids)
        }
    }

    impl<'v> AttrResolutionContext<'v> for Ctx<'_, 'v> {
        fn starlark_module(&self) -> &Module<'v> {
            self.module
        }

        fn get_dep(
            &mut self,
            target: &ConfiguredProvidersLabel,
        ) -> buck2_error::Result<FrozenValueTyped<'v, ProviderCollection<'v>>> {
            Ok(self
                .deps
                .get(target)
                .duped()
                .internal_error("missing dep")?
                .add_heap_ref(self.module.heap()))
        }

        fn resolve_unkeyed_placeholder(
            &mut self,
            name: &str,
        ) -> buck2_error::Result<Option<CommandLineArg<'v>>> {
            for providers in self.deps.values() {
                let resolved = providers.value.by_ref_with_reconstructor(|collection, r| {
                    let Some(placeholders) = collection
                        .as_ref()
                        .builtin_provider::<TemplatePlaceholderInfo>()?
                    else {
                        return buck2_error::Ok(None);
                    };
                    let Some(value) = placeholders.unkeyed_variables()?.get(name).copied() else {
                        return Ok(None);
                    };
                    Ok(Some(r.edge(self.module.heap()).rebrand(value)))
                })?;
                if let Some(value) = resolved {
                    return Ok(Some(value));
                }
            }
            Ok(None)
        }

        fn resolve_query(&mut self, _query: &str) -> buck2_error::Result<Arc<AnalysisQueryResult>> {
            unimplemented!("This test resolution context doesn't handle queries")
        }

        fn execution_platform_resolution(&self) -> &ExecutionPlatformResolution {
            unimplemented!("This test resolution context doesn't use execution platform resolution")
        }
    }
    let (deps_env, deps, provider_ids) = Ctx::simple_deps();
    (
        Ctx {
            module,
            _deps_env: deps_env,
            deps,
        },
        provider_ids,
    )
}
