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
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use buck2_agent_interactive::Advice;
use buck2_cli_proto::EvictTenantRequest;
use buck2_client_ctx::client_ctx::BuckSubcommand;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::common::CommonCommandOptions;
use buck2_client_ctx::common::CommonEventLogOptions;
use buck2_client_ctx::common::target_cfg::TargetCfgUnusedOptions;
use buck2_client_ctx::common::ui::ConsoleType;
use buck2_client_ctx::daemon::client::BuckdLifecycleLock;
use buck2_client_ctx::daemon::client::connect::establish_connection_existing;
use buck2_client_ctx::daemon::client::connect::shared_daemon_is_inactive;
use buck2_client_ctx::daemon::client::kill::kill_command_impl;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::final_console::FinalConsole;
use buck2_client_ctx::startup_deadline::StartupDeadline;
use buck2_client_ctx::subscribers::superconsole::StatefulSuperConsole;
use buck2_common::daemon_dir::DaemonDir;
use buck2_common::invocation_paths::TenantDiskLock;
use buck2_core::buck2_env;
use buck2_error::BuckErrorContext;
use buck2_error::ErrorTag;
use buck2_fs::error::IoError;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::abs_path::AbsPath;
use buck2_util::threads::directory_mutation_parallelism;
use dupe::Dupe;
use superconsole::Line;
use superconsole::SuperConsole;
use superconsole::components::Spinner;
use threadpool::ThreadPool;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::commands::clean_stale::CleanStaleCommand;
use crate::commands::clean_stale::parse_clean_stale_args;

/// Delete generated files and caches.
///
/// The command also kills the buck2 daemon.
///
/// With `BUCK2_SHARE_DAEMON=true`, it evicts the selected tenant from the shared daemon instead.
#[derive(Debug, clap::Parser)]
pub struct CleanCommand {
    #[clap(
        long = "dry-run",
        help = "Performs a dry-run and prints the paths that would be removed."
    )]
    dry_run: bool,

    #[clap(
        long = "background",
        help = "Run the clean operation in the background"
    )]
    background: bool,

    /// Command doesn't need these flags, but they are used in mode files, so we need to keep them.
    #[clap(flatten)]
    _target_cfg: TargetCfgUnusedOptions,

    #[clap(flatten)]
    clean_stale_opts: CleanStaleOptions,

    #[clap(flatten)]
    common_opts: CommonCommandOptions,
}

#[derive(Debug, clap::Parser)]
#[clap(next_help_heading = "Clean Stale Options")]
struct CleanStaleOptions {
    // TODO(scottcao): Make stale duration be specified on a separate flag so that
    // there is no potential confusion in behavior between `--stale` and `--stale=7d`
    #[clap(
        long = "stale",
        help = "Delete artifacts from buck-out using the configured clean-stale
policy or a duration if specified, without killing the daemon",
        value_name = "DURATION"
    )]
    stale: Option<Option<humantime::Duration>>,

    // Like stale but since a specific timestamp, for testing
    #[clap(long = "keep-since-time", conflicts_with = "stale", hide = true)]
    keep_since_time: Option<i64>,

    /// Only considers tracked artifacts for cleanup.
    ///
    /// `buck-out` can contain untracked artifacts for different reasons:
    ///  - Outputs from aborted actions
    ///  - State getting deleted (e.g., new buckversion that changes the on-disk state format)
    ///  - Writing to `buck-out` without being expected by Buck
    #[clap(long = "tracked-only", requires = "stale")]
    tracked_only: bool,

    /// Enable adaptive low-disk promotion: after the regular stale scan,
    /// promote retained, non-active artifacts (oldest-access first) to stale
    /// until projected free disk % rises above this threshold (0.0 - 100.0).
    #[clap(
        long = "adaptive-low-disk-threshold",
        value_name = "PERCENT",
        requires = "stale"
    )]
    adaptive_low_disk_threshold: Option<f64>,

    /// Minimum TTL floor for adaptive low-disk promotion: retained artifacts
    /// accessed within this duration of now are never promoted, even under
    /// disk pressure. Ignored unless `--adaptive-low-disk-threshold` is set.
    #[clap(
        long = "adaptive-min-ttl",
        value_name = "DURATION",
        requires = "adaptive_low_disk_threshold",
        default_value = "12h"
    )]
    adaptive_min_ttl: humantime::Duration,

    /// Allow adaptive cleaning to unmaterialize active remote-backed
    /// intermediate artifacts as a final escalation step.
    #[clap(
        long = "adaptive-unmaterialize-active",
        requires = "adaptive_low_disk_threshold"
    )]
    adaptive_unmaterialize_active: bool,
}

