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
async fn test_read_root_config(buck: &Buck) -> anyhow::Result<()> {
    let output = buck.build(&["//:"]).await?;
    assert!(output.stderr.contains("<<root=regular>>"));
    assert!(output.stderr.contains("<<root_ignore_default=regular>>"));
    assert!(output.stderr.contains("<<root_use_default=predict>>"));
    assert!(output.stderr.contains("<<local=regular>>"));

    let output = buck.build(&["other//:"]).await?;
    assert!(output.stderr.contains("{{root=regular}}"));
    assert!(output.stderr.contains("{{root_ignore_default=regular}}"));
    assert!(output.stderr.contains("{{root_use_default=quantity}}"));
    assert!(output.stderr.contains("{{local=guerrilla}}"));
    Ok(())
}
