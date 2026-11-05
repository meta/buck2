/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_data::error::ErrorTag;

use crate::ExitCode;

/// When there's no tag, but we want to put something in Scuba, we use this.
pub const ERROR_TAG_UNCLASSIFIED: &str = "UNCLASSIFIED";

#[derive(
    allocative::Allocative,
    PartialEq,
    Eq,
    Copy,
    Clone,
    Debug,
    PartialOrd,
    Ord
)]
pub enum Tier {
    // Expected errors in inputs explicitly tracked by buck.
    Input,
    // Errors that may be triggered by issues with the host,
    // resource limits, non-explicit dependencies or potentially
    // ambiguous input errors.
    // These can be tracked but not eliminated.
    Environment,
    // Unexpected errors in buck2 or core dependencies.
    // It should be possible to eliminate these, in theory.
    Tier0,
}

impl std::fmt::Display for Tier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Tier::Environment => "ENVIRONMENT",
            Tier::Tier0 => "INFRA",
            Tier::Input => "USER",
        };
        write!(f, "{}", s)
    }
}

struct TagMetadata {
    category: Option<Tier>,
    rank: u32,
    // If true and an error includes non-generic tags,
    // generic tags will be excluded from category key.
    generic: bool,
    // Hidden tags are only used to determine category,
    // they are excluded from both category keys and error_tags
    // reported externally.
    // These should be removed/avoided if possible.
    hidden: bool,
    // Root cause tags explain all other tags on an error, e.g. a full disk or an unmounted
    // filesystem. If an error has any, its category key is only the best-ranked one of them.
    root_cause: bool,
    // Allows overriding the exit code derived from the error tier.
    exit_code: ExitCode,
}

impl TagMetadata {
    fn generic(self, generic: bool) -> Self {
        Self { generic, ..self }
    }

    fn hidden(self) -> Self {
        Self {
            hidden: true,
            ..self
        }
    }

    fn root_cause(self) -> Self {
        Self {
            root_cause: true,
            ..self
        }
    }

    fn exit_code(self, exit_code: ExitCode) -> Self {
        Self { exit_code, ..self }
    }
}

macro_rules! rank {
    ( $tier:ident ) => {
        match stringify!($tier) {
            "environment" => TagMetadata {
                category: Some(Tier::Environment),
                rank: line!(),
                generic: false,
                hidden: false,
                root_cause: false,
                exit_code: ExitCode::InfraError,
            },
            "tier0" => TagMetadata {
                category: Some(Tier::Tier0),
                rank: line!(),
                generic: false,
                hidden: false,
                root_cause: false,
                exit_code: ExitCode::InfraError,
            },
            "input" => TagMetadata {
                category: Some(Tier::Input),
                rank: line!(),
                generic: false,
                hidden: false,
                root_cause: false,
                exit_code: ExitCode::UserError,
            },
            "unspecified" => TagMetadata {
                category: None,
                rank: line!(),
                generic: true,
                hidden: false,
                root_cause: false,
                exit_code: ExitCode::UnknownFailure,
            },
            _ => unreachable!(),
        }
    };
}

#[derive(derive_more::Display, Debug, PartialEq)]
pub enum ErrorSourceArea {
    Buck2,
    Eden,
    Re,
    Watchman,
    Sapling,
    TestExecutor,
    Installer,
}

pub trait ErrorTagExtra {
    fn source_area(&self) -> ErrorSourceArea;
    fn exit_code(&self) -> ExitCode;
}

impl ErrorTagExtra for ErrorTag {
    fn source_area(&self) -> ErrorSourceArea {
        let tag_name = self.as_str_name();
        if tag_name.starts_with("IO_EDEN") {
            ErrorSourceArea::Eden
        } else if tag_name.starts_with("RE") {
            ErrorSourceArea::Re
        } else if tag_name.starts_with("WATCHMAN") {
            ErrorSourceArea::Watchman
        } else if tag_name.starts_with("SAPLING") {
            ErrorSourceArea::Sapling
        } else if matches!(
            self,
            crate::ErrorTag::Tpx
                | crate::ErrorTag::TestExecutor
                | crate::ErrorTag::TestExecutorSignaled
                | crate::ErrorTag::TestExecutorNonZeroExit
                | crate::ErrorTag::TestExecutorNoEndOfTests
        ) {
            ErrorSourceArea::TestExecutor
        } else if tag_name.starts_with("INSTALLER") {
            ErrorSourceArea::Installer
        } else {
            ErrorSourceArea::Buck2
        }
    }

