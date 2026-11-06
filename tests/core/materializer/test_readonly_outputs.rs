/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! With `buck2.readonly_outputs` on, every file the materializer tracks is read-only from the
//! moment it is tracked, directories stay writable, and an incremental action's previous outputs
//! are made writable for its run and read-only again after it.

use std::io::ErrorKind;
use std::path::Path;

use buck2_e2e::prelude::*;
use buck2_e2e::utils::random_string;

fn read_only(path: &Path) -> std::io::Result<bool> {
    Ok(std::fs::metadata(path)?.permissions().readonly())
}

#[buck_test]
async fn test_tracked_files_are_read_only(buck: &Buck) -> anyhow::Result<()> {
    for target in ["//:write", "//:produce", "//:copy_src"] {
        let res = buck.build(&[target, "--local-only"]).await?;
        let out = res.build_report()?.output_for_target(target)?;
        assert!(read_only(&out)?, "{target}");
        assert!(
            !read_only(out.parent().expect("an output has a parent"))?,
            "directories stay writable"
        );
        match std::fs::write(&out, "edited") {
            Err(e) if e.kind() == ErrorKind::PermissionDenied => {}
            Err(e) => return Err(e.into()),
            Ok(()) => panic!("{target}: a write to the output went through"),
        }
    }
    Ok(())
}

#[buck_test]
async fn test_an_incremental_action_writes_its_previous_output(buck: &Buck) -> anyhow::Result<()> {
    let mut expected = "foo".to_owned();
    let mut seed = String::new();
    for _ in 0..3 {
        seed = random_string();
        let res = buck
            .run(&[
                "//:incremental",
                "--local-only",
                "-c",
                &format!("test.seed={seed}"),
            ])
            .await?;
        assert_eq!(res.stdout, expected);
        expected += " bar";
    }

    // Read-only again once the run is over.
    let res = buck
        .build(&[
            "//:incremental",
            "--local-only",
            "-c",
            &format!("test.seed={seed}"),
        ])
        .await?;
    assert!(read_only(
        &res.build_report()?.output_for_target("//:incremental")?
    )?);
    Ok(())
}

#[buck_test]
async fn test_clean_removes_read_only_outputs(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.build(&["//:produce", "--local-only"]).await?;
    let out = res.build_report()?.output_for_target("//:produce")?;
    assert!(out.exists());
    buck.clean(&[]).await?;
    assert!(!out.exists());
    let res = buck.build(&["//:produce", "--local-only"]).await?;
    assert!(read_only(
        &res.build_report()?.output_for_target("//:produce")?
    )?);
    Ok(())
}
