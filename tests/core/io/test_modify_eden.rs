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

#[buck_test(setup_eden = true)]
async fn test_modify_src_eden(buck: &Buck) -> anyhow::Result<()> {
    let path = buck.cwd().join("src.txt");

    std::fs::write(&path, "HELLO\n")?;
    let result = buck.build(&["root//:copy_file"]).await?;
    let output = result
        .build_report()?
        .output_for_target("root//:copy_file")?;
    assert_eq!(std::fs::read_to_string(output)?, "HELLO\n");

    std::fs::write(&path, "GOODBYE\n")?;
    let result = buck.build(&["root//:copy_file"]).await?;
    let output = result
        .build_report()?
        .output_for_target("root//:copy_file")?;
    assert_eq!(std::fs::read_to_string(output)?, "GOODBYE\n");
    Ok(())
}