    fn exit_code(&self) -> ExitCode {
        tag_metadata(*self).exit_code
    }
}

/// Ordering determines tag rank, more interesting tags first
fn tag_metadata(tag: ErrorTag) -> TagMetadata {
    match tag {
        // Environment errors
        ErrorTag::NoValidCerts => rank!(environment),
        ErrorTag::CertExpired => rank!(environment),
        ErrorTag::ServerSigterm => rank!(environment),
        ErrorTag::IoMaterializerFileBusy => rank!(environment),
        ErrorTag::IoEdenDaemonRestarted => rank!(environment),
        ErrorTag::IoClientBrokenPipe => rank!(environment)
            .exit_code(ExitCode::ClientIoBrokenPipe)
            .root_cause(),
        ErrorTag::IoReadOnlyFilesystem => rank!(environment).root_cause(),
        ErrorTag::WatchmanRootNotConnectedError => rank!(environment),
        ErrorTag::WatchmanCheckoutInProgress => rank!(environment),
        ErrorTag::ServerTransportError => rank!(environment),
        ErrorTag::ServerMemoryPressure => rank!(environment),
        ErrorTag::DaemonOomKilled => rank!(environment)
            .exit_code(ExitCode::FatalOom)
            .root_cause(),
        // Note: This is only true internally due to buckwrapper
        ErrorTag::NoBuckRoot => rank!(environment),
        ErrorTag::InstallerEnvironment => rank!(environment).hidden(),
        ErrorTag::IoNotConnected => rank!(environment).root_cause(), // This typically means eden is not mounted
        // Typically due to poor network performance and large artifacts.
        ErrorTag::ReDeadlineExceeded => rank!(environment),
        // Typically due to network configuration/x2p
        ErrorTag::ReConnection => rank!(environment),
        // Means a new command 'clear'ed the DICE version (e.g. from merge base change) and an old command was rejected.
        ErrorTag::DiceRejected => rank!(environment),
        ErrorTag::HttpForbidden => rank!(environment),
        ErrorTag::HttpUnauthorized => rank!(environment),
        // Http 4xx errors could be either systemic problems or caused by user input.
        // Treat them as environment errors for alerting and SLIs, but input errors so that they aren't ignored by CI.
        ErrorTag::HttpClient => rank!(environment).exit_code(ExitCode::UserError),
        // Mostly caused by network related operation being too slow/timeout.
        ErrorTag::IoEdenNetworkCurlTimedout => rank!(environment),
        ErrorTag::IoWindowsSharingViolation => rank!(environment),
        ErrorTag::IoWindowsVirtualizationUnavailable => rank!(environment),
        ErrorTag::IoWindowsInternalError => rank!(environment),
        ErrorTag::IoWindowsPrivilegeNotHeld => rank!(environment),
        ErrorTag::IoWindowsNoSystemResources => rank!(environment),
        ErrorTag::RePermissionDenied => rank!(environment),
        ErrorTag::ReUserBadCerts => rank!(environment),
        ErrorTag::EPerm => rank!(environment),
        ErrorTag::IoPermissionDenied => rank!(environment),
        ErrorTag::IoStorageFull => rank!(environment).root_cause(),
        // Test runner reported test failure exit code and no test failures.
        ErrorTag::TestStatusUnknown => rank!(environment),
        // Test runner returned exit code 1. This can include input errors (CLI arg parse errors)
        ErrorTag::TestRunnerInternal => rank!(environment),
        // Test runner hit infra errors during test execution
        ErrorTag::TestInfraFailure => rank!(environment),
        ErrorTag::ThriftTimeout => rank!(environment),
        ErrorTag::ThriftLoadshedding => rank!(environment),
        // Often caused by eden loadshedding
        ErrorTag::IoInputOutputError => rank!(environment).root_cause(),
        ErrorTag::IoBadAddress => rank!(environment),
        ErrorTag::IoStaleNfsHandle => rank!(environment),
        ErrorTag::InvalidBuckOut => rank!(environment),
        ErrorTag::BuckdExeDeleted => rank!(environment),
        ErrorTag::MissingProjectRoot => rank!(environment),
        ErrorTag::MissingHomeDir => rank!(environment),
        // Typically a checkout replaced the directory the daemon was started in.
        ErrorTag::DaemonStaleWorkingDir => rank!(environment),
        ErrorTag::ActionOom => rank!(environment).root_cause(),
        ErrorTag::DotslashError => rank!(environment),
        ErrorTag::SaplingNotFound => rank!(environment),
        ErrorTag::SaplingNetwork => rank!(environment),
        ErrorTag::Tls => rank!(environment),
        // Typically CI tearing down the workspace out from under a running command.
        ErrorTag::MissingRepo => rank!(environment),
        ErrorTag::MissingWorkingDir => rank!(environment),
        ErrorTag::BlockedByPolicy => rank!(environment),

        // Tier 0 errors
        ErrorTag::ServerJemallocAssert => rank!(tier0),
        ErrorTag::ServerStackOverflow => rank!(tier0),
        ErrorTag::ServerPanicked => rank!(tier0),
        ErrorTag::ServerSegv => rank!(tier0),
        ErrorTag::ServerSigbusExecutable => rank!(tier0),
        ErrorTag::ServerSigbus => rank!(tier0),
        ErrorTag::ServerSigabrt => rank!(tier0),
        ErrorTag::ClientStartupTimeout => rank!(tier0),
        ErrorTag::DaemonLaunchFailed => rank!(tier0),
        ErrorTag::DaemonLaunchTimeout => rank!(tier0),
        ErrorTag::DaemonStartupFailed => rank!(tier0),
        ErrorTag::ExitStatusUnknown => rank!(tier0),
        ErrorTag::DaemonNestedConstraintsMismatch => rank!(tier0),
        ErrorTag::DaemonConstraintsWrongAfterStart => rank!(tier0),
        ErrorTag::DaemonDirCleanupFailed => rank!(tier0),
        ErrorTag::DaemonKillFailed => rank!(tier0),
        ErrorTag::BuckdLifecycleLock => rank!(tier0),
        ErrorTag::BuckdInfoMissing => rank!(tier0),
        ErrorTag::BuckdInfoParseError => rank!(tier0),
        ErrorTag::MissingCommandResult => rank!(tier0),
        ErrorTag::DaemonWontDieFromKill => rank!(tier0),
        ErrorTag::GrpcResponseMessageTooLarge => rank!(tier0),
        ErrorTag::ReClientCrash => rank!(tier0),
        ErrorTag::ReUnknownTcode => rank!(tier0),
        ErrorTag::ReCancelled => rank!(tier0),
        ErrorTag::ReUnknown => rank!(tier0),
        ErrorTag::ReInvalidArgument => rank!(tier0),
        // This must outrank ReNotFound, which is otherwise always an infra error.
        ErrorTag::DeclaredArtifactNotFound => rank!(input),
        ErrorTag::ReNotFound => rank!(tier0),
        ErrorTag::ReAlreadyExists => rank!(tier0),
        ErrorTag::ReAborted => rank!(tier0),
        ErrorTag::ReOutOfRange => rank!(tier0),
        ErrorTag::ReUnimplemented => rank!(tier0),
        ErrorTag::ReInternal => rank!(tier0),
        ErrorTag::ReUnavailable => rank!(tier0),
        ErrorTag::ReDataLoss => rank!(tier0),
        ErrorTag::ReUnauthenticated => rank!(tier0),
        ErrorTag::ReCasArtifactWrongNumberOfInputs => rank!(tier0),
        ErrorTag::ReCasArtifactWrongNumberOfOutputs => rank!(tier0),
        ErrorTag::ReCasArtifactGetDigestExpirationError => rank!(tier0),
        ErrorTag::ReCasArtifactInvalidExpiration => rank!(tier0),
        ErrorTag::ReCasArtifactExpired => rank!(tier0),
        ErrorTag::ReInvalidGetCasResponse => rank!(tier0),

        ErrorTag::Clap => rank!(tier0),
        ErrorTag::Hex => rank!(tier0),
        ErrorTag::Hyper => rank!(tier0),
        ErrorTag::Nix => rank!(tier0),
        ErrorTag::Prost => rank!(tier0),
        ErrorTag::Regex => rank!(tier0),
        ErrorTag::RelativePath => rank!(tier0),
        ErrorTag::Rusqlite => rank!(tier0),
        ErrorTag::Uuid => rank!(tier0),
        ErrorTag::SerdeJson => rank!(tier0),
        ErrorTag::StdSlice => rank!(tier0),
        ErrorTag::StdTime => rank!(tier0),
        ErrorTag::StdInfallible => rank!(tier0),
        ErrorTag::StdStripPrefix => rank!(tier0),
        ErrorTag::ParseNum => rank!(tier0),
        ErrorTag::ParseBool => rank!(tier0),
        ErrorTag::IntConversion => rank!(tier0),
        ErrorTag::StringUtf8 => rank!(tier0),
        ErrorTag::StringConversion => rank!(tier0),
        ErrorTag::CstringNul => rank!(tier0),

        ErrorTag::CacheUploadFailed => rank!(tier0),
        ErrorTag::InstallIdMismatch => rank!(tier0),
        ErrorTag::SymlinkParentMissing => rank!(tier0),

        ErrorTag::WorkerInit => rank!(tier0),
        ErrorTag::WorkerDirectoryExists => rank!(tier0),
        ErrorTag::WorkerCancelled => rank!(tier0),

        ErrorTag::DispatcherUnavailable => rank!(tier0),
        ErrorTag::DaemonStatus => rank!(tier0),
        ErrorTag::DaemonRedirect => rank!(tier0),
        ErrorTag::ReExperimentName => rank!(tier0),
        ErrorTag::CsvParse => rank!(tier0),
        ErrorTag::CasBlobCountMismatch => rank!(tier0),
        ErrorTag::DownloadSizeMismatch => rank!(tier0),

        ErrorTag::DigestTtlMismatch => rank!(tier0),
        ErrorTag::DigestTtlInvalidResponse => rank!(tier0),

        ErrorTag::Bxl => rank!(tier0),
        ErrorTag::Certs => rank!(tier0),
        ErrorTag::LogCmd => rank!(tier0),
        ErrorTag::HealthCheck => rank!(tier0),
        ErrorTag::OfflineArchive => rank!(tier0),
        ErrorTag::Profile => rank!(tier0),
        ErrorTag::Lsp => rank!(tier0),
        ErrorTag::CleanStale => rank!(tier0),
        ErrorTag::Explain => rank!(tier0),
        ErrorTag::Interpreter => rank!(tier0),
        ErrorTag::StarlarkServer => rank!(tier0),
        ErrorTag::KillAll => rank!(tier0),
        ErrorTag::BuildReport => rank!(tier0), // build report generation should never be an input error

        ErrorTag::InvalidEvent => rank!(tier0),
        ErrorTag::InvalidDigest => rank!(tier0),
        ErrorTag::InvalidDuration => rank!(tier0),
        ErrorTag::InvalidAuthToken => rank!(tier0),
        ErrorTag::InvalidUsername => rank!(tier0),
        ErrorTag::InvalidAbsPath => rank!(tier0),
        ErrorTag::InvalidBuckOutPath => rank!(tier0),
        ErrorTag::InvalidErrorReport => rank!(tier0),

        ErrorTag::WindowsUnsupported => rank!(tier0),

        ErrorTag::LocalResourceSetup => rank!(tier0),
        ErrorTag::TestOrchestrator => rank!(tier0),
        ErrorTag::TestStatusInvalid => rank!(tier0),
        ErrorTag::TestStatus => rank!(tier0),
        ErrorTag::TestRunnerUnknownExitCode => rank!(tier0),

        ErrorTag::CleanOutputs => rank!(tier0),
        ErrorTag::SaplingInvalidOutput => rank!(tier0),
        ErrorTag::CrashRequested => rank!(tier0),
        ErrorTag::CpuStats => rank!(tier0),
        ErrorTag::FailedToKill => rank!(tier0),
        ErrorTag::LogFilter => rank!(tier0),
        ErrorTag::TestOnly => rank!(tier0),
        ErrorTag::Bail => rank!(tier0),

        ErrorTag::EventLogUpload => rank!(tier0),
        ErrorTag::EventLogEof => rank!(tier0),
        ErrorTag::EventLogNotOpen => rank!(tier0),
        ErrorTag::EventLogDownload => rank!(tier0),

        ErrorTag::UnhashedOutputSymlink => rank!(tier0),
        // Validations declare their result as an output, so buck2 having materialized it and
        // then not finding it is a buck2 bug.
        ErrorTag::ValidationResultRead => rank!(tier0),
        // buck-out is also removed by concurrent `buck2 clean` invocations, which buck2 could
        // serialize.
        ErrorTag::CleanBuckOut => rank!(tier0),
        // These files are not tracked by the file watcher, so they can change mid-command.
        ErrorTag::BuckconfigRead => rank!(tier0),

        ErrorTag::MallocStats => rank!(tier0),
        ErrorTag::Mallctl => rank!(tier0),

        ErrorTag::SuperConsole => rank!(tier0),
        ErrorTag::SuperConsoleInvalidWhitespace => rank!(tier0),

        ErrorTag::IoConnectionAborted => rank!(tier0),
        ErrorTag::IoTimeout => rank!(tier0),
        ErrorTag::IoEdenMountNotReady => rank!(tier0),
        ErrorTag::IoEdenConfigError => rank!(tier0),
        ErrorTag::IoEdenVersionError => rank!(tier0),
        ErrorTag::IoEdenThriftError => rank!(tier0),
        // TODO(minglunli): Check how often Win32 Errors are actually hit, potentially do the same as POSIX
        ErrorTag::IoEdenWin32Error => rank!(tier0),
        ErrorTag::IoEdenHresultError => rank!(tier0),
        ErrorTag::IoEdenArgumentError => rank!(tier0),
        ErrorTag::IoEdenGenericError => rank!(tier0),
        ErrorTag::IoEdenMountGenerationChanged => rank!(tier0),
        ErrorTag::IoEdenJournalTruncated => rank!(tier0),
        ErrorTag::IoEdenOutOfDateParent => rank!(tier0),
        ErrorTag::IoEdenListMounts => rank!(tier0),
        ErrorTag::IoEdenRequestError => rank!(tier0),
        ErrorTag::IoEdenUnknownField => rank!(tier0),
        ErrorTag::IoEdenAttributeUnavailable => rank!(tier0),
        ErrorTag::IoEdenDataCorruption => rank!(tier0),
        ErrorTag::IoEdenNetworkTls => rank!(tier0),
        ErrorTag::IoEdenNetworkUncategorized => rank!(tier0),
        ErrorTag::IoEdenUncategorized => rank!(tier0),
        ErrorTag::WatchmanClient => rank!(tier0),
        ErrorTag::WatchmanTimeout => rank!(tier0),
        ErrorTag::WatchmanConnectionError => rank!(tier0),
        ErrorTag::WatchmanConnectionLost => rank!(tier0),
        ErrorTag::WatchmanConnectionDiscovery => rank!(tier0),
        ErrorTag::WatchmanServerError => rank!(tier0),
        ErrorTag::WatchmanResponseError => rank!(tier0),
        ErrorTag::WatchmanMissingField => rank!(tier0),
        ErrorTag::WatchmanDeserialize => rank!(tier0),
        ErrorTag::WatchmanSerialize => rank!(tier0),
        ErrorTag::WatchmanConnect => rank!(tier0),
        ErrorTag::WatchmanRequestError => rank!(tier0),
        ErrorTag::NotifyWatcher => rank!(tier0),
        ErrorTag::HttpServiceUnavailable => rank!(tier0),
        ErrorTag::HttpServer => rank!(tier0),
        ErrorTag::StarlarkInternal => rank!(tier0),
        ErrorTag::DeferredFieldRead => rank!(tier0),
        ErrorTag::ActionMismatchedOutputs => rank!(tier0),
        ErrorTag::DiceDuplicatedChange => rank!(tier0),
        ErrorTag::DiceChangedToInvalid => rank!(tier0),
        ErrorTag::DiceInjectedKeyGotInvalidation => rank!(tier0),
        ErrorTag::DiceDuplicateActivationData => rank!(tier0),
        // A dependency cycle between the user's keys.
        ErrorTag::DiceCycle => rank!(input),
        // "Resource temporarily unavailable"
        ErrorTag::EAgain => rank!(tier0),
        ErrorTag::InstallerUnknown => rank!(tier0),
        ErrorTag::InstallerTier0 => rank!(tier0).hidden(),
        ErrorTag::InternalError => rank!(tier0),
        ErrorTag::SubscriptionEmptyRequest => rank!(tier0),
        ErrorTag::CriticalPathError => rank!(tier0),
        ErrorTag::Environment => rank!(environment).hidden(),
        ErrorTag::Tier0 => rank!(tier0).hidden(),
        // Daemon disconnected with nothing in stderr, likely SIGKILLed.
        ErrorTag::DaemonDisconnect => rank!(environment),
        // Could not determine the BuckVersion
        ErrorTag::BuckVersionError => rank!(tier0),
        ErrorTag::MaterializeCopyMissingFile => rank!(tier0),

        // Input errors
        ErrorTag::AgentAdviceBlocked => rank!(input),
        ErrorTag::ClapMatch => rank!(input),
        ErrorTag::CopyOutputs => rank!(input),
        ErrorTag::ReFailedPrecondition => rank!(input),
        // FIXME(JakobDegen): Make this bad experience once that's available. Usually when this
        // happens, it's probably because the user tried to shut down with Ctrl+C and something
        // about that didn't work
        ErrorTag::InterruptedByDaemonShutdown => rank!(input),
        ErrorTag::DaemonIsBusy => rank!(input).exit_code(ExitCode::DaemonIsBusy),
        ErrorTag::DaemonPreempted => rank!(input).exit_code(ExitCode::DaemonPreempted),
        ErrorTag::ConfigureAttr => rank!(input),
        ErrorTag::DepOnlyIncompatible => rank!(input),
        ErrorTag::TargetIncompatible => rank!(input),
        ErrorTag::IoEdenCheckoutInProgress => rank!(input), // User switching branches during Eden operation
        ErrorTag::IoExecutableFileBusy => rank!(input),
        ErrorTag::IoEdenMountDoesNotExist => rank!(input),
        ErrorTag::IoEdenFileNotFound => rank!(input), // user likely specified non-existing path
        ErrorTag::MissingTarget => rank!(input),
        ErrorTag::ActionMissingOutputs => rank!(input),
        ErrorTag::ActionWrongOutputType => rank!(input),
        ErrorTag::ActionCommandFailure => rank!(input),
        ErrorTag::ProjectMissingPath => rank!(input),
        ErrorTag::ArtifactMissingFilename => rank!(input),
        ErrorTag::MissingInputPath => rank!(input),
        ErrorTag::InputPathNotADirectory => rank!(input),
        ErrorTag::StarlarkFail => rank!(input),
        ErrorTag::StarlarkStackOverflow => rank!(input),
        ErrorTag::StarlarkValue => rank!(input),
        ErrorTag::StarlarkFunction => rank!(input),
        ErrorTag::StarlarkScope => rank!(input),
        ErrorTag::StarlarkParser => rank!(input),
        ErrorTag::StarlarkNativeInput => rank!(input),
        ErrorTag::StarlarkRuntimeTypeError => rank!(input),
        ErrorTag::Visibility => rank!(input),
        ErrorTag::TestDeadlineExpired => rank!(input),
        ErrorTag::TestListingFailed => rank!(input),
        ErrorTag::TestFailed => rank!(input),
        ErrorTag::TestTimeout => rank!(input),
        ErrorTag::Unimplemented => rank!(input),
        ErrorTag::InstallerInput => rank!(input).hidden(),
        ErrorTag::BuildDeadlineExpired => rank!(input),
        ErrorTag::EventLogIndexOutOfBounds => rank!(input),
        ErrorTag::EventLogNotFound => rank!(input),
        // RE quota exhaustion is actionable by the quota-pool owner, not the build author.
        // Input category keeps these out of buck2's non-user-error alerting; infra exit code
        // so CI can regex-on-stderr detect RE resource exhaustion, prevent retries on an already
        // exhausted resource, and set the signal to yellow, not red.
        ErrorTag::ReResourceExhausted => rank!(input).exit_code(ExitCode::InfraError),
        ErrorTag::ReUserQuota => rank!(input).exit_code(ExitCode::InfraError),
        // Test runner hit fatal errors during test execution
        ErrorTag::TestFatal => rank!(input),

        ErrorTag::Input => rank!(input).hidden(),

        // Tags with unspecified category, these can represent:
        // - Tags not specific enough to determine infra vs user categorization.
        // - Tags not specific enough to usefully disambiguate category keys.
        // - Something that isn't actually an error.
        // - A phase of the build.
        // By default these are generic (excluded from category keys)
        ErrorTag::ClientGrpc => rank!(unspecified),
        ErrorTag::ClientGrpcStream => rank!(unspecified),
        ErrorTag::CompatibilityError => rank!(unspecified),
        ErrorTag::IoBrokenPipe => rank!(unspecified),
        ErrorTag::IoNotFound => rank!(unspecified),
        ErrorTag::IoNotADirectory => rank!(unspecified),
        ErrorTag::IoSource => rank!(unspecified),
        ErrorTag::IoSystem => rank!(unspecified),
        ErrorTag::IoEden => rank!(unspecified).generic(false),
        // Only says which subsystem failed, so unlike IoEden above it is left out of the
        // category key whenever a root cause tag is also present.
        ErrorTag::Sapling => rank!(unspecified),
        ErrorTag::IoEdenConnectionError => rank!(unspecified),
        ErrorTag::MaterializationError => rank!(unspecified),
        ErrorTag::CleanInterrupt => rank!(unspecified),
        ErrorTag::Tpx => rank!(unspecified),
        ErrorTag::TestExecutor => rank!(unspecified),
        ErrorTag::TestExecutorSignaled => rank!(unspecified),
        ErrorTag::TestExecutorNonZeroExit => rank!(unspecified),
        ErrorTag::TestExecutorNoEndOfTests => rank!(unspecified),
        ErrorTag::IoBlockingExecutor => rank!(unspecified),
        ErrorTag::Http => rank!(unspecified),
        ErrorTag::DownloadFileHeadRequest => rank!(unspecified),
        ErrorTag::StarlarkError => rank!(unspecified),
        ErrorTag::UnusedDefaultTag => rank!(unspecified),
        ErrorTag::BuildSketchError => rank!(unspecified),
        ErrorTag::Tokio => rank!(unspecified),
        ErrorTag::Tonic => rank!(unspecified),
        ErrorTag::Thrift => rank!(unspecified),
        ErrorTag::MissingInternalPath => rank!(unspecified),
        ErrorTag::ExitStatus => rank!(unspecified),
        ErrorTag::ActionCommandInfraFailure => rank!(unspecified),
        // Build phases
        ErrorTag::DaemonStateInitFailed => rank!(unspecified),
        ErrorTag::DaemonConnect => rank!(unspecified),
        ErrorTag::Analysis => rank!(unspecified),
        ErrorTag::Install => rank!(unspecified),
        ErrorTag::AnyActionExecution => rank!(unspecified),
    }
}