impl CleanCommand {
    pub fn exec(
        self,
        matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        if let Some(mut keep_since_arg) = parse_clean_stale_args(
            self.clean_stale_opts.stale,
            self.clean_stale_opts.keep_since_time,
        )? {
            if let Some(t) = self.clean_stale_opts.adaptive_low_disk_threshold {
                if !(0.0..=100.0).contains(&t) || t.is_nan() {
                    return ExitResult::bail(format!(
                        "`--adaptive-low-disk-threshold` must be between 0.0 and 100.0, got `{t}`"
                    ));
                }
                if matches!(
                    keep_since_arg,
                    crate::commands::clean_stale::KeepSinceArg::Configured
                ) {
                    keep_since_arg = crate::commands::clean_stale::KeepSinceArg::Duration(
                        jiff::SignedDuration::from_hours(24 * 7),
                    );
                }
            }
            let cmd = CleanStaleCommand {
                common_opts: self.common_opts,
                keep_since_arg,
                dry_run: self.dry_run,
                tracked_only: self.clean_stale_opts.tracked_only,
                adaptive_low_disk_threshold: self.clean_stale_opts.adaptive_low_disk_threshold,
                adaptive_min_ttl: Some(self.clean_stale_opts.adaptive_min_ttl.into()),
                adaptive_unmaterialize_active: self.clean_stale_opts.adaptive_unmaterialize_active,
            };
            ctx.exec(cmd, matches, events_ctx)
        } else {
            ctx.exec(
                InnerCleanCommand {
                    dry_run: self.dry_run,
                    background: self.background,
                    common_opts: self.common_opts,
                },
                matches,
                events_ctx,
            )
        }
    }

    pub fn command_name(&self) -> &'static str {
        if let Ok(Some(_)) = parse_clean_stale_args(
            self.clean_stale_opts.stale,
            self.clean_stale_opts.keep_since_time,
        ) {
            "clean-stale"
        } else {
            "clean"
        }
    }
}

struct CleanGuard {
    lifecycle: Option<BuckdLifecycleLock>,
    disk_lock: Arc<TenantDiskLock>,
}

struct InnerCleanCommand {
    dry_run: bool,
    background: bool,
    common_opts: CommonCommandOptions,
}

impl BuckSubcommand for InnerCleanCommand {
    const COMMAND_NAME: &'static str = "clean";

