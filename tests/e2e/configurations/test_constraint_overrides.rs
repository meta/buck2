/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::Write;
use std::path::Path;

use anyhow::Context;
use buck2_e2e::prelude::*;

const REGISTRIES: [&str; 3] = [
    "",
    "prelude//:constraint_override_registry",
    "root//:registry",
];

fn write_python_constraints(root: &Path) -> anyhow::Result<()> {
    let packages = [
        (
            "os/constraints",
            r#"setting(name = "os")
value(name = "linux", setting = ":os")"#,
        ),
        (
            "build_mode/constraints",
            r#"setting(name = "mode")
value(name = "dev", setting = ":mode")
value(name = "opt", setting = ":mode")
setting(name = "link")
value(name = "static", setting = ":link")
value(name = "static_pic", setting = ":link")
group(name = "default_link_style", entries = {"static": ":static", "static_pic": ":static_pic"})
setting(name = "info")
value(name = "full", setting = ":info")
group(name = "fbcode-build-info-mode", entries = {"full": ":full"})
setting(name = "debug")
value(name = "supported", setting = ":debug")
group(name = "native-debugging", entries = {"supported": ":supported"})"#,
        ),
        (
            "build_mode",
            r#"setting(name = "sanitizer")
value(name = "no-san", setting = ":sanitizer")
value(name = "asan-ubsan-dev", setting = ":sanitizer")
group(name = "sanitizer_type", entries = {"no-san": ":no-san", "asan-ubsan-dev": ":asan-ubsan-dev"})
setting(name = "arvr")
value(name = "enabled", setting = ":arvr")
group(name = "arvr_mode", entries = {"enabled": ":enabled"})"#,
        ),
        (
            "platform/execution/constraints",
            r#"setting(name = "execution")
value(name = "execution-platform-transitioned", setting = ":execution")"#,
        ),
        (
            "build_mode/default_opt_cxx",
            r#"setting(name = "opt_cxx")
value(name = "enabled", setting = ":opt_cxx")"#,
        ),
        (
            "cpu/constraints",
            r#"setting(name = "cpu")
value(name = "x86_64", setting = ":cpu")"#,
        ),
        (
            "toolchain/cxx/constraints",
            r#"setting(name = "mcmodel")
value(name = "mcmodel-large", setting = ":mcmodel")"#,
        ),
    ];
    for (package, definitions) in packages {
        let path = root.join("configuration").join(package).join("BUCK");
        std::fs::create_dir_all(path.parent().context("A package directory")?)?;
        let mut definitions = definitions.replace(")\n", ", visibility = [\"PUBLIC\"])\n");
        if let Some(rest) = definitions.strip_suffix(')') {
            definitions = format!("{rest}, visibility = [\"PUBLIC\"])");
        }
        std::fs::write(
            &path,
            format!(
                "load(\"@root//:defs.bzl\", \"setting\", \"value\", \"group\")\n{definitions}\n"
            ),
        )?;
    }
    Ok(())
}

/// The JSON a target's one default output holds.
fn read_output(result: &buck2_e2e::BuckOutput, target: &str) -> anyhow::Result<serde_json::Value> {
    let output = result.build_report()?.output_for_target(target)?;
    Ok(serde_json::from_str(&std::fs::read_to_string(output)?)?)
}

#[buck_test(inplace = false)]
async fn test_constraint_override_registry_equivalence(buck: &Buck) -> anyhow::Result<()> {
    let expected = [
        ("unchanged", ["red", "keep"]),
        ("constraint", ["blue", "keep"]),
        ("ordered", ["blue", "keep"]),
        ("platform", ["blue", "keep"]),
        ("platform_constraint", ["red", "keep"]),
        ("subtarget", ["blue", "keep"]),
    ];
    let targets: Vec<String> = expected
        .iter()
        .map(|(name, _)| format!("root//:{name}"))
        .collect();
    for registry in REGISTRIES {
        let config = format!("buck2.constraint_override_registry={registry}");
        let mut args: Vec<&str> = targets.iter().map(String::as_str).collect();
        args.extend(["-c", &config]);
        let result = buck.build(&args).await?;
        for (name, selected) in expected {
            assert_eq!(
                read_output(&result, &format!("root//:{name}"))?,
                serde_json::json!(selected)
            );
        }
        let query = buck.cquery(&["root//:unchanged", "-c", &config]).await?;
        assert!(query.stdout.contains("root//:original#"));
    }
    Ok(())
}

