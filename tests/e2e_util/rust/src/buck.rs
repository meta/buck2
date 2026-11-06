/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::future::Future;
use std::future::IntoFuture;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::ExitStatus;
use std::process::Stdio;

use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

use crate::output::BuckError;
use crate::output::BuckOutput;

/// The buck2 under test, set up in the test's project. Its methods start commands: awaiting a
/// [`BuckCommand`] runs it and gives its [`BuckOutput`], or a [`BuckError`] if buck2 failed.
///
/// ```ignore
/// buck.build(&["//:target"]).await?;
/// let out = buck.uquery(&["deps(//:target)"]).env("BUCK_LOG", "info").await?;
/// ```
#[derive(Debug, Clone)]
pub struct Buck {
    executable: PathBuf,
    cwd: PathBuf,
    env: BTreeMap<OsString, OsString>,
    isolation_prefix: Option<String>,
    write_invocation_record: bool,
}

macro_rules! subcommands {
    ($($method:ident => [$($word:literal),+];)*) => {
        $(
            #[doc = concat!("`buck2 ", $($word, " ",)+ "<args>`.")]
            pub fn $method(&self, args: &[&str]) -> BuckCommand<'_> {
                self.subcommand(&[$($word),+], args)
            }
        )*
    };
}

impl Buck {
    pub(crate) fn new(
        executable: PathBuf,
        cwd: PathBuf,
        env: BTreeMap<OsString, OsString>,
        write_invocation_record: bool,
    ) -> Self {
        let mut buck = Self {
            executable,
            cwd,
            env,
            isolation_prefix: None,
            write_invocation_record,
        };
        buck.set_env("NO_BUCKD", "0");
        buck
    }

    /// The project directory commands run in.
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// Sets an environment variable for every later command.
    pub fn set_env(&mut self, key: impl Into<OsString>, value: impl Into<OsString>) {
        self.env.insert(key.into(), value.into());
    }

    /// Unsets an environment variable, inherited from the test's environment or set, for every
    /// later command.
    pub fn remove_env(&mut self, key: impl AsRef<OsStr>) {
        self.env.remove(key.as_ref());
    }

    /// The value an environment variable has for commands.
    pub fn env_var(&self, key: impl AsRef<OsStr>) -> Option<&OsStr> {
        self.env.get(key.as_ref()).map(OsString::as_os_str)
    }

    /// Runs every later command with `--isolation-dir prefix`.
    pub fn set_isolation_prefix(&mut self, prefix: impl Into<String>) {
        self.isolation_prefix = Some(prefix.into());
    }

    /// Runs every later command in `cwd` instead of the project directory, for a test that makes
    /// another checkout of the project.
    pub fn set_cwd(&mut self, cwd: impl Into<PathBuf>) {
        self.cwd = cwd.into();
    }

    /// The directory buck2 keeps its user settings in for this test.
    pub fn settings_home_dir(&self) -> PathBuf {
        PathBuf::from(
            self.env_var("BUCK2_TEST_SETTINGS_HOME_DIR")
                .expect("the test fixture sets `BUCK2_TEST_SETTINGS_HOME_DIR`"),
        )
    }

    /// The buck2 executable commands run.
    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// Runs every later command with `executable` instead, as a test that compares buck2 versions
    /// does.
    pub fn set_executable(&mut self, executable: impl Into<PathBuf>) {
        self.executable = executable.into();
    }

    /// The environment variables every command gets, for a process other than buck2 that must
    /// see what buck2 sees.
    pub fn envs(&self) -> impl Iterator<Item = (&OsStr, &OsStr)> {
        self.env.iter().map(|(k, v)| (k.as_os_str(), v.as_os_str()))
    }

