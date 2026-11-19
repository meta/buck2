/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Display;
use std::sync::Arc;

use buck2_core::cells::cell_path::CellPath;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_hash::BuckMutSet;

/// Argv contains the bare process argv and the "expanded" argv. The expanded argv is
/// the argv after processing flagfiles (args like @mode/opt and --flagfile mode/opt)
/// and after possibly replacing `argv[0]` with a more representative value.
#[derive(Clone)]
pub struct Argv {
    pub argv: Vec<String>,
    pub expanded_argv: ExpandedArgv,
}

#[derive(Clone)]
pub struct ExpandedArgv {
    args: Vec<(String, ExpandedArgSource)>,
}

#[derive(Clone)]
pub enum ExpandedArgSource {
    Inline,
    Flagfile(Arc<FlagfileArgSource>),
}

#[derive(Eq, PartialEq)]
pub struct FlagfileArgSource {
    pub kind: ArgFileKind,
    pub parent: Option<Arc<FlagfileArgSource>>,
}

#[derive(Clone, Debug, Eq, PartialEq, derive_more::Display)]
pub enum ArgFilePath {
    Project(CellPath),
    External(AbsNormPathBuf),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ArgFileKind {
    PythonExecutable(ArgFilePath, Option<String>),
    Path(ArgFilePath),
    Stdin,
}

impl Display for ArgFileKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArgFileKind::PythonExecutable(abs_path_buf, Some(flag)) => {
                write!(f, "@{abs_path_buf}#{flag}")
            }
            ArgFileKind::PythonExecutable(abs_path_buf, None) => write!(f, "@{abs_path_buf}"),
            ArgFileKind::Path(abs_path_buf) => write!(f, "@{abs_path_buf}"),
            ArgFileKind::Stdin => f.write_str("@-"),
        }
    }
}

impl ExpandedArgv {
    pub fn new() -> Self {
        Self::from_literals(Vec::new())
    }

    pub fn from_literals(args: Vec<String>) -> Self {
        Self {
            args: args
                .into_iter()
                .map(|v| (v, ExpandedArgSource::Inline))
                .collect::<Vec<_>>(),
        }
    }

    fn redacted(self, to_redact: &BuckMutSet<&String>) -> ExpandedArgv {
        Self {
            args: self
                .args
                .into_iter()
                .filter(|(arg, _)| !to_redact.contains(arg))
                .collect(),
        }
    }

    pub fn args(&self) -> impl Iterator<Item = &str> {
        self.args.iter().map(|(v, _)| v as _)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &ExpandedArgSource)> {
        self.args.iter().map(|(l, r)| (l as _, r))
    }
}

pub struct ExpandedArgvBuilder {
    current_argfile: Option<Arc<FlagfileArgSource>>,
    argv: ExpandedArgv,
}

impl ExpandedArgvBuilder {
    pub fn new() -> Self {
        Self {
            argv: ExpandedArgv::new(),
            current_argfile: None,
        }
    }

    pub fn replace(&mut self, idx: usize, val: String) {
        self.argv.args[idx].0 = val;
    }

    pub fn push(&mut self, next_arg: String) {
        let source = match &self.current_argfile {
            Some(argfile) => ExpandedArgSource::Flagfile(argfile.clone()),
            None => ExpandedArgSource::Inline,
        };
        self.argv.args.push((next_arg, source));
    }

    pub fn build(self) -> ExpandedArgv {
        self.argv
    }

    /// Whether `kind` is one of the argfiles whose expansion is in progress.
    pub fn is_expanding(&self, kind: &ArgFileKind) -> bool {
        let mut current = self.current_argfile.as_ref();
        while let Some(argfile) = current {
            if argfile.kind == *kind {
                return true;
            }
            current = argfile.parent.as_ref();
        }
        false
    }

    pub fn argfile_scope<R>(&mut self, kind: ArgFileKind, func: impl FnOnce(&mut Self) -> R) -> R {
        let parent = self.current_argfile.take();
        self.current_argfile = Some(Arc::new(FlagfileArgSource { kind, parent }));
        let res = func(self);
        let current = self.current_argfile.take().unwrap();
        self.current_argfile = current.parent.clone();
        res
    }
}

