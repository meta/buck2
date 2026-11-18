/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::OnceCell;
use std::fs::File;
use std::io;
use std::io::BufRead;
use std::path::Path;
use std::process::Command;
use std::str;

use buck2_common::argv::ArgFileKind;
use buck2_common::argv::ArgFilePath;
use buck2_common::argv::ExpandedArgv;
use buck2_common::argv::ExpandedArgvBuilder;
use buck2_core::is_open_source;
use buck2_error::BuckErrorContext;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::abs_path::AbsPath;
use buck2_fs::working_dir::AbsWorkingDir;
use buck2_util::process::background_command;
use termwiz::istty::IsTty;

use crate::immediate_config::ImmediateConfigContext;

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Input)]
enum ArgExpansionError {
    #[error("Missing flag file path after --flagfile argument")]
    MissingFlagFilePath,
    #[error("Unable to read flag file at `{path}`")]
    MissingFlagFileOnDisk { path: String },
    #[error("Unable to read flag file at `{path}`")]
    MissingFlagFileOnDiskWithSource {
        source: buck2_error::Error,
        path: String,
    },
    #[error("Unable to read line in flag file `{path}`")]
    FlagFileReadError {
        source: buck2_error::Error,
        path: String,
    },
    #[error("Python mode file `{path}` output is not UTF-8")]
    PythonOutputNotUtf8 { path: String },
    #[error("Flag file `{path}` includes itself")]
    CyclicInclusion { path: String },
    #[error("No flag file path after @ symbol in argfile argument")]
    MissingFlagFilePathInArgfile,
    #[error("Python argfile at `{path}` exited with non-zero status, stderr: {err:?}")]
    PythonExecutableFailed { path: String, err: String },
    #[error("Python argfile command ({cmd:?}) execution failed")]
    PythonExecutionFailed { source: io::Error, cmd: Command },
    #[error("Unable to read line from stdin")]
    StdinReadError { source: buck2_error::Error },
}

/// Log that a relative flag file was not found in CWD, but was found, and used, from the cell root
///
/// This prints directly to stderr (sometimes in color). This should be safe, because flagfile
/// expansion runs *very* early in the CLI process lifetime.
pub fn log_relative_path_from_cell_root(requested_path: &str) -> buck2_error::Result<()> {
    let (prefix, reset) = if io::stderr().is_tty() {
        ("\x1b[33m", "\x1b[0m")
    } else {
        ("WARNING: ", "")
    };
    crate::eprintln!(
        "{}`@{}` was specified, but not found. Using file at `//{}`.{}",
        prefix,
        requested_path,
        requested_path,
        reset
    )?;
    Ok(())
}

/// The arguments read from the `@-` argfile.
///
/// Stdin can be read only once, so the lines are kept: a command that is restarted in this
/// process expands its arguments again, and gets the ones it read the first time.
#[derive(Default)]
pub struct StdinArgfile(OnceCell<Vec<String>>);

impl StdinArgfile {
    fn lines(&self) -> buck2_error::Result<&[String]> {
        if let Some(lines) = self.0.get() {
            return Ok(lines);
        }
        let lines = io::stdin()
            .lock()
            .lines()
            .filter_map(|line| match line {
                Ok(x) if x.is_empty() => None,
                Ok(x) => Some(Ok(x)),
                Err(err) => Some(Err(ArgExpansionError::StdinReadError {
                    source: err.into(),
                }
                .into())),
            })
            .collect::<buck2_error::Result<Vec<String>>>()?;
        Ok(self.0.get_or_init(|| lines))
    }
}

