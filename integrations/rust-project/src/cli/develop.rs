/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::BufWriter;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use rustc_hash::FxHashMap;
use serde::Deserialize;
use serde::Serialize;
use tracing::info;

use super::Input;
use crate::Command;
use crate::buck;
use crate::buck::Buck;
use crate::buck::select_mode;
use crate::buck::to_project_json;
use crate::path::safe_canonicalize;
use crate::project_json::ProjectJson;
use crate::project_json::Sysroot;
use crate::sysroot::SysrootConfig;
use crate::sysroot::resolve_buckconfig_sysroot;
use crate::sysroot::resolve_rustup_sysroot;
use crate::target::Target;

#[derive(Debug)]
pub(crate) struct Develop {
    pub(crate) sysroot: SysrootConfig,
    pub(crate) buck: buck::Buck,
    pub(crate) check_cycles: bool,
    pub(crate) invoked_by_ra: bool,
    pub(crate) include_all_buildfiles: bool,
    pub(crate) rustc_target: Option<String>,
}

pub(crate) struct OutputCfg {
    out: Output,
    pretty: bool,
    max_extra_targets: usize,
}

#[derive(Debug)]
pub(crate) enum Output {
    Path(PathBuf),
    Stdout,
}

impl Develop {
    pub(crate) fn from_command(
        command: Command,
        project_root: Option<PathBuf>,
        isolation_dir: Option<String>,
    ) -> (Develop, Input, OutputCfg) {
        if let crate::Command::Develop {
            files,
            targets,
            out,
            stdout,
            prefer_rustup_managed_toolchain,
            sysroot,
            pretty,
            mode,
            check_cycles,
            buck2_command,
            include_all_buildfiles,
            max_extra_targets,
            rustc_target,
            ..
        } = command
        {
            let out = if stdout {
                Output::Stdout
            } else {
                Output::Path(out)
            };

            let sysroot = if prefer_rustup_managed_toolchain {
                SysrootConfig::Rustup
            } else if let Some(sysroot) = sysroot {
                SysrootConfig::Sysroot(sysroot)
            } else {
                SysrootConfig::BuckConfig
            };

            let mode = select_mode(mode.as_deref());
            let buck = buck::Buck::new(
                buck2_command.clone(),
                mode,
                project_root.clone(),
                isolation_dir,
            );

            let develop = Develop {
                sysroot,
                buck,
                check_cycles,
                invoked_by_ra: false,
                include_all_buildfiles,
                rustc_target,
            };
            let max_extra_targets = max_extra_targets.unwrap_or(DEFAULT_EXTRA_TARGETS);
            let out = OutputCfg {
                out,
                pretty,
                max_extra_targets,
            };

            let input = if !targets.is_empty() {
                let targets = targets.into_iter().map(Target::new).collect();
                Input::Targets(targets)
            } else {
                Input::Files(files)
            };

            return (develop, input, out);
        }

        if let crate::Command::DevelopJson {
            sysroot_mode,
            args,
            buck2_command,
            max_extra_targets,
            mode,
            rustc_target,
            ..
        } = command
        {
            let out = Output::Stdout;

            let sysroot = match sysroot_mode {
                crate::SysrootMode::BuckConfig => SysrootConfig::BuckConfig,
                crate::SysrootMode::Rustc => SysrootConfig::Rustup,
                crate::SysrootMode::FullPath(path) => SysrootConfig::Sysroot(path),
                crate::SysrootMode::Command(cmd_args) => {
                    let cmd = cmd_args[0].clone();
                    let args = cmd_args[1..].to_vec();
                    let output = std::process::Command::new(cmd).args(args).output().unwrap();
                    let path = String::from_utf8(output.stdout).unwrap();
                    SysrootConfig::Sysroot(PathBuf::from(path.trim()))
                }
            };

            let mode = select_mode(mode.as_deref());
            let buck = buck::Buck::new(buck2_command.clone(), mode, project_root, isolation_dir);

            let develop = Develop {
                sysroot,
                buck,
                check_cycles: false,
                invoked_by_ra: true,
                include_all_buildfiles: false,
                rustc_target,
            };
            let max_extra_targets = max_extra_targets.unwrap_or(DEFAULT_EXTRA_TARGETS);
            let out = OutputCfg {
                out,
                pretty: false,
                max_extra_targets,
            };

            let input = match args {
                crate::JsonArguments::Path(path) => Input::Files(vec![path]),
                crate::JsonArguments::Buildfile(path) => Input::Buildfile(vec![path]),
                crate::JsonArguments::Label(target) => Input::Targets(vec![Target::new(target)]),
            };

            return (develop, input, out);
        }

        unreachable!("No other subcommand is supported.")
    }
}