    async fn exec_impl(
        self,
        _matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let paths = ctx.paths()?;
        let buck_out_dir = paths.buck_out_path();
        let daemon_dir = paths.daemon_dir()?;
        let sharing = buck2_env!("BUCK2_SHARE_DAEMON", type=bool, default=false)?;
        let metadata_to_clean = (!sharing).then(|| daemon_dir.clone());
        let trash_dir = paths.trash_dir();
        let console = &self.common_opts.console_opts.final_console();

        if self.dry_run {
            return clean(
                buck_out_dir,
                metadata_to_clean,
                trash_dir,
                console,
                self.common_opts.console_opts.console_type,
                None,
                self.background,
            )
            .await
            .into();
        }

        events_ctx
            .handle_agent_advice(ctx.trace_id.dupe(), ctx.agent_advice(Advice::Clean)?)
            .await?;

        // Kill the daemon and make sure a new daemon does not spin up while we're performing clean up operations
        // This will ensure we have exclusive access to the directories in question
        let lifecycle = if sharing {
            match establish_connection_existing(&daemon_dir).await {
                Ok(client) => {
                    client
                        .to_connector()
                        .with_flushing()
                        .evict_tenant(
                            EvictTenantRequest {
                                context: Some(ctx.empty_client_context(Self::COMMAND_NAME)?),
                            },
                            events_ctx,
                        )
                        .await?;
                    None
                }
                Err(error) => {
                    let lock = BuckdLifecycleLock::lock_with_timeout(
                        daemon_dir.clone(),
                        StartupDeadline::duration_from_now(Duration::from_secs(10))?,
                    )
                    .await?;
                    if !shared_daemon_is_inactive(&daemon_dir).await? {
                        return error.into();
                    }
                    Some(lock)
                }
            }
        } else {
            let lock = BuckdLifecycleLock::lock_with_timeout(
                daemon_dir.clone(),
                StartupDeadline::duration_from_now(Duration::from_secs(10))?,
            )
            .await?;
            kill_command_impl(&lock, "`buck2 clean` was invoked", false).await?;
            Some(lock)
        };
        let disk_lock = TenantDiskLock::acquire(&paths.tenant_paths(), Duration::from_secs(30))
            .await
            .buck_error_context("Waiting for tenant disk work to drain; no files were deleted")?;
        let guard = CleanGuard {
            lifecycle,
            disk_lock: Arc::new(disk_lock),
        };

        clean(
            buck_out_dir,
            metadata_to_clean,
            trash_dir,
            console,
            self.common_opts.console_opts.console_type,
            Some(&guard),
            self.background,
        )
        .await
        .into()
    }

    fn event_log_opts(&self) -> &CommonEventLogOptions {
        &self.common_opts.event_log_opts
    }
}

async fn clean(
    buck_out_dir: AbsNormPathBuf,
    daemon_dir: Option<DaemonDir>,
    trash_dir: AbsNormPathBuf,
    console: &FinalConsole,
    console_type: ConsoleType,
    // None means "dry run".
    guard: Option<&CleanGuard>,
    background: bool,
) -> buck2_error::Result<()> {
    let paths_to_clean = if background && guard.is_some() {
        let trash_uuid = Uuid::new_v4();
        let trash_target = trash_dir.as_abs_path().join(trash_uuid.to_string());

        // Create trash directory if it doesn't exist
        if !trash_dir.exists() {
            fs_util::create_dir_all(&trash_dir)?;
        }

        // Move buck-out to trash folder
        if buck_out_dir.exists() {
            console.print_stderr(&format!(
                "Moving {} to {}",
                buck_out_dir.display(),
                trash_target.display()
            ))?;
            fs_util::rename(&buck_out_dir, &trash_target)
                .categorize_tagged(ErrorTag::CleanBuckOut)?;
        }

        // Clean the daemon_dir first
        let mut paths_to_clean = Vec::new();
        if let Some(daemon_dir) = &daemon_dir
            && daemon_dir.path.exists()
        {
            paths_to_clean.push(daemon_dir.to_string());
            if let Some(lock) = guard.and_then(|guard| guard.lifecycle.as_ref()) {
                lock.clean_daemon_dir(false)?;
            }
        }

        console.print_stderr("Buck-out moved to trash. Now cleaning up...")?;
        console.print_stderr(
            "Tip: Use Ctrl-Z to put this in the background, or run in a new terminal.",
        )?;
        console.print_stderr("You can run other buck2 commands while this completes.")?;

        // Delete the moved directory
        let trash_target_normalized = AbsNormPathBuf::new(trash_target.to_path_buf())?;
        if trash_target_normalized.exists() {
            paths_to_clean.extend(
                collect_paths_to_clean(&trash_target_normalized)?
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>(),
            );
            let disk_lock = guard.map(|guard| guard.disk_lock.clone());
            tokio::task::spawn_blocking(move || {
                let _disk_lock = disk_lock;
                clean_buck_out_with_retry(&trash_target_normalized, console_type)
            })
            .await?
            .buck_error_context("Failed to spawn clean")?;
        }
        paths_to_clean
    } else {
        let mut paths_to_clean = Vec::new();

        if buck_out_dir.exists() {
            paths_to_clean = collect_paths_to_clean(&buck_out_dir)?
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>();
            if let Some(guard) = guard {
                let disk_lock = guard.disk_lock.clone();
                tokio::task::spawn_blocking(move || {
                    let _disk_lock = disk_lock;
                    clean_buck_out_with_retry(&buck_out_dir, console_type)
                })
                .await?
                .buck_error_context("Failed to spawn clean")?;
            }
        }

        if let Some(daemon_dir) = &daemon_dir
            && daemon_dir.path.exists()
        {
            paths_to_clean.push(daemon_dir.to_string());
            if let Some(lock) = guard.and_then(|guard| guard.lifecycle.as_ref()) {
                lock.clean_daemon_dir(false)?;
            }
        }

        paths_to_clean
    };

    if paths_to_clean.is_empty() {
        console.print_stderr("Nothing to clean.")?;
    }
    for path in paths_to_clean {
        console.print_stderr(&path)?;
    }

    Ok(())
}

