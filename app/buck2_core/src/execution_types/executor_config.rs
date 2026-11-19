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
use std::fmt::Formatter;
use std::hash::Hash;
use std::hash::Hasher;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::LazyLock;
use std::time::Duration;

use allocative::Allocative;
use buck2_data::NetworkAccess as BuckNetworkAccess;
use buck2_hash::BuckFixedSortedMap;
use buck2_hash::BuckHasher;
use buck2_hash::BuckMutInsertionMap;
use derive_more::Display;
use dupe::Dupe;
use itertools::Itertools;
use pagable::Pagable;
use static_interner::Intern;
use static_interner::interner;

#[derive(Debug, Eq, Hash, PartialEq, Clone, Dupe, Allocative, Pagable)]
pub struct LocalExecutorOptions {
    pub use_persistent_workers: bool,
}

impl Default for LocalExecutorOptions {
    fn default() -> Self {
        Self {
            use_persistent_workers: true,
        }
    }
}

#[derive(Debug, Eq, Hash, PartialEq, Clone, Allocative, Pagable)]
pub struct RemoteEnabledExecutorOptions {
    pub executor: RemoteEnabledExecutor,
    pub re_properties: RePlatformFields,
    pub re_use_case: RemoteExecutorUseCase,
    pub re_action_key: Option<String>,
    pub cache_upload_behavior: CacheUploadBehavior,
    pub remote_cache_enabled: bool,
    pub remote_dep_file_cache_enabled: bool,
    pub dependencies: Vec<RemoteExecutorDependency>,
    pub gang_workers: Vec<ReGangWorker>,
    pub custom_image: Option<Box<RemoteExecutorCustomImage>>,
    pub meta_internal_extra_params: Arc<MetaInternalExtraParams>,
    pub priority: Option<i32>,
}

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum RemoteExecutorDependencyErrors {
    #[error("RE dependency requires `{0}` to be set")]
    MissingField(&'static str),
    #[error("too many fields set for RE dependency: `{0}`")]
    UnsupportedFields(String),
}

#[derive(Debug, Eq, Hash, Pagable, PartialEq, Clone, Allocative)]
pub struct ImagePackageIdentifier {
    pub name: String,
    pub uuid: String,
}

#[derive(Debug, Eq, PartialEq, Clone, Hash, Pagable, Allocative)]
pub struct RemoteExecutorCafFbpkg {
    pub name: String,
    pub uuid: String,
    pub tag: Option<String>,
    pub permissions: Option<String>,
}

#[derive(Debug, Eq, Hash, Pagable, PartialEq, Clone, Allocative)]
pub struct RemoteExecutorCustomImage {
    pub identifier: ImagePackageIdentifier,
    pub drop_host_mount_globs: Vec<String>,
}

/// A Remote Action can specify a list of dependencies that are required before starting the execution `https://fburl.com/wiki/offzl3ox`
#[derive(Debug, Eq, PartialEq, Clone, Hash, Pagable, Allocative)]
pub struct RemoteExecutorDependency {
    /// The SMC tier that the Remote Executor will query to try to acquire the dependency
    pub smc_tier: String,
    /// The id of the dependency to acquire
    pub id: String,
    interpolate_revision: bool,
}

/// Describes a worker in a gang for Remote Execution.
/// A gang is a collection of workers that are scheduled together for distributed execution.
/// Each worker specifies its capabilities (platform requirements).
#[derive(Debug, Eq, PartialEq, Clone, Hash, Allocative, Pagable)]
pub struct ReGangWorker {
    /// The platform capabilities required for this gang worker
    pub capabilities: BuckFixedSortedMap<String, String>,
}

impl ReGangWorker {
    pub fn parse(worker_map: BuckMutInsertionMap<&str, &str>) -> buck2_error::Result<ReGangWorker> {
        let capabilities: BuckFixedSortedMap<String, String> = worker_map
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        Ok(ReGangWorker { capabilities })
    }
}

/// Locality constraint for gang scheduling.
/// Determines how gang workers should be co-located.
#[derive(Debug, Eq, PartialEq, Clone, Copy, Hash, Allocative, Pagable)]
pub enum ReGangLocality {
    /// Workers can be in any location
    Unspecified,
    /// Workers must be in the same region
    Region,
    /// Workers must be in the same datacenter
    Datacenter,
    /// Workers must be in the same network domain
    NetworkDomain,
    /// Workers must be in the same rack
    Rack,
}

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum ReGangLocalityErrors {
    #[error(
        "Invalid locality value `{0}`. Expected one of: region, datacenter, network_domain, rack"
    )]
    InvalidLocality(String),
}

