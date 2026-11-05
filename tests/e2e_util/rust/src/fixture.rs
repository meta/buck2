/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Once;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use anyhow::Context;
use futures::FutureExt;
use sha2::Digest;
use sha2::Sha256;

use crate::buck::Buck;
use crate::macro_support::BoxFuture;
use crate::macro_support::Fixture;
use crate::macro_support::TestResult;

const SKIPPABLE_OSES: [&str; 3] = ["darwin", "linux", "windows"];

/// How a `#[buck_test]` runs: each option of the attribute calls the method of the same name.
pub struct BuckTest {
    test_name: String,
    needs_buck: bool,
    inplace: Option<bool>,
    data_dir: Option<String>,
    skip_for_os: Vec<String>,
    skip_for_materializer: Vec<String>,
    allow_soft_errors: bool,
    allow_daemon_panics: bool,
    extra_buck_config: Vec<(String, String, String)>,
    skip_final_kill: bool,
    setup_eden: bool,
    disable_daemon_cgroup: bool,
    write_invocation_record: bool,
    env: Vec<(String, String)>,
}

impl BuckTest {
    #[doc(hidden)]
    pub fn new(module: &str, name: &str) -> Self {
        Self {
            test_name: format!("{module}::{name}"),
            needs_buck: true,
            inplace: None,
            data_dir: None,
            skip_for_os: Vec::new(),
            skip_for_materializer: Vec::new(),
            allow_soft_errors: false,
            allow_daemon_panics: false,
            extra_buck_config: Vec::new(),
            skip_final_kill: false,
            setup_eden: false,
            disable_daemon_cgroup: true,
            write_invocation_record: false,
            env: Vec::new(),
        }
    }

    /// Whether the test runs in the repository instead of a project of its own. Tests outside
    /// `tests/core` must say; `tests/core` tests always get a project of their own.
    pub fn inplace(mut self, inplace: bool) -> Self {
        self.inplace = Some(inplace);
        self
    }

    /// The subdirectory of the target's test data to set the project up from, instead of all of it.
    pub fn data_dir(mut self, data_dir: &str) -> Self {
        self.data_dir = Some(data_dir.to_owned());
        self
    }

