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

#[buck_test]
async fn test_sub_packages(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.targets(&["root//:"]).await?;
    assert!(res.stderr.contains(r#"Pkgs: ["cat/x", "dog"]"#));
    let res = buck.targets(&["root//dog:"]).await?;
    assert!(res.stderr.contains(r#"Pkgs: ["y"]"#));
    let res = buck.targets(&["root//cat/x:"]).await?;
    assert!(res.stderr.contains("Pkgs: []"));
    Ok(())
}
