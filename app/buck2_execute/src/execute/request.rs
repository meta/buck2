/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;
use std::time::Duration;

use allocative::Allocative;
use buck2_artifact::artifact::output_type::OutputType;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_common::local_resource_state::LocalResourceState;
use buck2_core::content_hash::ContentBasedPathHash;
use buck2_core::execution_types::executor_config::MetaInternalExtraParams;
use buck2_core::execution_types::executor_config::ReGangWorker;
use buck2_core::execution_types::executor_config::RemoteExecutorCustomImage;
use buck2_core::execution_types::executor_config::RemoteExecutorDependency;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_core::fs::buck_out_path::BuckOutScratchPath;
use buck2_core::fs::buck_out_path::BuckOutTestPath;
use buck2_core::fs::buck_out_path::BuildArtifactPath;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
pub use buck2_data::NetworkAccess;
use buck2_directory::directory::dashmap_directory_interner::DashMapDirectoryInterner;
use buck2_directory::directory::directory::Directory;
use buck2_directory::directory::directory_iterator::DirectoryIterator;
use buck2_directory::directory::fingerprinted_directory::FingerprintedDirectory;
use buck2_error::buck2_error;
use buck2_hash::BuckFixedSortedMap;
use buck2_hash::BuckFixedSortedSet;
use buck2_hash::BuckMutInsertionSet;
use derive_more::Display;
use dupe::Dupe;
use host_sharing::host_sharing::HostSharingRequirements;
use itertools::Itertools;
use pagable::Pagable;
use prost::Message;
use remote_execution as RE;

use super::dep_file_digest::DepFileDigest;
use crate::artifact::group::artifact_group_values_dyn::ArtifactGroupValuesDyn;
use crate::digest_config::DigestConfig;
use crate::directory::ActionDirectoryEntry;
use crate::directory::ActionDirectoryMember;
use crate::directory::ActionImmutableDirectory;
use crate::directory::ActionSharedDirectory;
use crate::execute::environment_inheritance::EnvironmentInheritance;
use crate::execute::inputs_directory::inputs_directory;

/// What protobuf messages can be stored in the action metadata blobs.
pub trait ActionMetadataBlobMessage: Message {}

impl ActionMetadataBlobMessage for RE::Action {}
impl ActionMetadataBlobMessage for RE::Command {}
impl ActionMetadataBlobMessage for RE::Tree {}

#[derive(Clone)]
pub struct ActionMetadataBlobData(pub Vec<u8>);

impl ActionMetadataBlobData {
    pub fn from_message(m: &impl ActionMetadataBlobMessage) -> ActionMetadataBlobData {
        let mut blob = Vec::new();
        // Unwrap is safe because it only fails in OOM conditions, which we pretend don't happen
        m.encode(&mut blob).unwrap();
        ActionMetadataBlobData(blob)
    }

    pub fn from_json(json: String) -> ActionMetadataBlobData {
        ActionMetadataBlobData(json.into_bytes())
    }
}

#[derive(Clone)]
pub struct ActionMetadataBlob {
    pub digest: TrackedFileDigest,
    pub path: BuildArtifactPath,
    pub content_hash: ContentBasedPathHash,
}

pub enum CommandExecutionInput {
    Artifact(Box<dyn ArtifactGroupValuesDyn>),
    ActionMetadata(ActionMetadataBlob),
    ScratchPath(BuckOutScratchPath),
    IncrementalRemoteOutput(
        ProjectRelativePathBuf,
        ActionDirectoryEntry<ActionSharedDirectory>,
    ),
}

#[derive(PartialEq, Eq, Debug, Copy, Clone, Dupe, Hash)]
pub enum OutputCreationBehavior {
    Create,
    Parent,
}

#[derive(Debug, buck2_error::Error)]
#[error("Incompatible executor preferences: `{}` & `{}`", a, b)]
#[buck2(input)]
struct IncompatibleExecutorPreferences {
    a: ExecutorPreference,
    b: ExecutorPreference,
}

