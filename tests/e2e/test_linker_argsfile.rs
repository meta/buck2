/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::mode::Mode;
use buck2_e2e::mode::get_mode_from_platform;
use buck2_e2e::prelude::*;

#[buck_test(inplace = true)]
async fn test_linker_argsfile_valid(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build(&[
            "fbcode//buck2/tests/targets/rules/cxx/hello_world:welcome[linker.argsfile]",
            "--show-full-output",
            get_mode_from_platform(Mode::Dev),
        ])
        .await?;
    let output_dict = result.target_to_build_output()?;
    assert_eq!(output_dict.len(), 1, "{output_dict:?}");
    let output_path = output_dict.values().next().expect("one output");
    // Ensure that the argsfile exists and is not empty.
    assert!(std::path::Path::new(output_path).exists());
    assert!(std::fs::metadata(output_path)?.len() > 0);
    Ok(())
}