    /// A buck2 command. `args` are everything after the executable, subcommand first.
    pub fn command(&self, args: &[&str]) -> BuckCommand<'_> {
        self.command_os(args.iter().map(OsString::from).collect())
    }

    fn subcommand(&self, words: &[&str], args: &[&str]) -> BuckCommand<'_> {
        self.command_os(words.iter().chain(args).map(OsString::from).collect())
    }

    fn command_os(&self, args: Vec<OsString>) -> BuckCommand<'_> {
        BuckCommand {
            buck: self,
            args,
            env: Vec::new(),
            rel_cwd: None,
            stdin: None,
            invocation_record: true,
        }
    }

    subcommands! {
        aquery => ["aquery"];
        audit => ["audit"];
        audit_config => ["audit", "config"];
        audit_configurations => ["audit", "configurations"];
        audit_dep_files => ["audit", "dep-files"];
        audit_output => ["audit", "output"];
        audit_visibility => ["audit", "visibility"];
        build_without_report => ["build"];
        bxl => ["bxl"];
        clean => ["clean"];
        completion => ["completion"];
        cquery => ["cquery"];
        ctargets => ["ctargets"];
        debug => ["debug"];
        docs => ["docs"];
        expand_external_cell => ["expand-external-cell"];
        explain => ["explain"];
        help => ["help"];
        help_env => ["help-env"];
        init => ["init"];
        install => ["install"];
        profile => ["profile"];
        query => ["query"];
        rage => ["rage"];
        root => ["root"];
        run => ["run"];
        server => ["server"];
        starlark => ["starlark"];
        targets => ["targets"];
        uquery => ["uquery"];
    }

    /// `buck2 build <args>`, with the build report on stdout (see [`BuckOutput::build_report`])
    /// unless `args` ask for a build report themselves.
    pub fn build(&self, args: &[&str]) -> BuckCommand<'_> {
        let mut args: Vec<&str> = args.to_vec();
        if !args.iter().any(|a| a.starts_with("--build-report")) {
            // Anything after `--` is a positional argument.
            let separator = args.iter().position(|a| *a == "--").unwrap_or(args.len());
            args.insert(separator, "--build-report=-");
        }
        self.subcommand(&["build"], &args)
    }

    /// `buck2 test <args>`. The test results go to `testOutput.xml` in the working directory unless
    /// `args` name another file with `--xml`. With `BUCK2_TPX` set, the tests run with that test
    /// executor; under tpx, disabled tests run too.
    pub fn test(&self, args: &[&str]) -> BuckCommand<'_> {
        let separator = args.iter().position(|a| *a == "--").unwrap_or(args.len());
        let (buck_args, test_args) = args.split_at(separator);
        let test_args = test_args.get(1..).unwrap_or_default();

        let mut argv: Vec<OsString> = vec!["test".into()];
        if !buck_args
            .iter()
            .any(|a| *a == "--xml" || a.starts_with("--xml="))
        {
            argv.extend(["--xml".into(), "testOutput.xml".into()]);
        }
        let executor = self
            .env_var("BUCK2_TPX")
            .map(|e| e.to_string_lossy().into_owned());
        if let Some(executor) = &executor {
            argv.extend([
                "--config".into(),
                format!("test.v2_test_executor={executor}").into(),
            ]);
        }
        argv.extend(buck_args.iter().map(OsString::from));
        argv.push("--".into());
        argv.extend(test_args.iter().map(OsString::from));
        if executor.as_ref().is_none_or(|e| e.contains("tpx")) {
            argv.push("--run-disabled".into());
        }
        self.command_os(argv)
    }

    /// `buck2 complete <args>`, with a completion timeout long enough for a loaded test host.
    pub fn complete(&self, args: &[&str]) -> BuckCommand<'_> {
        self.subcommand(&["complete"], args)
            .env("BUCK2_COMPLETION_TIMEOUT", "30000")
    }

    /// `buck2 kill`.
    pub fn kill(&self) -> BuckCommand<'_> {
        self.subcommand(&["kill"], &[])
    }

    /// `buck2 log <args>`. It writes no invocation record.
    pub fn log(&self, args: &[&str]) -> BuckCommand<'_> {
        self.subcommand(&["log"], args).without_invocation_record()
    }

    /// `buck2 status <args>`. It writes no invocation record.
    pub fn status(&self, args: &[&str]) -> BuckCommand<'_> {
        self.subcommand(&["status"], args)
            .without_invocation_record()
    }

    /// The daemon's directory, as `buck2 debug daemon-dir` prints it.
    pub async fn daemon_dir(&self) -> Result<PathBuf, BuckError> {
        Ok(PathBuf::from(
            // `debug daemon-dir` doesn't accept `--unstable-write-invocation-record`.
            self.debug(&["daemon-dir"])
                .without_invocation_record()
                .await?
                .stdout
                .trim(),
        ))
    }

    /// What the running daemon wrote to stderr.
    pub async fn daemon_stderr(&self) -> anyhow::Result<String> {
        Ok(std::fs::read_to_string(
            self.daemon_dir().await?.join("buckd.stderr"),
        )?)
    }

    /// What the previous daemon wrote to stderr.
    pub async fn prev_daemon_stderr(&self) -> anyhow::Result<String> {
        Ok(std::fs::read_to_string(
            self.daemon_dir().await?.join("prev").join("buckd.stderr"),
        )?)
    }
}