#[derive(Copy, Clone, Dupe, Display, Debug, Allocative, PartialEq, Pagable)]
pub enum ExecutorPreference {
    Default,
    /// Fails when executed by a remote-only executor
    LocalRequired,
    /// Does not fail when executed by a remote-only executor
    LocalPreferred,
    /// Fails when executed by a local-only executor
    RemoteRequired,
    /// Does not fail when executed by a local-only executor
    RemotePreferred,
    /// When and-ed with an ExecutorPreference that doesn't *require* local or remote, this erases
    /// any preferences.
    DefaultErasePreferences,
}

impl ExecutorPreference {
    pub fn and(self, other: Self) -> buck2_error::Result<Self> {
        let requires_remote = self.requires_remote() || other.requires_remote();
        let requires_local = self.requires_local() || other.requires_local();

        if requires_remote && requires_local {
            return Err(IncompatibleExecutorPreferences { a: self, b: other }.into());
        }

        if requires_local {
            return Ok(Self::LocalRequired);
        }

        if requires_remote {
            return Ok(Self::RemoteRequired);
        }

        if self.erases_preferences() || other.erases_preferences() {
            return Ok(Self::DefaultErasePreferences);
        }

        for pref in [self, other] {
            if pref.prefers_local() {
                return Ok(Self::LocalPreferred);
            }

            if pref.prefers_remote() {
                return Ok(Self::RemotePreferred);
            }
        }

        Ok(Self::Default)
    }

    pub fn requires_remote(&self) -> bool {
        match self {
            Self::LocalRequired => false,
            Self::LocalPreferred => false,
            Self::RemoteRequired => true,
            Self::RemotePreferred => false,
            Self::Default => false,
            Self::DefaultErasePreferences => false,
        }
    }

    pub fn requires_local(&self) -> bool {
        match self {
            Self::LocalRequired => true,
            Self::LocalPreferred => false,
            Self::RemoteRequired => false,
            Self::RemotePreferred => false,
            Self::Default => false,
            Self::DefaultErasePreferences => false,
        }
    }

    pub fn prefers_local(&self) -> bool {
        match self {
            Self::LocalRequired => true,
            Self::LocalPreferred => true,
            Self::RemoteRequired => false,
            Self::RemotePreferred => false,
            Self::Default => false,
            Self::DefaultErasePreferences => false,
        }
    }

    pub fn prefers_remote(&self) -> bool {
        match self {
            Self::LocalRequired => false,
            Self::LocalPreferred => false,
            Self::RemoteRequired => true,
            Self::RemotePreferred => true,
            Self::Default => false,
            Self::DefaultErasePreferences => false,
        }
    }

    fn erases_preferences(self) -> bool {
        matches!(self, Self::DefaultErasePreferences)
    }
}

pub struct CommandExecutionPaths {
    inputs: Vec<CommandExecutionInput>,
    outputs: BuckMutInsertionSet<CommandExecutionOutput>,

    input_directory: ActionImmutableDirectory,
    output_paths: Vec<(ProjectRelativePathBuf, OutputType)>,

    /// Total size of input files.
    input_files_bytes: u64,
}