/// Parsed config flag value extracted from argv.
pub enum ConfigFlagValue {
    ConfigFlag(String),
    ConfigFile(String),
    Modifier(String),
    TargetPlatforms(String),
    TargetUniverse(String),
}

/// Returns the innermost project-internal flagfile source, if any.
/// External argfiles and stdin are ignored since they can't be expressed as compact references.
pub fn get_flagfile_for_logging(flagfile: &FlagfileArgSource) -> Option<&FlagfileArgSource> {
    if let Some(parent) = &flagfile.parent {
        if let Some(v) = get_flagfile_for_logging(parent) {
            return Some(v);
        }
    }
    match &flagfile.kind {
        ArgFileKind::Path(ArgFilePath::External(_))
        | ArgFileKind::PythonExecutable(ArgFilePath::External(_), _)
        | ArgFileKind::Stdin => None,
        _ => Some(flagfile),
    }
}

/// Parses config-related flags from expanded argv.
/// Returns (ConfigFlagValue, source) pairs for each recognized config flag.
pub fn parse_config_flags(
    expanded_argv: &ExpandedArgv,
) -> Vec<(ConfigFlagValue, &ExpandedArgSource)> {
    enum State {
        None,
        Matched(&'static str),
        Finished,
    }

    let mut state = State::None;
    expanded_argv
        .iter()
        .filter_map(move |(value, source)| {
            match state {
                State::None => match value {
                    "-c" | "--config" => {
                        state = State::Matched("-c");
                        None
                    }
                    "--config-file" => {
                        state = State::Matched("--config-file");
                        None
                    }
                    "-m" | "--modifier" => {
                        state = State::Matched("-m");
                        None
                    }
                    "--target-platforms" => {
                        state = State::Matched("--target-platforms");
                        None
                    }
                    "--target-universe" | "-u" => {
                        state = State::Matched("--target-universe");
                        None
                    }
                    v if v.starts_with("-m") && !v.starts_with("-m=") => Some(
                        ConfigFlagValue::Modifier(v.split_at("-m".len()).1.trim().to_owned()),
                    ),
                    v if v.starts_with("--config=") || v.starts_with("-c=") => Some(
                        ConfigFlagValue::ConfigFlag(v.split_once('=').unwrap().1.to_owned()),
                    ),
                    v if v.starts_with("-c") => Some(ConfigFlagValue::ConfigFlag(
                        v.split_at("-c".len()).1.trim().to_owned(),
                    )),
                    v if v.starts_with("--config-file=") => Some(ConfigFlagValue::ConfigFile(
                        v.split_at("--config-file=".len()).1.to_owned(),
                    )),
                    v if v.starts_with("--modifier=") || v.starts_with("-m=") => Some(
                        ConfigFlagValue::Modifier(v.split_once('=').unwrap().1.to_owned()),
                    ),
                    v if v.starts_with("--target-platforms=") => Some(
                        ConfigFlagValue::TargetPlatforms(v.split_once('=').unwrap().1.to_owned()),
                    ),
                    v if v.starts_with("--target-universe=") || v.starts_with("-u=") => Some(
                        ConfigFlagValue::TargetUniverse(v.split_once('=').unwrap().1.to_owned()),
                    ),
                    // Handle concatenated form: `-u//target` (no space or `=`)
                    v if v.starts_with("-u") && v.len() > 2 => Some(
                        ConfigFlagValue::TargetUniverse(v.split_at("-u".len()).1.trim().to_owned()),
                    ),
                    "--" => {
                        state = State::Finished;
                        None
                    }
                    _ => None,
                },
                State::Matched(flag) => {
                    state = State::None;
                    match flag {
                        "-c" => Some(ConfigFlagValue::ConfigFlag(value.to_owned())),
                        "--config-file" => Some(ConfigFlagValue::ConfigFile(value.to_owned())),
                        "-m" => Some(ConfigFlagValue::Modifier(value.to_owned())),
                        "--target-platforms" => {
                            Some(ConfigFlagValue::TargetPlatforms(value.to_owned()))
                        }
                        "--target-universe" => {
                            Some(ConfigFlagValue::TargetUniverse(value.to_owned()))
                        }
                        _ => unreachable!("impossible flag"),
                    }
                }
                State::Finished => None,
            }
            .map(|flag_value| (flag_value, source))
        })
        .collect()
}

/// Returns config flags in compact string form, collapsing argfile references.
pub fn get_representative_config_flags(expanded_argv: &ExpandedArgv) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    let mut last_flagfile: Option<&FlagfileArgSource> = None;

    for (flag_value, source) in parse_config_flags(expanded_argv) {
        let flagfile = match source {
            ExpandedArgSource::Inline => None,
            ExpandedArgSource::Flagfile(file) => get_flagfile_for_logging(file),
        };

        match flagfile {
            Some(flagfile) => {
                if Some(flagfile) != last_flagfile {
                    result.push(flagfile.kind.to_string());
                }
            }
            None => {
                let formatted = match flag_value {
                    ConfigFlagValue::ConfigFlag(v) => format!("-c {v}"),
                    ConfigFlagValue::ConfigFile(v) => format!("--config-file {v}"),
                    ConfigFlagValue::Modifier(v) => format!("-m {v}"),
                    ConfigFlagValue::TargetPlatforms(v) => format!("--target-platforms {v}"),
                    ConfigFlagValue::TargetUniverse(v) => format!("--target-universe {v}"),
                };
                result.push(formatted);
            }
        }
        last_flagfile = flagfile;
    }

    result
}