impl ReGangLocality {
    pub fn parse(s: &str) -> buck2_error::Result<ReGangLocality> {
        match s.to_lowercase().as_str() {
            "region" => Ok(ReGangLocality::Region),
            "datacenter" => Ok(ReGangLocality::Datacenter),
            "network_domain" => Ok(ReGangLocality::NetworkDomain),
            "rack" => Ok(ReGangLocality::Rack),
            _ => Err(ReGangLocalityErrors::InvalidLocality(s.to_owned()).into()),
        }
    }
}

/// Describes a constrained gang for Remote Execution.
/// All workers in the gang will have the same capabilities specification.
#[derive(Debug, Eq, PartialEq, Clone, Hash, Allocative, Pagable)]
pub struct ReGang {
    /// The platform capabilities required for all gang workers
    pub capabilities: BuckFixedSortedMap<String, String>,
    /// Number of workers in the gang
    pub num_of_workers: i32,
    /// Optional locality constraint for gang scheduling
    pub locality: Option<ReGangLocality>,
    /// Optional number of sub-groups for locality partitioning
    pub num_sub_groups: Option<i32>,
    /// Optional resource_units each worker claims on its host
    pub resource_units: Option<i32>,
}

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum ReGangErrors {
    #[error("RE gang requires `{0}` to be set")]
    MissingField(&'static str),
    #[error("RE gang `num_of_workers` must be positive, got `{0}`")]
    InvalidNumOfWorkers(i32),
    #[error(
        "RE gang `num_sub_groups` must be at least 1 and divide `num_of_workers` evenly, got num_sub_groups={0}, num_of_workers={1}"
    )]
    InvalidNumSubGroups(i32, i32),
}

impl ReGang {
    pub fn parse(
        capabilities: BuckFixedSortedMap<String, String>,
        num_of_workers: i32,
        locality: Option<ReGangLocality>,
        num_sub_groups: Option<i32>,
        resource_units: Option<i32>,
    ) -> buck2_error::Result<ReGang> {
        if num_of_workers <= 0 {
            return Err(ReGangErrors::InvalidNumOfWorkers(num_of_workers).into());
        }

        if capabilities.is_empty() {
            return Err(ReGangErrors::MissingField("capabilities").into());
        }

        if let Some(n) = num_sub_groups {
            if n < 1 || num_of_workers % n != 0 {
                return Err(ReGangErrors::InvalidNumSubGroups(n, num_of_workers).into());
            }
        }

        Ok(ReGang {
            capabilities,
            num_of_workers,
            locality,
            num_sub_groups,
            resource_units,
        })
    }
}

fn interpolate_dependency_id(id: &str, username: Option<&str>, hostname: Option<&str>) -> String {
    id.replace("$(username)", username.unwrap_or_default())
        .replace("$(hostname)", hostname.unwrap_or_default())
}

impl RemoteExecutorDependency {
    pub fn parse(
        dep_map: BuckMutInsertionMap<&str, &str>,
    ) -> buck2_error::Result<RemoteExecutorDependency> {
        fn username() -> Option<String> {
            #[cfg(fbcode_build)]
            {
                user::current_username()
                    .ok()
                    .filter(|u| user::is_human_unixname(u))
            }
            #[cfg(not(fbcode_build))]
            {
                None
            }
        }

        fn current_hostname() -> Option<String> {
            hostname::get().ok()?.into_string().ok()
        }

        let smc_tier = dep_map
            .get("smc_tier")
            .ok_or(RemoteExecutorDependencyErrors::MissingField("smc_tier"))?;
        let id = dep_map
            .get("id")
            .ok_or(RemoteExecutorDependencyErrors::MissingField("id"))?;
        let interpolate = dep_map.get("enable_interpolation").unwrap_or(&"false");

        let interpolate = *interpolate == "true";
        let interpolate_revision = interpolate && id.contains("$(revision)");
        let id = if interpolate {
            let username = id.contains("$(username)").then(username).flatten();
            let hostname = if id.contains("$(hostname)") {
                current_hostname()
            } else {
                None
            };
            interpolate_dependency_id(id, username.as_deref(), hostname.as_deref())
        } else {
            id.to_string()
        };

        if dep_map.len() > 3 {
            return Err(RemoteExecutorDependencyErrors::UnsupportedFields(
                dep_map.keys().join(", "),
            )
            .into());
        }
        Ok(RemoteExecutorDependency {
            smc_tier: smc_tier.to_string(),
            id,
            interpolate_revision,
        })
    }