impl CommandExecutionPaths {
    pub fn new(
        inputs: Vec<CommandExecutionInput>,
        outputs: BuckMutInsertionSet<CommandExecutionOutput>,
        fs: &ArtifactFs,
        digest_config: DigestConfig,
        interner: Option<&DashMapDirectoryInterner<ActionDirectoryMember, TrackedFileDigest>>,
    ) -> buck2_error::Result<Self> {
        let mut builder = inputs_directory(&inputs, digest_config, fs)?;

        // RE spec requires outputs to be sorted:
        // https://github.com/bazelbuild/remote-apis/blob/1f36c310b28d762b258ea577ed08e8203274efae/build/bazel/remote/execution/v2/remote_execution.proto#L667-L669
        // We sort early here and not when we create RE action in order for local and remote actions to be in-sync.
        let outputs: BuckMutInsertionSet<_> = outputs
            .into_iter()
            .sorted_by_key(|e| {
                let resolved = e
                    .as_ref()
                    .resolve(fs, Some(&ContentBasedPathHash::for_output_artifact()))
                    .expect("Failed to resolve output path");
                resolved.into_path()
            })
            .collect();

        let output_paths = outputs
            .iter()
            .map(|o| {
                let resolved = o
                    .as_ref()
                    .resolve(fs, Some(&ContentBasedPathHash::for_output_artifact()))?;
                if let Some(dir) = resolved.path_to_create() {
                    builder.mkdir(dir)?;
                }
                let output_type = resolved.output_type;
                Ok((resolved.into_path(), output_type))
            })
            .collect::<buck2_error::Result<Vec<_>>>()?;

        let input_directory = builder.fingerprint(digest_config.as_directory_serializer());

        let input_directory = match interner {
            Some(i) => input_directory.shared(i).as_immutable(),
            None => input_directory,
        };

        let input_files_bytes = if buck2_core::faster_directories::is_enabled() {
            input_directory.size()
        } else {
            Self::calculate_inputs_size_bytes(&input_directory)
        };

        Ok(Self {
            inputs,
            outputs,
            input_directory,
            output_paths,
            input_files_bytes,
        })
    }

    fn calculate_inputs_size_bytes(input_directory: &ActionImmutableDirectory) -> u64 {
        let mut input_files_bytes = 0;

        for entry in input_directory.unordered_walk_leaves().without_paths() {
            if let ActionDirectoryMember::File(f) = entry {
                input_files_bytes += f.digest.size();
            }
        }

        input_files_bytes
    }

    pub fn add_outputs_as_inputs(
        self,
        output_paths: impl IntoIterator<Item = CommandExecutionInput>,
        fs: &ArtifactFs,
        digest_config: DigestConfig,
        interner: Option<&DashMapDirectoryInterner<ActionDirectoryMember, TrackedFileDigest>>,
    ) -> buck2_error::Result<Self> {
        let Self {
            mut inputs,
            outputs,
            input_directory: _,
            output_paths: _,
            input_files_bytes: _,
        } = self;
        inputs.extend(output_paths);
        Self::new(inputs, outputs, fs, digest_config, interner)
    }

    pub fn input_directory(&self) -> &ActionImmutableDirectory {
        &self.input_directory
    }

    pub fn output_paths(&self) -> &[(ProjectRelativePathBuf, OutputType)] {
        &self.output_paths
    }

    pub fn input_files_bytes(&self) -> u64 {
        self.input_files_bytes
    }
}

#[derive(Copy, Clone, Dupe, Debug, Display, Allocative, Hash, PartialEq, Eq)]
pub struct WorkerId(pub u64);

pub struct WorkerSpec {
    pub id: WorkerId,
    pub exe: Vec<String>,
    pub env: BuckFixedSortedMap<String, String>,
    pub concurrency: Option<usize>,
    pub streaming: bool,
    pub remote_key: Option<TrackedFileDigest>,
    pub input_paths: CommandExecutionPaths,
}

impl WorkerSpec {
    pub fn inputs(&self) -> &[CommandExecutionInput] {
        &self.input_paths.inputs
    }
}

pub struct RemoteWorkerSpec {
    pub id: WorkerId,
    pub init: Vec<String>,
    pub env: BuckFixedSortedMap<String, String>,
    pub input_paths: CommandExecutionPaths,
    pub concurrency: Option<usize>,
}

/// Bytes folded into an action's digest without being part of its command or inputs,
/// so that the action key covers dependencies the command observes only by content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionSalt(Vec<u8>);