/// Errors can be categorized by tags only if they have any non-generic tags.
pub fn tag_is_generic(tag: &ErrorTag) -> bool {
    let metadata = tag_metadata(*tag);
    metadata.generic || metadata.hidden
}

/// Tags that explain all other tags on an error; see `TagMetadata::root_cause`.
pub fn tag_is_root_cause(tag: &ErrorTag) -> bool {
    tag_metadata(*tag).root_cause
}

/// Hidden tags only used internally, for categorization.
pub fn tag_is_hidden(tag: &ErrorTag) -> bool {
    tag_metadata(*tag).hidden
}

pub trait ErrorLike {
    fn best_tag(&self) -> Option<ErrorTag>;

    fn error_rank(&self) -> u32;

    fn category(&self) -> Tier;
}

impl ErrorLike for buck2_data::ErrorReport {
    fn best_tag(&self) -> Option<ErrorTag> {
        best_tag(self.tags.iter().filter_map(|t| {
            // This should never be `None`, but with weak prost types,
            // it is safer to just ignore incorrect integers.
            ErrorTag::try_from(*t).ok()
        }))
    }

    fn error_rank(self: &buck2_data::ErrorReport) -> u32 {
        self.best_tag().map_or(u32::MAX, tag_rank)
    }

    fn category(&self) -> Tier {
        self.best_tag()
            .and_then(|t| tag_metadata(t).category)
            .unwrap_or(Tier::Tier0)
    }
}