/// A buck2 command that runs when awaited, giving its [`BuckOutput`] or, if buck2 exits
/// unsuccessfully, a [`BuckError`].
#[must_use = "a buck2 command runs only when awaited"]
pub struct BuckCommand<'a> {
    buck: &'a Buck,
    args: Vec<OsString>,
    env: Vec<(OsString, OsString)>,
    rel_cwd: Option<PathBuf>,
    stdin: Option<Vec<u8>>,
    invocation_record: bool,
}

pub(crate) struct Prepared {
    pub(crate) argv: Vec<OsString>,
    pub(crate) args: Vec<OsString>,
    pub(crate) cwd: PathBuf,
    pub(crate) env: BTreeMap<OsString, OsString>,
    pub(crate) invocation_record_path: Option<PathBuf>,
}

impl<'a> BuckCommand<'a> {
    /// Sets an environment variable for this command, over the [`Buck`]'s.
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Runs the command in `rel_cwd`, relative to the project directory.
    pub fn rel_cwd(mut self, rel_cwd: impl AsRef<Path>) -> Self {
        self.rel_cwd = Some(rel_cwd.as_ref().to_owned());
        self
    }

    /// Writes `input` to the command's stdin. Without it, stdin is empty.
    pub fn stdin(mut self, input: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(input.into());
        self
    }

    /// The command as a blocking process, with its output discarded.
    pub(crate) fn blocking(&self) -> std::process::Command {
        let prepared = self.prepare(&uuid::Uuid::new_v4().to_string());
        let mut command = std::process::Command::new(&prepared.argv[0]);
        command
            .args(&prepared.argv[1..])
            .current_dir(&prepared.cwd)
            .env_clear()
            .envs(&prepared.env)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    pub(crate) fn without_invocation_record(mut self) -> Self {
        self.invocation_record = false;
        self
    }

    pub(crate) fn prepare(&self, invocation_id: &str) -> Prepared {
        let mut env = self.buck.env.clone();
        env.extend(self.env.iter().cloned());
        env.entry("BUCK_WRAPPER_UUID".into())
            .or_insert_with(|| invocation_id.into());

        let cwd = match &self.rel_cwd {
            Some(rel_cwd) => self.buck.cwd.join(rel_cwd),
            None => self.buck.cwd.clone(),
        };

        let mut args = self.args.clone();
        let invocation_record_path = if self.buck.write_invocation_record && self.invocation_record
        {
            let path = cwd
                .join("buck-out")
                .join("tmp")
                .join(format!("{invocation_id}.json"));
            let separator = args.iter().position(|a| a == "--").unwrap_or(args.len());
            args.splice(
                separator..separator,
                [
                    "--unstable-write-invocation-record".into(),
                    path.clone().into_os_string(),
                ],
            );
            Some(path)
        } else {
            None
        };

        let mut argv = vec![self.buck.executable.clone().into_os_string()];
        if let Some(prefix) = &self.buck.isolation_prefix {
            argv.extend(["--isolation-dir".into(), prefix.into()]);
        }
        argv.extend(args.iter().cloned());

        Prepared {
            argv,
            args,
            cwd,
            env,
            invocation_record_path,
        }
    }

    /// Runs the command to completion.
    pub async fn run(self) -> Result<BuckOutput, BuckError> {
        self.spawn().wait().await
    }

    /// Starts the command, to wait for it later. Its stdout and stderr are collected meanwhile.
    pub fn spawn(self) -> BuckProcess {
        let invocation_id = uuid::Uuid::new_v4().to_string();
        let prepared = self.prepare(&invocation_id);
        assert!(
            prepared.cwd.exists(),
            "The working directory `{}` does not exist",
            prepared.cwd.display()
        );
        if let Some(parent) = prepared
            .invocation_record_path
            .as_deref()
            .and_then(Path::parent)
        {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|e| panic!("Could not create `{}`: {e}", parent.display()));
        }

        let mut command = tokio::process::Command::new(&prepared.argv[0]);
        command
            .args(&prepared.argv[1..])
            .current_dir(&prepared.cwd)
            .env_clear()
            .envs(&prepared.env)
            .stdin(if self.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().unwrap_or_else(|e| {
            panic!(
                "Could not run `{}`: {e}",
                prepared.argv[0].to_string_lossy()
            )
        });
        let stdin = self.stdin.map(|input| {
            let mut pipe = child.stdin.take().expect("stdin is piped");
            tokio::spawn(async move {
                // buck2 may exit without reading all of it, which is not this command's failure.
                let _ignored = pipe.write_all(&input).await;
            })
        });
        let stdout = read_all(child.stdout.take().expect("stdout is piped"));
        let stderr = read_all(child.stderr.take().expect("stderr is piped"));
        BuckProcess {
            child,
            stdin,
            stdout,
            stderr,
            command: prepared.argv,
            args: prepared
                .args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect(),
            working_dir: prepared.cwd,
            invocation_record_path: prepared.invocation_record_path,
        }
    }
}

pub(crate) fn read_all(
    mut pipe: impl tokio::io::AsyncRead + Unpin + Send + 'static,
) -> tokio::task::JoinHandle<Vec<u8>> {
    tokio::spawn(async move {
        let mut buffer = Vec::new();
        let _ignored = pipe.read_to_end(&mut buffer).await;
        buffer
    })
}

/// A started buck2 command. Dropping it kills the process.
pub struct BuckProcess {
    child: tokio::process::Child,
    stdin: Option<tokio::task::JoinHandle<()>>,
    stdout: tokio::task::JoinHandle<Vec<u8>>,
    stderr: tokio::task::JoinHandle<Vec<u8>>,
    command: Vec<OsString>,
    args: Vec<String>,
    working_dir: PathBuf,
    invocation_record_path: Option<PathBuf>,
}

impl BuckProcess {
    /// The process id, until the process has been waited for.
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    /// The exit status, if the process has exited.
    pub fn try_wait(&mut self) -> Option<ExitStatus> {
        self.child
            .try_wait()
            .unwrap_or_else(|e| panic!("Could not check on the process: {e}"))
    }