impl ActionSalt {
    pub fn from_digest(digest: &TrackedFileDigest) -> Self {
        Self(digest.raw_digest().as_bytes().to_vec())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// How a local test is killed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TestKillPolicy {
    /// How long a test that hit its timeout gets to exit after `SIGTERM` before it and
    /// everything left in its process group get `SIGKILL`. `None` kills immediately.
    pub timeout_grace_period: Option<Duration>,
    /// On Linux, kill the processes a test left behind outside its process group, such as
    /// daemons, when it is killed. Only applies to tests that do not run in an action cgroup. Such
    /// a test adopts its orphaned descendants, which then stay zombies until it exits instead of
    /// being reaped by init.
    pub kill_detached_processes: bool,
}

/// The data contains the information about the command to be executed.
pub struct CommandExecutionRequest {
    /// Optional arguments including executable prepended to `args` to get full command line.
    /// This is used by workers to separate worker arguments from executable arguments.
    exe: Vec<String>,
    args: Vec<String>,
    paths: CommandExecutionPaths,
    env: BuckFixedSortedMap<String, String>,
    timeout: Option<Duration>,
    pub executor_preference: ExecutorPreference,
    host_sharing_requirements: Arc<HostSharingRequirements>,
    // Used to disable the low pass filter for concurrent local actions. Enabled by default
    low_pass_filter: bool,
    /// Working directory, relative to the project root.
    working_directory: ProjectRelativePathBuf,
    /// Whether we should always prefetch stderr when executing. When it's needed, this lets us
    /// overlap stderr download with output downloads, which might be marginally useful to improve
    /// latency.
    prefetch_lossy_stderr: bool,
    /// Whether to cleanup outputs
    pub outputs_cleanup: bool,
    /// What environment variables to inherit from the Buck2 daemon.
    local_environment_inheritance: Option<EnvironmentInheritance>,
    /// Whether this command should override the fallback-only behavior on an hybrid executor and
    /// thus always run as if the executor was full-hybrid, assuming it is capable.
    force_full_hybrid_if_capable: bool,
    /// Whether to disable capturing performance counters for this execution.
    disable_miniperf: bool,
    /// Whether to treat this action as a cache miss everywhere: buck2's remote lookups skip it and
    /// the remote execution service is told not to serve it from its own cache.
    induced_cache_miss: bool,
    required_local_resources: BuckFixedSortedSet<LocalResourceState>,
    /// Persistent worker to use for execution
    worker: Option<WorkerSpec>,
    /// Persistent remote worker to use for execution
    remote_worker: Option<RemoteWorkerSpec>,
    /// Whether the executor should guarantee that the inodes for all inputs are unique (i.e. avoid
    /// hardlinking identical input files, for example)
    unique_input_inodes: bool,
    salt: Option<ActionSalt>,
    /// Remote dep file key, if the action has a dep file.
    /// If this key is set and remote dep file caching is enabled, it will be used to query the cache.
    pub remote_dep_file_key: Option<DepFileDigest>,
    /// RE gang workers for gang scheduling.
    re_gang_workers: Vec<ReGangWorker>,
    /// RE dependencies to pass in action metadata.
    remote_execution_dependencies: Vec<RemoteExecutorDependency>,
    /// RE custom tupperware image.
    remote_execution_custom_image: Option<RemoteExecutorCustomImage>,
    /// RE execution policy.
    meta_internal_extra_params: Arc<MetaInternalExtraParams>,
    /// Failed action outputs to materialize
    outputs_for_error_handler: Vec<BuildArtifactPath>,
    /// String representation of a key that uniquely identifies a RunAction
    run_action_key: Option<String>,

    /// Set for tests only.
    test_kill_policy: Option<TestKillPolicy>,
    /// Whether to skip resource control (cgroup) for this command.
    /// Set for local resource setup commands whose backgrounded processes
    /// must survive after the setup script exits.
    skip_resource_control: bool,

    network_access: Option<NetworkAccess>,