// Expands any argfiles passed as command line parameters. There are
// two ways to do: `@argfile` or `--flagfile PATH`.
//
// Caveats:
//  - `--` and `--flagfile` cannot be values of other options
//  - `--flagfile=X` is _not_ supported, you need to pass
//    `--flagfile X` instead.
//  - `--flagfil` is _not_ supported.
//
// TODO: This function should also return tracking information, so
//       that we know where args come from. This would be useful
//       in cases where the argfiles contain `--config` flags.
pub fn expand_argv(
    arg0_override: Option<&str>,
    args: Vec<String>,
    context: &mut ImmediateConfigContext,
    cwd: &AbsWorkingDir,
    stdin: &StdinArgfile,
) -> buck2_error::Result<ExpandedArgv> {
    let mut expanded_args = ExpandedArgvBuilder::new();
    expand_argfiles_with_context(&mut expanded_args, args, context, cwd, stdin)?;

    // Override arg0 in `buck2 help`.
    if let Some(arg0) = arg0_override {
        expanded_args.replace(0, arg0.to_owned());
    }

    Ok(expanded_args.build())
}

fn expand_argfiles_with_context(
    expanded_args: &mut ExpandedArgvBuilder,
    args: Vec<String>,
    context: &mut ImmediateConfigContext,
    cwd: &AbsWorkingDir,
    stdin: &StdinArgfile,
) -> buck2_error::Result<()> {
    let mut arg_iterator = args.into_iter();

    while let Some(next_arg) = arg_iterator.next() {
        match next_arg.as_str() {
            "--" => {
                expanded_args.push(next_arg);
                for arg in arg_iterator {
                    expanded_args.push(arg);
                }
                break;
            }
            "--flagfile" => {
                let flagfile = match arg_iterator.next() {
                    Some(val) => val,
                    None => {
                        return Err(ArgExpansionError::MissingFlagFilePath.into());
                    }
                };
                resolve_and_expand_argfile(expanded_args, &flagfile, context, cwd, stdin)?;
            }
            next_arg if next_arg.starts_with('@') => {
                let flagfile = next_arg.strip_prefix('@').unwrap();
                if flagfile.is_empty() {
                    return Err(ArgExpansionError::MissingFlagFilePathInArgfile.into());
                }
                resolve_and_expand_argfile(expanded_args, flagfile, context, cwd, stdin)?;
            }
            comment if comment.starts_with("--#") => {}
            _ => expanded_args.push(next_arg),
        }
    }

    Ok(())
}

// Resolves a path argument to an absolute path, reads the flag file and expands
// it into a list of arguments.
fn resolve_and_expand_argfile(
    expanded: &mut ExpandedArgvBuilder,
    path: &str,
    context: &mut ImmediateConfigContext,
    cwd: &AbsWorkingDir,
    stdin: &StdinArgfile,
) -> buck2_error::Result<()> {
    let flagfile = resolve_flagfile(path, context, cwd)
        .with_buck_error_context(|| format!("Error resolving flagfile `{path}`"))?;
    if expanded.is_expanding(&flagfile) {
        return Err(ArgExpansionError::CyclicInclusion {
            path: flagfile.to_string(),
        }
        .into());
    }
    let flagfile_lines = expand_argfile_contents(context, &flagfile, stdin)?;
    expanded.argfile_scope(flagfile, |expanded| {
        expand_argfiles_with_context(expanded, flagfile_lines, context, cwd, stdin)
    })
}

fn argfile_abs_path(
    context: &ImmediateConfigContext,
    path: &ArgFilePath,
) -> buck2_error::Result<AbsNormPathBuf> {
    match path {
        ArgFilePath::Project(path) => context.resolve_project_path(*path),
        ArgFilePath::External(path) => Ok(path.clone()),
    }
}