    /// The operating systems (`darwin`, `linux`, `windows`) the test passes on without running.
    pub fn skip_for_os<'a>(mut self, oses: impl IntoIterator<Item = &'a str>) -> Self {
        self.skip_for_os = oses.into_iter().map(str::to_owned).collect();
        self
    }

    /// The materializer implementations (`BUCK2_TEST_MATERIALIZER` values) the test passes on
    /// without running, for a test that asserts on one implementation's own state or output. See
    /// `buck2_core_tests`.
    pub fn skip_for_materializer<'a>(
        mut self,
        materializers: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        self.skip_for_materializer = materializers.into_iter().map(str::to_owned).collect();
        self
    }

    /// Lets buck2 report soft errors instead of failing on them.
    pub fn allow_soft_errors(mut self, allow_soft_errors: bool) -> Self {
        self.allow_soft_errors = allow_soft_errors;
        self
    }

    /// Lets an isolated test pass though its daemon panicked, even after a command finished: for a
    /// test that makes the daemon panic on purpose, which includes expecting a soft error from the
    /// daemon, since `BUCK2_HARD_ERROR` turns those into panics.
    pub fn allow_daemon_panics(mut self, allow_daemon_panics: bool) -> Self {
        self.allow_daemon_panics = allow_daemon_panics;
        self
    }

    /// Extra buckconfig values, as `(section, key, value)`.
    pub fn extra_buck_config<'a>(
        mut self,
        config: impl IntoIterator<Item = (&'a str, &'a str, &'a str)>,
    ) -> Self {
        self.extra_buck_config.extend(
            config
                .into_iter()
                .map(|(s, k, v)| (s.to_owned(), k.to_owned(), v.to_owned())),
        );
        self
    }

    /// Leaves the daemon running after the test instead of running `buck2 clean`.
    pub fn skip_final_kill(mut self, skip_final_kill: bool) -> Self {
        self.skip_final_kill = skip_final_kill;
        self
    }

    /// Sets the project up on EdenFS.
    pub fn setup_eden(mut self, setup_eden: bool) -> Self {
        self.setup_eden = setup_eden;
        self
    }

    /// Whether the daemon stays out of a cgroup of its own. On by default.
    pub fn disable_daemon_cgroup(mut self, disable_daemon_cgroup: bool) -> Self {
        self.disable_daemon_cgroup = disable_daemon_cgroup;
        self
    }

    /// Has every command write an invocation record; see [`crate::BuckOutput::invocation_record`].
    pub fn write_invocation_record(mut self, write_invocation_record: bool) -> Self {
        self.write_invocation_record = write_invocation_record;
        self
    }

    /// Environment variables for every command, as `(key, value)`.
    pub fn env<'a>(mut self, env: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        self.env
            .extend(env.into_iter().map(|(k, v)| (k.to_owned(), v.to_owned())));
        self
    }

    #[doc(hidden)]
    pub fn without_buck(mut self) -> Self {
        self.needs_buck = false;
        self
    }

    #[doc(hidden)]
    pub fn run(self, body: for<'a> fn(&'a mut Fixture) -> BoxFuture<'a, TestResult>) -> TestResult {
        self.validate()?;
        if self.skipped() {
            // A skipped test is a failure to tpx, so the test passes without running instead.
            return Ok(());
        }
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(self.run_async(body))
    }

    fn validate(&self) -> anyhow::Result<()> {
        if is_core_test() {
            anyhow::ensure!(
                self.inplace.is_none(),
                "Don't set `inplace` in `tests/core` - these tests are always isolated"
            );
        } else {
            anyhow::ensure!(
                self.inplace.is_some(),
                "`inplace` must be set for a `#[buck_test]` outside `tests/core`"
            );
        }
        anyhow::ensure!(
            !(self.inplace == Some(true) && self.setup_eden),
            "EdenFS for e2e tests is not supported for inplace tests"
        );
        anyhow::ensure!(
            !(self.inplace == Some(true) && self.data_dir.is_some()),
            "`data_dir` is not an allowed parameter for an inplace test"
        );
        for os in &self.skip_for_os {
            anyhow::ensure!(
                SKIPPABLE_OSES.contains(&os.as_str()),
                "`skip_for_os` takes {SKIPPABLE_OSES:?}, not `{os}`"
            );
        }
        Ok(())
    }

    fn skipped(&self) -> bool {
        // Without a materializer variant's environment a test runs against the default
        // implementation.
        let materializer =
            std::env::var("BUCK2_TEST_MATERIALIZER").unwrap_or_else(|_| "deferred".to_owned());
        // `buck_e2e_test`'s own `skip_for_os` applies to all its tests.
        let target_skip_for_os = std::env::var("BUCK2_E2E_SKIP_FOR_OS").unwrap_or_default();
        self.skip_for_os.iter().any(|os| os == current_os())
            || target_skip_for_os.split(',').any(|os| os == current_os())
            || self.skip_for_materializer.contains(&materializer)
    }

    async fn run_async(
        self,
        body: for<'a> fn(&'a mut Fixture) -> BoxFuture<'a, TestResult>,
    ) -> TestResult {
        let base_dir = tempfile::Builder::new()
            .tempdir_in(common_dir().await)
            .context("Creating the test's temporary directory")?
            .keep();
        clean_up_on_panic();
        LEFTOVERS.set(Some(Leftovers {
            test_name: self.test_name.clone(),
            base_dir: base_dir.clone(),
            kill: None,
            eden: None,
        }));

        let mut fixture = match self.set_up(&base_dir).await {
            Ok(fixture) => fixture,
            Err(e) => {
                if let Some(leftovers) = LEFTOVERS.take() {
                    leftovers.clean_up_after_failure();
                }
                return Err(e.context("Setting up the test's project"));
            }
        };

        let outcome = AssertUnwindSafe(body(&mut fixture)).catch_unwind().await;

        // After a panic, the panic hook has cleaned up already.
        let leftovers = LEFTOVERS.take();
        match outcome {
            Err(panic) => {
                if let Some(leftovers) = leftovers {
                    leftovers.clean_up_after_failure();
                }
                std::panic::resume_unwind(panic)
            }
            Ok(Err(e)) => {
                if let Some(leftovers) = leftovers {
                    leftovers.clean_up_after_failure();
                }
                Err(e)
            }
            Ok(Ok(())) => {
                let finished = match &fixture.buck {
                    Some(buck) => self.finish(buck).await,
                    None => Ok(()),
                };
                if let Some(leftovers) = leftovers {
                    leftovers.remove();
                }
                finished
            }
        }
    }

    /// After a passed test: fails it if its daemon panicked, and stops buck2.
    async fn finish(&self, buck: &Buck) -> anyhow::Result<()> {
        let mut panic = None;
        if !(self.allow_daemon_panics || self.inplace == Some(true)) {
            if !self.skip_final_kill {
                // Kill (not clean) first: it waits for the daemon to exit and leaves its stderr in
                // place for the check.
                buck.kill().await?;
            }
            panic = daemon_panic(buck).await;
        }
        if !self.skip_final_kill {
            stop(buck).await?;
        }
        match panic {
            Some(message) => Err(anyhow::anyhow!(message)),
            None => Ok(()),
        }
    }

    async fn set_up(&self, base_dir: &Path) -> anyhow::Result<Fixture> {
        let tmp_path = base_dir.join("tmp");
        fs::create_dir(&tmp_path)?;
        let buck = if self.needs_buck {
            let buck = self.create_buck(base_dir).await?;
            if !self.skip_final_kill {
                let kill = buck.kill().without_invocation_record().blocking();
                LEFTOVERS.with_borrow_mut(|leftovers| {
                    if let Some(leftovers) = leftovers {
                        leftovers.kill = Some(kill);
                    }
                });
            }
            Some(buck)
        } else {
            None
        };
        Ok(Fixture { buck, tmp_path })
    }

    async fn create_buck(&self, base_dir: &Path) -> anyhow::Result<Buck> {
        let mut env: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
        absolutize_path_vars(&mut env, &std::env::current_dir()?);
        let executable = env
            .get(OsStr::new("TEST_EXECUTABLE"))
            .cloned()
            .context("`TEST_EXECUTABLE` is not set; the buck e2e test macros set it")?;
        let mut set = |key: &str, value: &str| {
            env.insert(key.into(), value.into());
        };

        set("BUCK2_SHARE_DAEMON", "false");
        if cfg!(windows) {
            set("CI", "true");
        } else {
            // This is necessary for static linking on Linux.
            set("BUCKD_STARTUP_TIMEOUT", "120");
            set("BUCKD_STARTUP_INIT_TIMEOUT", "120");
        }
        // An inplace test runs against the repository, whose soft errors are not the test's.
        if self.allow_soft_errors || self.inplace == Some(true) {
            set("BUCK2_HARD_ERROR", "false");
        }
        // A very small stdin buffer catches any scenario in which buck2 does not handle partial
        // input.
        set("BUCK2_TEST_STDIN_BUFFER_SIZE", "8");
        // Log uploads are disabled, but blocked on: the upload process also writes locally, and
        // that should be synchronous.
        set("BUCK2_TEST_DISABLE_LOG_UPLOAD", "true");
        set("BUCK2_TEST_BLOCK_ON_UPLOAD", "true");
        set("ENFORCE_DISPATCHER_SET", "true");
        set(
            "BUCK2_SELF_TEST_TIMEOUT_S",
            &self_test_timeout_s(std::env::var("TPX_TIMEOUT_SEC").ok().as_deref()).to_string(),
        );
        // Watchman requests often hang.
        set("BUCK2_WATCHMAN_TIMEOUT", "30");
        set("BUCK2_RUNTIME_THREADS", "8");
        set("BUCK2_IGNORE_VERSION_EXTRACTION_FAILURE", "true");
        set("SUPERCONSOLE_TESTING_WIDTH", "100");
        set("SUPERCONSOLE_TESTING_HEIGHT", "100");
        if self.disable_daemon_cgroup {
            set("BUCK2_TEST_DISABLE_DAEMON_CGROUP", "true");
        }
        set("BUCK2_TEST_SKIP_DEFAULT_EXTERNAL_CONFIG", "true");
        env.remove(OsStr::new("BUCK2_DAEMON_GROUP"));
        // Windows uses blocking threads for subprocess I/O, so buck2 gets no limit.
        env.remove(OsStr::new("BUCK2_MAX_BLOCKING_THREADS"))
            .context("`BUCK2_MAX_BLOCKING_THREADS` is not set; the buck e2e test macros set it")?;
        // A test that runs a test through buck2 must not look like it runs under TestPilot itself.
        env.remove(OsStr::new("TEST_PILOT"));
        // The test runner sets this for the Rust test itself; the buck2 under test and the compilers
        // it runs must not get it.
        env.remove(OsStr::new("RUSTC_BOOTSTRAP"));

        let mut extra_config = String::new();
        if is_core_test() {
            // Core tests run all their RE actions under `buck2-testing`, which has an isolated CAS
            // namespace sized for their small actions.
            extra_config.push_str("[buck2_re_client]\noverride_use_case = buck2-testing\n");
        }

        let project_dir = base_dir.join("project");
        fs::create_dir(&project_dir)?;
        if cfg!(target_os = "macos") {
            ensure_fseventsd_running(base_dir.parent().unwrap_or(base_dir));
        }
        if self.setup_eden {
            let eden = Eden {
                dir: base_dir.join("eden"),
                project_dir: project_dir.clone(),
                env: env.clone(),
            };
            LEFTOVERS.with_borrow_mut(|leftovers| {
                if let Some(leftovers) = leftovers {
                    leftovers.eden = Some(eden.clone());
                }
            });
            eden.mount()?;
        }
        let (cwd, isolation_prefix) = if self.inplace == Some(true) {
            // Not true of every Linux host, but the tests rely on it.
            if cfg!(target_os = "linux") {
                extra_config.push_str("[host_features]\ngvfs = true\n");
            }
            extra_config.push_str("[buildfile]\nextra_for_test = TARGETS.test\n");
            (cell_root()?, Some(self.isolation_prefix()?))
        } else {
            if let Some(data) = self.data_source()? {
                copy_tree(&data, &project_dir)
                    .with_context(|| format!("Copying the test data from `{}`", data.display()))?;
                set_up_prelude_and_ovr_config(&project_dir, &env)?;
                if is_core_test() {
                    check_core_test_cells(&project_dir)?;
                }
                fs::write(
                    project_dir.join(".watchmanconfig"),
                    // The FSEvents watcher is more reliable than the default.
                    r#"{"ignore_dirs": ["buck-out", ".hg"], "fsevents_watch_files": true, "prefer_split_fsevents_watcher": false}"#,
                )?;
            }
            if self.setup_eden {
                extra_config
                    .push_str("[buck2]\nfile_watcher = edenfs\n[buck2]\nallow_eden_io = true\n");
            } else {
                // The `edenfs` watcher needs EdenFS, which is too slow to set up for every test.
                extra_config.push_str("[buck2]\nfile_watcher = fs_hash_crawler\n");
            }
            (project_dir, None)
        };

        // Core tests run with read-only outputs, under either materializer.
        extra_config.push_str("[buck2]\nreadonly_outputs = true\n");
        // A target variant can run its suite against another materializer implementation; see
        // `buck2_core_tests`.
        match std::env::var("BUCK2_TEST_MATERIALIZER").ok().as_deref() {
            None => {}
            Some("v2") => {
                // Every `ROLLOUT_VERSION` is at least 0.
                extra_config.push_str("[buck2]\nmaterializer_v2_version = 0\n");
                // The disk-state twins run strict: a root leaves disk the moment its last lease
                // does, so a reader without one fails here rather than by chance in use.
                extra_config.push_str("[buck2]\nmaterializer_strict_leases = true\n");
            }
            Some(other) => anyhow::bail!("Unknown `BUCK2_TEST_MATERIALIZER` value `{other}`"),
        }
        for (section, key, value) in &self.extra_buck_config {
            extra_config.push_str(&format!("[{section}]\n{key} = {value}\n"));
        }

        let extra_config_path = base_dir.join("extra.bcfg");
        fs::write(&extra_config_path, extra_config)?;
        env.insert(
            "BUCK2_TEST_EXTRA_EXTERNAL_CONFIG".into(),
            extra_config_path.into_os_string(),
        );
        let settings_home_dir = base_dir.join("settings_home");
        fs::create_dir(&settings_home_dir)?;
        env.insert(
            "BUCK2_TEST_SETTINGS_HOME_DIR".into(),
            settings_home_dir.into_os_string(),
        );

        let mut buck = Buck::new(
            PathBuf::from(executable),
            cwd,
            env,
            self.write_invocation_record,
        );
        if let Some(isolation_prefix) = isolation_prefix {
            buck.set_isolation_prefix(isolation_prefix);
        }
        for (key, value) in &self.env {
            buck.set_env(key, value);
        }
        Ok(buck)
    }

    /// An inplace test case gets an isolation dir of its own, named after the test target and
    /// case as with Python: the dir's paths, and with them the action cache keys of what the test
    /// builds, are the same on every machine.
    fn isolation_prefix(&self) -> anyhow::Result<String> {
        let target = std::env::var("BUCK2_E2E_TARGET")
            .context("`BUCK2_E2E_TARGET` is not set; the buck e2e test macros set it")?;
        let hash = hex::encode(Sha256::digest(format!("{target}{}", self.test_name)));
        // FIXME(T136079642): buck2 on Windows has problems with relative symlinks over 260
        // characters.
        Ok(if cfg!(windows) {
            hash[..5].to_owned()
        } else {
            format!(".buck_e2e{}", &hash[..40])
        })
    }

    /// The directory the project is set up from: the target's test data, or the `data_dir` in it.
    fn data_source(&self) -> anyhow::Result<Option<PathBuf>> {
        let data = std::env::var_os("TEST_REPO_DATA");
        match (&self.data_dir, data) {
            (None, data) => Ok(data.map(PathBuf::from)),
            (Some(data_dir), Some(data)) => Ok(Some(Path::new(&data).join(data_dir))),
            (Some(data_dir), None) => anyhow::bail!(
                "The test asks for `data_dir = \"{data_dir}\"`, but its target has no test data"
            ),
        }
    }
}

