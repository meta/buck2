/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::ErrorKind;
use std::io::Write;

use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::common::ui::CommonConsoleOptions;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::final_console::FinalConsole;
use buck2_client_ctx::path_arg::PathArg;
use buck2_common::argv::Argv;
use buck2_common::argv::SanitizedArgv;
use buck2_error::BuckErrorContext;
use buck2_error::ErrorTag;
use buck2_error::buck2_error;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_path::AbsPath;
use buck2_util::process::background_command;

/// Initializes a buck2 project at the provided path.
#[derive(Debug, clap::Parser)]
#[clap(name = "init", about = "Initialize a buck2 project")]
pub struct InitCommand {
    /// The path to initialize the project in. The folder does not need to exist.
    #[clap(default_value = ".")]
    path: PathArg,

    /// Don't include the standard prelude or generate toolchain definitions.
    #[clap(long)]
    no_prelude: bool,

    /// Initialize the project even if the git repo at \[PATH\] has uncommitted changes.
    #[clap(long)]
    allow_dirty: bool,

    /// Also initialize a git repository at the given path, and set up an appropriate `.gitignore`
    /// file.
    #[clap(long)]
    git: bool,

    #[clap(flatten)]
    console_opts: CommonConsoleOptions,
}

impl InitCommand {
    pub fn exec(self, _matches: BuckArgMatches<'_>, ctx: ClientCommandContext<'_>) -> ExitResult {
        let console = self.console_opts.final_console();

        match exec_impl(self, ctx, &console) {
            Ok(_) => ExitResult::success(),
            Err(e) => {
                // include the backtrace with the error output
                // (same behaviour as returning the Error from main)
                buck2_error!(ErrorTag::Tier0, "{:?}", e).into()
            }
        }
    }

    pub fn sanitize_argv(&self, argv: Argv) -> SanitizedArgv {
        argv.no_need_to_sanitize()
    }
}

fn exec_impl(
    cmd: InitCommand,
    ctx: ClientCommandContext<'_>,
    console: &FinalConsole,
) -> buck2_error::Result<()> {
    let path = cmd.path.resolve(&ctx.working_dir);
    fs_util::create_dir_all(&path)?;
    let absolute = fs_util::canonicalize(&path).categorize_internal()?;
    let git = cmd.git;

    if absolute.is_file() {
        return Err(buck2_error!(
            buck2_error::ErrorTag::Input,
            "Target path {} cannot be an existing file",
            absolute.display()
        ));
    }

    if git {
        let status = match background_command("git")
            .args(["status", "--porcelain"])
            .current_dir(&absolute)
            .output()
        {
            Err(e) if e.kind().eq(&ErrorKind::NotFound) => {
                console.print_error(
                    "Warning: no git found on path, can't check for dirty repo. Proceeding anyway.",
                )?;
                None
            }
            r => Some(r.buck_error_context("Couldn't detect dirty status of folder.")?),
        };

        let changes = status.filter(|o| o.status.success()).map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .lines()
                .any(|l| !l.starts_with("??"))
        });

        if let (Some(true), false) = (changes, cmd.allow_dirty) {
            return Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "Refusing to initialize in a dirty repo. Stash your changes or use `--allow-dirty` to override."
            ));
        }
    }

    set_up_project(&absolute, git, !cmd.no_prelude)
}

fn initialize_buckconfig(repo_root: &AbsPath, prelude: bool, git: bool) -> buck2_error::Result<()> {
    let mut buckconfig = std::fs::File::create(repo_root.join(".buckconfig"))?;
    writeln!(buckconfig, "[cells]")?;
    writeln!(buckconfig, "  root = .")?;

    // Add additional configs that depend on prelude / no-prelude mode
    if prelude {
        writeln!(buckconfig, "  prelude = prelude")?;
        writeln!(buckconfig, "  toolchains = toolchains")?;
        writeln!(buckconfig, "  none = none")?;
        writeln!(buckconfig)?;
        writeln!(buckconfig, "[cell_aliases]")?;
        writeln!(buckconfig, "  config = prelude")?;
        writeln!(buckconfig, "  ovr_config = prelude")?;
        writeln!(buckconfig, "  fbcode = none")?;
        writeln!(buckconfig, "  fbsource = none")?;
        writeln!(buckconfig, "  fbcode_macros = none")?;
        writeln!(buckconfig, "  buck = none")?;
        writeln!(buckconfig)?;
        writeln!(
            buckconfig,
            "# Uses a copy of the prelude bundled with the buck2 binary. You can alternatively delete this"
        )?;
        writeln!(
            buckconfig,
            "# section and vendor a copy of the prelude to the `prelude` directory of your project."
        )?;
        writeln!(buckconfig, "[external_cells]")?;
        writeln!(buckconfig, "  prelude = bundled")?;
        writeln!(buckconfig)?;
        writeln!(buckconfig, "[parser]")?;
        writeln!(
            buckconfig,
            "  target_platform_detector_spec = target:root//...->prelude//platforms:default \\
    target:prelude//...->prelude//platforms:default \\
    target:toolchains//...->prelude//platforms:default"
        )?;
        writeln!(buckconfig)?;
        writeln!(buckconfig, "[build]")?;
        writeln!(
            buckconfig,
            "  execution_platforms = prelude//platforms:default"
        )?;
    }

    if git {
        writeln!(buckconfig)?;
        writeln!(buckconfig, "[project]")?;
        writeln!(buckconfig, "  ignore = .git")?;
    }
    Ok(())
}

