/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io;
use std::path::Path;

use crate::bundled_shell;
use crate::extract_from_outputs;
use crate::runtime::ZshRuntime;

pub(crate) fn run_zsh(
    completion_name: &str,
    script: &str,
    input: &str,
    tempdir: &Path,
) -> io::Result<Vec<String>> {
    let home = tempdir;

    // A couple of locals need to be set based on the zsh install dir, if we installed into
    // a tempdir as we do on Linux
    let extra_fpath;
    let extra_mpath;
    if let Some(base_path) = bundled_shell("zsh") {
        let version_dir = base_path.join("usr/lib64/zsh");
        let version = std::fs::read_dir(version_dir)?
            .next()
            .unwrap()?
            .file_name()
            .to_str()
            .unwrap()
            .to_owned();
        let base_path = base_path.to_str().unwrap();
        extra_fpath = format!("{base_path}/usr/share/zsh/{version}/functions");
        extra_mpath = format!("module_path={base_path}/usr/lib64/zsh/{version}");
    } else {
        extra_fpath = String::new();
        extra_mpath = String::new();
    }

    // Copy and paste of `ZshRuntime::new` which works around a zsh bug in which completions are not
    // autoloaded completely
    let config_path = home.join(".zshenv");
    let config = format!(
        "\
fpath=($ZDOTDIR/zsh {extra_fpath} $fpath)
{extra_mpath}
autoload -U +X compinit && compinit -u # bypass compaudit security checking
precmd_functions=\"\"  # avoid the prompt being overwritten
PS1='%% '
PROMPT='%% '
_{completion_name} >/dev/null 2>/dev/null ; # Force the completion to be loaded
"
    );
    std::fs::write(config_path, config)?;

    let mut r = ZshRuntime::with_home(home.to_owned())?;
    r.register(completion_name, script)?;

    extract_from_outputs(
        input,
        std::iter::empty()
            .chain(std::iter::once_with(|| r.complete(&format!("{input}\t"))))
            .chain(std::iter::once_with(|| r.complete(&format!("{input}\t\t")))),
    )
}