/// An EdenFS checkout of an empty repository, with a daemon of its own.
#[derive(Clone)]
struct Eden {
    dir: PathBuf,
    project_dir: PathBuf,
    env: BTreeMap<OsString, OsString>,
}

impl Eden {
    /// Starts the daemon in `dir` and mounts the checkout at `project_dir`.
    fn mount(&self) -> anyhow::Result<()> {
        for sub in ["config", "etc", "home"] {
            fs::create_dir_all(self.dir.join(sub))?;
        }
        run(&mut self.eden(["start"]))?;

        let repo = self.dir.join("temp_repo");
        run(self.command("hg").arg("init").arg(&repo))?;
        // The redirection is set up when the checkout is mounted. The number of APFS volumes macOS
        // can have is limited, and cleaning disk image redirections up is not trivial, so it is a
        // symlink there.
        let redirection = if cfg!(target_os = "macos") {
            "symlink"
        } else {
            "bind"
        };
        fs::write(
            repo.join(".eden-redirections"),
            format!("[redirections]\n\"buck-out\" = \"{redirection}\"\n"),
        )?;
        // Keeps the repository clean when EdenFS creates the redirection.
        fs::write(repo.join(".gitignore"), "/buck-out\n")?;
        run(self
            .command("hg")
            .args(["commit", "--addremove", "-m", "init"])
            .current_dir(&repo))?;

        let mut clone = self.eden(["clone"]);
        clone
            .arg(&repo)
            .arg(&self.project_dir)
            .args(["--allow-empty-repo", "--case-insensitive"]);
        if cfg!(windows) {
            clone.arg("--enable-windows-symlinks");
        }
        run(&mut clone)
    }

