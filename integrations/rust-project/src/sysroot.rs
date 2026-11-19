/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;

#[cfg(not(fbcode_build))]
use anyhow::Context;
use tracing::instrument;

use crate::buck::Buck;
use crate::buck::truncate_line_ending;
use crate::buck::utf8_output;
use crate::cli::develop_with_sysroot;
use crate::project_json::Sysroot;
use crate::target::Target;

#[derive(Debug)]
pub(crate) enum SysrootConfig {
    Sysroot(PathBuf),
    BuckConfig,
    Rustup,
}

/// Choose sysroot and sysroot_src based on platform.
///
/// `sysroot` is the directory that contains std crates:
/// <https://doc.rust-lang.org/rustc/command-line-arguments.html#--sysroot-override-the-system-root>
/// and also contains libexec helpers such as rust-analyzer-proc-macro-srv.
///
/// `sysroot_src` is the directory that contains the source to std crates:
/// <https://rust-analyzer.github.io/manual.html#non-cargo-based-projects>
#[instrument(skip_all)]
pub(crate) fn resolve_buckconfig_sysroot(
    buck: &Buck,
    project_root: &Path,
    universe_targets: &[Target],
) -> Result<Sysroot, anyhow::Error> {
    #[cfg(fbcode_build)]
    let sysroot = project_root.join("third-party/rust-toolchain/rustup/current/basic");
    #[cfg(not(fbcode_build))]
    let sysroot: PathBuf = {
        let mut sysroot_cmd = Command::new("rustc");
        sysroot_cmd
            .arg("--print=sysroot")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let sysroot_child = sysroot_cmd.spawn()?;

        let mut sysroot = utf8_output(sysroot_child.wait_with_output(), &sysroot_cmd)
            .context("error asking rustc for sysroot")?;
        truncate_line_ending(&mut sysroot);
        sysroot.into()
    };

    let (sysroot_src, sysroot_project) = match buck.resolve_sysroot_src()? {
        Some(sysroot_src) => {
            let sysroot_targets = buck.query_sysroot_targets(
                &format!("fbsource//{}:", sysroot_src.to_string_lossy()),
                universe_targets,
            );

            // the `library` path component needs to be appended to the `sysroot_src_path`
            // so that rust-analyzer will be able to find standard library sources.
            let sysroot_src = project_root.join(sysroot_src).join("library");

            let mut sysroot_project = develop_with_sysroot(
                buck,
                sysroot_targets,
                Sysroot {
                    sysroot: sysroot.clone(),
                    sysroot_src: Some(sysroot_src.clone()),
                    sysroot_project: None,
                },
                true,
                false,
                false,
                &[], // sysroot doesn't get any extra cfgs
                &[],
                None,
            )?;
            for krate in &mut sysroot_project.crates {
                if let Some(display_name) = &mut krate.display_name {
                    *display_name = display_name
                        .strip_suffix("-0.0.0") // rust-analyzer identifies lang crates by name, so we need `core-0.0.0` to be `core`
                        .unwrap_or(display_name)
                        .to_owned();
                }
            }

            (Some(sysroot_src), Some(sysroot_project))
        }
        None => (None, None),
    };

    Ok(Sysroot {
        sysroot,
        sysroot_src,
        sysroot_project,
    })
}

#[instrument(ret)]
pub(crate) fn resolve_rustup_sysroot() -> Result<Sysroot, anyhow::Error> {
    let mut cmd = Command::new("rustc");
    cmd.arg("--print=sysroot")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut output = utf8_output(cmd.output(), &cmd)?;
    truncate_line_ending(&mut output);
    let sysroot = PathBuf::from(output);
    let sysroot_src = sysroot
        .join("lib")
        .join("rustlib")
        .join("src")
        .join("rust")
        .join("library");

    let sysroot = Sysroot {
        sysroot,
        sysroot_src: Some(sysroot_src),
        sysroot_project: None, // rustup sysroot is not buckified
    };
    Ok(sysroot)
}