    /// When set, the local executor runs this action without network isolation,
    /// ignoring the inherited `network_access` policy; no effect on RE. Set by the test
    /// orchestrator's `disable_local_network_isolation`, which explains the rationale.
    disable_local_network_isolation: bool,
}

impl CommandExecutionRequest {
    pub fn new(
        exe: Vec<String>,
        args: Vec<String>,
        paths: CommandExecutionPaths,
        env: BuckFixedSortedMap<String, String>,
    ) -> Self {
        Self {
            exe,
            args,
            paths,
            env,
            timeout: None,
            executor_preference: ExecutorPreference::Default,
            host_sharing_requirements: Arc::new(HostSharingRequirements::default()),
            low_pass_filter: true,
            working_directory: ProjectRelativePathBuf::default(),
            prefetch_lossy_stderr: false,
            outputs_cleanup: true,
            local_environment_inheritance: None,
            force_full_hybrid_if_capable: false,
            disable_miniperf: false,
            induced_cache_miss: false,
            required_local_resources: BuckFixedSortedSet::new(),
            worker: None,
            remote_worker: None,
            unique_input_inodes: false,
            salt: None,
            remote_dep_file_key: None,
            re_gang_workers: Vec::new(),
            remote_execution_dependencies: Vec::new(),
            remote_execution_custom_image: None,
            meta_internal_extra_params: MetaInternalExtraParams::default_arc(),
            outputs_for_error_handler: Vec::new(),
            run_action_key: None,
            test_kill_policy: None,
            skip_resource_control: false,
            network_access: None,
            disable_local_network_isolation: false,
        }
    }

    pub fn paths(&self) -> &CommandExecutionPaths {
        &self.paths
    }