#[buck_test(inplace = false)]
async fn test_constraint_override_registry_errors_and_invalidation(
    buck: &Buck,
) -> anyhow::Result<()> {
    for registry in REGISTRIES {
        let registry = format!("buck2.constraint_override_registry={registry}");
        let config = ["-c", registry.as_str()];
        buck.build(&[&["root//:unsupported"], &config[..]].concat())
            .await
            .expect_failure()
            .assert_stderr_matches("Constraint value override not supported: root//:keep");
        buck.build(&[&["root//:malformed"], &config[..]].concat())
            .await
            .expect_failure()
            .assert_stderr_matches("Build target must be fully qualified");
        buck.build(&[&["root//:constraint"], &config[..]].concat())
            .await?;
        buck.build(
            &[
                &["root//:constraint"],
                &config[..],
                &["-c", "buck2.constraints=root//:red"],
            ]
            .concat(),
        )
        .await
        .expect_failure()
        .assert_stderr_matches("Constraint value override not supported: alias//:blue");
        buck.build(&[&["root//:constraint"], &config[..]].concat())
            .await?;
        let path = buck.cwd().join("TARGETS.fixture");
        let original = std::fs::read_to_string(&path)?;
        let result = async {
            std::fs::write(
                &path,
                original.replace(
                    r#"entries = {"blue": ":blue"}"#,
                    r#"entries = {"blue": ":red"}"#,
                ),
            )?;
            let result = buck
                .build(&[&["root//:subtarget"], &config[..]].concat())
                .await?;
            assert_eq!(
                read_output(&result, "root//:subtarget")?,
                serde_json::json!(["red", "keep"])
            );
            anyhow::Ok(())
        }
        .await;
        std::fs::write(&path, &original)?;
        result?;
    }
    Ok(())
}

#[buck_test(inplace = false)]
async fn test_legacy_constraint_override_private_visibility(buck: &Buck) -> anyhow::Result<()> {
    let path = buck.cwd().join("TARGETS.fixture");
    std::fs::write(
        &path,
        std::fs::read_to_string(&path)?.replace(
            r#"value(name = "blue", setting = ":color", visibility = ["PUBLIC"])"#,
            r#"value(name = "blue", setting = ":color")"#,
        ),
    )?;
    buck.build(&[
        "root//:constraint",
        "-c",
        "buck2.constraint_override_registry=",
    ])
    .await?;
    buck.build(&[
        "root//:constraint",
        "-c",
        "buck2.constraint_override_registry=prelude//:constraint_override_registry",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("not visible");
    Ok(())
}

#[buck_test(inplace = false)]
async fn test_python_constraint_override_registry(buck: &Buck) -> anyhow::Result<()> {
    write_python_constraints(buck.cwd())?;
    let config = buck.cwd().join(".buckconfig");
    std::fs::write(
        &config,
        std::fs::read_to_string(&config)?
            .replace(
                "  prelude = prelude",
                "  prelude = prelude\n  config = configuration",
            )
            .replace("ovr_config = prelude", "ovr_config = config"),
    )?;
    let targets = buck.cwd().join("TARGETS.fixture");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&targets)?
        .write_all(
            br#"
load(":defs.bzl", "python_probe")
platform(name = "python_dev", values = ["config//os/constraints:linux", "config//build_mode/constraints:dev", "config//build_mode:sanitizer_type[no-san]"], visibility = ["PUBLIC"])
python_selection = select({"config//build_mode/constraints:dev": ["dev"], "config//build_mode/constraints:opt": ["opt"], "DEFAULT": ["unknown"]})
python_probe(name = "python_unchanged", selected = python_selection)
python_probe(name = "python_opt", selected = python_selection, opt_by_default_enabled = True)
"#,
        )?;
    // @oss-disable: let expected_opt = ["opt"];
    let expected_opt = ["dev"]; // @oss-enable
    for registry in REGISTRIES {
        let result = buck
            .build(&[
                "root//:python_unchanged",
                "root//:python_opt",
                "--target-platforms",
                "root//:python_dev",
                "-c",
                &format!("buck2.constraint_override_registry={registry}"),
            ])
            .await?;
        for (name, expected) in [("python_unchanged", ["dev"]), ("python_opt", expected_opt)] {
            assert_eq!(
                read_output(&result, &format!("root//:{name}"))?,
                serde_json::json!(expected)
            );
        }
    }
    Ok(())
}