/// The "sanitized" argv is the argv and expanded argv after stripping some possibly sensitive
/// arguments. What's considered sensitive is command-specific and usually determined by an implementation
/// of `StreamingCommand::sanitize_argv`.
///
/// For example, for the run command this will strip out the arguments passed to the executed command (i.e. those after `--`).
#[derive(Clone)]
#[allow(clippy::manual_non_exhaustive)] // #[non_exhaustive] would allow this crate to create these.
pub struct SanitizedArgv {
    pub argv: Vec<String>,
    pub expanded_argv: ExpandedArgv,
    _priv: (), // Ensure that all ways of creating this are in this file.
}

impl Argv {
    pub fn no_need_to_sanitize(self) -> SanitizedArgv {
        let Argv {
            argv,
            expanded_argv,
        } = self;
        SanitizedArgv {
            argv,
            expanded_argv,
            _priv: (),
        }
    }

    pub fn redacted(self, to_redact: BuckMutSet<&String>) -> SanitizedArgv {
        SanitizedArgv {
            argv: self
                .argv
                .into_iter()
                .filter(|arg| !to_redact.contains(arg))
                .collect(),
            expanded_argv: self.expanded_argv.redacted(&to_redact),
            _priv: (),
        }
    }
}

#[cfg(test)]
mod tests {
    use buck2_core::cells::cell_path::CellPath;
    use buck2_core::fs::project::ProjectRootTemp;
    use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;

    use super::*;

