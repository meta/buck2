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

use buck2_build_api::actions::execute::dice_data::set_fallback_executor_config;
use buck2_build_api::analysis::calculation::RuleAnalysisCalculation;
use buck2_build_api::build::detailed_aggregated_metrics::dice::SetDetailedAggregatedMetricsHandle;
use buck2_build_api::build::detailed_aggregated_metrics::events::DetailedAggregatedMetricsHandle;
use buck2_build_api::interpreter::rule_defs::provider::builtin::default_info::DefaultInfoCallable;
use buck2_build_api::interpreter::rule_defs::provider::callable::register_provider;
use buck2_build_api::interpreter::rule_defs::provider::registration::register_builtin_providers;
use buck2_build_api::keep_going::HasKeepGoing;
use buck2_build_api::spawner::BuckSpawner;
use buck2_common::dice::data::testing::SetTestingIoProvider;
use buck2_common::legacy_configs::configs::LegacyBuckConfig;
use buck2_common::package_listing::listing::PackageListing;
use buck2_common::package_listing::listing::testing::PackageListingExt;
use buck2_configured::execution::ExecutionPlatformsKey;
use buck2_core::build_file_path::BuildFilePath;
use buck2_core::bzl::ImportPath;
use buck2_core::cells::CellAliasResolver;
use buck2_core::cells::CellResolver;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::cell_path_with_allowed_relative_dir::CellPathWithAllowedRelativeDir;
use buck2_core::cells::cell_root_path::CellRootPathBuf;
use buck2_core::cells::name::CellName;
use buck2_core::configuration::data::ConfigurationData;
use buck2_core::execution_types::executor_config::CommandExecutorConfig;
use buck2_core::fs::project::ProjectRootTemp;
use buck2_core::package::PackageLabel;
use buck2_core::pattern::pattern::InferTargetNames;
use buck2_core::provider::id::ProviderId;
use buck2_core::provider::id::testing::ProviderIdExt;
use buck2_core::target::label::label::TargetLabel;
use buck2_events::dispatch::EventDispatcher;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::digest_config::SetDigestConfig;
use buck2_hash::BuckFixedInsertionMap;
use buck2_hash::IntentionallyStdHashMap;
use buck2_interpreter::dice::starlark_debug::SetStarlarkDebugger;
use buck2_interpreter::extra::InterpreterHostArchitecture;
use buck2_interpreter::extra::InterpreterHostPlatform;
use buck2_interpreter::file_loader::LoadedModules;
use buck2_interpreter::paths::module::OwnedStarlarkModulePath;
use buck2_interpreter_for_build::interpreter::calculation::InterpreterResultsKey;
use buck2_interpreter_for_build::interpreter::configuror::BuildInterpreterConfiguror;
use buck2_interpreter_for_build::interpreter::dice_calculation_delegate::testing::EvalImportKey;
use buck2_interpreter_for_build::interpreter::interpreter_setup::setup_interpreter_basic;
use buck2_interpreter_for_build::interpreter::testing::Tester;
use buck2_interpreter_for_build::rule::register_rule_function;
use buck2_node::nodes::eval_result::EvaluationResult;
use buck2_node::nodes::targets_map::TargetsMap;
use buck2_node::package_visibility::HasPackageVisibilityDefaultIntersection;
use buck2_node::super_package::SuperPackage;
use buck2_node::visibility::VisibilityIntersection;
use buck2_node::visibility::VisibilityLayerOrigin;
use buck2_node::visibility::VisibilitySpecification;
use buck2_settings::PackageVisibilityDefaultIntersection;
use buck2_util::arc_str::ThinArcSlice;
use dice::DetectCycles;
use dice::Dice;
use dice::UserComputationData;
use dice::testing::DiceBuilder;
use dupe::Dupe;
use indoc::indoc;
use itertools::Itertools;

