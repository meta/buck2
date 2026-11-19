/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! This modules contains common options that are shared between different commands.
//! They are shared by composition together with flattening of the options.
//!
//! For example, to adopt config options, add the following field to the
//! command definition:
//!
//! ```ignore
//! #[derive(Debug, clap::Parser)]
//! struct MyCommand {
//!    #[clap(flatten)]
//!    config_opts: CommonConfigOptions,
//!    ...
//! }
//! ```

pub mod build;
pub mod profiling;
pub mod target_cfg;
pub mod timeout;
pub mod ui;

use std::path::Path;

use buck2_cli_proto::ConfigOverride;
use buck2_cli_proto::RepresentativeConfigFlag;
use buck2_cli_proto::config_override::ConfigType;
use buck2_cli_proto::representative_config_flag::Source as RepresentativeConfigFlagSource;
use buck2_common::argv::ExpandedArgSource;
use buck2_common::argv::ExpandedArgv;
use buck2_fs::paths::abs_path::AbsPath;
use buck2_fs::working_dir::AbsWorkingDir;
use dupe::Dupe;

use crate::common::profiling::BuckProfileMode;
use crate::common::ui::CommonConsoleOptions;
use crate::immediate_config::ImmediateConfigContext;
use crate::path_arg::PathArg;

pub const EVENT_LOG: &str = "event-log";
pub const NO_EVENT_LOG: &str = "no-event-log";

#[derive(Debug, buck2_error::Error)]
#[error("indices len is not equal to collection len for flag `{flag_name}`")]
#[buck2(tag = buck2_error::ErrorTag::InternalError)]
struct IndicesLengthMismatchError {
    flag_name: String,
}

#[derive(
    Debug,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Dupe,
    Copy,
    clap::ValueEnum
)]
#[clap(rename_all = "lower")]
pub enum HostPlatformOverride {
    Default,
    Linux,
    MacOs,
    Windows,
}

#[derive(
    Debug,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Dupe,
    Copy,
    clap::ValueEnum,
    Default
)]
#[clap(rename_all = "lower")]
pub enum PreemptibleWhen {
    /// (default) When another command starts that cannot run in parallel with this one, block that command.
    #[default]
    Never, // Read; "If I am Never, then never preempt me" (the default)
    /// When another command starts, interrupt this command, *even if they could run in
    /// parallel*. There is no good reason to use this other than that it provides slightly nicer
    /// superconsole output.
    Always,
    /// When another command starts that cannot run in parallel with this one,
    /// interrupt this command.
    OnDifferentState, // Read; "if a command comes in, preempt me on different state"
}

#[derive(
    Debug,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Dupe,
    Copy,
    clap::ValueEnum,
    Default
)]
#[clap(rename_all = "lower")]
pub enum ExitWhen {
    /// (default) Execute this command normally.
    #[default]
    Never,
    /// Fail this command if another command is already running with a different state.
    DifferentState,
    /// Fail this command if another command is already running (regardless of daemon state).
    NotIdle,
}

#[derive(
    Debug,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Dupe,
    Copy,
    clap::ValueEnum
)]
#[clap(rename_all = "lower")]
pub enum HostArchOverride {
    Default,
    AArch64,
    X86_64,
}

/// Defines options related to commands that involves a streaming daemon command.
#[derive(Debug, clap::Parser, serde::Serialize, serde::Deserialize, Default)]
#[clap(next_help_heading = "Event Log Options")]
pub struct CommonEventLogOptions {
    /// Write events to this log file
    #[clap(value_name = "PATH", long = EVENT_LOG)]
    pub event_log: Option<PathArg>,

    /// Do not write any event logs. Overrides --event-log. Used from `replay` to avoid recursive logging
    #[clap(long = NO_EVENT_LOG, hide = true)]
    pub no_event_log: bool,

    /// Write command invocation id into this file.
    #[clap(long, value_name = "PATH")]
    pub(crate) write_build_id: Option<PathArg>,

    /// Write the invocation record (as JSON) to this path. No guarantees whatsoever are made
    /// regarding the stability of the format.
    #[clap(long, value_name = "PATH")]
    pub(crate) unstable_write_invocation_record: Option<PathArg>,