fn collect_paths_to_clean(
    buck_out_path: &AbsNormPathBuf,
) -> buck2_error::Result<Vec<AbsNormPathBuf>> {
    if !buck_out_path.exists() {
        return Ok(vec![]);
    }
    let mut paths_to_clean = vec![];
    let dir = fs_util::read_dir(buck_out_path).categorize_tagged(ErrorTag::CleanBuckOut)?;
    for entry in dir {
        let entry = entry?;
        let path = entry.path();
        paths_to_clean.push(path);
    }

    Ok(paths_to_clean)
}

/// Upper bound on whole-tree removal passes. A pass attempts every remaining entry exactly
/// once; a later pass re-walks whatever could not be removed plus anything created since
/// (e.g. by stray processes left over from killed builds, or — on Windows — files the
/// just-killed daemon still held open, failing with `The process cannot access the file
/// because it is being used by another process`).
const CLEAN_PASSES: usize = 3;

/// Minimum spacing between pass starts, so a process that is still writing gets a moment to
/// finish before the tree is re-walked.
const CLEAN_PASS_SPACING: Duration = Duration::from_secs(1);

fn clean_buck_out_with_retry(
    path: &AbsNormPathBuf,
    console_type: ConsoleType,
) -> buck2_error::Result<()> {
    let state = Arc::new(CleanProgressState::new());

    // Show progress using superconsole, respecting the --console option.
    // Use the same console_builder() as other buck2 commands to ensure consistent behavior.
    let _progress_handle = match console_type {
        ConsoleType::None
        | ConsoleType::Simple
        | ConsoleType::SimpleNoTty
        | ConsoleType::SimpleTty => None,
        ConsoleType::Auto | ConsoleType::Super => StatefulSuperConsole::console_builder()
            .build()
            .ok()
            .flatten()
            .map(|console| CleanProgressHandle::new(state.dupe(), console)),
    };

    let mut pass = 0;
    loop {
        pass += 1;
        let pass_start = Instant::now();
        let removed_before = state.files_deleted() + state.dirs_deleted();
        let outcome = clean_buck_out_pass(path, &state);
        let removed = state.files_deleted() + state.dirs_deleted() - removed_before;

        let Some(e) = outcome.first_error else {
            return Ok(());
        };
        // Even a pass that removed nothing is worth retrying: a file that was merely in
        // use (e.g. still held by the just-killed daemon on Windows) or contended by a
        // racing process may be removable a moment later.
        if pass >= CLEAN_PASSES {
            return Err(e);
        }
        tracing::info!(
            "Retrying buck-out clean: {} paths could not be removed ({} removed this pass): {:#}",
            outcome.failed,
            removed,
            e
        );
        if let Some(remaining) = CLEAN_PASS_SPACING.checked_sub(Instant::now() - pass_start) {
            std::thread::sleep(remaining);
        }
    }
}

