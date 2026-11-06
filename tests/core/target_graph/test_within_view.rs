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
async fn test_within_view(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.targets(&["//a/...", "--json-lines"]).await?;
    assert_eq!(res.target_list()?, ["prelude//a:a"]);
    Ok(())
}

#[buck_test]
async fn test_within_view_outside_view(buck: &Buck) {
    buck.targets(&["//b/..."])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "Target's `within_view` attribute does not allow dependency `prelude//a:a`",
        );
}

#[buck_test]
async fn test_within_view_default_outofview(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.targets(&["//default/...", "--json-lines"]).await?;
    assert_eq!(res.target_list()?, ["prelude//default:a"]);
    Ok(())
}

#[buck_test]
async fn test_within_view_default_outofview_withnone(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .targets(&["//default_withnone/none/...", "--json-lines"])
        .await?;
    assert_eq!(
        res.target_list()?,
        ["prelude//default_withnone/none:target"]
    );
    Ok(())
}

#[buck_test]
async fn test_within_view_default_outofview_withnoneselect(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .targets(&["//default_withnone/select/...", "--json-lines"])
        .await?;
    assert_eq!(
        res.target_list()?,
        ["prelude//default_withnone/select:target"]
    );
    Ok(())
}

#[buck_test]
async fn test_within_view_default_outofview_withdefault(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .targets(&["//default_withvalue/value/...", "--json-lines"])
        .await?;
    assert_eq!(
        res.target_list()?,
        ["prelude//default_withvalue/value:target"]
    );
    Ok(())
}

#[buck_test]
async fn test_within_view_default_outofview_withdefaultselect(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .targets(&["//default_withvalue/select/...", "--json-lines"])
        .await?;
    assert_eq!(
        res.target_list()?,
        ["prelude//default_withvalue/select:target"]
    );
    Ok(())
}