    /// Write the command report to this path. A command report is always
    /// written to `buck-out/v2/<uuid>/command_report` even without this flag.
    #[clap(long, value_name = "PATH")]
    pub(crate) command_report_path: Option<PathArg>,
}

impl CommonEventLogOptions {
    pub fn default_ref() -> &'static Self {
        static DEFAULT: CommonEventLogOptions = CommonEventLogOptions {
            event_log: None,
            no_event_log: false,
            write_build_id: None,
            command_report_path: None,
            unstable_write_invocation_record: None,
        };
        &DEFAULT
    }

    pub fn no_event_log_ref() -> &'static Self {
        static NO_EVENT_LOG: CommonEventLogOptions = CommonEventLogOptions {
            event_log: None,
            no_event_log: true,
            write_build_id: None,
            command_report_path: None,
            unstable_write_invocation_record: None,
        };
        &NO_EVENT_LOG
    }
}

/// Defines options for config and configuration related things. Any command that involves the build
/// graph should include these options.
#[derive(Debug, clap::Parser, serde::Serialize, serde::Deserialize, Default)]
#[clap(next_help_heading = "Buckconfig Options")]
pub struct CommonBuildConfigurationOptions {
    #[clap(
        value_name = "SECTION.OPTION=VALUE",
        long = "config",
        short = 'c',
        help = "List of config options",
        // Needs to be explicitly set, otherwise will treat `-c a b c` -> [a, b, c]
        // rather than [a] and other positional arguments `b c`.
        num_args = 1
    )]
    pub config_values: Vec<String>,

    #[clap(
        value_name = "PATH",
        long = "config-file",
        help = "List of config file paths",
        num_args = 1
    )]
    pub config_files: Vec<String>,

    #[clap(long, ignore_case = true, value_name = "HOST", value_enum)]
    pub fake_host: Option<HostPlatformOverride>,

    #[clap(long, ignore_case = true, value_name = "ARCH", value_enum)]
    pub fake_arch: Option<HostArchOverride>,

    /// Value must be formatted as: version-build (e.g., 14.3.0-14C18 or 14.1-14B47b)
    #[clap(long, value_name = "VERSION-BUILD")]
    pub fake_xcode_version: Option<String>,

    /// Re-uses any `--config` values (inline or via modefiles) if there's
    /// a previous command, otherwise the flag is ignored.
    ///
    /// If there is a previous command and `--reuse-current-config` is set,
    /// then the old config is used, ignoring any overrides.
    ///
    /// If there is no previous command but the flag was set, then the flag is ignored,
    /// the command behaves as if the flag was not set at all.
    #[clap(long)]
    pub reuse_current_config: bool,

    /// Used to configure when this command could be preempted by another command for the same isolation dir.
    ///
    /// Normally, when you run two commands - from different terminals, say - buck2 will attempt
    /// to run them in parallel. However, if the two commands are based on different state, that
    /// is they either have different configs or different filesystem states, buck2 cannot run them
    /// in parallel. The default behavior in this case is to block the second command until the
    /// first completes.
    #[clap(long, ignore_case = true, value_enum)]
    pub preemptible: Option<PreemptibleWhen>,
    /// Whether to proceed with or fail this invocation based on the daemon state.
    #[clap(long, ignore_case = true, value_enum)]
    pub exit_when: Option<ExitWhen>,
}