    /// Kills the process, then waits for it.
    pub async fn kill(mut self) -> Result<BuckOutput, BuckError> {
        // It may have exited already.
        let _ignored = self.child.start_kill();
        self.wait().await
    }

    /// Waits for the process to exit, giving its [`BuckOutput`] or, if buck2 exited
    /// unsuccessfully, a [`BuckError`].
    pub async fn wait(mut self) -> Result<BuckOutput, BuckError> {
        let status = self
            .child
            .wait()
            .await
            .unwrap_or_else(|e| panic!("Could not wait for the process: {e}"));
        if let Some(stdin) = self.stdin {
            let _ignored = stdin.await;
        }
        let stdout = self.stdout.await.expect("reading stdout");
        let stderr = self.stderr.await.expect("reading stderr");
        let result = BuckOutput {
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            status,
            command: self.command,
            args: self.args,
            working_dir: self.working_dir,
            invocation_record_path: self.invocation_record_path,
        };
        if result.status.success() {
            Ok(result)
        } else {
            Err(BuckError(Box::new(result)))
        }
    }
}

impl<'a> IntoFuture for BuckCommand<'a> {
    type Output = Result<BuckOutput, BuckError>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buck(write_invocation_record: bool) -> Buck {
        Buck::new(
            PathBuf::from("/bin/buck2"),
            PathBuf::from("/project"),
            BTreeMap::new(),
            write_invocation_record,
        )
    }