    pub fn with_outputs_paths_added_as_inputs(
        self,
        output_paths: impl IntoIterator<Item = CommandExecutionInput>,
        fs: &ArtifactFs,
        digest_config: DigestConfig,
        interner: Option<&DashMapDirectoryInterner<ActionDirectoryMember, TrackedFileDigest>>,
    ) -> buck2_error::Result<Self> {
        let override_paths =
            self.paths
                .add_outputs_as_inputs(output_paths, fs, digest_config, interner)?;
        Ok(Self {
            paths: override_paths,
            ..self
        })
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn with_executor_preference(mut self, executor_preference: ExecutorPreference) -> Self {
        self.executor_preference = executor_preference;
        self
    }

    pub fn with_host_sharing_requirements(
        mut self,
        host_sharing_requirements: Arc<HostSharingRequirements>,
    ) -> Self {
        self.host_sharing_requirements = host_sharing_requirements;
        self
    }

    pub fn with_low_pass_filter(mut self, low_pass_filter: bool) -> Self {
        self.low_pass_filter = low_pass_filter;
        self
    }

    pub fn with_working_directory(mut self, working_directory: ProjectRelativePathBuf) -> Self {
        self.working_directory = working_directory;
        self
    }

    pub fn with_prefetch_lossy_stderr(mut self, prefetch_lossy_stderr: bool) -> Self {
        self.prefetch_lossy_stderr = prefetch_lossy_stderr;
        self
    }

    pub fn with_outputs_cleanup(mut self, outputs_cleanup: bool) -> Self {
        self.outputs_cleanup = outputs_cleanup;
        self
    }

    pub fn with_remote_dep_file_key(mut self, remote_dep_file_key: &DepFileDigest) -> Self {
        self.remote_dep_file_key = Some(remote_dep_file_key.dupe());
        self
    }

    pub fn remote_dep_file_key(&self) -> &Option<DepFileDigest> {
        &self.remote_dep_file_key
    }

    pub fn prefetch_lossy_stderr(&self) -> bool {
        self.prefetch_lossy_stderr
    }

    pub fn outputs_cleanup(&self) -> bool {
        self.outputs_cleanup
    }

    pub fn all_args(&self) -> impl Iterator<Item = &String> {
        self.exe.iter().chain(self.args.iter())
    }

    pub fn all_args_vec(&self) -> Vec<String> {
        self.all_args().cloned().collect()
    }

    pub fn all_args_str(&self) -> String {
        self.all_args().join(" ")
    }

    pub fn exe(&self) -> &[String] {
        &self.exe
    }

    pub fn args(&self) -> &[String] {
        &self.args
    }

    pub fn worker(&self) -> &Option<WorkerSpec> {
        &self.worker
    }

    pub fn remote_worker(&self) -> &Option<RemoteWorkerSpec> {
        &self.remote_worker
    }

    pub fn with_worker(mut self, worker: Option<WorkerSpec>) -> Self {
        self.worker = worker;
        self
    }

    pub fn with_remote_worker(mut self, remote_worker: Option<RemoteWorkerSpec>) -> Self {
        self.remote_worker = remote_worker;
        self
    }

    pub fn inputs(&self) -> &[CommandExecutionInput] {
        &self.paths.inputs
    }

    pub fn outputs(&self) -> impl Iterator<Item = CommandExecutionOutputRef<'_>> + '_ {
        self.paths.outputs.iter().map(|output| output.as_ref())
    }

    pub fn env(&self) -> &BuckFixedSortedMap<String, String> {
        &self.env
    }

    pub fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    pub fn executor_preference(&self) -> ExecutorPreference {
        self.executor_preference
    }

    pub fn host_sharing_requirements(&self) -> &HostSharingRequirements {
        &self.host_sharing_requirements
    }

    pub fn low_pass_filter(&self) -> bool {
        self.low_pass_filter
    }

    pub fn working_directory(&self) -> &ProjectRelativePath {
        &self.working_directory
    }

    pub fn with_local_environment_inheritance(
        mut self,
        local_environment_inheritance: EnvironmentInheritance,
    ) -> Self {
        self.local_environment_inheritance = Some(local_environment_inheritance);
        self
    }

    pub fn local_environment_inheritance(&self) -> Option<&EnvironmentInheritance> {
        self.local_environment_inheritance.as_ref()
    }

    pub fn with_force_full_hybrid_if_capable(mut self, force_full_hybrid_if_capable: bool) -> Self {
        self.force_full_hybrid_if_capable = force_full_hybrid_if_capable;
        self
    }

    pub fn force_full_hybrid_if_capable(&self) -> bool {
        self.force_full_hybrid_if_capable
    }

    pub fn with_disable_miniperf(mut self, disable_miniperf: bool) -> Self {
        self.disable_miniperf = disable_miniperf;
        self
    }

    pub fn disable_miniperf(&self) -> bool {
        self.disable_miniperf
    }

    pub fn with_induced_cache_miss(mut self, induced_cache_miss: bool) -> Self {
        self.induced_cache_miss = induced_cache_miss;
        self
    }

    pub fn induced_cache_miss(&self) -> bool {
        self.induced_cache_miss
    }

    pub fn with_required_local_resources(
        mut self,
        required_local_resources: Vec<LocalResourceState>,
    ) -> buck2_error::Result<Self> {
        let original_len = required_local_resources.len();
        self.required_local_resources = required_local_resources.into_iter().collect();
        if self.required_local_resources.len() != original_len {
            return Err(buck2_error!(
                buck2_error::ErrorTag::Tier0,
                "Each provided local resource state is supposed to come from a different target."
            ));
        }
        Ok(self)
    }

    pub fn required_local_resources(&self) -> &BuckFixedSortedSet<LocalResourceState> {
        &self.required_local_resources
    }

    pub fn with_unique_input_inodes(mut self, unique_input_inodes: bool) -> Self {
        self.unique_input_inodes = unique_input_inodes;
        self
    }

    pub fn unique_input_inodes(&self) -> bool {
        self.unique_input_inodes
    }

    pub fn with_salt(mut self, salt: Option<ActionSalt>) -> Self {
        self.salt = salt;
        self
    }

    pub fn salt(&self) -> Option<&ActionSalt> {
        self.salt.as_ref()
    }

    pub fn with_re_gang_workers(mut self, re_gang_workers: Vec<ReGangWorker>) -> Self {
        self.re_gang_workers = re_gang_workers;
        self
    }

    pub fn re_gang_workers(&self) -> &Vec<ReGangWorker> {
        &self.re_gang_workers
    }

    pub fn with_remote_execution_dependencies(
        mut self,
        remote_execution_dependencies: Vec<RemoteExecutorDependency>,
    ) -> Self {
        self.remote_execution_dependencies = remote_execution_dependencies;
        self
    }

    pub fn remote_execution_dependencies(&self) -> &Vec<RemoteExecutorDependency> {
        &self.remote_execution_dependencies
    }

    pub fn with_outputs_for_error_handler(
        mut self,
        outputs_for_error_handler: Vec<BuildArtifactPath>,
    ) -> Self {
        self.outputs_for_error_handler = outputs_for_error_handler;
        self
    }

    pub fn outputs_for_error_handler(&self) -> &Vec<BuildArtifactPath> {
        &self.outputs_for_error_handler
    }

    pub fn with_remote_execution_custom_image(
        mut self,
        remote_execution_custom_image: Option<RemoteExecutorCustomImage>,
    ) -> Self {
        self.remote_execution_custom_image = remote_execution_custom_image;
        self
    }

    pub fn remote_execution_custom_image(&self) -> &Option<RemoteExecutorCustomImage> {
        &self.remote_execution_custom_image
    }

    pub fn with_meta_internal_extra_params(
        mut self,
        meta_internal_extra_params: Arc<MetaInternalExtraParams>,
    ) -> Self {
        self.meta_internal_extra_params = meta_internal_extra_params;
        self
    }

    pub fn meta_internal_extra_params(&self) -> &MetaInternalExtraParams {
        &self.meta_internal_extra_params
    }

    pub fn with_run_action_key(mut self, run_action_key: Option<String>) -> Self {
        self.run_action_key = run_action_key;
        self
    }

    pub fn run_action_key(&self) -> &Option<String> {
        &self.run_action_key
    }

    pub fn with_is_test(mut self, kill_policy: TestKillPolicy) -> Self {
        self.test_kill_policy = Some(kill_policy);
        self
    }

    pub fn is_test(&self) -> bool {
        self.test_kill_policy.is_some()
    }

    pub fn test_kill_policy(&self) -> Option<&TestKillPolicy> {
        self.test_kill_policy.as_ref()
    }

    pub fn with_skip_resource_control(mut self) -> Self {
        self.skip_resource_control = true;
        self
    }

    pub fn skip_resource_control(&self) -> bool {
        self.skip_resource_control
    }

    pub fn with_network_access(mut self, v: Option<NetworkAccess>) -> Self {
        self.network_access = v;
        self
    }

    pub fn network_access(&self) -> Option<NetworkAccess> {
        self.network_access
    }

    pub fn with_disable_local_network_isolation(mut self, v: bool) -> Self {
        self.disable_local_network_isolation = v;
        self
    }

    pub fn disable_local_network_isolation(&self) -> bool {
        self.disable_local_network_isolation
    }
}

#[derive(PartialEq, Eq, Hash, Debug)]
pub enum CommandExecutionOutputRef<'a> {
    BuildArtifact {
        path: &'a BuildArtifactPath,
        output_type: OutputType,
    },
    TestPath {
        path: &'a BuckOutTestPath,
        create: OutputCreationBehavior,
    },
}

impl CommandExecutionOutputRef<'_> {
    /// Resolve this output to a ResolvedCommandExecutionOutput that allows access to the output
    /// path as well as any dirs to create.
    pub fn resolve(
        &self,
        fs: &ArtifactFs,
        content_hash: Option<&ContentBasedPathHash>,
    ) -> buck2_error::Result<ResolvedCommandExecutionOutput> {
        match self {
            Self::BuildArtifact { path, output_type } => Ok(ResolvedCommandExecutionOutput {
                path: fs.resolve_build(path, content_hash)?,
                create: OutputCreationBehavior::Parent,
                output_type: *output_type,
            }),
            Self::TestPath { path, create } => Ok(ResolvedCommandExecutionOutput {
                path: fs.buck_out_path_resolver().resolve_test(path),
                create: *create,
                output_type: OutputType::FileOrDirectory,
            }),
        }
    }