    /// Unmounts the checkout and stops the daemon, as well as they go.
    fn remove(&self) {
        let mut remove = self.eden(["remove"]);
        remove.arg(&self.project_dir).arg("-y");
        let _ignored = remove.status();
        let _ignored = self.eden(["shutdown"]).status();
    }

    fn command(&self, program: &str) -> std::process::Command {
        let mut command = std::process::Command::new(program);
        command
            .env_clear()
            .envs(&self.env)
            .stdout(std::io::stderr())
            .stderr(std::io::stderr());
        command
    }

    fn eden<'a>(&self, args: impl IntoIterator<Item = &'a str>) -> std::process::Command {
        let mut command = self.command("eden");
        command
            .arg("--config-dir")
            .arg(self.dir.join("config"))
            .arg("--home-dir")
            .arg(self.dir.join("home"))
            .arg("--etc-eden-dir")
            .arg(self.dir.join("etc"))
            .args(args);
        command
    }
}

fn run(command: &mut std::process::Command) -> anyhow::Result<()> {
    let status = command
        .status()
        .with_context(|| format!("Running {command:?}"))?;
    anyhow::ensure!(status.success(), "{command:?} failed: {status}");
    Ok(())
}

/// Sandcastle Mac VMs boot with fseventsd disabled and ship a script that starts it, which sudo
/// allows. A failure is reported, and the next test in this process tries again.
fn ensure_fseventsd_running(lock_dir: &Path) {
    static READY: AtomicBool = AtomicBool::new(false);
    if READY.load(Ordering::Relaxed) {
        return;
    }
    match start_fseventsd(lock_dir) {
        Ok(()) => READY.store(true, Ordering::Relaxed),
        Err(e) => eprintln!("could not start fseventsd: {e:#}"),
    }
}