    pub fn requires_revision(&self) -> bool {
        self.interpolate_revision
    }

    /// Resolve a deferred revision placeholder using the repository selected for execution.
    pub fn with_revision(&self, revision: Option<&str>) -> Self {
        let mut dependency = self.clone();
        if self.interpolate_revision {
            dependency.id = dependency
                .id
                .replace("$(revision)", revision.unwrap_or_default());
        }
        dependency
    }
}

#[derive(Clone, Debug, Display, Eq, PartialEq, Hash, Allocative, Pagable)]
struct RemoteExecutorUseCaseData(String);

interner!(
    USE_CASE_INTERNER,
    BuckHasher,
    RemoteExecutorUseCaseData,
    String,
    str
);

#[derive(Debug, Eq, PartialEq, Copy, Clone, Dupe, Display, Allocative, Pagable)]
pub struct RemoteExecutorUseCase(Intern<RemoteExecutorUseCaseData>);

impl RemoteExecutorUseCase {
    pub fn new(use_case: String) -> Self {
        Self(USE_CASE_INTERNER.intern(RemoteExecutorUseCaseData(use_case)))
    }

    pub fn as_str(&self) -> &'static str {
        self.0.deref_static()
    }

    /// The "buck2-default" use case. This is meant to be used when no use case is configured. It's
    /// not meant to be used for convenience when a use case is not available where it's needed!
    pub fn buck2_default() -> Self {
        static USE_CASE: LazyLock<RemoteExecutorUseCase> =
            LazyLock::new(|| RemoteExecutorUseCase::new("buck2-default".to_owned()));
        *USE_CASE
    }
}

// The derived PartialEq (which uses pointer equality on the interned data) is still correct.
#[allow(clippy::derived_hash_with_manual_eq)]
impl Hash for RemoteExecutorUseCase {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl FromStr for RemoteExecutorUseCase {
    type Err = buck2_error::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(RemoteExecutorUseCase::new(s.to_owned()))
    }
}

#[derive(Debug, Default, Eq, PartialEq, Clone, Hash, Allocative, Pagable)]
pub struct RemoteExecutorOptions {
    pub re_max_input_files_bytes: Option<u64>,
    pub re_max_queue_time: Option<Duration>,
    pub re_resource_units: Option<i64>,
}

/// The actual executor portion of a RemoteEnabled executor. It's possible for a RemoteEnabled
/// executor to wrap a local executor, which is a glorified way of saying "this is a local executor
/// with a RE backend for caching".
#[derive(Display, Debug, Eq, PartialEq, Clone, Hash, Allocative, Pagable)]
pub enum RemoteEnabledExecutor {
    #[display("local")]
    Local(LocalExecutorOptions),
    #[display("remote")]
    Remote(RemoteExecutorOptions),
    #[display("hybrid")]
    Hybrid {
        local: LocalExecutorOptions,
        remote: RemoteExecutorOptions,
        level: HybridExecutionLevel,
    },
}

/// Normalized `remote_execution::Platform`. Also implements `Eq`, `Hash`.
#[derive(Default, Debug, Clone, PartialEq, Eq, Hash, Pagable, Allocative)]
pub struct RePlatformFields {
    pub properties: Arc<BuckFixedSortedMap<String, Vec<String>>>,
}

#[derive(Debug, Eq, PartialEq, Clone, Hash, Pagable, Allocative)]
#[allow(clippy::large_enum_variant)]
pub enum Executor {
    /// This executor only runs local commands.
    Local(LocalExecutorOptions),

    /// This executor interacts with a RE backend. It may use that to read or write to caches, or
    /// to execute commands.
    RemoteEnabled(RemoteEnabledExecutorOptions),
    /// Can't run any actions
    None,
}