impl CommonBuildConfigurationOptions {
    /// Produces a single, ordered list of config overrides. A `ConfigOverride`
    /// represents either a file, passed via `--config-file`, or a config value,
    /// passed via `-c`/`--config`. The relative order of those are important,
    /// hence they're merged into a single list.
    pub fn config_overrides(
        &self,
        matches: BuckArgMatches<'_>,
        immediate_ctx: &ImmediateConfigContext<'_>,
        cwd: &AbsWorkingDir,
    ) -> buck2_error::Result<Vec<ConfigOverride>> {
        fn with_indices<'a, T>(
            collection: &'a [T],
            name: &str,
            matches: BuckArgMatches<'a>,
        ) -> buck2_error::Result<impl Iterator<Item = (usize, &'a T)> + use<'a, T>> {
            let indices: Vec<usize> = if collection.is_empty() {
                Vec::new()
            } else {
                let indices = matches.inner.indices_of(name);
                let indices = indices.unwrap_or_default();
                indices.into_iter().collect()
            };
            if indices.len() != collection.len() {
                return Err(buck2_error::Error::from(IndicesLengthMismatchError {
                    flag_name: name.to_owned(),
                }));
            }
            Ok(indices.into_iter().zip(collection))
        }

        let config_values_args = with_indices(&self.config_values, "config_values", matches)?
            .map(|(index, config_value)| {
                let (cell, raw_arg) = match config_value.split_once("//") {
                    Some((cell, val)) if !cell.contains('=') => {
                        let cell = immediate_ctx
                            .resolve_alias_to_path_in_cwd(cell)?
                            .to_string();
                        (Some(cell), val)
                    }
                    _ => (None, config_value.as_str()),
                };

                buck2_error::Ok((
                    index,
                    ConfigOverride {
                        cell,
                        config_override: raw_arg.to_owned(),
                        config_type: ConfigType::Value as i32,
                    },
                ))
            })
            .collect::<buck2_error::Result<Vec<_>>>()?;

        let config_file_args = with_indices(&self.config_files, "config_files", matches)?
            .map(|(index, file)| {
                let (cell, path) = match file.split_once("//") {
                    Some((cell, val)) => {
                        // This should also reject =?
                        let cell = immediate_ctx
                            .resolve_alias_to_path_in_cwd(cell)?
                            .to_string();
                        (Some(cell), val.to_owned())
                    }
                    None => {
                        let abs_path = match AbsPath::new(file) {
                            Ok(p) => p.to_owned(),
                            Err(_) => cwd.resolve(Path::new(file)),
                        };
                        (None, abs_path.to_string())
                    }
                };
                Ok((
                    index,
                    ConfigOverride {
                        cell,
                        config_override: path,
                        config_type: ConfigType::File as i32,
                    },
                ))
            })
            .collect::<buck2_error::Result<Vec<_>>>()?;

        let mut ordered_merged_configs: Vec<(usize, ConfigOverride)> = config_file_args;
        ordered_merged_configs.extend(config_values_args);
        ordered_merged_configs.sort_by_key(|(lhs_index, _)| *lhs_index);

        Ok(ordered_merged_configs
            .into_iter()
            .map(|(_, config_arg)| config_arg)
            .collect::<Vec<_>>())
    }

    pub fn host_platform_override(&self) -> HostPlatformOverride {
        match &self.fake_host {
            Some(v) => *v,
            None => HostPlatformOverride::Default,
        }
    }
    pub fn host_arch_override(&self) -> HostArchOverride {
        match (&self.fake_arch, &self.fake_host) {
            (Some(v), _) => *v,
            (None, Some(HostPlatformOverride::MacOs)) => HostArchOverride::AArch64,
            (None, _) => HostArchOverride::Default,
        }
    }
    pub fn host_xcode_version_override(&self) -> Option<String> {
        self.fake_xcode_version.to_owned()
    }

    pub fn default_ref() -> &'static Self {
        static DEFAULT: CommonBuildConfigurationOptions = CommonBuildConfigurationOptions {
            config_values: vec![],
            config_files: vec![],
            fake_host: None,
            fake_arch: None,
            fake_xcode_version: None,
            reuse_current_config: false,
            preemptible: Some(PreemptibleWhen::Never),
            exit_when: None,
        };
        &DEFAULT
    }

    pub fn reuse_current_config_and_preemptible_ref() -> &'static Self {
        static OPTS: CommonBuildConfigurationOptions = CommonBuildConfigurationOptions {
            config_values: vec![],
            config_files: vec![],
            fake_host: None,
            fake_arch: None,
            fake_xcode_version: None,
            reuse_current_config: true,
            preemptible: Some(PreemptibleWhen::OnDifferentState),
            exit_when: None,
        };
        &OPTS
    }
}

#[derive(Debug, clap::Parser, serde::Serialize, serde::Deserialize, Default)]
#[clap(next_help_heading = "Starlark Options")]
pub struct CommonStarlarkOptions {
    /// Disable runtime type checking in Starlark interpreter.
    ///
    /// This option is not stable, and can be used only locally
    /// to diagnose evaluation performance problems.
    #[clap(long)]
    pub disable_starlark_types: bool,

