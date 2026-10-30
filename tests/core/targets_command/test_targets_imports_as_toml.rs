/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::prelude::*;
use serde_json::Value;
use serde_json::json;

async fn streaming_imports(buck: &Buck, extra_args: &[&str]) -> anyhow::Result<Vec<Value>> {
    let mut args = vec!["//...", "--json", "--streaming", "--imports"];
    args.extend_from_slice(extra_args);
    let result = buck.targets(&args).await?;
    Ok(serde_json::from_str(&result.stdout)?)
}

/// A `load(":<file>?as=toml", ...)` is reported as a leaf import by `targets --streaming
/// --imports`, and is parsed as TOML, even though the file's extension (`.lock`) does not imply
/// TOML. The `?as=toml` hint is stripped from the reported path. Covers both a nested load (via a
/// `.bzl`) and a top-level load directly in the build file.
#[buck_test]
async fn test_imports_as_toml(buck: &Buck) -> anyhow::Result<()> {
    let xs = streaming_imports(buck, &[]).await?;

    let mut found_targets = false;
    let mut found_bzl = false;
    let mut found_lock = false;
    let mut found_direct = false;

    for x in &xs {
        let Some(imports) = x.get("buck.imports") else {
            continue;
        };
        let imports = imports.as_array().expect("`buck.imports` is a list");
        let file = &x["buck.file"];

        if file == "root//TARGETS.fixture" {
            assert!(
                imports.contains(&json!("root//uses_lock.bzl")),
                "{imports:?}"
            );
            // Top-level `?as=toml` load, reported without the hint.
            assert!(imports.contains(&json!("root//direct.lock")), "{imports:?}");
            found_targets = true;
        } else if file == "root//uses_lock.bzl" {
            // The `?as=toml` hint is not part of the reported import path.
            assert!(imports.contains(&json!("root//uv.lock")), "{imports:?}");
            found_bzl = true;
        } else if file == "root//uv.lock" {
            // Parsed as TOML (empty sub-imports), not as a bzl file (which would carry an implicit
            // `prelude//prelude.bzl` import).
            assert!(imports.is_empty(), "{imports:?}");
            found_lock = true;
        } else if file == "root//direct.lock" {
            assert!(imports.is_empty(), "{imports:?}");
            found_direct = true;
        }
    }

    assert!(found_targets, "TARGETS.fixture imports should be reported");
    assert!(
        found_bzl,
        "uses_lock.bzl imports (including uv.lock) should be reported"
    );
    assert!(
        found_lock,
        "nested uv.lock?as=toml should appear as a leaf import with empty sub-imports"
    );
    assert!(
        found_direct,
        "top-level direct.lock?as=toml should appear as a leaf import with empty sub-imports"
    );
    Ok(())
}

/// `*` in `buck2.load_as_allowlist` allows every file.
#[buck_test]
async fn test_imports_as_toml_star_allows_all(buck: &Buck) -> anyhow::Result<()> {
    let xs = streaming_imports(buck, &["-c", "buck2.load_as_allowlist=*"]).await?;
    assert!(
        xs.iter()
            .any(|x| x["buck.file"] == "root//uv.lock" && x["buck.imports"] == json!([])),
        "uv.lock?as=toml should load when all extensions are allowed via `*`"
    );
    Ok(())
}

/// An entry in `buck2.load_as_allowlist` may be a whole file name (e.g. `uv.lock`), not just an
/// extension, so specific package-manager lockfiles can be allowed without opening up every
/// `*.lock` file.
#[buck_test]
async fn test_imports_as_toml_whole_filename_allowlist(buck: &Buck) -> anyhow::Result<()> {
    let xs =
        streaming_imports(buck, &["-c", "buck2.load_as_allowlist=uv.lock,direct.lock"]).await?;
    let loaded: Vec<&Value> = xs
        .iter()
        .filter(|x| x.get("buck.imports").is_some())
        .map(|x| &x["buck.file"])
        .collect();
    assert!(loaded.contains(&&json!("root//uv.lock")), "{loaded:?}");
    assert!(loaded.contains(&&json!("root//direct.lock")), "{loaded:?}");
    Ok(())
}