fn expand_argfile_contents(
    context: &ImmediateConfigContext,
    flagfile: &ArgFileKind,
    stdin: &StdinArgfile,
) -> buck2_error::Result<Vec<String>> {
    match flagfile {
        ArgFileKind::Path(path) => {
            let mut lines = Vec::new();
            let file = File::open(argfile_abs_path(context, path)?).map_err(|source| {
                ArgExpansionError::MissingFlagFileOnDiskWithSource {
                    source: source.into(),
                    path: path.to_string(),
                }
            })?;
            let reader = io::BufReader::new(file);
            for line_result in reader.lines() {
                let line = line_result.map_err(|source| ArgExpansionError::FlagFileReadError {
                    source: source.into(),
                    path: path.to_string(),
                })?;
                if line.is_empty() {
                    continue;
                }
                lines.push(line);
            }
            Ok(lines)
        }
        ArgFileKind::PythonExecutable(path, flag) => {
            let mut cmd = background_command(if is_open_source() {
                "python3"
            } else {
                "fbpython"
            });
            cmd.env("BUCK2_ARG_FILE", "1");
            cmd.arg(argfile_abs_path(context, path)?.as_os_str());
            if let Some(flag) = flag.as_deref() {
                cmd.args(["--flavors", flag]);
            }
            let cmd_out = cmd
                .output()
                .map_err(|source| ArgExpansionError::PythonExecutionFailed { cmd, source })?;
            if cmd_out.status.success() {
                Ok(str::from_utf8(&cmd_out.stdout)
                    .map_err(|_| ArgExpansionError::PythonOutputNotUtf8 {
                        path: path.to_string(),
                    })?
                    .lines()
                    .filter(|line| !line.is_empty())
                    .map(|s| s.to_owned())
                    .collect::<Vec<String>>())
            } else {
                Err(ArgExpansionError::PythonExecutableFailed {
                    path: path.to_string(),
                    err: String::from_utf8_lossy(&cmd_out.stderr).to_string(),
                }
                .into())
            }
        }
        ArgFileKind::Stdin => Ok(stdin.lines()?.to_vec()),
    }
}

// Resolves a path argument to an absolute path, so that it can be read.
fn resolve_flagfile(
    path: &str,
    context: &mut ImmediateConfigContext,
    cwd: &AbsWorkingDir,
) -> buck2_error::Result<ArgFileKind> {
    if path == "-" {
        return Ok(ArgFileKind::Stdin);
    }

    let (path_part, flag) = match path.split_once('#') {
        Some((pypath, pyflag)) => (pypath, Some(pyflag)),
        None => (path, None),
    };

    let resolved_path = match path_part.split_once("//") {
        Some((cell_alias, cell_relative_path)) => context
            .resolve_cell_path(cell_alias, cell_relative_path)
            .buck_error_context("Error resolving cell path")?
            .into_abs_path_buf(),
        None => {
            let p = Path::new(path_part);
            match AbsPath::new(path_part) {
                Ok(abs_path) => {
                    // FIXME(JakobDegen): Checks for normalization for historical reasons, not sure
                    // why we'd want that
                    AbsNormPathBuf::new(abs_path.as_path().to_path_buf())?.into_abs_path_buf()
                }
                Err(_) => {
                    let abs_p = cwd.resolve(p);
                    if fs_util::try_exists(&abs_p)? {
                        abs_p
                    } else {
                        let cell_relative_path = context.resolve_cell_path("", path_part)?;
                        // If the relative path does not exist relative to the cwd,
                        // attempt to make it relative to the cell root. If *that*
                        // doesn't exist, just report the original error back, and
                        // don't tip users off that they can use relative-to-cell paths.
                        // We want to deprecate that.
                        match fs_util::try_exists(&cell_relative_path) {
                            Ok(true) => {
                                log_relative_path_from_cell_root(path_part)?;
                                cell_relative_path.into_abs_path_buf()
                            }
                            _ => {
                                return Err(ArgExpansionError::MissingFlagFileOnDisk {
                                    path: p.to_path_buf().to_string_lossy().into_owned(),
                                }
                                .into());
                            }
                        }
                    }
                }
            }
        }
    };

    // FIXME(JakobDegen): Don't canonicalize
    // input path from --flagfile/@argfile (errors on path missing)
    let canonicalized_path = fs_util::canonicalize(resolved_path).categorize_input()?;
    context.push_trace(&canonicalized_path);
    context.resolve_argfile_kind(canonicalized_path, flag)
}

#[cfg(test)]
mod tests {
    use buck2_common::argv::ExpandedArgSource;
    use buck2_common::argv::FlagfileArgSource;
    use buck2_fs::fs_util::uncategorized as fs_util;
    use buck2_fs::paths::abs_path::AbsPath;
    use buck2_fs::paths::abs_path::AbsPathBuf;
    use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    use indoc::indoc;