    /// Same as `resolve`, but the underlying output path that is returned uses the
    /// configuration hash regardless of whether the output is content-based or not.
    pub fn resolve_configuration_hash_path(
        &self,
        fs: &ArtifactFs,
    ) -> buck2_error::Result<ResolvedCommandExecutionOutput> {
        match self {
            Self::BuildArtifact { path, output_type } => Ok(ResolvedCommandExecutionOutput {
                path: fs.resolve_build_configuration_hash_path(path)?,
                create: OutputCreationBehavior::Parent,
                output_type: *output_type,
            }),
            Self::TestPath { path, create } => Ok(ResolvedCommandExecutionOutput {
                path: fs.buck_out_path_resolver().resolve_test(path),
                create: *create,
                output_type: OutputType::FileOrDirectory,
            }),
        }
    }

    pub fn cloned(&self) -> CommandExecutionOutput {
        match self {
            Self::BuildArtifact { path, output_type } => CommandExecutionOutput::BuildArtifact {
                path: (*path).dupe(),
                output_type: *output_type,
            },
            Self::TestPath { path, create } => CommandExecutionOutput::TestPath {
                path: (*path).clone(),
                create: *create,
            },
        }
    }

    pub fn has_content_based_path(&self) -> bool {
        match self {
            Self::BuildArtifact { path, .. } => path.is_content_based_path(),
            Self::TestPath { .. } => false,
        }
    }
}

