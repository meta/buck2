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
async fn test_compatible_with(buck: &Buck) -> anyhow::Result<()> {
    for good in ["root//:pass", "root//:pass2"] {
        let out = buck.cquery(&[good]).await?;
        // The first line is `<good> (<configuration>)`.
        let first_line = out.stdout.split_inclusive('\n').next().unwrap_or_default();
        assert!(
            first_line.starts_with(&format!("{good} (")) && first_line.ends_with(")\n"),
            "{}",
            out.stdout
        );
    }

    for bad in ["root//:fail", "root//:fail2"] {
        let out = buck.cquery(&[bad]).await?;
        assert_eq!(out.stdout, "");
    }
    Ok(())
}