const DEFAULT_EXTRA_TARGETS: usize = 50;

/// The final rust-project.json result for the rust-analyzer discovery protocol.
///
/// <https://rust-analyzer.github.io/book/configuration.html#workspace-discovery-protocol>
#[derive(Serialize, Deserialize)]
pub(crate) struct DiscoverProjectFinished {
    pub(crate) kind: String,
    pub(crate) buildfile: PathBuf,
    pub(crate) project: ProjectJson,
}

impl Develop {
    pub(crate) fn run(self, input: Input, cfg: OutputCfg) -> Result<(), anyhow::Error> {
        let start = std::time::Instant::now();
        let input = match input {
            Input::Targets(targets) => Input::Targets(targets),
            Input::Files(files) => {
                let canonical_files = files
                    .into_iter()
                    .map(|p| safe_canonicalize(&p))
                    .collect::<Vec<_>>();

                Input::Files(canonical_files)
            }
            Input::Buildfile(buildfiles) => Input::Buildfile(buildfiles),
        };
        let mut writer: BufWriter<Box<dyn Write>> = match cfg.out {
            Output::Path(ref p) => {
                let out = std::fs::File::create(p)?;
                BufWriter::new(Box::new(out))
            }
            Output::Stdout => BufWriter::new(Box::new(std::io::stdout())),
        };

        let targets = self.related_targets(input.clone(), cfg.max_extra_targets)?;
        if targets.is_empty() {
            let err = match input {
                Input::Targets(targets) => {
                    let pretty_targets = targets
                        .iter()
                        .map(|t| format!("{}", t))
                        .collect::<Vec<_>>()
                        .join(", ");
                    anyhow::anyhow!("Could not find targets {}", pretty_targets)
                }
                Input::Files(paths) => {
                    let pretty_paths = paths
                        .iter()
                        .map(|p| format!("{}", p.display()))
                        .collect::<Vec<_>>()
                        .join(", ");
                    anyhow::anyhow!("Could not find buck targets that own {}", pretty_paths)
                }
                Input::Buildfile(paths) => {
                    let pretty_paths = paths
                        .iter()
                        .map(|p| format!("{}", p.display()))
                        .collect::<Vec<_>>()
                        .join(", ");
                    anyhow::anyhow!("Could not find any Rust targets in {}", pretty_paths)
                }
            };

            return Err(err);
        }

        if self.invoked_by_ra {
            for (buildfile, targets) in targets {
                let project = self.run_inner(targets)?;

                // we have to log before we write the output, because rust-analyzer will kill us after the write
                crate::scuba::log_develop(start.elapsed(), input.clone(), self.invoked_by_ra);

                let out = DiscoverProjectFinished {
                    buildfile,
                    project,
                    kind: "finished".to_owned(),
                };
                let out = serde_json::to_string(&out)?;
                println!("{out}");
            }
        } else {
            let mut targets = targets.into_values().flatten().collect::<Vec<_>>();
            targets.sort();
            targets.dedup();

            let project = self.run_inner(targets)?;
            crate::scuba::log_develop(start.elapsed(), input, self.invoked_by_ra);

            if cfg.pretty {
                serde_json::to_writer_pretty(&mut writer, &project)?;
            } else {
                serde_json::to_writer(&mut writer, &project)?;
            }
            writeln!(writer)?;
            match &cfg.out {
                Output::Path(p) => info!(file = ?p, "wrote rust-project.json"),
                Output::Stdout => info!("wrote rust-project.json to stdout"),
            }
        }

        Ok(())
    }