    /// Typecheck bzl and bxl files during evaluation.
    #[clap(long, hide = true)]
    pub unstable_typecheck: bool,

    /// Record or show target call stacks.
    ///
    /// Starlark call stacks will be included in duplicate targets error.
    ///
    /// If a command outputs targets (like `targets` command),
    /// starlark call stacks will be printed after the targets.
    #[clap(long = "stack")]
    pub target_call_stacks: bool,

    /// If there are targets with duplicate names in `BUCK` file,
    /// skip all the duplicates but the first one.
    /// This is a hack for TD. Do not use this option.
    #[clap(long, hide = true)]
    pub(crate) skip_targets_with_duplicate_names: bool,

    /// Enables profiling for all evaluations whose evaluation identifier matches one of the provided patterns.
    ///
    /// Some examples identifiers:
    ///    analysis/cell//buck2/app/buck2_action_impl:buck2_action_impl (cfg:linux-x86_64#27ac5723e0c99706)
    ///    load/cell//build_defs/json.bzl
    ///    load/prelude//playground/test.bxl
    ///    load/cell//build_defs/json.bzl@other_cell
    ///    load_buildfile/fbcode//third-party-buck/platform010/build/ncurses
    ///    load_packagefile/fbcode//cli/rust/cli_delegate
    ///    anon_analysis/anon//:_anon_link_rule (anon: 766183dc9b6f680a) (fbcode//buck2/platform/execution:linux-x86_64#08961b14cfb182aa)
    ///    bxl/prelude//playground/test.bxl:playground
    ///
    /// You can pass `--profile-patterns=.*` to enable no-op profiling for everything (additionally pass `--profile-patterns-mode=none` to
    /// use no-op profiling to just get a list of all the identifiers).
    ///
    /// The profile results will be written to individual .profile files in `<ROOT_OUTPUT>/<data+time>-<uuid>/` where ROOT_OUTPUT comes from
    /// the --profile-patterns-output flag. In that directory there will also be a file listing all the identifiers that were profiled.
    ///
    /// Enabling/disabling profiling of an evaluation will invalidate the results of that evaluation and it will be recomputed. In some
    /// cases, this will cause other work to also need to be redone (for example, invalidating the result of loading PACKAGE files
    /// causes all consumers to be recomputed). But if you keep profiling options consistent between commands, only the work that is
    /// otherwise invalidated will be redone (and only for those would profiling results be created).
    ///
    /// You must also pass --profile-patterns-mode and --profile-patterns-output.
    #[clap(
        long,
        requires = "profile_patterns_output",
        requires = "profile_patterns_mode"
    )]
    pub(crate) profile_patterns: Option<Vec<String>>,

    #[clap(long, value_name = "PATH")]
    profile_patterns_output: Option<PathArg>,

    /// Profile mode.
    ///
    /// Memory profiling modes have suffixes either `-allocated` or `-retained`.
    ///
    /// `-retained` means memory kept in frozen starlark heaps after analysis completes.
    /// `-retained` does not work when profiling loading,
    /// because no memory is retained after loading and frozen heap is not even created.
    /// This is probably what you want when profiling analysis.
    ///
    /// `-allocated` means allocated memory, including memory which is later garbage collected.
    #[clap(long, value_enum)]
    profile_patterns_mode: Option<BuckProfileMode>,
}

impl CommonStarlarkOptions {
    pub fn default_ref() -> &'static Self {
        static DEFAULT: CommonStarlarkOptions = CommonStarlarkOptions {
            disable_starlark_types: false,
            unstable_typecheck: false,
            target_call_stacks: false,
            skip_targets_with_duplicate_names: false,
            profile_patterns: None,
            profile_patterns_output: None,
            profile_patterns_mode: None,
        };
        &DEFAULT
    }

    pub(crate) fn profile_pattern_opts(
        &self,
        working_dir: &AbsWorkingDir,
    ) -> Option<buck2_cli_proto::client_context::ProfilePatternOptions> {
        self.profile_patterns.as_ref().map(|v| {
            buck2_cli_proto::client_context::ProfilePatternOptions {
                profile_patterns: v.clone(),
                profile_mode: self.profile_patterns_mode.as_ref().unwrap().to_proto() as i32,
                profile_output: self
                    .profile_patterns_output
                    .as_ref()
                    .unwrap()
                    .resolve(working_dir)
                    .to_string(),
            }
        })
    }
}