/// Pick the most interesting error by best tag.
pub fn best_error<'a>(
    tags: impl IntoIterator<Item = &'a buck2_data::ErrorReport>,
) -> Option<&'a buck2_data::ErrorReport> {
    tags.into_iter().min_by_key(|e| e.error_rank())
}

/// Pick the most interesting tag from a list of tags.
pub fn best_tag(tags: impl IntoIterator<Item = ErrorTag>) -> Option<ErrorTag> {
    tags.into_iter().min_by_key(|t| tag_rank(*t))
}

/// Tag rank: smaller is more interesting.
fn tag_rank(tag: ErrorTag) -> u32 {
    tag_metadata(tag).rank
}

/// Some tags are known to be either infrastructure or user errors.
pub(crate) fn error_tag_category(tag: ErrorTag) -> Option<Tier> {
    tag_metadata(tag).category
}

// Buck2 is the fallback/default source area, use the first non-buck2 source area.
pub fn source_area(tags: impl IntoIterator<Item = ErrorTag>) -> ErrorSourceArea {
    tags.into_iter()
        .find_map(|tag| {
            let area = tag.source_area();
            if area != ErrorSourceArea::Buck2 {
                Some(area)
            } else {
                None
            }
        })
        .unwrap_or(ErrorSourceArea::Buck2)
}