    use super::*;

    #[test]
    fn test_resolve_argfile_kind_with_project() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let root = AbsPathBuf::new(fs_util::canonicalize(root)?)?;

        let project_root = root.join("project");
        let symlinked_project_root = root.join("symlinked-project");

        fs_util::create_dir(&project_root)?;
        fs_util::write(project_root.join(".buckconfig"), "[cells]\nroot = .")?;
        fs_util::symlink(&project_root, &symlinked_project_root)?;

        let external_mode_file = root.join("mode-file");
        fs_util::write(&external_mode_file, "")?;

        let project_mode_file = project_root.join("mode-file");
        fs_util::write(&project_mode_file, "")?;

        let project_mode_file_in_symlink_root = symlinked_project_root.join("mode-file");

        let cwd = AbsWorkingDir::unchecked_new(AbsNormPathBuf::new(project_root.to_path_buf())?);
        let context = ImmediateConfigContext::new(&cwd);

        let kind =
            context.resolve_argfile_kind(fs_util::canonicalize(external_mode_file)?, None)?;
        assert!(
            matches!(kind, ArgFileKind::Path(ArgFilePath::External(_))),
            "{kind:?}"
        );

        let kind = context.resolve_argfile_kind(
            AbsNormPathBuf::new(project_mode_file.into_path_buf())?,
            None,
        )?;
        assert!(
            matches!(kind, ArgFileKind::Path(ArgFilePath::Project(_))),
            "{kind:?}"
        );

        let kind = context.resolve_argfile_kind(
            AbsNormPathBuf::new(
                fs_util::canonicalize(project_mode_file_in_symlink_root)?.into_path_buf(),
            )?,
            None,
        )?;
        assert!(
            matches!(kind, ArgFileKind::Path(ArgFilePath::Project(_))),
            "{kind:?}"
        );