    fn argv(command: BuckCommand<'_>) -> Vec<String> {
        command
            .prepare("ID")
            .argv
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn test_subcommands_come_after_the_isolation_dir() {
        let mut buck = buck(false);
        buck.set_isolation_prefix("iso");
        assert_eq!(
            argv(buck.audit_config(&["--style=json"])),
            [
                "/bin/buck2",
                "--isolation-dir",
                "iso",
                "audit",
                "config",
                "--style=json"
            ]
        );
    }

    #[test]
    fn test_build_asks_for_the_build_report_before_the_positional_arguments() {
        assert_eq!(
            argv(buck(false).build(&["//:a", "--", "x"])),
            ["/bin/buck2", "build", "//:a", "--build-report=-", "--", "x"]
        );
    }

    #[test]
    fn test_build_keeps_a_build_report_the_caller_asked_for() {
        assert_eq!(
            argv(buck(false).build(&["//:a", "--build-report=report.json"])),
            ["/bin/buck2", "build", "//:a", "--build-report=report.json"]
        );
    }

    #[test]
    fn test_the_invocation_record_goes_before_the_positional_arguments() {
        let record = PathBuf::from("/project")
            .join("buck-out")
            .join("tmp")
            .join("ID.json");
        assert_eq!(
            argv(buck(true).run(&["//:a", "--", "x"])),
            [
                "/bin/buck2",
                "run",
                "//:a",
                "--unstable-write-invocation-record",
                record.to_str().unwrap(),
                "--",
                "x"
            ]
        );
    }

    #[test]
    fn test_log_writes_no_invocation_record() {
        assert_eq!(
            argv(buck(true).log(&["show"])),
            ["/bin/buck2", "log", "show"]
        );
    }

    #[test]
    fn test_tpx_runs_disabled_tests() {
        let mut buck = buck(false);
        buck.set_env("BUCK2_TPX", "/bin/buck2-tpx");
        assert_eq!(
            argv(buck.test(&["//:t", "--", "--regex", "x"])),
            [
                "/bin/buck2",
                "test",
                "--xml",
                "testOutput.xml",
                "--config",
                "test.v2_test_executor=/bin/buck2-tpx",
                "//:t",
                "--",
                "--regex",
                "x",
                "--run-disabled"
            ]
        );
    }

    #[test]
    fn test_another_test_executor_does_not_run_disabled_tests() {
        let mut buck = buck(false);
        buck.set_env("BUCK2_TPX", "/bin/runner");
        assert_eq!(
            argv(buck.test(&["//:t", "--xml", "out.xml"])),
            [
                "/bin/buck2",
                "test",
                "--config",
                "test.v2_test_executor=/bin/runner",
                "//:t",
                "--xml",
                "out.xml",
                "--"
            ]
        );
    }

    #[test]
    fn test_a_removed_variable_reaches_no_command() {
        let mut buck = buck(false);
        buck.set_env("HTTP_PROXY", "http://proxy");
        buck.remove_env("HTTP_PROXY");
        assert_eq!(buck.env_var("HTTP_PROXY"), None);
        assert_eq!(
            buck.kill().prepare("ID").env.get(OsStr::new("HTTP_PROXY")),
            None
        );
    }

    #[test]
    fn test_a_command_gets_an_invocation_id_unless_the_test_set_one() {
        let buck = buck(false);
        assert_eq!(
            buck.kill()
                .prepare("ID")
                .env
                .get(OsStr::new("BUCK_WRAPPER_UUID")),
            Some(&OsString::from("ID"))
        );
        assert_eq!(
            buck.kill()
                .env("BUCK_WRAPPER_UUID", "mine")
                .prepare("ID")
                .env
                .get(OsStr::new("BUCK_WRAPPER_UUID")),
            Some(&OsString::from("mine"))
        );
    }

    #[test]
    fn test_commands_run_where_the_buck_was_moved() {
        let mut buck = buck(false);
        buck.set_cwd("/checkout");
        assert_eq!(buck.cwd(), Path::new("/checkout"));
        assert_eq!(buck.kill().prepare("ID").cwd, Path::new("/checkout"));
    }

    #[test]
    fn test_envs_are_what_every_command_gets() {
        let mut buck = buck(false);
        buck.set_env("KEY", "value");
        let envs: BTreeMap<&OsStr, &OsStr> = buck.envs().collect();
        assert_eq!(envs.get(OsStr::new("KEY")), Some(&OsStr::new("value")));
        assert_eq!(envs.get(OsStr::new("NO_BUCKD")), Some(&OsStr::new("0")));
        assert_eq!(buck.executable(), Path::new("/bin/buck2"));
    }

    #[test]
    fn test_later_commands_run_the_new_executable() {
        let mut buck = buck(false);
        buck.set_executable("buck2");
        assert_eq!(argv(buck.kill()), ["buck2", "kill"]);
    }
}