#[cfg(test)]
mod tests {
    use buck2_data::ErrorReport;
    use buck2_data::error::ErrorTag;

    use super::*;
    use crate::classify::best_tag;

    #[test]
    fn test_best_tag() {
        assert_eq!(
            Some(ErrorTag::ServerPanicked),
            best_tag([ErrorTag::ServerPanicked, ErrorTag::WatchmanTimeout])
        )
    }

    #[test]
    fn test_rank() {
        assert!(
            super::tag_rank(ErrorTag::ServerJemallocAssert)
                < super::tag_rank(ErrorTag::UnusedDefaultTag)
        )
    }

    #[test]
    fn test_re_quota_user_category_infra_exit_code() {
        for tag in [ErrorTag::ReResourceExhausted, ErrorTag::ReUserQuota] {
            assert_eq!(
                error_tag_category(tag),
                Some(Tier::Input),
                "RE quota errors must stay USER so they are excluded from non-user-error alerting"
            );
            assert_eq!(
                tag.exit_code().exit_code(),
                ExitCode::InfraError.exit_code(),
                "RE quota errors must exit as infra so CI does not blame the diff"
            );
        }
    }

    #[test]
    fn test_first_error_best_error() {
        let mut errors = vec![
            ErrorReport {
                tags: vec![ErrorTag::Input as i32],
                ..ErrorReport::default()
            },
            ErrorReport {
                tags: vec![ErrorTag::Tier0 as i32],
                ..ErrorReport::default()
            },
        ];

        let best_error = best_error(&errors).unwrap().clone();
        assert_eq!(best_error.category(), Tier::Tier0);

        // Test that first error in sorted list is equivalent to best_error.
        errors.sort_by_key(|e| e.error_rank());
        assert_eq!(errors.first(), Some(&best_error));
    }