#[tokio::test]
async fn test_analysis_calculation() -> buck2_error::Result<()> {
    let bzlfile = ImportPath::testing_new("cell//pkg:foo.bzl");
    let resolver = CellResolver::testing_with_names_and_paths(&[
        (
            CellName::testing_new("root"),
            CellRootPathBuf::testing_new(""),
        ),
        (
            CellName::testing_new("cell"),
            CellRootPathBuf::testing_new("cell"),
        ),
    ]);
    let mut interpreter = Tester::with_cells((
        CellAliasResolver::new(
            CellName::testing_new("cell"),
            IntentionallyStdHashMap::new(),
        )?,
        resolver.dupe(),
        LegacyBuckConfig::empty(),
        CellPathWithAllowedRelativeDir::new(CellPath::testing_new("cell//pkg"), None),
    ))?;
    interpreter.additional_globals(register_rule_function);
    interpreter.additional_globals(register_provider);
    interpreter.additional_globals(register_builtin_providers);
    let module = interpreter
        .eval_import(
            &bzlfile,
            indoc!(r#"
                            FooInfo = provider(fields=["str"])

                            def impl(ctx):
                                str = ""
                                if ctx.attrs.dep:
                                    str = ctx.attrs.dep[FooInfo].str
                                return [FooInfo(str=(str + ctx.attrs.str)), DefaultInfo()]
                            foo_binary = rule(impl=impl, attrs={"dep": attrs.option(attrs.dep(providers=[FooInfo]), default = None), "str": attrs.string()})
                        "#),
            LoadedModules::default(),
        )?;

    let buildfile = BuildFilePath::testing_new("cell//pkg:BUCK");
    let eval_res = interpreter.eval_build_file_with_loaded_modules(
        &buildfile,
        indoc!(
            r#"
                    load(":foo.bzl", "FooInfo", "foo_binary")

                    foo_binary(
                        name = "rule1",
                        str = "a",
                        dep = ":rule2",
                    )
                    foo_binary(
                        name = "rule2",
                        str = "b",
                        dep = ":rule3",
                    )
                    foo_binary(
                        name = "rule3",
                        str = "c",
                        dep = None,
                    )
                "#
        ),
        LoadedModules {
            map: BuckFixedInsertionMap::from_iter([(
                OwnedStarlarkModulePath::LoadFile(bzlfile.clone()),
                module.dupe(),
            )]),
        },
        PackageListing::testing_new(&[], "BUCK"),
    )?;

    let fs = ProjectRootTemp::new()?;
    let mut dice = DiceBuilder::new()
        .mock_and_return(
            EvalImportKey(OwnedStarlarkModulePath::LoadFile(bzlfile.clone())),
            Ok(module),
        )
        .mock_and_return(
            InterpreterResultsKey(PackageLabel::testing_parse("cell//pkg")),
            Ok(Arc::new(eval_res)),
        )
        .mock_and_return(ExecutionPlatformsKey, Ok(None))
        .set_data(|data| {
            data.set_testing_io_provider(&fs);
            data.set_digest_config(DigestConfig::testing_default());
            data.set_detailed_aggregated_metrics_handle(DetailedAggregatedMetricsHandle::new());
        })
        .build(testing_user_data(PackageVisibilityDefaultIntersection::Off))
        .await
        .unwrap();
    setup_interpreter_basic(
        &mut dice,
        resolver,
        BuildInterpreterConfiguror::new(
            None,
            InterpreterHostPlatform::Linux,
            InterpreterHostArchitecture::X86_64,
            None,
            false,
            false,
            InferTargetNames::No,
            None,
        )?,
    )?;
    let dice = dice.commit().await;

    let mut ctx = dice.ctx();
    let analysis = ctx
        .get_analysis_result(
            &TargetLabel::testing_parse("cell//pkg:rule1")
                .configure(ConfigurationData::testing_new()),
        )
        .await
        .require_compatible()?;

    assert_eq!(analysis.analysis_values()?.num_actions(), 0);

    assert_eq!(
        analysis
            .providers()
            .unwrap()
            .value()
            .provider_names()
            .iter()
            .sorted()
            .eq(vec!["DefaultInfo", "FooInfo"]),
        true
    );

    assert_eq!(
        analysis
            .providers()
            .unwrap()
            .value()
            .get_provider_raw(&ProviderId::testing_new(*bzlfile.path(), "FooInfo"))
            .unwrap()
            .is_some(),
        true
    );
    assert_eq!(
        analysis
            .providers()
            .unwrap()
            .value()
            .get_provider_raw(DefaultInfoCallable::provider_id())
            .unwrap()
            .is_some(),
        true
    );

    Ok(())
}

fn testing_user_data(mode: PackageVisibilityDefaultIntersection) -> UserComputationData {
    let mut data = UserComputationData::new();
    data.set_keep_going(true);
    data.set_starlark_debugger_handle(None);
    set_fallback_executor_config(&mut data.data, CommandExecutorConfig::testing_local());
    data.data.set(EventDispatcher::null());
    data.spawner = Arc::new(BuckSpawner::current_runtime().unwrap());
    data.set_package_visibility_default_intersection(mode);
    data
}

#[tokio::test]
async fn test_audit_downgrades_only_default_layers() -> buck2_error::Result<()> {
    let bzlfile = ImportPath::testing_new("cell//pkg:foo.bzl");
    let resolver = CellResolver::testing_with_names_and_paths(&[
        (
            CellName::testing_new("root"),
            CellRootPathBuf::testing_new(""),
        ),
        (
            CellName::testing_new("cell"),
            CellRootPathBuf::testing_new("cell"),
        ),
    ]);
    let mut interpreter = Tester::with_cells((
        CellAliasResolver::new(
            CellName::testing_new("cell"),
            IntentionallyStdHashMap::new(),
        )?,
        resolver.dupe(),
        LegacyBuckConfig::empty(),
        CellPathWithAllowedRelativeDir::new(CellPath::testing_new("cell//pkg"), None),
    ))?;
    interpreter.additional_globals(register_rule_function);
    interpreter.additional_globals(register_provider);
    interpreter.additional_globals(register_builtin_providers);
    let module = interpreter
        .eval_import(
            &bzlfile,
            indoc!(r#"
                            FooInfo = provider(fields=["str"])

                            def impl(ctx):
                                str = ""
                                if ctx.attrs.dep:
                                    str = ctx.attrs.dep[FooInfo].str
                                return [FooInfo(str=(str + ctx.attrs.str)), DefaultInfo()]
                            foo_binary = rule(impl=impl, attrs={"dep": attrs.option(attrs.dep(providers=[FooInfo]), default = None), "str": attrs.string()})
                        "#),
            LoadedModules::default(),
        )?;

    let loaded_modules = LoadedModules {
        map: BuckFixedInsertionMap::from_iter([(
            OwnedStarlarkModulePath::LoadFile(bzlfile.clone()),
            module.dupe(),
        )]),
    };
    let pkg_eval_res = Arc::new(interpreter.eval_build_file_with_loaded_modules(
        &BuildFilePath::testing_new("cell//pkg:BUCK"),
        indoc!(
            r#"
                    load(":foo.bzl", "FooInfo", "foo_binary")

                    foo_binary(
                        name = "rule1",
                        str = "a",
                        dep = "//lib:rule2",
                    )
                "#
        ),
        loaded_modules.clone(),
        PackageListing::testing_new(&[], "BUCK"),
    )?);
    let lib_buck = |rule2_visibility: &str| {
        format!(
            r#"
load(":foo.bzl", "FooInfo", "foo_binary")

foo_binary(
    name = "rule2",
    str = "b",
    dep = ":rule3",
    visibility = [{rule2_visibility}],
)
foo_binary(
    name = "rule3",
    str = "c",
    dep = None,
)
"#
        )
    };
    let lib_eval_res_public = Arc::new(interpreter.eval_build_file_with_loaded_modules(
        &BuildFilePath::testing_new("cell//lib:BUCK"),
        &lib_buck("\"PUBLIC\""),
        loaded_modules.clone(),
        PackageListing::testing_new(&[], "BUCK"),
    )?);
    let lib_eval_res_restricted = Arc::new(interpreter.eval_build_file_with_loaded_modules(
        &BuildFilePath::testing_new("cell//lib:BUCK"),
        &lib_buck("\"//other:thing\""),
        loaded_modules,
        PackageListing::testing_new(&[], "BUCK"),
    )?);

    let fs = ProjectRootTemp::new()?;

    // `Tester` has no PACKAGE files, so inject the intersection into both the
    // `SuperPackage` and each node's embedded `Package` (what `is_visible_to` reads).
    let lib_eval_res_with_intersection =
        |base: &EvaluationResult, intersection: VisibilityIntersection| {
            Arc::new(EvaluationResult::new(
                base.buildfile_path().dupe(),
                base.imports().to_vec(),
                SuperPackage::new(
                    base.super_package().package_values().dupe(),
                    base.super_package().visibility().dupe(),
                    base.super_package().within_view().dupe(),
                    intersection.dupe(),
                    base.super_package().within_view_cap().dupe(),
                    base.super_package().cfg_constructor().cloned(),
                    base.super_package().cfg_modifiers().cloned(),
                    base.super_package().test_config_unification_rollout(),
                )
                .unwrap(),
                TargetsMap::from_iter(base.targets().values().map(|node| {
                    node.to_owned()
                        .testing_with_visibility_intersection(intersection.dupe())
                })),
            ))
        };

    // Fresh DICE per mode: the mode is per-transaction data, not a DICE key,
    // so flipping it cannot invalidate an existing instance.
    let restrictive = |origin| {
        VisibilityIntersection::unrestricted().with_layer(
            VisibilitySpecification::testing_parse(&["cell//other/..."]).0,
            ThinArcSlice::empty(),
            origin,
        )
    };
    for (case, base_lib_eval_res, mode, intersection, should_fail) in [
        (
            "unrestricted baseline",
            &lib_eval_res_public,
            PackageVisibilityDefaultIntersection::Off,
            VisibilityIntersection::unrestricted(),
            false,
        ),
        (
            "intersection-induced edge",
            &lib_eval_res_public,
            PackageVisibilityDefaultIntersection::Audit,
            restrictive(VisibilityLayerOrigin::Default),
            false,
        ),
        (
            "intersection-induced edge",
            &lib_eval_res_public,
            PackageVisibilityDefaultIntersection::Enforce,
            restrictive(VisibilityLayerOrigin::Default),
            true,
        ),
        (
            "marker boundary",
            &lib_eval_res_public,
            PackageVisibilityDefaultIntersection::Off,
            restrictive(VisibilityLayerOrigin::Marker),
            true,
        ),
        (
            "marker boundary",
            &lib_eval_res_public,
            PackageVisibilityDefaultIntersection::Audit,
            restrictive(VisibilityLayerOrigin::Marker),
            true,
        ),
        (
            "marker boundary",
            &lib_eval_res_public,
            PackageVisibilityDefaultIntersection::Enforce,
            restrictive(VisibilityLayerOrigin::Marker),
            true,
        ),
        (
            "genuine violation",
            &lib_eval_res_restricted,
            PackageVisibilityDefaultIntersection::Off,
            VisibilityIntersection::unrestricted(),
            true,
        ),
        (
            "genuine violation",
            &lib_eval_res_restricted,
            PackageVisibilityDefaultIntersection::Audit,
            VisibilityIntersection::unrestricted(),
            true,
        ),
        (
            "genuine violation under a default layer",
            &lib_eval_res_restricted,
            PackageVisibilityDefaultIntersection::Audit,
            restrictive(VisibilityLayerOrigin::Default),
            true,
        ),
        (
            "genuine violation",
            &lib_eval_res_restricted,
            PackageVisibilityDefaultIntersection::Enforce,
            VisibilityIntersection::unrestricted(),
            true,
        ),
    ] {
        let mut data_builder = Dice::builder();
        data_builder.set_testing_io_provider(&fs);
        data_builder.set_digest_config(DigestConfig::testing_default());
        data_builder.set_detailed_aggregated_metrics_handle(DetailedAggregatedMetricsHandle::new());
        let dice = data_builder.build(DetectCycles::Enabled);
        let root = dice.new_root().await;

        let target = TargetLabel::testing_parse("cell//pkg:rule1")
            .configure(ConfigurationData::testing_new());

        let mut updater = dice.updater_with_data(root, testing_user_data(mode));
        updater.changed_to(vec![(
            EvalImportKey(OwnedStarlarkModulePath::LoadFile(bzlfile.clone())),
            Ok(module.dupe()),
        )])?;
        updater.changed_to(vec![(
            InterpreterResultsKey(PackageLabel::testing_parse("cell//pkg")),
            Ok(pkg_eval_res.dupe()),
        )])?;
        updater.changed_to(vec![(
            InterpreterResultsKey(PackageLabel::testing_parse("cell//lib")),
            Ok(lib_eval_res_with_intersection(
                base_lib_eval_res,
                intersection,
            )),
        )])?;
        updater.changed_to(vec![(ExecutionPlatformsKey, Ok(None))])?;
        setup_interpreter_basic(
            &mut updater,
            resolver.dupe(),
            BuildInterpreterConfiguror::new(
                None,
                InterpreterHostPlatform::Linux,
                InterpreterHostArchitecture::X86_64,
                None,
                false,
                false,
                InferTargetNames::No,
                None,
            )?,
        )?;
        let txn = updater.commit().await;
        let mut ctx = txn.ctx();
        if should_fail {
            let Err(e) = ctx.get_analysis_result(&target).await.require_compatible() else {
                panic!("{case} must fail in {mode:?} mode");
            };
            let msg = format!("{e:?}");
            assert!(
                msg.contains("is not visible to"),
                "{case} must fail in {mode:?} mode, got: {msg}"
            );
        } else {
            let analysis = ctx
                .get_analysis_result(&target)
                .await
                .require_compatible()?;
            assert!(
                analysis
                    .providers()
                    .unwrap()
                    .value()
                    .get_provider_raw(&ProviderId::testing_new(*bzlfile.path(), "FooInfo"))
                    .unwrap()
                    .is_some(),
                "{case} must actually run analysis, not vacuously succeed"
            );
        }
    }

    Ok(())
}