impl Display for Executor {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local(options) => {
                write!(
                    f,
                    "Local + use persistent workers {}",
                    options.use_persistent_workers
                )
            }
            Self::RemoteEnabled(options) => {
                let cache = match options.remote_cache_enabled {
                    true => "enabled",
                    false => "disabled",
                };
                let dep_file_cache = match options.remote_dep_file_cache_enabled {
                    true => "enabled",
                    false => "disabled",
                };
                write!(
                    f,
                    "RemoteEnabled + executor {} + remote cache {} + cache upload {} + remote dep file cache {}",
                    options.executor, cache, options.cache_upload_behavior, dep_file_cache
                )
            }
            Self::None => write!(f, "None"),
        }
    }
}

#[derive(Debug, Eq, PartialEq, Clone, Copy, Dupe, Hash, Pagable, Allocative)]
pub enum PathSeparatorKind {
    Unix,
    Windows,
}

impl PathSeparatorKind {
    pub fn system_default() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }
}

/// Controls how we implement output_dirs, output_files, output_paths in RE actions.
#[derive(Debug, Eq, PartialEq, Clone, Copy, Dupe, Hash, Pagable, Allocative)]
pub enum OutputPathsBehavior {
    /// Ask for things as either files or directories.
    Strict,
    /// Ask for things as either directories when certain or files AND directories.
    Compatibility,
    /// Ask for things using output_paths.
    OutputPaths,
}

impl FromStr for OutputPathsBehavior {
    type Err = buck2_error::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "strict" => Ok(OutputPathsBehavior::Strict),
            "compatibility" => Ok(OutputPathsBehavior::Compatibility),
            #[cfg(not(fbcode_build))]
            "output_paths" => Ok(OutputPathsBehavior::OutputPaths),
            _ => Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::Input,
                "Invalid OutputPathsBehavior: `{}`",
                s
            )),
        }
    }
}

impl Default for OutputPathsBehavior {
    fn default() -> Self {
        if crate::is_open_source() {
            Self::OutputPaths
        } else {
            Self::Compatibility
        }
    }
}

#[derive(
    Display, Debug, Eq, PartialEq, Clone, Copy, Dupe, Hash, Pagable, Allocative
)]
#[derive(Default)]
pub enum CacheUploadBehavior {
    #[display("enabled")]
    Enabled { max_bytes: Option<u64> },
    #[display("disabled")]
    #[default]
    Disabled,
}

pub const NETWORK_ACCESS_VALUES: &[&str] = &["all", "none", "loopback", "strict", "private"];

#[derive(Debug, Eq, PartialEq, Clone, Copy, Dupe, Hash, Pagable, Allocative)]
pub enum ExecutorNetworkAccess {
    All,
    None,
    Loopback,
    Strict,
    Private,
}

impl From<ExecutorNetworkAccess> for BuckNetworkAccess {
    fn from(value: ExecutorNetworkAccess) -> Self {
        match value {
            ExecutorNetworkAccess::All => BuckNetworkAccess::All,
            ExecutorNetworkAccess::None => BuckNetworkAccess::None,
            ExecutorNetworkAccess::Loopback => BuckNetworkAccess::Loopback,
            ExecutorNetworkAccess::Strict => BuckNetworkAccess::Strict,
            ExecutorNetworkAccess::Private => BuckNetworkAccess::Private,
        }
    }
}

impl From<BuckNetworkAccess> for ExecutorNetworkAccess {
    fn from(value: BuckNetworkAccess) -> Self {
        match value {
            BuckNetworkAccess::All => ExecutorNetworkAccess::All,
            BuckNetworkAccess::None => ExecutorNetworkAccess::None,
            BuckNetworkAccess::Loopback => ExecutorNetworkAccess::Loopback,
            BuckNetworkAccess::Strict => ExecutorNetworkAccess::Strict,
            BuckNetworkAccess::Private => ExecutorNetworkAccess::Private,
        }
    }
}

pub fn parse_network_access(s: &str) -> buck2_error::Result<BuckNetworkAccess> {
    match s {
        "all" => Ok(BuckNetworkAccess::All),
        "none" => Ok(BuckNetworkAccess::None),
        "loopback" => Ok(BuckNetworkAccess::Loopback),
        "strict" => Ok(BuckNetworkAccess::Strict),
        "private" => Ok(BuckNetworkAccess::Private),
        _ => Err(buck2_error::buck2_error!(
            buck2_error::ErrorTag::Input,
            "Invalid network_access value `{}`, expected one of [{}]",
            s,
            NETWORK_ACCESS_VALUES
                .iter()
                .map(|v| format!("`{v}`"))
                .join(", ")
        )),
    }
}