fn start_fseventsd(lock_dir: &Path) -> anyhow::Result<()> {
    const KICKSTART: &str = "/usr/local/bin/kickstart-fseventsd";
    fn running(process: &str) -> bool {
        std::process::Command::new("/usr/bin/pgrep")
            .args(["-qx", process])
            .status()
            .is_ok_and(|status| status.success())
    }
    fn wait_until(condition: impl Fn() -> bool, failure: &str) -> anyhow::Result<()> {
        for _ in 0..300 {
            if condition() {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        anyhow::bail!("{failure}")
    }

    // Test processes run in parallel. The lock serializes the watchman restart below, which is per
    // user like the daemon; the kick itself is idempotent.
    let lock = File::create(lock_dir.join("fseventsd.lock"))?;
    lock.lock()?;
    if running("fseventsd") {
        return Ok(());
    }
    anyhow::ensure!(
        Path::new(KICKSTART).is_file(),
        "fseventsd is not running and {KICKSTART} is missing"
    );
    run(std::process::Command::new("/usr/bin/sudo").args(["-n", KICKSTART]))?;
    wait_until(|| running("fseventsd"), "fseventsd did not start")?;
    // A daemon that failed to watch while fseventsd was down may keep failing, and a client that
    // spawns a daemon while the old one is still shutting down times out on its pid lock, so the
    // daemon is replaced.
    let watchman = std::env::var_os("PATH")
        .and_then(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join("watchman"))
                .find(|watchman| watchman.is_file())
        })
        .unwrap_or_else(|| PathBuf::from("/usr/local/bin/watchman"));
    let shut_down = std::process::Command::new(&watchman)
        .args(["--no-spawn", "shutdown-server"])
        .status()
        .is_ok_and(|status| status.success());
    if !shut_down {
        let _ignored = std::process::Command::new("/usr/bin/pkill")
            .args(["-x", "watchman"])
            .status();
    }
    wait_until(
        || !running("watchman"),
        "the old watchman daemon did not exit",
    )?;
    run(std::process::Command::new(&watchman).arg("version"))
}

/// What a test leaves behind, and how to clean it up. Rust tests here are built to abort on a panic,
/// so a test that panics never returns to the harness: a panic hook cleans up instead.
struct Leftovers {
    test_name: String,
    base_dir: PathBuf,
    /// `buck2 kill`, unless the test keeps its daemon.
    kill: Option<std::process::Command>,
    eden: Option<Eden>,
}

thread_local! {
    /// The leftovers of the test running on this thread.
    static LEFTOVERS: RefCell<Option<Leftovers>> = const { RefCell::new(None) };
}

impl Leftovers {
    /// After a failure, the daemon is only stopped.
    fn clean_up_after_failure(mut self) {
        if let Some(kill) = &mut self.kill {
            let _ignored = kill.status();
        }
        self.remove();
    }

    /// Deletes the test's directory, unless `BUCK_E2E_KEEP_TEMP=1`.
    fn remove(self) {
        if keep_temp() {
            eprintln!(
                "Not deleting the temporary directory of {} at {}",
                self.test_name,
                self.base_dir.display()
            );
            return;
        }
        if let Some(eden) = &self.eden {
            eden.remove();
        }
        remove_dir_all(&self.base_dir);
    }
}

fn clean_up_on_panic() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // First: the test runner's hook exits the process.
            if let Ok(Some(leftovers)) = LEFTOVERS.try_with(|l| l.borrow_mut().take()) {
                leftovers.clean_up_after_failure();
            }
            previous(info);
        }));
    });
}

