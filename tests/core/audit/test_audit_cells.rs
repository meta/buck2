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

#[buck_test]
async fn test_cell_ordering(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.audit(&["cell"]).await?;
    // The repository should be in the list, not the alias
    assert!(res.stdout.contains("source:"));
    assert!(!res.stdout.contains("a:"));
    assert!(!res.stdout.contains("z:"));

    let res = buck.audit(&["cell", "--aliases"]).await?;
    assert!(res.stdout.contains("source:"));
    assert!(res.stdout.contains("a:"));
    assert!(res.stdout.contains("z:"));
    Ok(())
}

#[buck_test]
async fn test_bxl_audit_cell(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//test_audit.bxl:audit_cell"]).await?;
    let path = |cell: &str| buck.cwd().join(cell).to_string_lossy().into_owned();

    // specify single cell
    let outputs: Vec<&str> = result.stdout.lines().collect();
    let single_result: Value = serde_json::from_str(outputs[0])?;
    assert_eq!(single_result["source"], path("fbs"));

    // don't specify cell - should return all cell aliases
    let all_result: Value = serde_json::from_str(outputs[1])?;
    assert_eq!(all_result["a"], path("fbs"));
    assert_eq!(all_result["z"], path("fbc"));
    assert_eq!(all_result["code"], path("fbc"));
    assert_eq!(all_result["source"], path("fbs"));
    Ok(())
}