/// State shared between the progress display and the deletion threads.
struct CleanProgressState {
    files_deleted: Arc<AtomicUsize>,
    dirs_deleted: Arc<AtomicUsize>,
    start_time: Instant,
}

impl CleanProgressState {
    fn new() -> Self {
        Self {
            files_deleted: Arc::new(AtomicUsize::new(0)),
            dirs_deleted: Arc::new(AtomicUsize::new(0)),
            start_time: Instant::now(),
        }
    }

    fn file_counter(&self) -> Arc<AtomicUsize> {
        self.files_deleted.dupe()
    }

    fn dir_counter(&self) -> Arc<AtomicUsize> {
        self.dirs_deleted.dupe()
    }

    fn files_deleted(&self) -> usize {
        self.files_deleted.load(Ordering::Relaxed)
    }

    fn dirs_deleted(&self) -> usize {
        self.dirs_deleted.load(Ordering::Relaxed)
    }

    fn format_message(&self) -> Line {
        let elapsed = Instant::now() - self.start_time;
        Line::sanitized(&format!(
            "Cleaning buck-out: {} files and {} directories deleted ({}s)",
            self.files_deleted(),
            self.dirs_deleted(),
            elapsed.as_secs()
        ))
    }

    fn format_final_message(&self) -> Line {
        let elapsed = Instant::now() - self.start_time;
        Line::sanitized(&format!(
            "Cleaned {} files and {} directories in {:.1}s",
            self.files_deleted(),
            self.dirs_deleted(),
            elapsed.as_secs_f64()
        ))
    }
}

/// Runs the progress display loop using superconsole.
fn run_superconsole_progress(
    mut console: SuperConsole,
    state: &CleanProgressState,
    stop: impl Fn() -> bool,
) {
    let mut tick = 0;
    while !stop() {
        let spinner = Spinner::new(tick, state.format_message());
        if console.render(&spinner).is_err() {
            break;
        }
        tick += 1;
        std::thread::sleep(Duration::from_millis(100));
    }
    // Finalize with the final message (no spinner prefix in Final mode)
    let final_spinner = Spinner::new(tick, state.format_final_message());
    drop(console.finalize(&final_spinner));
}

/// Handle for the superconsole-based progress display.
/// When dropped, it stops the display thread and shows the completion message.
struct CleanProgressHandle {
    stop_flag: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl CleanProgressHandle {
    fn new(state: Arc<CleanProgressState>, console: SuperConsole) -> Self {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_clone = stop_flag.dupe();

        let handle = std::thread::spawn(move || {
            run_superconsole_progress(console, &state, || stop_flag_clone.load(Ordering::Relaxed));
        });

        Self {
            stop_flag,
            handle: Some(handle),
        }
    }
}

impl Drop for CleanProgressHandle {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            drop(handle.join());
        }
    }
}

/// Failures within a pass are counted and the first error kept; nothing aborts mid-pass, so
/// everything removable is removed before the pass reports.
struct CleanFailures {
    first_error: Mutex<Option<buck2_error::Error>>,
    failed: AtomicUsize,
}

impl CleanFailures {
    fn new() -> Self {
        Self {
            first_error: Mutex::new(None),
            failed: AtomicUsize::new(0),
        }
    }

    fn record(&self, e: buck2_error::Error) {
        self.failed.fetch_add(1, Ordering::Relaxed);
        let mut error = self.first_error.lock().unwrap();
        if error.is_none() {
            *error = Some(e);
        }
    }
}

/// What a single removal pass ran into; `first_error` is `None` when nothing failed.
struct CleanPassOutcome {
    failed: usize,
    first_error: Option<buck2_error::Error>,
}

/// A removal that finds nothing to remove has achieved its goal: something else got there first.
fn ok_if_not_found(res: Result<(), IoError>) -> buck2_error::Result<()> {
    match res {
        Err(e) if e.io_error_kind() == Some(io::ErrorKind::NotFound) => Ok(()),
        res => res
            .categorize_tagged(ErrorTag::CleanBuckOut)
            .map_err(Into::into),
    }
}