/// Deletes `dir` as well as it goes, after making the read-only directories buck2 leaves writable.
fn remove_dir_all(dir: &Path) {
    if fs::remove_dir_all(dir).is_ok() {
        return;
    }
    fn make_writable(dir: &Path) {
        if let Ok(metadata) = fs::symlink_metadata(dir) {
            if metadata.is_dir() {
                let mut permissions = metadata.permissions();
                #[expect(
                    clippy::permissions_set_readonly_false,
                    reason = "The harness owns the directory and is deleting it"
                )]
                permissions.set_readonly(false);
                let _ignored = fs::set_permissions(dir, permissions);
                if let Ok(entries) = fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        make_writable(&entry.path());
                    }
                }
            }
        }
    }
    make_writable(dir);
    let _ignored = fs::remove_dir_all(dir);
}

const HARD_ERROR_UPGRADE: &str = "Upgraded warning to failure via $BUCK2_HARD_ERROR";

/// A failure message if any daemon the test used panicked. A daemon that panics after answering a
/// command goes unnoticed: the client already has its result and the next command starts a new
/// daemon. So this reads the daemons' stderr after the test, the previous daemon's copy in `prev/`
/// included.
async fn daemon_panic(buck: &Buck) -> Option<String> {
    if !buck.cwd().is_dir() {
        // The test deleted its own project.
        return None;
    }
    // Without a project root there is no daemon.
    let daemon_dir = buck.daemon_dir().await.ok()?;
    let project_daemon_dir = daemon_dir.parent()?;
    let mut daemon_dirs: Vec<PathBuf> = fs::read_dir(project_daemon_dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .collect();
    daemon_dirs.sort();
    let stderrs = daemon_dirs
        .iter()
        .map(|dir| dir.join("buckd.stderr"))
        .chain(
            daemon_dirs
                .iter()
                .map(|dir| dir.join("prev").join("buckd.stderr")),
        );
    for stderr in stderrs {
        let Ok(bytes) = fs::read(&stderr) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        if !text.contains("panicked at") {
            continue;
        }
        let what = if text.contains(HARD_ERROR_UPGRADE) {
            "The buck2 daemon hit a soft error during this test. Tests run with BUCK2_HARD_ERROR, \
             which turns soft errors into panics. If this soft error is expected, mark the test \
             `allow_soft_errors = true` (or `allow_daemon_panics = true` to keep hard errors for \
             everything else)."
        } else {
            "The buck2 daemon panicked during this test. If that is expected, mark the test \
             `allow_daemon_panics = true`."
        };
        return Some(format!(
            "{what} Daemon stderr ({}):\n\n{text}",
            stderr.display()
        ));
    }
    None
}

/// A passed test's buck2 is done: it cleans up after itself, or, to keep its outputs, only stops.
async fn stop(buck: &Buck) -> anyhow::Result<()> {
    if keep_temp() {
        buck.kill().await?;
    } else {
        buck.clean(&[]).await?;
    }
    Ok(())
}

/// The test target's environment gives paths to outputs relative to the project root, where the
/// test starts, and the buck2 under test runs elsewhere: the variables `BUCK2_E2E_PATH_VARS` names,
/// which may be `;`-separated lists of paths, become absolute.
fn absolutize_path_vars(env: &mut BTreeMap<OsString, OsString>, cwd: &Path) {
    let names = env
        .get(OsStr::new("BUCK2_E2E_PATH_VARS"))
        .map(|names| names.to_string_lossy().into_owned())
        .unwrap_or_default();
    for name in names.split(',').filter(|name| !name.is_empty()) {
        if let Some(value) = env.get_mut(OsStr::new(name)) {
            let absolute: Vec<String> = value
                .to_string_lossy()
                .split(';')
                .map(|path| {
                    if path.is_empty() || Path::new(path).is_absolute() {
                        path.to_owned()
                    } else {
                        cwd.join(path).to_string_lossy().into_owned()
                    }
                })
                .collect();
            *value = absolute.join(";").into();
        }
    }
}

fn is_core_test() -> bool {
    std::env::var("BUCK2_E2E_TEST_FLAVOR").as_deref() == Ok("isolated")
}

fn keep_temp() -> bool {
    std::env::var("BUCK_E2E_KEEP_TEMP").as_deref() == Ok("1")
}

fn current_os() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        os => os,
    }
}

/// How long the daemon may live before it terminates itself (with a grace period of 30s, see
/// `self_test_timeout.rs`), so that it goes before tpx kills the test. tpx publishes the test's
/// budget, which already includes the multipliers of labels like `long_running`, in
/// `TPX_TIMEOUT_SEC`; the daemon gets that minus headroom for the grace period and the cleanup,
/// and never less than 600s.
fn self_test_timeout_s(tpx_timeout_s: Option<&str>) -> u64 {
    tpx_timeout_s
        .and_then(|s| s.parse::<u64>().ok())
        .map_or(600, |s| s.saturating_sub(90).max(600))
}

/// Where the test's temporary directory goes: `mkscratch`'s directory, where it has one (the same
/// one on every call), and the system's temporary directory otherwise.
async fn common_dir() -> PathBuf {
    // `--hash` over `--subdir`: the path would be too long for EdenFS's socket otherwise.
    match tokio::process::Command::new("mkscratch")
        .args(["path", "--hash"])
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if path.is_empty() {
                std::env::temp_dir()
            } else {
                // It prints `/` on Windows too; the path gets the platform's separators, as with
                // Python's `pathlib`.
                Path::new(&path).components().collect()
            }
        }
        _ => std::env::temp_dir(),
    }
}

