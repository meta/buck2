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
async fn test_audit_deferred_materializer_list(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.audit(&["deferred-materializer", "list"]).await?;
    assert_eq!(res.stdout.trim(), "");

    buck.build(&["//:simple"]).await?;

    let res = buck.audit(&["deferred-materializer", "list"]).await?;
    assert!(res.stdout.trim().contains("__simple__"));
    Ok(())
}