    #[test]
    fn test_get_representative_config_flags() -> buck2_error::Result<()> {
        let mut argv = ExpandedArgvBuilder::new();

        argv.push("-c".to_owned());
        argv.push("section.option=value".to_owned());
        argv.push("-c section1.option=value".to_owned());
        argv.push("-csection2.option=value".to_owned());
        argv.push("--other-flag".to_owned());
        argv.push("value".to_owned());
        argv.push("--other-flag2".to_owned());
        argv.push("value".to_owned());
        argv.push("--config".to_owned());
        argv.push("section.option2=value".to_owned());
        argv.push("--config=section.option3=value".to_owned());
        argv.push("-c=section.option4=value".to_owned());
        argv.push("--config-file=//1.bcfg".to_owned());
        argv.push("--config-file".to_owned());
        argv.push("//2.bcfg".to_owned());
        argv.push("-m".to_owned());
        argv.push("//bar:baz".to_owned());
        argv.push("-m //bar1:baz".to_owned());
        argv.push("-m//bar2:baz".to_owned());
        argv.push("--modifier=//foo:bar".to_owned());
        argv.push("-m=//bar3:baz".to_owned());
        argv.push("--modifier".to_owned());
        argv.push("//bar:foo".to_owned());
        argv.push("--target-platforms=ovr_config//platforms/linux:some_linux_platform".to_owned());
        argv.push("--target-universe".to_owned());
        argv.push("//some:target".to_owned());
        argv.push("-u".to_owned());
        argv.push("//other:target".to_owned());
        argv.push("--target-universe=//third:target".to_owned());
        argv.push("-u=//fourth:target".to_owned());
        argv.push("-u//fifth:target".to_owned());

        let argv = argv.build();
        let flags = get_representative_config_flags(&argv);

        assert_eq!(
            flags,
            vec![
                "-c section.option=value",
                "-c section1.option=value",
                "-c section2.option=value",
                "-c section.option2=value",
                "-c section.option3=value",
                "-c section.option4=value",
                "--config-file //1.bcfg",
                "--config-file //2.bcfg",
                "-m //bar:baz",
                "-m //bar1:baz",
                "-m //bar2:baz",
                "-m //foo:bar",
                "-m //bar3:baz",
                "-m //bar:foo",
                "--target-platforms ovr_config//platforms/linux:some_linux_platform",
                "--target-universe //some:target",
                "--target-universe //other:target",
                "--target-universe //third:target",
                "--target-universe //fourth:target",
                "--target-universe //fifth:target",
            ]
        );
        Ok(())
    }

    #[test]
    fn test_get_representative_config_flags_for_flagfiles() -> buck2_error::Result<()> {
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
        argv.push("-m".to_owned());
        argv.push("//bar:baz".to_owned());
        argv.argfile_scope(ArgFileKind::Path(project_argfile("root//mode/1")), |argv| {
            argv.push("-c=a.b=c".to_owned());
            argv.push("-c=a.b2=c".to_owned());
            argv.push("-c=a.b3=c".to_owned());
            argv.push("--modifier=//foo:bar".to_owned());
            argv.push("--modifier".to_owned());
            argv.push("//bar:foo".to_owned());
        });
        argv.argfile_scope(ArgFileKind::Path(external_argfile("mode/1")), |argv| {
            argv.argfile_scope(ArgFileKind::Path(external_argfile("mode/2")), |argv| {
                argv.argfile_scope(ArgFileKind::Path(project_argfile("root//mode/2")), |argv| {
                    argv.push("-c=a.b4=c".to_owned());
                });
                argv.push("-c=a.b5=c".to_owned());
            });
            argv.push("-c=a.b6=c".to_owned());
        });
        argv.argfile_scope(ArgFileKind::Path(project_argfile("root//mode/3")), |argv| {
            argv.push("--other-flag".to_owned());
        });

        let argv = argv.build();
        let flags = get_representative_config_flags(&argv);

        assert_eq!(
            flags,
            vec![
                "-m //bar:baz",
                "@root//mode/1",
                "@root//mode/2",
                "-c a.b5=c",
                "-c a.b6=c"
            ]
        );
        Ok(())
    }

    #[test]
    fn test_get_representative_config_flags_stops_at_double_dash() -> buck2_error::Result<()> {
        let mut argv = ExpandedArgvBuilder::new();
        argv.push("-c".to_owned());
        argv.push("section.option=value".to_owned());
        argv.push("--config".to_owned());
        argv.push("section.option2=value".to_owned());
        argv.push("--".to_owned());
        argv.push("-c".to_owned());
        argv.push("section.ignored=value".to_owned());
        argv.push("--config".to_owned());
        argv.push("section.ignored2=value".to_owned());

        let argv = argv.build();
        let flags = get_representative_config_flags(&argv);

        assert_eq!(
            flags,
            vec!["-c section.option=value", "-c section.option2=value",]
        );
        Ok(())
    }
}