/// A test outside `tests/core` can get a copy of the prelude (`PRELUDE`, the path to it) in its
/// project, and of the cells with the toolchain platform definitions the prelude refers to
/// (`OVR_CONFIG`).
fn set_up_prelude_and_ovr_config(
    project_dir: &Path,
    env: &BTreeMap<OsString, OsString>,
) -> anyhow::Result<()> {
    let prelude = env.get(OsStr::new("PRELUDE"));
    let ovr_config = env.contains_key(OsStr::new("OVR_CONFIG"));
    if prelude.is_some() || ovr_config {
        anyhow::ensure!(
            !is_core_test(),
            "Don't set `PRELUDE` or `OVR_CONFIG` in `tests/core` - these tests are always isolated"
        );
    }
    if let Some(prelude) = prelude
        && !project_dir.join("prelude").exists()
    {
        copy_tree(Path::new(prelude), &project_dir.join("prelude"))?;
    }
    if !ovr_config {
        return Ok(());
    }
    // A Rust test starts in the project root.
    let repo = std::env::current_dir()?;
    for cell in [
        "arvr/tools/build_defs/config",
        "tools/build_defs/fbcode_macros",
    ] {
        copy_tree(&repo.join(cell), &project_dir.join(cell))?;
        fs::write(project_dir.join(cell).join(".buckconfig"), "")?;
    }
    let mut buckconfig = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(project_dir.join(".buckconfig"))?;
    // The fixture's `.buckconfig` may not end with a newline.
    buckconfig.write_all(
        b"\n# Following lines are added by the e2e test harness\n\
        [repositories]\n\
        ovr_config = arvr/tools/build_defs/config\n\
        fbcode_macros = tools/build_defs/fbcode_macros\n\
        config = arvr/tools/build_defs/config\n",
    )?;
    Ok(())
}

/// Where an inplace test runs buck2: the root of the test target's cell, as for a Python test. A
/// Rust test starts in the project root, and `buck_e2e_test` names the cell.
fn cell_root() -> anyhow::Result<PathBuf> {
    let project_root = std::env::current_dir()?;
    let cell = std::env::var("BUCK2_E2E_CELL")
        .context("`BUCK2_E2E_CELL` is not set; the buck e2e test macros set it")?;
    let buckconfig = fs::read_to_string(project_root.join(".buckconfig"))
        .context("Reading the project's `.buckconfig`")?;
    let path = buckconfig_cell_path(&buckconfig, &cell)
        .with_context(|| format!("The project's `.buckconfig` has no cell `{cell}`"))?;
    Ok(project_root.join(path))
}

/// The path of `cell` in a `.buckconfig`'s `[cells]` (or older `[repositories]`) section.
fn buckconfig_cell_path(buckconfig: &str, cell: &str) -> Option<String> {
    let find = |wanted: &str| {
        let mut in_section = false;
        for line in buckconfig.lines().map(str::trim) {
            if let Some(section) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                in_section = section.trim() == wanted;
            } else if in_section
                && let Some((key, value)) = line.split_once('=')
                && key.trim() == cell
            {
                return Some(value.trim().to_owned());
            }
        }
        None
    };
    find("cells").or_else(|| find("repositories"))
}

/// Copies the contents of `src` into `dst`, except `buck-out`, following symlinks. The copies are
/// writable by their owner: the test writes to its copy of fixtures that buck materializes
/// read-only.
fn copy_tree(src: &Path, dst: &Path) -> anyhow::Result<()> {
    fn copy_dir(src: &Path, dst: &Path, top: bool) -> anyhow::Result<()> {
        fs::create_dir_all(dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            if top && entry.file_name() == "buck-out" {
                continue;
            }
            let from = entry.path();
            let to = dst.join(entry.file_name());
            if fs::metadata(&from)
                .with_context(|| format!("Reading `{}`", from.display()))?
                .is_dir()
            {
                copy_dir(&from, &to, false)?;
            } else {
                copy_file(&from, &to)?;
            }
        }
        Ok(())
    }

    fn copy_file(from: &Path, to: &Path) -> anyhow::Result<()> {
        fs::copy(from, to)
            .with_context(|| format!("Copying `{}` to `{}`", from.display(), to.display()))?;
        let metadata = fs::metadata(from)?;
        let mut permissions = metadata.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(permissions.mode() | 0o200);
        }
        #[cfg(not(unix))]
        #[expect(
            clippy::permissions_set_readonly_false,
            reason = "The test writes to its copy"
        )]
        permissions.set_readonly(false);
        fs::set_permissions(to, permissions)?;
        File::options()
            .write(true)
            .open(to)?
            .set_modified(metadata.modified()?)?;
        Ok(())
    }

    copy_dir(src, dst, true)
}