    pub(crate) fn run_inner(&self, targets: Vec<Target>) -> Result<ProjectJson, anyhow::Error> {
        let Develop {
            sysroot,
            buck,
            check_cycles,
            include_all_buildfiles,
            rustc_target,
            ..
        } = self;

        info!(kind = "progress", "finding std source code");
        let sysroot = match &sysroot {
            SysrootConfig::Sysroot(path) => Sysroot {
                sysroot: safe_canonicalize(&expand_tilde(path)?),
                sysroot_src: None,
                sysroot_project: None,
            },
            SysrootConfig::BuckConfig => {
                let project_root = buck.resolve_project_root()?;
                resolve_buckconfig_sysroot(buck, &project_root, &targets)?
            }
            SysrootConfig::Rustup => resolve_rustup_sysroot()?,
        };

        let exclude_workspaces =
            std::env::var("RUST_PROJECT_EXCLUDE_WORKSPACES").is_ok_and(|it| it != "0");

        // For first-party code, assume cfg(test) is active so people can work on tests
        // on any project in the monorepo.
        //
        // For third-party code imported with reindeer, we don't import the test-only
        // dev-dependencies specified in the Cargo.toml, so we don't want cfg(test) to be active.
        let first_party_extra_cfgs = &["test".to_owned()];

        // FIXME(JakobDegen): This should be set via a configuration mechanism of some kind.
        #[cfg(not(fbcode_build))]
        let global_extra_cfgs: &[String] = &[];
        #[cfg(fbcode_build)]
        let global_extra_cfgs = &["fbcode_build".to_owned()];

        develop_with_sysroot(
            buck,
            targets,
            sysroot,
            exclude_workspaces,
            *check_cycles,
            *include_all_buildfiles,
            global_extra_cfgs,
            first_party_extra_cfgs,
            rustc_target.as_ref(),
        )
    }

    /// For every Rust file, return the relevant buck targets that should be used to configure rust-analyzer.
    pub(crate) fn related_targets(
        &self,
        input: Input,
        max_extra_targets: usize,
    ) -> Result<FxHashMap<PathBuf, Vec<Target>>, anyhow::Error> {
        // We want to load additional targets from the enclosing buildfile, to help users
        // who have a bunch of small targets in their buildfile. However, we want to set a limit
        // so we don't try to load everything in very large generated buildfiles.

        // We always want the targets that directly own these Rust files.
        self.buck.query_owners(input, max_extra_targets)
    }
}

fn expand_tilde(path: &Path) -> Result<PathBuf, anyhow::Error> {
    if path.starts_with("~") {
        let path = path.strip_prefix("~")?;
        let home = std::env::var("HOME")?;
        let home = PathBuf::from(home);
        Ok(home.join(path))
    } else {
        Ok(path.to_path_buf())
    }
}

pub(crate) fn develop_with_sysroot(
    buck: &Buck,
    targets: Vec<Target>,
    sysroot: Sysroot,
    exclude_workspaces: bool,
    check_cycles: bool,
    include_all_buildfiles: bool,
    global_extra_cfgs: &[String],
    first_party_extra_cfgs: &[String],
    rustc_target: Option<&String>,
) -> Result<ProjectJson, anyhow::Error> {
    info!(kind = "progress", "building generated code");
    let expanded_and_resolved = buck.expand_and_resolve(&targets, exclude_workspaces)?;

    info!(kind = "progress", "resolving aliased libraries");
    let aliased_libraries =
        buck.query_aliased_libraries(&expanded_and_resolved.expanded_targets, &targets)?;

    info!(kind = "progress", "generating rust-project.json");
    let rust_project = to_project_json(
        sysroot,
        expanded_and_resolved,
        aliased_libraries,
        check_cycles,
        include_all_buildfiles,
        global_extra_cfgs,
        first_party_extra_cfgs,
        buck,
        rustc_target,
    )?;

    Ok(rust_project)
}