/// Common options for commands like `build` or `query`.
/// Not all the commands have all the options.
#[derive(Debug, clap::Parser, serde::Serialize, serde::Deserialize, Default)]
pub struct CommonCommandOptions {
    /// Buckconfig and similar options.
    #[clap(flatten)]
    pub config_opts: CommonBuildConfigurationOptions,

    /// Starlark options.
    #[clap(flatten)]
    pub starlark_opts: CommonStarlarkOptions,

    /// UI options.
    #[clap(flatten)]
    pub console_opts: CommonConsoleOptions,

    /// Event-log options.
    #[clap(flatten)]
    pub event_log_opts: CommonEventLogOptions,
}

#[derive(Debug, PartialEq)]
pub enum PrintOutputsFormat {
    Plain,
    Simple,
    Json,
}

#[derive(Clone, Copy)]
pub struct BuckArgMatches<'a> {
    inner: &'a clap::ArgMatches,
    expanded_argv: &'a ExpandedArgv,
}

impl<'a> BuckArgMatches<'a> {
    pub fn from_clap(inner: &'a clap::ArgMatches, expanded_argv: &'a ExpandedArgv) -> Self {
        Self {
            inner,
            expanded_argv,
        }
    }

    pub fn unwrap_subcommand(&self) -> Self {
        match self.inner.subcommand().map(|s| s.1) {
            Some(submatches) => Self {
                inner: submatches,
                expanded_argv: self.expanded_argv,
            },
            None => panic!("Parsed a subcommand but couldn't extract subcommand argument matches"),
        }
    }

    pub fn get_representative_config_flags(&self) -> Vec<String> {
        buck2_common::argv::get_representative_config_flags(self.expanded_argv)
    }

    pub fn get_representative_config_flags_by_source(&self) -> Vec<RepresentativeConfigFlag> {
        use buck2_common::argv::ConfigFlagValue;
        use buck2_common::argv::get_flagfile_for_logging;
        use buck2_common::argv::parse_config_flags;

        let mut args: Vec<RepresentativeConfigFlag> = Vec::new();
        let mut last_flagfile = None;

        for (flag_value, source) in parse_config_flags(self.expanded_argv) {
            let flagfile = match source {
                ExpandedArgSource::Inline => None,
                ExpandedArgSource::Flagfile(file) => get_flagfile_for_logging(file),
            };

            match flagfile {
                Some(flagfile) => {
                    if Some(flagfile) != last_flagfile {
                        args.push(RepresentativeConfigFlag {
                            source: Some(RepresentativeConfigFlagSource::ModeFile(
                                flagfile.kind.to_string(),
                            )),
                        });
                    }
                }
                None => {
                    let source = match flag_value {
                        ConfigFlagValue::ConfigFlag(v) => {
                            RepresentativeConfigFlagSource::ConfigFlag(v)
                        }
                        ConfigFlagValue::ConfigFile(v) => {
                            RepresentativeConfigFlagSource::ConfigFile(v)
                        }
                        ConfigFlagValue::Modifier(v) => RepresentativeConfigFlagSource::Modifier(v),
                        ConfigFlagValue::TargetPlatforms(v) => {
                            RepresentativeConfigFlagSource::TargetPlatforms(v)
                        }
                        ConfigFlagValue::TargetUniverse(v) => {
                            RepresentativeConfigFlagSource::TargetUniverse(v)
                        }
                    };
                    args.push(RepresentativeConfigFlag {
                        source: Some(source),
                    });
                }
            }
            last_flagfile = flagfile;
        }

        args
    }
}

#[cfg(test)]
mod tests {
    use buck2_cli_proto::RepresentativeConfigFlag;
    use buck2_cli_proto::representative_config_flag::Source as RepresentativeConfigFlagSource;
    use buck2_common::argv::ArgFileKind;
    use buck2_common::argv::ArgFilePath;
    use buck2_common::argv::ExpandedArgvBuilder;
    use buck2_core::cells::cell_path::CellPath;
    use buck2_core::fs::project::ProjectRootTemp;
    use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;

    use super::*;

    fn source(flag: RepresentativeConfigFlagSource) -> RepresentativeConfigFlag {
        RepresentativeConfigFlag { source: Some(flag) }
    }