/// Rejects `tests/core` fixtures that use a bundled cell other than the nano prelude: these tests
/// are of buck2 itself, and anything that needs the real prelude belongs in `tests/e2e`.
fn check_core_test_cells(project_dir: &Path) -> anyhow::Result<()> {
    if std::env::var_os("BUCK2_E2E_TEST_ALLOW_BUNDLED_CELLS").is_some() {
        return Ok(());
    }
    let mut buckconfigs = Vec::new();
    find_buckconfigs(project_dir, &mut buckconfigs)?;
    for buckconfig in buckconfigs {
        if let Some(cell) = bundled_cell_other_than_nano_prelude(&fs::read_to_string(&buckconfig)?)
        {
            anyhow::bail!(
                "`{}` sets `{cell} = bundled`, but `nano_prelude` is the only bundled cell \
                 `tests/core` tests may use: they test buck2 itself, not the prelude. Tests that \
                 need prelude code belong in `tests/e2e`; see `tests/README.md`.",
                buckconfig
                    .strip_prefix(project_dir)
                    .unwrap_or(&buckconfig)
                    .display(),
            );
        }
    }
    Ok(())
}

/// The `.buckconfig` files under `dir`, without following symlinks: an EdenFS checkout has some
/// that lead nowhere.
fn find_buckconfigs(dir: &Path, found: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("Listing `{}`", dir.display()))? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            find_buckconfigs(&entry.path(), found)?;
        } else if entry.file_name() == ".buckconfig" {
            found.push(entry.path());
        }
    }
    Ok(())
}

fn bundled_cell_other_than_nano_prelude(buckconfig: &str) -> Option<String> {
    let mut section = "";
    for line in buckconfig.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.starts_with('[') {
            section = line;
        } else if section == "[external_cells]" {
            if let Some((cell, kind)) = line.split_once('=') {
                let (cell, kind) = (cell.trim(), kind.trim());
                if kind == "bundled" && cell != "nano_prelude" {
                    return Some(cell.to_owned());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cells_are_found_in_the_buckconfig() {
        let buckconfig = "[repositories]\n  old = old/path\n\n[cells]\n  root = .\n  fbcode = fbcode\n[other]\n  fbcode = no\n";
        assert_eq!(
            buckconfig_cell_path(buckconfig, "fbcode").as_deref(),
            Some("fbcode")
        );
        assert_eq!(
            buckconfig_cell_path(buckconfig, "root").as_deref(),
            Some(".")
        );
        assert_eq!(
            buckconfig_cell_path(buckconfig, "old").as_deref(),
            Some("old/path")
        );
        assert_eq!(buckconfig_cell_path(buckconfig, "missing"), None);
    }

    #[test]
    fn test_named_path_vars_become_absolute() {
        let absolute = if cfg!(windows) { r"C:\abs\c" } else { "/abs/c" };
        let list = format!("buck-out/b;{absolute}");
        let mut env: BTreeMap<OsString, OsString> = [
            ("BUCK2_E2E_PATH_VARS", "A,LIST"),
            ("A", "buck-out/a"),
            ("LIST", list.as_str()),
            ("OTHER", "buck-out/d"),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect();
        let project = Path::new("/project");
        absolutize_path_vars(&mut env, project);
        let get = |k: &str| env[OsStr::new(k)].to_string_lossy().into_owned();
        assert_eq!(get("A"), project.join("buck-out/a").to_string_lossy());
        assert_eq!(
            get("LIST"),
            format!("{};{absolute}", project.join("buck-out/b").display())
        );
        assert_eq!(get("OTHER"), "buck-out/d");
    }

    #[test]
    fn test_the_daemon_gets_the_tpx_budget_minus_headroom() {
        assert_eq!(self_test_timeout_s(Some("6000")), 5910);
    }

    #[test]
    fn test_the_daemon_gets_at_least_600s() {
        assert_eq!(self_test_timeout_s(Some("120")), 600);
        assert_eq!(self_test_timeout_s(None), 600);
        assert_eq!(self_test_timeout_s(Some("soon")), 600);
    }

    #[test]
    fn test_only_the_nano_prelude_may_be_bundled() {
        assert_eq!(
            bundled_cell_other_than_nano_prelude(
                "[cells]\n  root = .\n[external_cells]\n  nano_prelude = bundled\n"
            ),
            None
        );
        assert_eq!(
            bundled_cell_other_than_nano_prelude(
                "[external_cells]\n  prelude = bundled # the real one\n"
            ),
            Some("prelude".to_owned())
        );
    }

    #[test]
    fn test_bundled_cells_count_only_in_the_external_cells_section() {
        assert_eq!(
            bundled_cell_other_than_nano_prelude("[other]\n  prelude = bundled\n"),
            None
        );
    }

    #[test]
    fn test_copies_skip_buck_out_and_are_writable() -> anyhow::Result<()> {
        let src = tempfile::tempdir()?;
        let dst = tempfile::tempdir()?;
        fs::create_dir_all(src.path().join("sub"))?;
        fs::create_dir_all(src.path().join("buck-out"))?;
        let file = src.path().join("sub/file");
        fs::write(&file, "contents")?;
        let mut permissions = fs::metadata(&file)?.permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&file, permissions)?;

        copy_tree(src.path(), dst.path())?;

        let copy = dst.path().join("sub/file");
        assert_eq!(fs::read_to_string(&copy)?, "contents");
        assert!(!fs::metadata(&copy)?.permissions().readonly());
        assert!(!dst.path().join("buck-out").exists());
        Ok(())
    }
}
