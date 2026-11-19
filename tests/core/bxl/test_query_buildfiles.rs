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
async fn test_uquery_allbuildfiles(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//:query_buildfiles.bxl:uquery_allbuildfiles"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_uquery_rbuildfiles(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//:query_buildfiles.bxl:uquery_rbuildfiles"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_cquery_allbuildfiles(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//:query_buildfiles.bxl:cquery_allbuildfiles"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_cquery_rbuildfiles(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//:query_buildfiles.bxl:cquery_rbuildfiles"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_lazy_uquery_allbuildfiles(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//:query_buildfiles.bxl:lazy_uquery_allbuildfiles"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_lazy_uquery_rbuildfiles(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//:query_buildfiles.bxl:lazy_uquery_rbuildfiles"])
        .await?;
    Ok(())
}