    #[test]
    fn test_by_source_inline_flags() -> buck2_error::Result<()> {
        let mut argv = ExpandedArgvBuilder::new();
        argv.push("-c".to_owned());
        argv.push("section.key=val".to_owned());
        argv.push("-m".to_owned());
        argv.push("//mod:bar".to_owned());
        argv.push("--config-file".to_owned());
        argv.push("//cfg.bcfg".to_owned());
        argv.push("--target-platforms=ovr//p:linux".to_owned());
        argv.push("--target-universe".to_owned());
        argv.push("//uni:target".to_owned());

        let argv = argv.build();
        let clap = clap::ArgMatches::default();
        let matches = BuckArgMatches::from_clap(&clap, &argv);
        let flags = matches.get_representative_config_flags_by_source();

        assert_eq!(
            flags,
            vec![
                source(RepresentativeConfigFlagSource::ConfigFlag(
                    "section.key=val".to_owned()
                )),
                source(RepresentativeConfigFlagSource::Modifier(
                    "//mod:bar".to_owned()
                )),
                source(RepresentativeConfigFlagSource::ConfigFile(
                    "//cfg.bcfg".to_owned()
                )),
                source(RepresentativeConfigFlagSource::TargetPlatforms(
                    "ovr//p:linux".to_owned()
                )),
                source(RepresentativeConfigFlagSource::TargetUniverse(
                    "//uni:target".to_owned()
                )),
            ]
        );
        Ok(())
    }

    #[test]
    fn test_by_source_argfile_collapsing() -> buck2_error::Result<()> {
        let project_argfile = |path: &str| ArgFilePath::Project(CellPath::testing_new(path));
        let external_root = ProjectRootTemp::new().unwrap();
        let external_root = external_root.path();
        let external_argfile = |path: &str| {
            ArgFilePath::External(
                external_root
                    .root()
                    .join(ForwardRelativePathBuf::new(path.to_owned()).unwrap()),
            )
        };

        let mut argv = ExpandedArgvBuilder::new();
        argv.push("-c".to_owned());
        argv.push("inline.key=val".to_owned());

        argv.argfile_scope(
            ArgFileKind::Path(project_argfile("root//mode/dev")),
            |argv| {
                argv.push("-c=from.mode=1".to_owned());
                argv.push("-m".to_owned());
                argv.push("//mod:x".to_owned());
            },
        );

        argv.argfile_scope(ArgFileKind::Path(external_argfile("ext/mode")), |argv| {
            argv.push("-c=external.key=val".to_owned());
        });

        let argv = argv.build();
        let clap = clap::ArgMatches::default();
        let matches = BuckArgMatches::from_clap(&clap, &argv);
        let flags = matches.get_representative_config_flags_by_source();

        assert_eq!(
            flags,
            vec![
                source(RepresentativeConfigFlagSource::ConfigFlag(
                    "inline.key=val".to_owned()
                )),
                source(RepresentativeConfigFlagSource::ModeFile(
                    "@root//mode/dev".to_owned()
                )),
                source(RepresentativeConfigFlagSource::ConfigFlag(
                    "external.key=val".to_owned()
                )),
            ]
        );
        Ok(())
    }

    #[test]
    fn test_config_overrides_with_default_opts_and_unregistered_args() -> buck2_error::Result<()> {
        use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
        use buck2_fs::working_dir::AbsWorkingDir;

        use crate::immediate_config::ImmediateConfigContext;

        let cwd = if cfg!(windows) {
            AbsWorkingDir::unchecked_new(AbsNormPathBuf::new("C:\\tmp".into()).unwrap())
        } else {
            AbsWorkingDir::unchecked_new(AbsNormPathBuf::new("/tmp".into()).unwrap())
        };
        let immediate_ctx = ImmediateConfigContext::new(&cwd);

        let opts = CommonBuildConfigurationOptions::default();
        let argv = ExpandedArgvBuilder::new().build();
        let clap = clap::ArgMatches::default();
        let matches = BuckArgMatches::from_clap(&clap, &argv);

        let overrides = opts.config_overrides(matches, &immediate_ctx, &cwd)?;
        assert!(overrides.is_empty());
        Ok(())
    }
}