fn initialize_toolchains_buck(repo_root: &AbsPath) -> buck2_error::Result<()> {
    std::fs::write(
        repo_root.join("BUCK"),
        r#"
load("@prelude//toolchains:demo.bzl", "system_demo_toolchains")

# All the default toolchains, suitable for a quick demo or early prototyping.
# Most real projects should copy/paste the implementation to configure them.
system_demo_toolchains()
"#
        .trim(),
    )?;
    Ok(())
}

fn initialize_root_buck(repo_root: &AbsPath, prelude: bool) -> buck2_error::Result<()> {
    let mut buck = std::fs::File::create(repo_root.join("BUCK"))?;

    if prelude {
        writeln!(
            buck,
            "# A list of available rules and their signatures can be found here: https://buck2.build/docs/prelude/globals/"
        )?;
        writeln!(buck)?;
        writeln!(buck, "genrule(")?;
        writeln!(buck, "    name = \"hello_world\",")?;
        writeln!(buck, "    out = \"out.txt\",")?;
        writeln!(buck, "    cmd = \"echo BUILT BY BUCK2> $OUT\",")?;
        writeln!(buck, ")")?;
    }
    // TODO: Add a doc pointers for rules
    Ok(())
}

fn set_up_gitignore(repo_root: &AbsPath) -> buck2_error::Result<()> {
    let gitignore = repo_root.join(".gitignore");
    // If .gitignore is empty or doesn't exist, add in buck-out
    if !gitignore.exists() || fs_util::metadata(&gitignore).categorize_internal()?.len() == 0 {
        fs_util::write(gitignore, "/buck-out\n").categorize_internal()?;
    }
    Ok(())
}

fn set_up_buckroot(repo_root: &AbsPath) -> buck2_error::Result<()> {
    fs_util::write(repo_root.join(".buckroot"), "").categorize_internal()?;
    Ok(())
}