#[derive(Debug, Eq, PartialEq, Clone, Copy, Dupe, Hash, Pagable, Allocative)]
pub struct CommandGenerationOptions {
    pub path_separator: PathSeparatorKind,
    pub output_paths_behavior: OutputPathsBehavior,
    pub use_bazel_protocol_remote_persistent_workers: bool,
    pub network_access: Option<ExecutorNetworkAccess>,
}

#[derive(Debug, Eq, PartialEq, Hash, Allocative, Clone, Pagable)]
pub struct CommandExecutorConfig {
    pub executor: Executor,
    pub options: CommandGenerationOptions,
}

#[derive(Debug, Eq, PartialEq, Clone, Copy, Dupe, Hash, Pagable, Allocative)]
pub enum HybridExecutionLevel {
    /// Expose both executors but only run it in one preferred executor.
    Limited,
    /// Expose both executors, fallback to the non-preferred executor if execution on the preferred
    /// executor doesn't provide a successful response. By default, we fallback only on errors (i.e.
    /// the infra failed), but not on failures (i.e. the job exited with 1). If
    /// `fallback_on_failure` is set, then we also fallback on failures.
    Fallback { fallback_on_failure: bool },
    /// Race both executors.
    Full {
        fallback_on_failure: bool,
        low_pass_filter: bool,
    },
}

impl CommandExecutorConfig {
    pub fn testing_local() -> Arc<CommandExecutorConfig> {
        Arc::new(CommandExecutorConfig {
            executor: Executor::Local(LocalExecutorOptions::default()),
            options: CommandGenerationOptions {
                path_separator: PathSeparatorKind::system_default(),
                output_paths_behavior: Default::default(),
                use_bazel_protocol_remote_persistent_workers: false,
                network_access: None,
            },
        })
    }

    pub fn re_cache_enabled(&self) -> bool {
        match &self.executor {
            Executor::Local(_) => false,
            Executor::RemoteEnabled(options) => options.remote_cache_enabled,
            Executor::None => false,
        }
    }
}

/// This struct is used to pass policy info about the action to RE, its data should
/// match the TExecutionPolicy in the RE thrift API.
/// affinity_keys is not defined here because it's already defined in ReActionIdentity
/// duration_ms is not supported because we can't unpack i64 from starlark easily
#[derive(Default, Debug, Clone, Eq, Hash, Pagable, PartialEq, Allocative)]
pub struct RemoteExecutionPolicy {
    pub priority: Option<i32>,
    pub region_preference: Option<String>,
    pub setup_preference_key: Option<String>,
    /// Maximum seconds the action may wait in the RE queue. Always positive. The RE scheduler
    /// uses `min(queue_timeout, platform schedule timeout)`, so a larger value just gets the
    /// platform default.
    pub queue_timeout_s: Option<i32>,
}

/// This struct is used to pass meta internal params to RE
#[derive(Default, Debug, Clone, Eq, Hash, Pagable, PartialEq, Allocative)]
pub struct MetaInternalExtraParams {
    pub remote_execution_policy: RemoteExecutionPolicy,
    pub remote_execution_caf_fbpkgs: Vec<RemoteExecutorCafFbpkg>,
    pub gang: Option<ReGang>,
    /// Allow RE workers to upload action results to the action cache even
    /// when the action runs without network isolation. Useful for actions
    /// that require network access but produce deterministic outputs.
    pub allow_unsandboxed_action_cache_uploads: bool,
    /// Directs RE to stop a started execution when buck2 closes the execution stream — either
    /// because the action's future was cancelled (e.g. build failure without --keep-going, ctrl-C,
    /// or command timeout) or because the process exited. A transient network drop also closes the
    /// stream. If RE has merged another identical request into this execution, it keeps running for
    /// that requester.
    pub cancel_on_client_disconnect: bool,
}