    #[test]
    fn test_default_is_infra() {
        let errors = vec![ErrorReport {
            tags: vec![ErrorTag::UnusedDefaultTag as i32],
            ..ErrorReport::default()
        }];

        assert_eq!(best_error(&errors).map(|e| e.category()), Some(Tier::Tier0));
    }

    #[test]
    fn test_ranked_infra() {
        let errors = vec![
            ErrorReport {
                tags: vec![ErrorTag::ServerJemallocAssert as i32],
                ..ErrorReport::default()
            },
            ErrorReport {
                tags: vec![ErrorTag::Tier0 as i32],
                ..ErrorReport::default()
            },
        ];

        assert_eq!(
            best_error(&errors).map(|e| e.tags.clone()),
            Some(vec![ErrorTag::ServerJemallocAssert as i32]),
        );
    }

    #[test]
    fn test_ranked_tags() {
        let errors = vec![ErrorReport {
            tags: vec![
                ErrorTag::DaemonDisconnect as i32,
                ErrorTag::ClientGrpc as i32,
            ],
            ..ErrorReport::default()
        }];

        assert_eq!(
            best_error(&errors).map(|e| e.category()),
            Some(Tier::Environment)
        );
    }

    #[test]
    fn test_source_area() {
        assert_eq!(
            ErrorTag::DaemonDisconnect.source_area(),
            ErrorSourceArea::Buck2
        );
        assert_eq!(ErrorTag::ReAborted.source_area(), ErrorSourceArea::Re);
        assert_eq!(
            ErrorTag::WatchmanConnect.source_area(),
            ErrorSourceArea::Watchman
        );
        assert_eq!(
            ErrorTag::IoEdenArgumentError.source_area(),
            ErrorSourceArea::Eden
        );
        assert_eq!(
            ErrorTag::TestExecutor.source_area(),
            ErrorSourceArea::TestExecutor
        );
    }
}