#[derive(PartialEq, Eq, Hash, Debug)]
pub enum CommandExecutionOutput {
    BuildArtifact {
        path: BuildArtifactPath,
        output_type: OutputType,
    },
    TestPath {
        path: BuckOutTestPath,
        create: OutputCreationBehavior,
    },
}

impl CommandExecutionOutput {
    pub fn build_artifact_path(&self) -> Option<&BuildArtifactPath> {
        match self {
            Self::BuildArtifact { path, .. } => Some(path),
            Self::TestPath { .. } => None,
        }
    }

    pub fn into_test_path(self) -> Option<BuckOutTestPath> {
        match self {
            Self::BuildArtifact { .. } => None,
            Self::TestPath { path, .. } => Some(path),
        }
    }

    pub fn as_ref(&self) -> CommandExecutionOutputRef<'_> {
        match self {
            Self::BuildArtifact { path, output_type } => CommandExecutionOutputRef::BuildArtifact {
                path,
                output_type: *output_type,
            },
            Self::TestPath { path, create } => CommandExecutionOutputRef::TestPath {
                path,
                create: *create,
            },
        }
    }

    pub fn has_content_based_path(&self) -> bool {
        match self {
            Self::BuildArtifact { path, .. } => path.is_content_based_path(),
            Self::TestPath { .. } => false,
        }
    }
}

/// The path for an output, and what should be created before running the action that produces this
/// output.
pub struct ResolvedCommandExecutionOutput {
    pub path: ProjectRelativePathBuf,
    pub output_type: OutputType,
    create: OutputCreationBehavior,
}

impl ResolvedCommandExecutionOutput {
    /// Return this output's path.
    pub fn path(&self) -> &ProjectRelativePath {
        &self.path
    }

    /// Return this output's path, ownedly.
    pub fn into_path(self) -> ProjectRelativePathBuf {
        self.path
    }

    /// Return the path that this output requires creating before running the action.
    pub fn path_to_create(&self) -> Option<&ProjectRelativePath> {
        match self.create {
            OutputCreationBehavior::Create => Some(&self.path),
            OutputCreationBehavior::Parent => self.path.parent(),
        }
    }
}