/// Removes a file, or a directory whose contents were just removed. Below the top level, the
/// containing directory is being removed as well, so the `_harder` variants may add owner
/// permissions to it; the root survives the clean, so its permissions are left alone. A directory
/// the walk couldn't list (no owner read or execute permission) still has its contents, so fall
/// back to `remove_dir_all`, which adds owner permissions to every directory in the tree.
fn remove_entry(root: &AbsPath, path: &AbsPath, is_dir: bool) -> Result<(), IoError> {
    let in_root = path.parent() == Some(root);
    if !is_dir {
        return if in_root {
            fs_util::remove_file(path)
        } else {
            fs_util::remove_file_harder(path)
        };
    }
    let res = if in_root {
        fs_util::remove_dir(path)
    } else {
        fs_util::remove_dir_harder(path)
    };
    match res {
        Err(e) if e.io_error_kind() == Some(io::ErrorKind::DirectoryNotEmpty) => {
            fs_util::remove_dir_all(path)
        }
        res => res,
    }
}

/// Chooses disjoint subtrees whose removal can proceed in parallel.
///
/// Starting from the top-level directories, descends level by level until at least `target_units`
/// subtree roots are available or the tree runs out. Returns the directories that were descended
/// past, in top-down order — they can only be removed after the subtrees below them — and the
/// subtree roots.
///
/// Generic over the directory representation so the logic can be tested without a filesystem;
/// `list_child_dirs` enumerates the immediate sub-directories of a directory.
fn split_into_subtree_roots<T>(
    top_level_dirs: Vec<T>,
    target_units: usize,
    mut list_child_dirs: impl FnMut(&T) -> Vec<T>,
) -> (Vec<T>, Vec<T>) {
    let mut above_split = Vec::new();
    let mut frontier = top_level_dirs;
    while frontier.len() < target_units {
        let next: Vec<T> = frontier.iter().flat_map(&mut list_child_dirs).collect();
        if next.is_empty() {
            break;
        }
        above_split.append(&mut frontier);
        frontier = next;
    }
    (above_split, frontier)
}