fn set_up_project(repo_root: &AbsPath, git: bool, prelude: bool) -> buck2_error::Result<()> {
    set_up_buckroot(repo_root)?;

    if git {
        if !background_command("git")
            .arg("init")
            .current_dir(repo_root)
            .status()?
            .success()
        {
            return Err(buck2_error!(
                buck2_error::ErrorTag::Tier0,
                "Failure when running `git init`."
            ));
        };
        set_up_gitignore(repo_root)?;
    }

    // If the project already contains a .buckconfig, leave it alone
    if repo_root.join(".buckconfig").exists() {
        buck2_client_ctx::println!(
            ".buckconfig already exists, not overwriting and not generating toolchains"
        )?;
        return Ok(());
    }

    initialize_buckconfig(repo_root, prelude, git)?;
    if prelude {
        let toolchains = repo_root.join("toolchains");
        if !toolchains.exists() {
            fs_util::create_dir(&toolchains).categorize_internal()?;
            initialize_toolchains_buck(&toolchains)?;
        }
    }
    if !repo_root.join("BUCK").exists() {
        initialize_root_buck(repo_root, prelude)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use buck2_fs::fs_util::uncategorized as fs_util;
    use buck2_fs::paths::abs_path::AbsPath;

    use crate::commands::init::initialize_buckconfig;
    use crate::commands::init::initialize_root_buck;
    use crate::commands::init::set_up_gitignore;
    use crate::commands::init::set_up_project;

    #[test]
    fn test_set_up_project_with_prelude_no_git() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let tempdir_path = tempdir.path();
        let tempdir_path = AbsPath::new(tempdir_path)?;
        fs_util::create_dir_all(tempdir_path)?;

        // no git, with prelude
        set_up_project(tempdir_path, false, true)?;
        assert!(tempdir_path.join(".buckconfig").exists());
        assert!(tempdir_path.join("toolchains").exists());
        assert!(tempdir_path.join("toolchains/BUCK").exists());
        assert!(tempdir_path.join("BUCK").exists());
        Ok(())
    }

    #[test]
    fn test_default_gitignore() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let tempdir_path = tempdir.path();
        let tempdir_path = AbsPath::new(tempdir_path)?;
        fs_util::create_dir_all(tempdir_path)?;

        // .gitignore does not exist yet
        set_up_gitignore(tempdir_path)?;
        let gitignore_path = tempdir_path.join(".gitignore");
        assert!(gitignore_path.exists());
        let actual = fs_util::read_to_string(&gitignore_path)?;
        let expected = "/buck-out\n";
        assert_eq!(actual, expected);

        // If an empty .buckconfig exists (this is the case we would hit after running `git init`), add `buck-out`
        fs_util::write(&gitignore_path, "")?;
        set_up_gitignore(tempdir_path)?;
        assert!(gitignore_path.exists());
        let actual = fs_util::read_to_string(&gitignore_path)?;
        assert_eq!(actual, expected);

        // If a non-empty.buckconfig exists, don't touch it
        fs_util::write(&gitignore_path, "foo\nbar\n")?;
        set_up_gitignore(tempdir_path)?;
        assert!(gitignore_path.exists());
        let actual = fs_util::read_to_string(&gitignore_path)?;
        let expected = "foo\nbar\n";
        assert_eq!(actual, expected);
        Ok(())
    }

    #[test]
    fn test_buckconfig_generation_with_prelude() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let tempdir_path = tempdir.path();
        let tempdir_path = AbsPath::new(tempdir_path)?;
        fs_util::create_dir_all(tempdir_path)?;

        let buckconfig_path = tempdir_path.join(".buckconfig");
        initialize_buckconfig(tempdir_path, true, true)?;
        let actual_buckconfig = fs_util::read_to_string(buckconfig_path)?;
        let expected_buckconfig = "[cells]
  root = .
  prelude = prelude
  toolchains = toolchains
  none = none

[cell_aliases]
  config = prelude
  ovr_config = prelude
  fbcode = none
  fbsource = none
  fbcode_macros = none
  buck = none

# Uses a copy of the prelude bundled with the buck2 binary. You can alternatively delete this
# section and vendor a copy of the prelude to the `prelude` directory of your project.
[external_cells]
  prelude = bundled

[parser]
  target_platform_detector_spec = target:root//...->prelude//platforms:default \\
    target:prelude//...->prelude//platforms:default \\
    target:toolchains//...->prelude//platforms:default

[build]
  execution_platforms = prelude//platforms:default

[project]
  ignore = .git
";
        assert_eq!(actual_buckconfig, expected_buckconfig);
        Ok(())
    }

    #[test]
    fn test_buckconfig_generation_without_prelude() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let tempdir_path = tempdir.path();
        let tempdir_path = AbsPath::new(tempdir_path)?;
        fs_util::create_dir_all(tempdir_path)?;

        let buckconfig_path = tempdir_path.join(".buckconfig");
        initialize_buckconfig(tempdir_path, false, false)?;
        let actual_buckconfig = fs_util::read_to_string(buckconfig_path)?;
        let expected_buckconfig = "[cells]
  root = .
";
        assert_eq!(actual_buckconfig, expected_buckconfig);

        Ok(())
    }

    #[test]
    fn test_buckfile_generation_with_prelude() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let tempdir_path = tempdir.path();
        let tempdir_path = AbsPath::new(tempdir_path)?;
        fs_util::create_dir_all(tempdir_path)?;

        let buck_path = tempdir_path.join("BUCK");
        initialize_root_buck(tempdir_path, true)?;
        let actual_buck = fs_util::read_to_string(buck_path)?;
        let expected_buck = "# A list of available rules and their signatures can be found here: https://buck2.build/docs/prelude/globals/

genrule(
    name = \"hello_world\",
    out = \"out.txt\",
    cmd = \"echo BUILT BY BUCK2> $OUT\",
)
";
        assert_eq!(actual_buck, expected_buck);
        Ok(())
    }
}