impl MetaInternalExtraParams {
    pub fn default_arc() -> Arc<MetaInternalExtraParams> {
        static DEFAULT: LazyLock<Arc<MetaInternalExtraParams>> =
            LazyLock::new(|| Arc::new(MetaInternalExtraParams::default()));
        DEFAULT.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_re_gang_worker_parse_success() {
        let mut worker_map = BuckMutInsertionMap::new();
        worker_map.insert("subplatform", "H100");
        worker_map.insert("rack", "rack_01");

        let result = ReGangWorker::parse(worker_map);
        assert!(result.is_ok());

        let worker = result.unwrap();
        assert_eq!(worker.capabilities.len(), 2);
        assert_eq!(
            worker.capabilities.get("subplatform"),
            Some(&"H100".to_owned())
        );
        assert_eq!(worker.capabilities.get("rack"), Some(&"rack_01".to_owned()));
    }

    #[test]
    fn test_dependency_parse_with_revision_interpolation_clean() {
        let mut dep_map = BuckMutInsertionMap::new();
        dep_map.insert("smc_tier", "my.tier");
        dep_map.insert("id", "Sandbox:host:$(revision)");
        dep_map.insert("enable_interpolation", "true");

        let dep = RemoteExecutorDependency::parse(dep_map)
            .unwrap()
            .with_revision(Some("abc123def456abc123def456abc123def456abcd"));
        assert_eq!(
            dep.id,
            "Sandbox:host:abc123def456abc123def456abc123def456abcd"
        );
        assert!(dep.requires_revision());
    }

    #[test]
    fn test_dependency_parse_with_revision_interpolation_dirty() {
        let mut dep_map = BuckMutInsertionMap::new();
        dep_map.insert("smc_tier", "my.tier");
        dep_map.insert("id", "Sandbox:host:$(revision)");
        dep_map.insert("enable_interpolation", "true");

        let dep = RemoteExecutorDependency::parse(dep_map)
            .unwrap()
            .with_revision(Some("abc123def456abc123def456abc123def456abcd+"));
        assert_eq!(
            dep.id,
            "Sandbox:host:abc123def456abc123def456abc123def456abcd+"
        );
    }

    #[test]
    fn test_dependency_parse_with_revision_interpolation_missing() {
        let mut dep_map = BuckMutInsertionMap::new();
        dep_map.insert("smc_tier", "my.tier");
        dep_map.insert("id", "Sandbox:host:$(revision)");
        dep_map.insert("enable_interpolation", "true");

        let dep = RemoteExecutorDependency::parse(dep_map)
            .unwrap()
            .with_revision(None);
        assert_eq!(dep.id, "Sandbox:host:");
    }

    #[test]
    fn test_dependency_id_interpolation_includes_hostname() {
        assert_eq!(
            interpolate_dependency_id(
                "Sandbox:$(username):$(hostname):$(revision)",
                Some("alice"),
                Some("devvm123.example.com"),
            ),
            "Sandbox:alice:devvm123.example.com:$(revision)",
        );
    }

    #[test]
    fn test_dependency_parse_without_revision_placeholder() {
        let mut dep_map = BuckMutInsertionMap::new();
        dep_map.insert("smc_tier", "my.tier");
        dep_map.insert("id", "Limit:foo");
        dep_map.insert("enable_interpolation", "true");

        let dep = RemoteExecutorDependency::parse(dep_map).unwrap();
        assert_eq!(
            dep.id, "Limit:foo",
            "id without placeholders should be unchanged"
        );
        assert!(!dep.requires_revision());
    }

    #[test]
    fn test_dependency_parse_without_interpolation_preserves_revision_placeholder() {
        let mut dep_map = BuckMutInsertionMap::new();
        dep_map.insert("smc_tier", "my.tier");
        dep_map.insert("id", "Sandbox:host:$(revision)");

        let dep = RemoteExecutorDependency::parse(dep_map)
            .unwrap()
            .with_revision(Some("abc123"));
        assert_eq!(dep.id, "Sandbox:host:$(revision)");
    }

    #[test]
    fn test_network_access_parse() {
        assert_eq!(
            parse_network_access("all").expect("all should parse"),
            BuckNetworkAccess::All
        );
        assert_eq!(
            parse_network_access("none").expect("none should parse"),
            BuckNetworkAccess::None
        );
        assert_eq!(
            parse_network_access("loopback").expect("loopback should parse"),
            BuckNetworkAccess::Loopback
        );
        assert_eq!(
            parse_network_access("strict").expect("strict should parse"),
            BuckNetworkAccess::Strict
        );
        assert_eq!(
            parse_network_access("private").expect("private should parse"),
            BuckNetworkAccess::Private
        );
        assert!(
            parse_network_access("default").is_err(),
            "Omitted network_access should use the default executor behavior"
        );
    }
}