        Ok(())
    }

    #[test]
    fn test_expand_argfile_content() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let cwd =
            AbsWorkingDir::unchecked_new(AbsNormPathBuf::new(root.canonicalize()?.join("foo"))?);
        let context = ImmediateConfigContext::new(&cwd);

        let mode_file = root.join("mode-file");
        // Test skips empty lines.
        fs_util::write(&mode_file, "a\n\nb\n")?;
        let kind =
            context.resolve_argfile_kind(AbsNormPathBuf::new(mode_file.into_path_buf())?, None)?;
        let lines = expand_argfile_contents(&context, &kind, &StdinArgfile::default())?;
        assert_eq!(vec!["a".to_owned(), "b".to_owned()], lines);
        Ok(())
    }

    #[test]
    fn test_comment() -> buck2_error::Result<()> {
        let content = indoc! {"
            --# Usage: buck2 build @mode/mrustc ...
            --#
            --# Generated by tools/build/buck/gen_modes.py
            --# \x40generated SignedSource<<e5d1ca23a1a7a4a0af60a49cc2ac8260>>
            --config=rust.compiler=mrustc
        "};
        let expected = ["--config=rust.compiler=mrustc".to_owned()];

        // Set up project directory
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        let cwd = AbsWorkingDir::unchecked_new(AbsNormPathBuf::new(root.to_path_buf())?);
        let mut context = ImmediateConfigContext::new(&cwd);

        // Write modefile
        let mode_dir = root.join("mode");
        fs_util::create_dir(&mode_dir)?;
        let mode_file = mode_dir.join("mrustc");
        fs_util::write(&mode_file, content)?;

        // Expand and check
        let mut expanded = ExpandedArgvBuilder::new();
        expand_argfiles_with_context(
            &mut expanded,
            vec!["@mode/mrustc".to_owned()],
            &mut context,
            &cwd,
            &StdinArgfile::default(),
        )
        .unwrap();
        assert_eq!(*expanded.build().args().collect::<Vec<_>>(), expected);
        Ok(())
    }

    #[test]
    fn test_relative_inclusion() {
        // Currently all @-files both on the command line and in files are relative to the current directory.
        // This matches gcc/clang, so write a test we don't inadvertantly change it.
        let tempdir = tempfile::tempdir().unwrap();
        let root = AbsPath::new(tempdir.path()).unwrap();
        fs_util::create_dir(root.join("foo")).unwrap();
        fs_util::create_dir(root.join("foo/bar")).unwrap();
        fs_util::write(root.join("foo/bar/arg1.txt"), "@bar/arg2.txt").unwrap();
        fs_util::write(root.join("foo/bar/arg2.txt"), "--magic").unwrap();
        fs_util::write(root.join(".buckconfig"), "[cells]\nroot = .").unwrap();
        let cwd = AbsWorkingDir::unchecked_new(
            fs_util::canonicalize(root)
                .unwrap()
                .join(ForwardRelativePath::unchecked_new("foo")),
        );
        let mut context = ImmediateConfigContext::new(&cwd);
        let mut args = ExpandedArgvBuilder::new();

        expand_argfiles_with_context(
            &mut args,
            vec!["@bar/arg1.txt".to_owned()],
            &mut context,
            &cwd,
            &StdinArgfile::default(),
        )
        .unwrap();
        let args = args.build();
        assert_eq!(args.args().collect::<Vec<_>>(), vec!["--magic"]);
    }

    #[test]
    fn test_arg_source() {
        // Currently all @-files both on the command line and in files are relative to the current directory.
        // This matches gcc/clang, so write a test we don't inadvertantly change it.
        let tempdir = tempfile::tempdir().unwrap();
        let root = AbsPathBuf::new(tempdir.path().canonicalize().unwrap()).unwrap();
        fs_util::create_dir(root.join("foo")).unwrap();
        fs_util::create_dir(root.join("foo/bar")).unwrap();
        let arg1_path = root.join("foo/bar/arg1.txt");
        let arg2_path = root.join("foo/bar/arg2.txt");
        fs_util::write(&arg1_path, "@bar/arg2.txt\n--other-arg").unwrap();
        fs_util::write(&arg2_path, "--magic").unwrap();
        fs_util::write(root.join(".buckconfig"), "[cells]\nroot = .").unwrap();
        let cwd = AbsWorkingDir::unchecked_new(
            fs_util::canonicalize(root)
                .unwrap()
                .join(ForwardRelativePath::unchecked_new("foo")),
        );
        let mut context = ImmediateConfigContext::new(&cwd);
        let mut args = ExpandedArgvBuilder::new();

        expand_argfiles_with_context(
            &mut args,
            vec![
                "--inline1".to_owned(),
                "@bar/arg1.txt".to_owned(),
                "--inline2".to_owned(),
            ],
            &mut context,
            &cwd,
            &StdinArgfile::default(),
        )
        .unwrap();
        let args = args.build();

        fn display(v: &ExpandedArgSource) -> String {
            fn display_flagfile(v: &FlagfileArgSource) -> String {
                format!(
                    "{}{}",
                    v.kind,
                    if let Some(v) = &v.parent {
                        display_flagfile(v)
                    } else {
                        "".to_owned()
                    }
                )
            }
            match v {
                ExpandedArgSource::Inline => "inline".to_owned(),
                ExpandedArgSource::Flagfile(v) => display_flagfile(v),
            }
        }
        assert_eq!(
            args.iter()
                .map(|(arg, location)| format!("{} {}", arg, display(location)))
                .collect::<Vec<_>>(),
            vec![
                "--inline1 inline".to_owned(),
                "--magic @root//foo/bar/arg2.txt@root//foo/bar/arg1.txt".to_owned(),
                "--other-arg @root//foo/bar/arg1.txt".to_owned(),
                "--inline2 inline".to_owned()
            ]
        )
    }

    /// `path#flavor` resolves for a relative and for an absolute Python argfile.
    #[test]
    fn test_absolute_python_argfile_with_flavor() {
        let tempdir = tempfile::tempdir().unwrap();
        let root = AbsPathBuf::new(tempdir.path().canonicalize().unwrap()).unwrap();
        fs_util::write(root.join("m.py"), "print('--x')").unwrap();
        let cwd = AbsWorkingDir::unchecked_new(AbsNormPathBuf::new(root.to_path_buf()).unwrap());

        let mut context = ImmediateConfigContext::new(&cwd);
        let kind = resolve_flagfile("m.py#o", &mut context, &cwd).unwrap();
        assert!(
            matches!(&kind, ArgFileKind::PythonExecutable(_, Some(f)) if f == "o"),
            "{kind:?}"
        );

        let abs = format!("{}#o", root.join("m.py").as_path().display());
        let mut context = ImmediateConfigContext::new(&cwd);
        let kind = resolve_flagfile(&abs, &mut context, &cwd).unwrap();
        assert!(
            matches!(&kind, ArgFileKind::PythonExecutable(_, Some(f)) if f == "o"),
            "{kind:?}"
        );
    }

    /// A command that is restarted expands its arguments again, after stdin has been read.
    #[test]
    fn test_stdin_argfile_expands_to_the_lines_already_read() {
        let tempdir = tempfile::tempdir().unwrap();
        let root = AbsPathBuf::new(tempdir.path().canonicalize().unwrap()).unwrap();
        fs_util::write(root.join("a.txt"), "--a").unwrap();
        let cwd = AbsWorkingDir::unchecked_new(AbsNormPathBuf::new(root.to_path_buf()).unwrap());
        let stdin = StdinArgfile(OnceCell::from(vec![
            "--stdin".to_owned(),
            "@a.txt".to_owned(),
        ]));

        for _attempt in 0..2 {
            let mut context = ImmediateConfigContext::new(&cwd);
            let argv = expand_argv(
                None,
                vec!["build".to_owned(), "@-".to_owned()],
                &mut context,
                &cwd,
                &stdin,
            )
            .unwrap();
            assert_eq!(
                argv.args().collect::<Vec<_>>(),
                vec!["build", "--stdin", "--a"]
            );
        }
    }

    /// An argfile that includes itself, directly or through another argfile, is an error.
    #[test]
    fn test_cyclic_argfile_is_rejected() {
        let tempdir = tempfile::tempdir().unwrap();
        let root = AbsPathBuf::new(tempdir.path().canonicalize().unwrap()).unwrap();
        fs_util::write(root.join("self.txt"), "@self.txt").unwrap();
        fs_util::write(root.join("a.txt"), "--a\n@b.txt").unwrap();
        fs_util::write(root.join("b.txt"), "@a.txt").unwrap();
        let cwd = AbsWorkingDir::unchecked_new(AbsNormPathBuf::new(root.to_path_buf()).unwrap());

        let mut context = ImmediateConfigContext::new(&cwd);
        let err = expand_argv(
            None,
            vec!["@self.txt".to_owned()],
            &mut context,
            &cwd,
            &StdinArgfile::default(),
        )
        .err()
        .unwrap();
        assert!(format!("{err:#}").contains("includes itself"), "{err:#}");

        let mut context = ImmediateConfigContext::new(&cwd);
        let err = expand_argv(
            None,
            vec!["@a.txt".to_owned()],
            &mut context,
            &cwd,
            &StdinArgfile::default(),
        )
        .err()
        .unwrap();
        assert!(format!("{err:#}").contains("includes itself"), "{err:#}");

        // The same file twice in a row is not a cycle.
        fs_util::write(root.join("c.txt"), "--c").unwrap();
        let mut context = ImmediateConfigContext::new(&cwd);
        let argv = expand_argv(
            None,
            vec!["@c.txt".to_owned(), "@c.txt".to_owned()],
            &mut context,
            &cwd,
            &StdinArgfile::default(),
        )
        .unwrap();
        assert_eq!(argv.args().collect::<Vec<_>>(), vec!["--c", "--c"]);
    }
}