/// One removal pass over the whole tree: every remaining entry gets exactly one removal
/// attempt; failures are counted and left for the caller to decide whether another pass is
/// worthwhile.
fn clean_buck_out_pass(path: &AbsNormPathBuf, state: &Arc<CleanProgressState>) -> CleanPassOutcome {
    let failures = Arc::new(CleanFailures::new());

    let file_counter = state.file_counter();
    let dir_counter = state.dir_counter();

    // Errors here are tolerated the same way walk errors are below: anything missed is picked up
    // by a later pass.
    let list_child_dirs = |dir: &AbsNormPathBuf| match fs_util::read_dir(dir) {
        Ok(entries) => entries
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
            .map(|entry| entry.path())
            .collect(),
        Err(_) => Vec::new(),
    };

    // Note that the root itself is never removed: buck's cwd is typically the directory that is
    // passed in here, which means that on Windows we often fail to delete it if we don't clean up
    // all our child processes. Leaving zombies around isn't great though...
    //
    // Aim for several subtrees per worker so that uneven subtree sizes still balance out.
    let (above_split, subtree_roots) = split_into_subtree_roots(
        list_child_dirs(path),
        directory_mutation_parallelism() * 4,
        list_child_dirs,
    );

    let pool = ThreadPool::new(directory_mutation_parallelism());
    for subtree_root in subtree_roots {
        let root = path.clone();
        let failures = failures.dupe();
        let dir_counter = dir_counter.dupe();
        let file_counter = file_counter.dupe();
        pool.execute(move || {
            // `contents_first` yields every directory after its contents, so each directory can
            // be removed the moment it is reached: its files were just unlinked and its
            // subdirectories already removed.
            let walk = WalkDir::new(&subtree_root).contents_first(true);
            for dir_entry in walk.into_iter().flatten() {
                let is_dir = dir_entry.file_type().is_dir();
                // The walk gives us back absolute paths since we give it absolute paths.
                let res = AbsPath::new(dir_entry.path())
                    .and_then(|p| ok_if_not_found(remove_entry(root.as_abs_path(), p, is_dir)));

                match res {
                    Ok(()) => {
                        let counter = if is_dir { &dir_counter } else { &file_counter };
                        counter.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(e) => failures.record(e),
                }
            }
        })
    }
    pool.join();

    // From here on, errors are still only counted, never returned immediately: undeletable
    // entries (e.g. root-owned files left by tests) should not stop everything else from being
    // removed.

    // These only become removable once the subtrees below them are gone. The subtree walks
    // cover only the split roots' subtrees, so a descended-past directory still owns its
    // direct non-directory entries (e.g. `v2/forkserver`'s socket); unlink those before
    // removing the directory itself.
    for dir in above_split.iter().rev() {
        if let Ok(entries) = fs_util::read_dir(dir) {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    continue;
                }
                match ok_if_not_found(remove_entry(
                    path.as_abs_path(),
                    entry.path().as_abs_path(),
                    false,
                )) {
                    Ok(()) => {
                        file_counter.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(e) => failures.record(e),
                }
            }
        }
        match ok_if_not_found(remove_entry(path.as_abs_path(), dir.as_abs_path(), true)) {
            Ok(()) => {
                dir_counter.fetch_add(1, Ordering::Relaxed);
            }
            Err(e) => failures.record(e),
        }
    }

    // Sweeps up what the subtree walks don't cover: non-directory entries in the root itself,
    // and directories that appeared after the split enumeration. Anything deeper that failed
    // above is re-walked by the next pass, not retried here.
    match fs_util::read_dir(path).categorize_tagged(ErrorTag::CleanBuckOut) {
        Ok(entries) => {
            for entry in entries.flatten() {
                // `file_type()` can fail transiently; re-stat rather than miscounting a
                // directory as a file in the progress totals.
                let is_dir = match entry.file_type() {
                    Ok(t) => t.is_dir(),
                    Err(_) => std::fs::symlink_metadata(entry.path())
                        .is_ok_and(|m| m.file_type().is_dir()),
                };
                let res = ok_if_not_found(remove_entry(
                    path.as_abs_path(),
                    entry.path().as_abs_path(),
                    is_dir,
                ));
                match res {
                    Ok(()) => {
                        let counter = if is_dir { &dir_counter } else { &file_counter };
                        counter.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(e) => failures.record(e),
                }
            }
        }
        Err(e) => failures.record(e.into()),
    }

    CleanPassOutcome {
        failed: failures.failed.load(Ordering::Relaxed),
        first_error: failures.first_error.lock().unwrap().take(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[cfg(unix)]
    #[test]
    fn test_clean_readonly_directory_with_empty_children() -> buck2_error::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let tempdir = tempfile::tempdir()?;
        let root = AbsNormPathBuf::new(tempdir.path().to_path_buf())?;
        let parent = tempdir.path().join("delta");
        std::fs::create_dir_all(parent.join("dev"))?;
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o555))?;

        let state = Arc::new(CleanProgressState::new());
        let outcome = clean_buck_out_pass(&root, &state);
        drop(std::fs::set_permissions(
            &parent,
            std::fs::Permissions::from_mode(0o755),
        ));

        assert_eq!(outcome.failed, 0, "{:?}", outcome.first_error);
        assert_eq!(std::fs::read_dir(&root)?.count(), 0);
        assert_eq!(state.dirs_deleted(), 2);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn test_clean_unlistable_directory() -> buck2_error::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let tempdir = tempfile::tempdir()?;
        let root = AbsNormPathBuf::new(tempdir.path().to_path_buf())?;
        let locked = tempdir.path().join("gamma").join("locked");
        std::fs::create_dir_all(locked.join("dir"))?;
        std::fs::write(locked.join("file"), b"content")?;
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000))?;

        let state = Arc::new(CleanProgressState::new());
        let outcome = clean_buck_out_pass(&root, &state);
        drop(std::fs::set_permissions(
            &locked,
            std::fs::Permissions::from_mode(0o755),
        ));

        assert_eq!(outcome.failed, 0, "{:?}", outcome.first_error);
        assert_eq!(std::fs::read_dir(&root)?.count(), 0);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn test_clean_leaves_root_permissions_alone() -> buck2_error::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let tempdir = tempfile::tempdir()?;
        let root_path = tempdir.path().join("root");
        std::fs::create_dir_all(root_path.join("dir"))?;
        std::fs::write(root_path.join("file"), b"content")?;
        let root = AbsNormPathBuf::new(root_path)?;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o555))?;

        let state = Arc::new(CleanProgressState::new());
        // Depending on capabilities (e.g. CAP_DAC_OVERRIDE) the removal may succeed or fail;
        // either way the root's permissions must be left untouched.
        drop(clean_buck_out_pass(&root, &state));
        let mode = std::fs::symlink_metadata(&root)?.permissions().mode();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755))?;

        assert_eq!(mode & 0o777, 0o555);
        Ok(())
    }

    fn fixture<'a>(
        children: &'a BTreeMap<&'static str, Vec<&'static str>>,
    ) -> impl FnMut(&&'static str) -> Vec<&'static str> + 'a {
        move |dir| children.get(dir).cloned().unwrap_or_default()
    }

    #[test]
    fn test_split_descends_until_enough_subtree_roots() {
        let children = BTreeMap::from([("a", vec!["a/b", "a/c"]), ("d", vec!["d/e"])]);
        let (above_split, roots) = split_into_subtree_roots(vec!["a", "d"], 3, fixture(&children));
        assert_eq!(
            above_split,
            vec!["a", "d"],
            "descended-past directories are removed after the subtrees below them"
        );
        assert_eq!(
            roots,
            vec!["a/b", "a/c", "d/e"],
            "3 roots meet the target of 3"
        );
    }

    #[test]
    fn test_split_stops_at_top_level_when_wide_enough() {
        let children = BTreeMap::from([("a", vec!["a/b"])]);
        let (above_split, roots) =
            split_into_subtree_roots(vec!["a", "b", "c"], 2, fixture(&children));
        assert!(above_split.is_empty(), "no descent needed, nothing above");
        assert_eq!(
            roots,
            vec!["a", "b", "c"],
            "top level already meets the target of 2"
        );
    }

    #[test]
    fn test_split_narrow_chain_descends_to_the_bottom() {
        let children = BTreeMap::from([("a", vec!["a/b"]), ("a/b", vec!["a/b/c"])]);
        let (above_split, roots) = split_into_subtree_roots(vec!["a"], 4, fixture(&children));
        assert_eq!(
            above_split,
            vec!["a", "a/b"],
            "the whole chain above the deepest level is descended past, in top-down order"
        );
        assert_eq!(
            roots,
            vec!["a/b/c"],
            "a chain never widens, so descent stops at the leaf"
        );
    }

    #[test]
    fn test_split_leaf_directories_move_above_when_descending() {
        // `d` has no children; descending past it must still keep it for later removal.
        let children = BTreeMap::from([("a", vec!["a/b", "a/c", "a/x"])]);
        let (above_split, roots) = split_into_subtree_roots(vec!["a", "d"], 4, fixture(&children));
        assert_eq!(
            above_split,
            vec!["a", "d"],
            "the childless `d` is kept in the above-split list"
        );
        assert_eq!(roots, vec!["a/b", "a/c", "a/x"]);
    }

    #[test]
    fn test_split_empty() {
        let (above_split, roots) =
            split_into_subtree_roots(Vec::<&str>::new(), 4, |_: &&str| -> Vec<&str> {
                unreachable!("there are no directories to list")
            });
        assert!(above_split.is_empty(), "no directories, nothing above");
        assert!(roots.is_empty(), "no directories, no subtree roots");
    }
}
