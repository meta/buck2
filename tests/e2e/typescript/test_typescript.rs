/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::Path;

use anyhow::Context;
use buck2_e2e::prelude::*;
use regex::escape;
use serde_json::Value;
use serde_json::json;

const FIXTURE: &str = "fbcode//buck2/tests/targets/typescript/first_class";
const PREBUILT_FIXTURE: &str = "fbcode//buck2/tests/targets/typescript/prebuilt";
const FBCODE_FACADE_FIXTURE: &str = "fbcode//buck2/tests/targets/typescript/fbcode_facade";

fn file_name(path: &Path) -> anyhow::Result<String> {
    Ok(path
        .file_name()
        .with_context(|| format!("`{}` has no file name", path.display()))?
        .to_string_lossy()
        .into_owned())
}

fn read_json(path: &Path) -> anyhow::Result<Value> {
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

#[buck_test(inplace = true)]
async fn test_typecheck_aggregates_dependency_markers(buck: &Buck) -> anyhow::Result<()> {
    let typecheck_target = format!("{FIXTURE}:typecheck");
    let typecheck = buck.build(&[&typecheck_target]).await?;
    let validation_markers = typecheck
        .build_report()?
        .outputs_for_target(&typecheck_target, "DEFAULT")?;
    let mut parents = validation_markers
        .iter()
        .map(|marker| file_name(marker.parent().context("A marker has a parent")?))
        .collect::<anyhow::Result<Vec<_>>>()?;
    parents.sort();
    assert_eq!(
        parents,
        [
            "__ambient-consumer__",
            "__ambient-provider__",
            "__app__",
            "__common__",
            "__diamond-left__",
            "__diamond-right__",
            "__diamond-root__",
        ]
    );
    for marker in &validation_markers {
        assert_eq!(file_name(marker)?, "success.json");
    }
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_typecheck_rejects_dependencies_without_validation_markers(buck: &Buck) {
    buck.build(&[&format!("{FIXTURE}:empty-typecheck")])
        .await
        .expect_failure()
        .assert_stderr_matches("dependencies provide no TypeScript validation markers");
}

#[buck_test(inplace = true)]
async fn test_library_publishes_declarations(buck: &Buck) -> anyhow::Result<()> {
    let app_target = format!("{FIXTURE}:app");
    let declarations = buck
        .build(&[&format!("{app_target}[declarations]")])
        .await?;
    let declaration_dir = declarations
        .build_report()?
        .output_for_sub_target(&app_target, "declarations")?;
    let declaration = declaration_dir.join("src").join("app").join("index.d.ts");
    assert!(declaration.is_file());
    assert!(std::fs::read_to_string(&declaration)?.contains("render(value: string): string"));
    assert_eq!(
        read_json(&declaration_dir.join("package.json"))?,
        json!({
            "name": "@e2e-example/ts-app",
            "types": "src/app/index.d.ts",
            "version": "0.0.0",
        })
    );
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_library_publishes_runtime(buck: &Buck) -> anyhow::Result<()> {
    let app_target = format!("{FIXTURE}:app");
    let runtime = buck.build(&[&format!("{app_target}[runtime]")]).await?;
    let runtime_dir = runtime
        .build_report()?
        .output_for_sub_target(&app_target, "runtime")?;
    assert!(
        runtime_dir
            .join("src")
            .join("app")
            .join("index.js")
            .is_file()
    );
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_source_info_reports_runtime_and_declaration_only_sources(
    buck: &Buck,
) -> anyhow::Result<()> {
    let source_info_target = format!("{FIXTURE}:source-info-consumer");
    let declaration_only_target = format!("{FIXTURE}:declaration-only-source-info-consumer");
    let source_info = buck
        .build(&[&source_info_target, &declaration_only_target])
        .await?;
    let source_artifacts = source_info
        .build_report()?
        .outputs_for_target(&source_info_target, "DEFAULT")?;
    assert_eq!(source_artifacts.len(), 1);
    assert_eq!(file_name(&source_artifacts[0])?, "index.ts");
    let declaration_only_artifacts = source_info
        .build_report()?
        .outputs_for_target(&declaration_only_target, "DEFAULT")?;
    assert_eq!(declaration_only_artifacts.len(), 1);
    assert_eq!(file_name(&declaration_only_artifacts[0])?, "index.d.ts");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_fbcode_typescript_facade_binary(buck: &Buck) -> anyhow::Result<()> {
    let binary = buck
        .run(&[&format!("{FBCODE_FACADE_FIXTURE}:binary")])
        .await?;
    assert_eq!(binary.stdout.trim(), "facade:binary");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_fbcode_typescript_facade_custom_run(buck: &Buck) -> anyhow::Result<()> {
    let custom = buck
        .run(&[&format!("{FBCODE_FACADE_FIXTURE}:custom-run")])
        .await?;
    assert_eq!(
        custom.stdout.trim().lines().collect::<Vec<_>>(),
        ["console.log('facade:bundle');", "facade:custom-runtime"]
    );
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_fbcode_typescript_facade_canonical_modules_run(buck: &Buck) -> anyhow::Result<()> {
    let canonical_modules = buck
        .run(&[&format!("{FBCODE_FACADE_FIXTURE}:canonical-modules-run")])
        .await?;
    assert_eq!(
        canonical_modules.stdout.trim(),
        "facade:canonical-module-bundle"
    );
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_fbcode_typescript_facade_custom_compiled_run(buck: &Buck) -> anyhow::Result<()> {
    let compiled = buck
        .run(&[&format!("{FBCODE_FACADE_FIXTURE}:custom-compiled-run")])
        .await?;
    assert_eq!(
        compiled.stdout.trim().lines().collect::<Vec<_>>(),
        [
            "console.log('facade:compiled-modules');",
            "console.log('facade:module-bundle');",
            "facade:bun-runtime",
        ]
    );
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_fbcode_typescript_facade_prebuilt_binary(buck: &Buck) -> anyhow::Result<()> {
    let prebuilt = buck
        .run(&[&format!("{FBCODE_FACADE_FIXTURE}:prebuilt-binary")])
        .await?;
    assert_eq!(prebuilt.stdout.trim(), "prebuilt:binary");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_fbcode_typescript_facade_typecheck(buck: &Buck) -> anyhow::Result<()> {
    let typecheck_target = format!("{FBCODE_FACADE_FIXTURE}:typecheck");
    let typecheck = buck.build(&[&typecheck_target]).await?;
    let markers = typecheck
        .build_report()?
        .outputs_for_target(&typecheck_target, "DEFAULT")?;
    assert_eq!(markers.len(), 1);
    assert_eq!(file_name(&markers[0])?, "success.json");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_transitive_diamond_is_deduplicated(buck: &Buck) -> anyhow::Result<()> {
    let target = format!("{FIXTURE}:diamond-root");
    let result = buck
        .build(&[
            &format!("{target}[declarations]"),
            &format!("{target}[resolver-manifest]"),
        ])
        .await?;
    let report = result.build_report()?;
    let declaration_dir = report.output_for_sub_target(&target, "declarations")?;
    let declaration = declaration_dir
        .join("src")
        .join("diamond_root")
        .join("index.d.ts");
    assert!(declaration.is_file());
    assert!(
        std::fs::read_to_string(&declaration)?.contains("renderDiamond(value: string): string")
    );
    let resolver_manifest = report.output_for_sub_target(&target, "resolver-manifest")?;
    let manifest = read_json(&resolver_manifest)?;
    let mut common_packages = Vec::new();
    for package in manifest
        .get("packages")
        .and_then(Value::as_array)
        .context("No `packages` in the resolver manifest")?
    {
        let package_name = package
            .get("package_name")
            .with_context(|| format!("A package without a `package_name`: {package}"))?;
        if package_name == "@e2e-example/ts-common" {
            common_packages.push(package);
        }
    }
    assert_eq!(common_packages.len(), 1);
    assert_eq!(
        common_packages[0].get("owner"),
        Some(&Value::from(format!("{FIXTURE}:common")))
    );
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_typed_ambient_dependency(buck: &Buck) -> anyhow::Result<()> {
    let target = format!("{FIXTURE}:ambient-consumer");
    let declarations = buck.build(&[&format!("{target}[declarations]")]).await?;
    let declaration_dir = declarations
        .build_report()?
        .output_for_sub_target(&target, "declarations")?;
    let declaration = declaration_dir
        .join("src")
        .join("ambient_consumer")
        .join("index.d.ts");
    assert!(
        std::fs::read_to_string(&declaration)?
            .contains("export declare const ambientGreeting: string;")
    );
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_package_info_runtime_contracts(buck: &Buck) -> anyhow::Result<()> {
    let expected_contracts = [
        (
            "runtime-package-info-consumer",
            json!({
                "direct_runtime_present": true,
                "has_runtime": true,
                "runtime_entry_point": "src/app/index.js",
                "runtime_module_format": "commonjs",
                "runtime_platform": "node",
            }),
        ),
        (
            "declaration-only-package-info-consumer",
            json!({
                "direct_runtime_present": false,
                "has_runtime": false,
                "runtime_entry_point": "",
                "runtime_module_format": "",
                "runtime_platform": "",
            }),
        ),
        (
            "transitive-runtime-package-info-consumer",
            json!({
                "direct_runtime_present": false,
                "has_runtime": false,
                "runtime_entry_point": "",
                "runtime_module_format": "commonjs",
                "runtime_platform": "node",
            }),
        ),
    ];
    let targets: Vec<String> = expected_contracts
        .iter()
        .map(|(name, _)| format!("{FIXTURE}:{name}"))
        .collect();
    let target_args: Vec<&str> = targets.iter().map(String::as_str).collect();
    let result = buck.build(&target_args).await?;
    let build_report = result.build_report()?;

    for (target, (_, expected_contract)) in targets.iter().zip(&expected_contracts) {
        let output = build_report.output_for_target(target)?;
        assert_eq!(&read_json(&output)?, expected_contract);
    }
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_declaration_only_library_has_no_runtime_subtarget(buck: &Buck) {
    buck.build(&[&format!("{FIXTURE}:ambient-provider[runtime]")])
        .await
        .expect_failure()
        .assert_stderr_matches("requested sub target named `runtime`.*is not available");
}

#[buck_test(inplace = true)]
async fn test_declaration_only_library_rejects_runtime_emission(buck: &Buck) {
    buck.build(&[&format!("{FIXTURE}:declaration-only-with-runtime")])
        .await
        .expect_failure()
        .assert_stderr_matches("emit_runtime=True.*set emit_runtime=False");
}

#[buck_test(inplace = true)]
async fn test_invalid_package_name_is_rejected_during_analysis(buck: &Buck) {
    buck.build(&[&format!("{FIXTURE}:invalid-package-name")])
        .await
        .expect_failure()
        .assert_stderr_matches(&escape("invalid package_name '../invalid'"));
    for (target, package_name) in [
        (
            "invalid-three-component-package-name",
            "invalid/package/name",
        ),
        ("invalid-dot-scope-package-name", "@./pkg"),
        ("invalid-dotdot-scope-package-name", "@../pkg"),
        ("invalid-character-package-name", "invalid package:name*"),
        ("invalid-leading-dot-package-name", ".invalid"),
        ("invalid-leading-underscore-package-name", "_invalid"),
        ("invalid-leading-dot-scope-package-name", "@.scope/pkg"),
        (
            "invalid-leading-underscore-scoped-package-name",
            "@scope/_pkg",
        ),
        ("invalid-tilde-package-name", "invalid~name"),
    ] {
        buck.build(&[&format!("{FIXTURE}:{target}")])
            .await
            .expect_failure()
            .assert_stderr_matches(&escape(&format!("invalid package_name '{package_name}'")));
    }
}

#[buck_test(inplace = true)]
async fn test_declaration_entry_point_is_rejected_for_runtime_package(buck: &Buck) {
    buck.build(&[&format!("{FIXTURE}:declaration-entry-with-runtime-source")])
        .await
        .expect_failure()
        .assert_stderr_matches("entry_point.*produces no runtime output.*runtime-producing entry point.*emit_runtime=False");
}

#[buck_test(inplace = true)]
async fn test_runtime_output_collision_is_rejected_during_analysis(buck: &Buck) {
    buck.build(&[&format!("{FIXTURE}:runtime-output-collision")])
        .await
        .expect_failure()
        .assert_stderr_matches(&escape(
            "produce the same runtime output 'src/collision/index.js'",
        ));
}

#[buck_test(inplace = true)]
async fn test_declaration_output_collisions_are_rejected_during_analysis(buck: &Buck) {
    for (target, output) in [
        (
            "declaration-output-collision-no-runtime",
            "src/collision/index.d.ts",
        ),
        (
            "declaration-source-output-collision",
            "src/declaration_collision/foo.d.ts",
        ),
    ] {
        buck.build(&[&format!("{FIXTURE}:{target}")])
            .await
            .expect_failure()
            .assert_stderr_matches(&escape(&format!(
                "produce the same declaration output '{output}'"
            )));
    }
    buck.build(&[&format!("{FIXTURE}:declaration-prefix-output-collision")])
        .await
        .expect_failure()
        .assert_stderr_matches(&escape(concat!(
            "conflicting declaration outputs ",
            "'src/declaration_prefix_collision/a.d.ts' and ",
            "'src/declaration_prefix_collision/a.d.ts/child.d.ts'",
        )));
}

#[buck_test(inplace = true)]
async fn test_duplicate_source_is_rejected_during_analysis(buck: &Buck) {
    buck.build(&[&format!("{FIXTURE}:duplicate-source")])
        .await
        .expect_failure()
        .assert_stderr_matches(&escape(
            "lists TypeScript source path 'src/common/index.ts' more than once",
        ));
}

#[buck_test(inplace = true)]
async fn test_distinct_artifacts_with_same_source_path_are_rejected(buck: &Buck) {
    buck.build(&[&format!("{FIXTURE}:distinct-artifact-source-collision")])
        .await
        .expect_failure()
        .assert_stderr_matches(&escape(
            "both map to TypeScript source path 'shared/index.ts'",
        ));
}

#[buck_test(inplace = true)]
async fn test_dependency_identity_mismatch_is_rejected_during_analysis(buck: &Buck) {
    buck.build(&[&format!("{FIXTURE}:identity-mismatch")])
        .await
        .expect_failure()
        .assert_stderr_matches(&format!(
            "{}[^']+'",
            escape("uses compiler identity 'typescript@mismatch', expected '")
        ));
    buck.build(&[&format!("{FIXTURE}:toolchain-identity-mismatch")])
        .await
        .expect_failure()
        .assert_stderr_matches(&format!(
            "{}[^']+'",
            escape("uses toolchain identity 'toolchain@mismatch', expected '")
        ));
}

#[buck_test(inplace = true)]
async fn test_composable_compile_bundle_and_runtime_stages(buck: &Buck) -> anyhow::Result<()> {
    let compiled = buck
        .run(&[&format!("{FIXTURE}:compiled-program-run")])
        .await?;
    assert_eq!(compiled.stdout.trim(), "app:compiled");

    let node = buck
        .run(&[&format!("{FIXTURE}:source-bundle-node")])
        .await?;
    assert_eq!(node.stdout.trim(), "source bundle");

    let bun_like = buck.run(&[&format!("{FIXTURE}:source-bundle-bun")]).await?;
    assert_eq!(
        bun_like.stdout.trim().lines().collect::<Vec<_>>(),
        ["console.log('source bundle');", "bun-like:source bundle"]
    );

    let custom_compiler = buck
        .run(&[&format!("{FIXTURE}:custom-compiler-run")])
        .await?;
    assert_eq!(custom_compiler.stdout.trim(), "custom compiler bundle");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_custom_stages_keep_typechecking_and_validate_compatibility(buck: &Buck) {
    buck.build(&[&format!("{FIXTURE}:invalid-custom-bundle")])
        .await
        .expect_failure()
        .assert_stderr_matches("Type 'number' is not assignable to type 'string'");
    let runtime_failure = buck
        .build(&[&format!("{FIXTURE}:incompatible-source-bundle-runtime")])
        .await
        .expect_failure();
    runtime_failure.assert_stderr_matches(&format!(
        "adapter 'fake-browser-runtime-v1' for target '{FIXTURE}:source-bundle.*does not accept platform 'node'"
    ));
    assert!(!runtime_failure.stderr.contains(&format!(
        "for target '{FIXTURE}:incompatible-source-bundle-runtime"
    )));
    buck.build(&[&format!("{FIXTURE}:incompatible-compiler-bundle")])
        .await
        .expect_failure()
        .assert_stderr_matches("does not accept module format 'esm'");
}

#[buck_test(inplace = true)]
async fn test_binary_compiles_bundles_runs_and_rejects_compiler_bypass(
    buck: &Buck,
) -> anyhow::Result<()> {
    let native = buck.run(&[&format!("{FIXTURE}:native-binary")]).await?;
    assert_eq!(native.stdout.trim(), "app:native");

    let default_entry_point = buck
        .run(&[&format!("{FIXTURE}:default-entry-point-binary")])
        .await?;
    assert_eq!(default_entry_point.stdout.trim(), "default:entry-point");
    let source_info_target = format!("{FIXTURE}:default-entry-point-source-info-consumer");
    let source_info = buck.build(&[&source_info_target]).await?;
    let source_artifacts = source_info
        .build_report()?
        .outputs_for_target(&source_info_target, "DEFAULT")?;
    let names = source_artifacts
        .iter()
        .map(|artifact| file_name(artifact))
        .collect::<anyhow::Result<Vec<_>>>()?;
    assert_eq!(names, ["index.ts"]);

    let esm = buck.run(&[&format!("{FIXTURE}:esm-source-binary")]).await?;
    assert_eq!(esm.stdout.trim(), "source bundle esm");

    buck.build(&[&format!("{FIXTURE}:compiler-bypass")])
        .await
        .expect_failure()
        .assert_stderr_matches("selects compiler.*but source bundler.*would bypass it");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_custom_compiler_rejects_incompatible_runtime_dependency(buck: &Buck) {
    buck.build(&[&format!(
        "{FIXTURE}:incompatible-custom-compiler-dependency"
    )])
    .await
    .expect_failure()
    .assert_stderr_matches(concat!(
        "dependency whose runtime modules are incompatible with compiled output ",
        "module format 'esm' and platform 'neutral'",
    ));
    buck.build(&[&format!(
        "{FIXTURE}:malformed-runtime-capability-dependency"
    )])
    .await
    .expect_failure()
    .assert_stderr_matches(
        "has_runtime=True but has no runtime module format and platform capability",
    );
    buck.build(&[&format!("{FIXTURE}:partial-runtime-capability-dependency")])
        .await
        .expect_failure()
        .assert_stderr_matches("must define runtime module format and platform together");
    buck.build(&[&format!(
        "{FIXTURE}:incompatible-transitive-runtime-dependency"
    )])
    .await
    .expect_failure()
    .assert_stderr_matches(concat!(
        "dependency whose runtime modules are incompatible with compiled output ",
        "module format 'esm' and platform 'neutral'",
    ));
}

#[buck_test(inplace = true)]
async fn test_prebuilt_packages_typecheck_and_run(buck: &Buck) -> anyhow::Result<()> {
    let declaration_target = format!("{PREBUILT_FIXTURE}:declaration-consumer-typecheck");
    let declaration_result = buck.build(&[&declaration_target]).await?;
    let declaration_markers = declaration_result
        .build_report()?
        .outputs_for_target(&declaration_target, "DEFAULT")?;
    assert_eq!(declaration_markers.len(), 1);
    assert_eq!(file_name(&declaration_markers[0])?, "success.json");

    let declaration_binary = buck
        .run(&[&format!("{PREBUILT_FIXTURE}:declaration-only-binary")])
        .await?;
    assert_eq!(declaration_binary.stdout.trim(), "declaration-only");

    let compatible_target = format!("{PREBUILT_FIXTURE}:compatible-cjs");
    let compatible = buck
        .run(&[&format!("{PREBUILT_FIXTURE}:compatible-binary")])
        .await?;
    assert_eq!(compatible.stdout.trim(), "prebuilt:compatible:subpath");
    let declarations = buck
        .build(&[&format!("{compatible_target}[declarations]")])
        .await?;
    let declarations_dir = declarations
        .build_report()?
        .output_for_sub_target(&compatible_target, "declarations")?;
    assert_eq!(
        read_json(&declarations_dir.join("package.json"))?,
        json!({
            "name": "@prebuilt/cjs",
            "types": "lib/index.d.ts",
            "version": "0.0.0",
        })
    );

    let nested_target = format!("{PREBUILT_FIXTURE}:nested-consumer-typecheck");
    let nested = buck.build(&[&nested_target]).await?;
    assert_eq!(
        nested
            .build_report()?
            .outputs_for_target(&nested_target, "DEFAULT")?
            .len(),
        1
    );
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_prebuilt_incompatible_runtime_typechecks_but_cannot_run(
    buck: &Buck,
) -> anyhow::Result<()> {
    buck.build(&[&format!(
        "{PREBUILT_FIXTURE}:incompatible-consumer-typecheck"
    )])
    .await?;
    buck.build(&[&format!("{PREBUILT_FIXTURE}:incompatible-consumer-binary")])
        .await
        .expect_failure()
        .assert_stderr_matches(concat!(
            "dependency whose runtime modules are incompatible with compiled output ",
            "module format 'commonjs' and platform 'node'",
        ));
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_prebuilt_rejects_malformed_layouts(buck: &Buck) {
    for (target, error) in [
        (
            "missing-declaration-entry",
            "declaration entry point 'missing.d.ts' does not exist",
        ),
        (
            "invalid-declaration-entry",
            "invalid declaration_entry_point",
        ),
        (
            "runtime-without-entry",
            "must set runtime and runtime_entry_point together",
        ),
        (
            "runtime-without-format",
            "must set module_format and platform exactly when runtime is set",
        ),
        (
            "format-without-runtime",
            "must set module_format and platform exactly when runtime is set",
        ),
        ("invalid-package-path", "must end with package_name"),
        (
            "invalid-scoped-node-modules-package-path",
            "invalid package_name '@scope/node_modules'",
        ),
        ("non-directory-declarations", "must be a directory"),
        ("invalid-prebuilt-typecheck", "TypeScriptTypecheckInfo"),
    ] {
        buck.build(&[&format!("{PREBUILT_FIXTURE}:{target}")])
            .await
            .expect_failure()
            .assert_stderr_matches(error);
    }
}
