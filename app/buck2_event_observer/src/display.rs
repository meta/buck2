/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

// TODO(brasselsprouts): move this onto the original core types and convert in events

use std::fmt;
use std::fmt::Write;
use std::sync::Arc;
use std::time::Duration;

use buck2_common::convert::ProstDurationExt;
use buck2_data::ActionKey;
use buck2_data::ActionName;
use buck2_data::AnonTarget;
use buck2_data::BxlFunctionKey;
use buck2_data::BxlFunctionLabel;
use buck2_data::ConfiguredTargetLabel;
use buck2_data::FileWatcherKind;
use buck2_data::TargetLabel;
use buck2_data::action_key;
use buck2_data::span_start_event::Data;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_events::BuckEvent;
use buck2_hash::BuckMutInsertionSet;
use buck2_test_api::data::TestStatus;
use buck2_util::commas::commas;
use buck2_util::truncate::truncate;
use dupe::Dupe;
use superconsole::Line;
use superconsole::Lines;
use superconsole::Span;
use superconsole::style::Stylize;
use termwiz::escape::Action;
use termwiz::escape::ControlCode;

use crate::action_sub_error_display::ActionSubErrorDisplay;
use crate::fmt_duration;
use crate::verbosity::Verbosity;
use crate::what_ran::command_to_string;
use crate::what_ran::worker_command_as_fallback_to_string;

#[derive(Copy, Clone, Dupe)]
pub struct TargetDisplayOptions {
    with_configuration: bool,
}

impl TargetDisplayOptions {
    pub fn for_log() -> Self {
        Self {
            with_configuration: true,
        }
    }

    pub fn for_build_report() -> Self {
        Self {
            with_configuration: true,
        }
    }

    pub fn for_console(with_configuration: bool) -> Self {
        Self { with_configuration }
    }

    pub fn for_chrome_trace() -> Self {
        Self {
            with_configuration: false,
        }
    }
}

pub fn display_configured_target_label(
    ctl: &ConfiguredTargetLabel,
    opts: TargetDisplayOptions,
) -> buck2_error::Result<String> {
    if let ConfiguredTargetLabel {
        label: Some(TargetLabel { package, name }),
        configuration: Some(configuration),
        // We never display execution configurations at the moment
        execution_configuration: _,
    } = ctl
    {
        Ok(if opts.with_configuration {
            format!("{}:{} ({})", package, name, configuration.full_name)
        } else {
            format!("{package}:{name}")
        })
    } else {
        Err(ParseEventError::InvalidConfiguredTargetLabel.into())
    }
}

fn display_configured_target_label_opt(
    ctl: Option<&ConfiguredTargetLabel>,
    opts: TargetDisplayOptions,
) -> buck2_error::Result<String> {
    Ok(match ctl {
        Some(ctl) => display_configured_target_label(ctl, opts)?,
        None => {
            // Should never happen, but better not error here.
            "unknown target".to_owned()
        }
    })
}

pub fn display_anon_target(ctl: &AnonTarget) -> buck2_error::Result<String> {
    if let AnonTarget {
        name: Some(TargetLabel { package, name }),
        // We currently never display execution configurations, only normal configurations
        execution_configuration: _,
        hash,
    } = ctl
    {
        Ok(format!("{package}:{name}@{hash}"))
    } else {
        Err(ParseEventError::InvalidAnonTarget.into())
    }
}

pub fn display_analysis_target(
    target: &buck2_data::analysis_start::Target,
    opts: TargetDisplayOptions,
) -> buck2_error::Result<String> {
    use buck2_data::analysis_start::Target;
    match target {
        Target::StandardTarget(ctl) => display_configured_target_label(ctl, opts),
        Target::AnonTarget(anon) => display_anon_target(anon),
        Target::DynamicLambda(dynamic) => {
            use buck2_data::dynamic_lambda_owner::Owner;
            match dynamic.owner.as_ref().internal_error("Missing `owner`")? {
                Owner::TargetLabel(target_label) => {
                    display_configured_target_label(target_label, opts)
                }
                Owner::BxlKey(bxl_key) => display_bxl_key(bxl_key),
                Owner::AnonTarget(anon_target) => display_anon_target(anon_target),
            }
        }
    }
}

pub fn display_bxl_key(ctl: &BxlFunctionKey) -> buck2_error::Result<String> {
    if let BxlFunctionKey {
        label: Some(BxlFunctionLabel { bxl_path, name }),
    } = ctl
    {
        Ok(format!("{bxl_path}:{name}"))
    } else {
        Err(ParseEventError::MissingBxlFunctionLabel.into())
    }
}

pub fn display_action_owner(
    owner: &action_key::Owner,
    opts: TargetDisplayOptions,
) -> buck2_error::Result<String> {
    match owner {
        action_key::Owner::TargetLabel(target_label)
        | action_key::Owner::TestTargetLabel(target_label)
        | action_key::Owner::LocalResourceSetup(target_label) => {
            display_configured_target_label(target_label, opts)
        }
        action_key::Owner::BxlKey(bxl_key) => display_bxl_key(bxl_key),
        action_key::Owner::AnonTarget(anon_target) => display_anon_target(anon_target),
    }
}

pub fn display_action_key(
    action_key: &ActionKey,
    opts: TargetDisplayOptions,
) -> buck2_error::Result<String> {
    if let ActionKey {
        owner: Some(owner), ..
    } = action_key
    {
        display_action_owner(owner, opts)
    } else {
        Err(ParseEventError::MissingActionOwner.into())
    }
}

fn action_key_leaf_name(action_key: &ActionKey) -> buck2_error::Result<&str> {
    let ActionKey {
        owner: Some(owner), ..
    } = action_key
    else {
        return Err(ParseEventError::MissingActionOwner.into());
    };

    match owner {
        action_key::Owner::TargetLabel(ConfiguredTargetLabel {
            label: Some(TargetLabel { package: _, name }),
            configuration: _,
            execution_configuration: _,
        })
        | action_key::Owner::TestTargetLabel(ConfiguredTargetLabel {
            label: Some(TargetLabel { package: _, name }),
            configuration: _,
            execution_configuration: _,
        })
        | action_key::Owner::LocalResourceSetup(ConfiguredTargetLabel {
            label: Some(TargetLabel { package: _, name }),
            configuration: _,
            execution_configuration: _,
        }) => Ok(name.as_str()),
        action_key::Owner::TargetLabel(ConfiguredTargetLabel {
            label: None,
            configuration: _,
            execution_configuration: _,
        })
        | action_key::Owner::TestTargetLabel(ConfiguredTargetLabel {
            label: None,
            configuration: _,
            execution_configuration: _,
        })
        | action_key::Owner::LocalResourceSetup(ConfiguredTargetLabel {
            label: None,
            configuration: _,
            execution_configuration: _,
        }) => Err(ParseEventError::InvalidConfiguredTargetLabel.into()),
        action_key::Owner::BxlKey(BxlFunctionKey {
            label: Some(BxlFunctionLabel { bxl_path: _, name }),
        }) => Ok(name.as_str()),
        action_key::Owner::BxlKey(BxlFunctionKey { label: None }) => {
            Err(ParseEventError::MissingBxlFunctionLabel.into())
        }
        action_key::Owner::AnonTarget(AnonTarget {
            name: Some(TargetLabel { package: _, name }),
            execution_configuration: _,
            hash: _,
        }) => Ok(name.as_str()),
        action_key::Owner::AnonTarget(AnonTarget {
            name: None,
            execution_configuration: _,
            hash: _,
        }) => Err(ParseEventError::InvalidAnonTarget.into()),
    }
}

pub fn display_action_identity(
    action_key: Option<&ActionKey>,
    name: Option<&ActionName>,
    opts: TargetDisplayOptions,
) -> buck2_error::Result<String> {
    let key_string = match action_key {
        Some(key) => display_action_key(key, opts),
        None => Err(ParseEventError::MissingActionKey.into()),
    }?;
    let action_string = match name {
        Some(ActionName {
            category,
            identifier,
        }) if !identifier.is_empty() => format!(" ({category} {identifier})"),
        Some(ActionName { category, .. }) => format!(" ({category})"),
        None => String::new(),
    };

    Ok(format!("{key_string}{action_string}"))
}

/// A rendered span event, split into the entity it acts on (`label`) and what is
/// happening to it (`detail`).
///
/// Keeping the two apart lets the console style and lay them out independently —
/// e.g. leave the target plain and color the action — instead of re-parsing a
/// flattened `"{label} -- {detail}"` string.
pub struct EventDisplay {
    pub label: Option<String>,
    pub detail: String,
    pub category: Option<String>,
}

impl EventDisplay {
    fn bare(detail: impl Into<String>) -> Self {
        Self {
            label: None,
            detail: detail.into(),
            category: None,
        }
    }

    fn labeled(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            label: Some(label.into()),
            detail: detail.into(),
            category: None,
        }
    }

    fn labeled_action(
        label: impl Into<String>,
        detail: impl Into<String>,
        category: impl Into<String>,
    ) -> Self {
        Self {
            label: Some(label.into()),
            detail: detail.into(),
            category: Some(category.into()),
        }
    }
}

impl fmt::Display for EventDisplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.label {
            Some(label) => write!(f, "{label} -- {}", self.detail),
            None => write!(f, "{}", self.detail),
        }
    }
}

/// Formats event payloads for display.
pub fn display_event(
    event: &BuckEvent,
    opts: TargetDisplayOptions,
) -> buck2_error::Result<EventDisplay> {
    let res: buck2_error::Result<_> = try {
        let data = match event.data() {
            buck2_data::buck_event::Data::SpanStart(start) => start.data.as_ref().unwrap(),
            _ => Err(buck2_error::Error::from(ParseEventError::UnexpectedEvent))?,
        };

        let res: buck2_error::Result<_> = match data {
            Data::ActionExecution(action) => match &action.key {
                Some(key) => {
                    let target = display_action_key(key, opts)?;
                    let event = match action.name.as_ref() {
                        Some(name) => {
                            let detail = if name.identifier.is_empty()
                                || name.identifier.as_str() == action_key_leaf_name(key)?
                            {
                                name.category.clone()
                            } else {
                                format!("{} {}", name.category, name.identifier)
                            };
                            EventDisplay::labeled_action(target, detail, name.category.clone())
                        }
                        None => EventDisplay::labeled(target, "unknown"),
                    };
                    Ok(event)
                }
                None => Err(ParseEventError::MissingActionKey.into()),
            },
            Data::FinalMaterialization(materialization) => {
                let build = &materialization
                    .artifact
                    .as_ref()
                    .ok_or_else(|| buck2_error::Error::from(ParseEventError::MissingArtifact))?;

                let key = display_action_key(
                    build.key.as_ref().ok_or_else(|| {
                        buck2_error::Error::from(ParseEventError::MissingActionKey)
                    })?,
                    opts,
                )?;
                let path = {
                    if build.path.is_empty() {
                        Err(buck2_error::Error::from(
                            ParseEventError::MissingMaterializationPath,
                        ))
                    } else {
                        Ok(&build.path)
                    }
                }?;
                Ok(EventDisplay::labeled(
                    key,
                    format!("materializing `{path}`"),
                ))
            }
            Data::Analysis(analysis) => match &analysis.target {
                Some(target) => {
                    let target = display_analysis_target(target, opts)?;
                    Ok(EventDisplay::labeled(target, "running analysis"))
                }
                None => Err(ParseEventError::MissingConfiguredTargetLabel.into()),
            },
            Data::AnalysisStage(info) => {
                let stage = info
                    .stage
                    .as_ref()
                    .internal_error("analysis stage is missing")?;
                let stage = display_analysis_stage(stage);
                Ok(EventDisplay::bare(stage))
            }
            Data::AnalysisResolveQueries(resolve_queries) => Ok(EventDisplay::labeled(
                display_configured_target_label_opt(
                    resolve_queries.standard_target.as_ref(),
                    opts,
                )?,
                "analysis queries",
            )),
            Data::LoadPackage(load) => Ok(EventDisplay::labeled(
                load.path.clone(),
                "loading package file tree",
            )),
            Data::Load(load) => Ok(EventDisplay::labeled(
                load.module_id.clone(),
                "evaluating build file",
            )),
            Data::ExecutorStage(info) => {
                let stage = info
                    .stage
                    .as_ref()
                    .internal_error("executor stage is missing")?;
                let stage = display_executor_stage_detail(stage)
                    .internal_error("unknown executor stage")?;
                Ok(EventDisplay::bare(stage))
            }
            Data::TestDiscovery(discovery) => Ok(EventDisplay::labeled(
                format!("Test {}", discovery.suite_name),
                "discovering tests",
            )),
            Data::TestRun(start) => match &start.suite {
                Some(suite) => {
                    let tests = {
                        if suite.test_names.len() < 100 {
                            suite.test_names.join(" ")
                        } else {
                            format!(
                                "{}...({} more)",
                                suite.test_names.first().unwrap(),
                                suite.test_names.len() - 1
                            )
                        }
                    };
                    Ok(EventDisplay::labeled(
                        format!("Test {}", suite.suite_name),
                        tests,
                    ))
                }
                None => Err(ParseEventError::MissingSuiteName.into()),
            },
            Data::CommandCritical(..) => Err(ParseEventError::UnexpectedEvent.into()),
            Data::Command(..) => Err(ParseEventError::UnexpectedEvent.into()),
            Data::FileWatcher(x) => Ok(EventDisplay::bare(format!(
                "Syncing file changes via {}",
                display_file_watcher(x.provider)
            ))),
            Data::MatchDepFiles(buck2_data::MatchDepFilesStart {
                checking_filtered_inputs,
                remote_cache,
            }) => {
                let location = if *remote_cache {
                    "remote cache"
                } else {
                    "local"
                };
                let detail = if *checking_filtered_inputs {
                    "full"
                } else {
                    "partial"
                };
                Ok(EventDisplay::bare(format!(
                    "dep_files({detail},{location})"
                )))
            }
            Data::SharedTask(buck2_data::SharedTaskStart { owner_trace_id }) => {
                Ok(EventDisplay::bare(format!(
                    "Waiting on task from another command: {owner_trace_id}"
                )))
            }
            Data::CacheUpload(_) => Ok(EventDisplay::bare("upload (action)")),
            Data::DepFileUpload(_) => Ok(EventDisplay::bare("upload (dep_file)")),
            Data::CreateOutputSymlinks(..) => Ok(EventDisplay::bare("Creating output symlinks")),
            Data::InstallEventInfo(info) => Ok(EventDisplay::bare(format!(
                "Sending {} at path {}",
                info.artifact_name, info.file_path
            ))),
            Data::DiceStateUpdate(..) => Ok(EventDisplay::bare("Syncing changes to graph")),
            Data::Materialization(..) => Ok(EventDisplay::bare("materializing")),
            Data::DiceCriticalSection(..) => Err(ParseEventError::UnexpectedEvent.into()),
            Data::DiceBlockConcurrentCommand(cmd) => Ok(EventDisplay::bare(format!(
                "Waiting for command [{}] to finish",
                truncate(&cmd.cmd_args, 200),
            ))),
            Data::DiceSynchronizeSection(..) => {
                Ok(EventDisplay::bare("Synchronizing buck2 internal state"))
            }
            Data::DiceCleanup(..) => Ok(EventDisplay::bare("Cleaning up graph state")),
            Data::ExclusiveCommandWait(buck2_data::ExclusiveCommandWaitStart { command_name }) => {
                if let Some(name) = command_name {
                    Ok(EventDisplay::bare(format!(
                        "Waiting for command [{name}] to finish"
                    )))
                } else {
                    Ok(EventDisplay::bare("Waiting for dice"))
                }
            }
            Data::DeferredPreparationStage(prep) => {
                use buck2_data::deferred_preparation_stage_start::Stage;
                match prep.stage.as_ref().internal_error("Missing `stage`")? {
                    Stage::MaterializedArtifacts(_) => {
                        Ok(EventDisplay::bare("local_materialize_inputs"))
                    }
                }
            }
            Data::DynamicLambda(lambda) => {
                use buck2_data::dynamic_lambda_start::Owner;

                let label = match lambda.owner.as_ref().internal_error("Missing `owner`")? {
                    Owner::TargetLabel(target_label) => {
                        display_configured_target_label(target_label, opts)
                    }
                    Owner::BxlKey(bxl_key) => display_bxl_key(bxl_key),
                    Owner::AnonTarget(anon_target) => display_anon_target(anon_target),
                }?;

                Ok(EventDisplay::labeled(label, "dynamic analysis"))
            }
            Data::BxlExecution(execution) => Ok(EventDisplay::bare(format!(
                "Executing BXL script `{}`",
                execution.name
            ))),
            Data::BxlDiceInvocation(..) => Ok(EventDisplay::bare("Waiting for graph computations")),
            Data::ReUpload(..) => Ok(EventDisplay::bare("re_upload")),
            Data::ReSubmit(..) => Ok(EventDisplay::bare("re_submit")),
            Data::ConnectToInstaller(buck2_data::ConnectToInstallerStart { tcp_port }) => Ok(
                EventDisplay::bare(format!("Connecting to installer on port {tcp_port}")),
            ),
            Data::Fake(fake) => Ok(EventDisplay::labeled(
                fake.caramba.clone(),
                "speak of the devil",
            )),
            Data::LocalResources(res) => {
                let target = display_configured_target_label_opt(res.target_label.as_ref(), opts)?;
                Ok(EventDisplay::labeled(target, "Local resources setup"))
            }
            Data::ReleaseLocalResources(..) => Ok(EventDisplay::bare("Releasing local resources")),
            Data::BxlEnsureArtifacts(..) => Err(ParseEventError::UnexpectedEvent.into()),
            Data::ActionErrorHandlerExecution(..) => Ok(EventDisplay::bare(
                "Running error handler on action failure",
            )),
            Data::CqueryUniverseBuild(..) => Ok(EventDisplay::bare("Building cquery universe")),
            Data::ComputeDetailedAggregatedMetrics(..) => {
                Ok(EventDisplay::bare("Computing detailed aggregated metrics"))
            }
        };

        // This shouldn't really be necessary, but that's how try blocks work :(
        res?
    };

    res.with_buck_error_context(|| InvalidBuckEvent(Arc::new(event.clone())).to_string())
}

fn display_file_watcher(provider: i32) -> &'static str {
    match buck2_data::FileWatcherProvider::try_from(provider) {
        Ok(buck2_data::FileWatcherProvider::Watchman) => "Watchman",
        Ok(buck2_data::FileWatcherProvider::RustNotify) => "notify",
        Ok(buck2_data::FileWatcherProvider::FsHashCrawler) => "fs_hash_crawler",
        Ok(buck2_data::FileWatcherProvider::EdenFs) => "EdenFS",
        Err(_) => "unknown mechanism",
    }
}

pub fn display_analysis_stage(stage: &buck2_data::analysis_stage_start::Stage) -> &'static str {
    use buck2_data::analysis_stage_start::Stage;

    match stage {
        Stage::EvaluateRule(()) => "evaluate_rule",
    }
}

pub fn display_file_watcher_end(file_watcher_end: &buck2_data::FileWatcherEnd) -> Vec<String> {
    const MAX_PRINT_MESSAGES: usize = 3;
    let mut res = Vec::new();

    if let Some(stats) = &file_watcher_end.stats {
        // The `FileWatcherEvent` contain no duplicates. However, there can be two distinct events
        // for the same file, e.g. `foo` was deleted as a file and then created as a directory.
        //
        // It looks really odd to print "File changed: foo" twice, so we dedupe user messages.
        //
        // If there are more file change records than we passed over, and some of the omitted ones are
        // duplicates on the same file, then our "additional file change events" count is slightly high.
        // Shouldn't be a big deal in practice, since it is rare, and fairly big numbers already.

        let is_fresh_instance = stats.fresh_instance;

        let mut to_print = BuckMutInsertionSet::new();
        for x in &stats.events {
            to_print.insert((&x.path, x.kind()));
        }
        for (path, kind) in to_print.iter().take(MAX_PRINT_MESSAGES) {
            let kind = match kind {
                FileWatcherKind::Directory => "Directory",
                FileWatcherKind::File | FileWatcherKind::Symlink => "File",
            };
            if is_fresh_instance {
                res.push(format!("{kind} changed (since mergebase): {path}"));
            } else {
                res.push(format!("{kind} changed: {path}"));
            }
        }
        let unprinted_paths =
            // those we have the names of but didn't print
            to_print.len().saturating_sub(MAX_PRINT_MESSAGES) +
                // plus those we didn't get the names for
                (stats.events_processed as usize).saturating_sub(stats.events.len());
        if unprinted_paths > 0 {
            if is_fresh_instance {
                res.push(format!(
                    "{unprinted_paths} additional file change events (since mergebase)"
                ));
            } else {
                res.push(format!("{unprinted_paths} additional file change events"));
            }
        }

        if let Some(fresh_instance) = &stats.fresh_instance_data {
            let file_watcher = if stats.watchman_version.is_some() {
                "Watchman"
            } else {
                "File Watcher"
            };

            let mut msg = format!("{file_watcher} fresh instance: ");
            let mut comma = commas();
            if fresh_instance.new_mergebase {
                comma(&mut msg).unwrap();
                write!(&mut msg, "new mergebase").unwrap();
            }
            if fresh_instance.cleared_dice {
                comma(&mut msg).unwrap();
                write!(&mut msg, "cleared graph state").unwrap();
            }
            if fresh_instance.cleared_dep_files {
                comma(&mut msg).unwrap();
                write!(&mut msg, "cleared dep files").unwrap();
            }
            res.push(msg);
        }
    }

    res
}

/// Like `display_executor_stage`, but names the exceeded quotas when RE reports
/// the action as over quota.
fn display_executor_stage_detail(
    stage: &buck2_data::executor_stage_start::Stage,
) -> Option<String> {
    use buck2_data::executor_stage_start::Stage;

    if let Stage::Re(re) = stage
        && let Some(buck2_data::re_stage::Stage::QueueOverQuota(over_quota)) = &re.stage
        && !over_quota.exceeded_quotas.is_empty()
    {
        return Some(format!(
            "re_queued(over_quota: {})",
            over_quota.exceeded_quotas.join(", ")
        ));
    }

    display_executor_stage(stage).map(str::to_owned)
}

pub fn display_executor_stage(
    stage: &buck2_data::executor_stage_start::Stage,
) -> Option<&'static str> {
    use buck2_data::executor_stage_start::Stage;

    let label = match stage {
        Stage::Prepare(..) => "prepare",
        Stage::CasArtifact(cas) => {
            use buck2_data::cas_artifact_stage::Phase;
            match Phase::try_from(cas.phase) {
                Ok(Phase::CheckExpiration) => "cas_check_expiration",
                Ok(Phase::ExtendExpiration) => "cas_extend_expiration",
                Ok(Phase::RecheckExpiration) => "cas_recheck_expiration",
                Ok(Phase::TreeMetadata) => "cas_tree_metadata",
                Ok(Phase::DeclareOutputs) => "cas_declare_outputs",
                Ok(Phase::Unspecified) | Err(_) => "cas_artifact",
            }
        }
        Stage::CacheQuery(cache_query) => {
            match buck2_data::CacheType::try_from(cache_query.cache_type).unwrap() {
                buck2_data::CacheType::ActionCache => "re_action_cache",
                buck2_data::CacheType::RemoteDepFileCache => "re_dep_file_cache",
            }
        }
        Stage::CacheHit(..) => "re_download",
        Stage::Re(re) => {
            use buck2_data::re_stage::Stage;

            match re.stage.as_ref()? {
                Stage::Execute(..) => "re_execute",
                Stage::Download(..) => "re_download",
                Stage::Queue(..) => "re_queued",
                Stage::QueueOverQuota(..) => "re_queued(over_quota)",
                Stage::QueueAcquiringDependencies(..) => "re_queued(waiting_on_deps)",
                Stage::QueueNoWorkerAvailable(..) => "re_queued(no_workers)",
                Stage::QueueCancelled(..) => "re_cancelled",
                Stage::WorkerDownload(..) => "re_worker_download",
                Stage::WorkerUpload(..) => "re_worker_upload",
                Stage::Unknown(..) => "re_unknown",
                Stage::MaterializeFailedInputs(..) => "re_materialize_failed_inputs",
                Stage::BeforeActionExecution(_) => "initialize_re_worker",
                Stage::AfterActionExecution(_) => "release_re_worker",
            }
        }
        Stage::Local(local) => {
            use buck2_data::local_stage::Stage;

            match local.stage.as_ref()? {
                Stage::Queued(..) => "local_queued",
                Stage::Execute(..) => "local_execute",
                Stage::MaterializeInputs(..) => "local_materialize_inputs",
                Stage::PrepareOutputs(_) => "local_prepare_outputs",
                Stage::FinalizeOutputs(_) => "local_finalize_outputs",
                Stage::AcquireLocalResource(_) => "acquire_local_resource",
                Stage::WorkerInit(_) => "initialize_worker",
                Stage::WorkerExecute(_) => "worker_execute",
                Stage::WorkerQueued(..) => "worker_queued",
                Stage::WorkerWait(_) => "initialize_worker",
            }
        }
    };

    Some(label)
}

/// Whether `event` is an executor stage that is actively running the action,
/// rather than queueing, fetching from cache/RE, or materializing inputs.
///
/// Gates the slow-action coloring so the warning reflects slow work, not time
/// spent waiting for a local slot or for inputs under contention.
pub fn is_active_execution_stage(event: &BuckEvent) -> bool {
    let buck2_data::buck_event::Data::SpanStart(start) = event.data() else {
        return false;
    };
    let Some(Data::ExecutorStage(info)) = start.data.as_ref() else {
        return false;
    };
    let Some(stage) = info.stage.as_ref() else {
        return false;
    };

    use buck2_data::executor_stage_start::Stage;
    match stage {
        Stage::Local(local) => {
            use buck2_data::local_stage::Stage as Local;
            match local.stage.as_ref() {
                Some(Local::Execute(..)) | Some(Local::WorkerExecute(..)) => true,
                Some(Local::Queued(..))
                | Some(Local::MaterializeInputs(..))
                | Some(Local::PrepareOutputs(..))
                | Some(Local::FinalizeOutputs(..))
                | Some(Local::AcquireLocalResource(..))
                | Some(Local::WorkerInit(..))
                | Some(Local::WorkerQueued(..))
                | Some(Local::WorkerWait(..))
                | None => false,
            }
        }
        Stage::Re(re) => {
            use buck2_data::re_stage::Stage as Re;
            match re.stage.as_ref() {
                Some(Re::Execute(..)) => true,
                Some(Re::Download(..))
                | Some(Re::Queue(..))
                | Some(Re::QueueOverQuota(..))
                | Some(Re::QueueAcquiringDependencies(..))
                | Some(Re::QueueNoWorkerAvailable(..))
                | Some(Re::QueueCancelled(..))
                | Some(Re::WorkerDownload(..))
                | Some(Re::WorkerUpload(..))
                | Some(Re::Unknown(..))
                | Some(Re::MaterializeFailedInputs(..))
                | Some(Re::BeforeActionExecution(..))
                | Some(Re::AfterActionExecution(..))
                | None => false,
            }
        }
        Stage::Prepare(..)
        | Stage::CacheQuery(..)
        | Stage::CacheHit(..)
        | Stage::CasArtifact(..) => false,
    }
}

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Input)]
enum ParseEventError {
    #[error("Missing configured target label")]
    MissingConfiguredTargetLabel,
    #[error("Invalid configured target label")]
    InvalidConfiguredTargetLabel,
    #[error("Invalid anon target")]
    InvalidAnonTarget,
    #[error("Missing action key")]
    MissingActionKey,
    #[error("Missing suite name")]
    MissingSuiteName,
    #[error("Missing artifact")]
    MissingArtifact,
    #[error("Missing materialization path")]
    MissingMaterializationPath,
    #[error("Missing action owner")]
    MissingActionOwner,
    #[error("Missing bxl function label")]
    MissingBxlFunctionLabel,
    #[error("Unexpected event")]
    UnexpectedEvent,
}

#[derive(buck2_error::Error, Debug)]
#[error("Invalid buck event: `{0:?}`")]
#[buck2(tag = Tier0)]
pub struct InvalidBuckEvent(pub Arc<BuckEvent>);

pub fn format_test_result(
    test_result: &buck2_data::TestResult,
    verbosity: Verbosity,
) -> buck2_error::Result<Option<Lines>> {
    let buck2_data::TestResult {
        name,
        status,
        duration,
        details,
        ..
    } = test_result;
    let status = TestStatus::try_from(*status)?;

    // Pass results normally have no details, unless the --print-passing-details is set.
    // Do not display anything for passing tests unless verbosity is high or details are present
    // to avoid cluttering the UI with unimportant test results.
    if matches!(&status, TestStatus::PASS | TestStatus::LISTING_SUCCESS)
        && details.is_empty()
        && !verbosity.print_all_commands()
    {
        return Ok(None);
    }

    let prefix = match status {
        TestStatus::FAIL => Span::new_styled("✗ Fail".to_owned().red()),
        TestStatus::SKIP => Span::new_styled("↷ Skip".to_owned().cyan()),
        TestStatus::OMITTED => Span::new_styled("\u{20E0} Omitted".to_owned().cyan()),
        TestStatus::FATAL => Span::new_styled("⚠ Fatal".to_owned().red()),
        TestStatus::TIMEOUT => Span::new_styled("✉ Timeout".to_owned().cyan()),
        TestStatus::INFRA_FAILURE => Span::new_styled("🛠 Infra Failure".to_owned().magenta()),
        TestStatus::PASS => Span::new_styled("✓ Pass".to_owned().green()),
        TestStatus::LISTING_SUCCESS => Span::new_styled("✓ Listing success".to_owned().green()),
        TestStatus::UNKNOWN => Span::new_styled("? Unknown".to_owned().cyan()),
        TestStatus::RERUN => Span::new_styled("↻ Rerun".to_owned().cyan()),
        TestStatus::LISTING_FAILED => Span::new_styled("⚠ Listing failed".to_owned().red()),
    }?;
    // A test name is free text from the test runner, so it can hold a tab or a
    // line break (a title interpolating a value, say), which a `Span` refuses.
    let name = name.replace(|c: char| c != ' ' && c.is_whitespace(), " ");
    let mut base = Line::from_iter([prefix, Span::new_unstyled(format!(": {name}",))?]);

    if let Some(duration) = duration {
        if let Ok(duration) = Duration::try_from(*duration) {
            base.push(Span::new_unstyled(format!(
                " ({})",
                fmt_duration::fmt_duration(duration)
            ))?);
        }
    }
    // If a test has details, we always show them. It's the test runner's
    // responsibility to withhold details when these are not relevant.
    // For instance, tpx will always withhold details of passing tests
    // unless the --print-passing-details is set.
    let mut lines = vec![base];
    if !details.is_empty() {
        lines.append(&mut Lines::from_multiline_string(details, Default::default()).0);
    }
    Ok(Some(Lines(lines)))
}

pub struct ActionErrorDisplay<'a> {
    pub action_id: String,
    pub reason: String,
    pub command: Option<&'a buck2_data::CommandExecutionDetails>,
    pub error_diagnostics: Option<&'a buck2_data::ActionErrorDiagnostics>,
}

fn strip_trailing_newline(stream_contents: &str) -> &str {
    match stream_contents.strip_suffix('\n') {
        None => stream_contents,
        Some(s) => s.strip_suffix('\r').unwrap_or(s),
    }
}

/// Controls whether stdout/stderr stream contents are included in action error
/// formatting.
#[derive(Copy, Clone, Debug)]
pub enum ActionErrorOutputFormat {
    /// Include stdout/stderr stream contents in the output.
    IncludeOutputStreams,
    /// Exclude stdout/stderr stream contents (metadata only).
    ExcludeOutputStreams,
    /// Exclude stdout/stderr and append a substitute message (e.g. output limit exceeded).
    SubstituteOutputStreams(&'static str),
}

impl ActionErrorDisplay<'_> {
    /// Format the error message in a way that is suitable for use with the build report
    ///
    /// The output may include terminal colors that need to be sanitized.
    pub fn simple_format_for_build_report(&self) -> String {
        let s = self.simple_format_inner(
            None::<&'static mut dyn for<'x> FnMut(&'x str) -> String>,
            ActionErrorOutputFormat::IncludeOutputStreams,
        );
        sanitize_output_colors(s.as_bytes())
    }

    /// Format the error message in a way that is suitable for use with the simpleconsole
    ///
    /// The output may include terminal colors that need to be sanitized
    pub fn simple_format_with_timestamps(
        &self,
        with_timestamps: impl FnMut(&str) -> String,
        output_format: ActionErrorOutputFormat,
    ) -> String {
        self.simple_format_inner(Some(with_timestamps), output_format)
    }

    fn simple_format_inner(
        &self,
        mut with_timestamps: Option<impl FnMut(&str) -> String>,
        output_format: ActionErrorOutputFormat,
    ) -> String {
        let mut s = String::new();
        macro_rules! append {
            ($fmt:expr $(, $args:expr)*) => {{
                let mut message = format!($fmt $(, $args)*);
                if let Some(with_timestamps) = &mut with_timestamps {
                    message = with_timestamps(&message);
                }
                writeln!(s, "{message}").unwrap();
            }};
        }
        append!("Action failed: {}", self.action_id);
        append!("{}", self.reason);
        let Some(command_failed) = &self.command else {
            return s;
        };
        if let Some(command_kind) = command_failed.command_kind.as_ref() {
            use buck2_data::command_execution_kind::Command;
            match command_kind.command.as_ref() {
                Some(Command::LocalCommand(local_command)) => {
                    append!("Local command: {}", command_to_string(local_command));
                }
                Some(Command::WorkerCommand(worker_command)) => {
                    append!(
                        "Local worker command: {}",
                        worker_command_as_fallback_to_string(worker_command)
                    );
                }
                Some(Command::WorkerInitCommand(worker_init_command)) => {
                    append!(
                        "Local worker initialization command: {}",
                        command_to_string(worker_init_command)
                    );
                }
                Some(Command::RemoteCommand(remote_command)) => {
                    if buck2_core::is_open_source() {
                        append!("Remote action digest: '{}'", remote_command.action_digest);
                    } else {
                        append!(
                            "Remote action{}, reproduce with: `frecli cas download-action {}`",
                            match remote_command.cache_hit_type() {
                                buck2_data::RemoteCommandOrigin::ActionCache
                                | buck2_data::RemoteCommandOrigin::RemoteDepFileCache => {
                                    " cache hit"
                                }
                                buck2_data::RemoteCommandOrigin::Executed => "",
                                buck2_data::RemoteCommandOrigin::ReServedFromCache => {
                                    " served from the remote execution cache"
                                }
                                buck2_data::RemoteCommandOrigin::ReDeduplicated => {
                                    " deduplicated by remote execution"
                                }
                            },
                            remote_command.action_digest
                        );
                    }
                }
                Some(Command::OmittedLocalCommand(..)) | None => {
                    // Nothing to show in this case.
                }
            };
        }

        match output_format {
            ActionErrorOutputFormat::IncludeOutputStreams => {}
            ActionErrorOutputFormat::ExcludeOutputStreams => return s,
            ActionErrorOutputFormat::SubstituteOutputStreams(msg) => {
                append!("{msg}");
                return s;
            }
        }

        let mut append_stream = |name, contents: &str| {
            if contents.is_empty() {
                append!("{name}: <empty>");
            } else {
                append!("{name}:");
                let contents = strip_trailing_newline(contents);
                writeln!(s, "{contents}").unwrap();
            }
        };

        append_stream("Stdout", &command_failed.cmd_stdout);
        append_stream("Stderr", &command_failed.cmd_stderr);

        if let Some(additional_info) = &command_failed.additional_message {
            if !additional_info.is_empty() {
                append_stream("Info", additional_info);
            }
        }

        if let Some(error_diagnostics) = self.error_diagnostics {
            match error_diagnostics.data.as_ref().unwrap() {
                buck2_data::action_error_diagnostics::Data::SubErrors(sub_errors) => {
                    let sub_errors = &sub_errors.sub_errors;
                    if !sub_errors.is_empty() {
                        let mut all_sub_errors = String::new();
                        for sub_error in sub_errors {
                            // Display errors based on show_in_stderr flag is true
                            if sub_error.show_in_stderr {
                                if let Some(display_msg) = sub_error.display() {
                                    writeln!(all_sub_errors, "- {}", display_msg).unwrap();
                                }
                            }
                        }
                        if !all_sub_errors.is_empty() {
                            append_stream(
                                "\nAction sub-errors produced by error handlers",
                                &all_sub_errors,
                            );
                        }
                    }
                }
                buck2_data::action_error_diagnostics::Data::HandlerInvocationError(error) => {
                    append_stream("\nCould not produce error diagnostics", error);
                }
            };
        }
        s
    }

    /// Returns an estimate of the stdout/stderr byte count in this error.
    pub fn output_stream_byte_count(&self) -> usize {
        let Some(command) = &self.command else {
            return 0;
        };
        command.cmd_stdout.len() + command.cmd_stderr.len()
    }
}

pub fn get_action_error_reason(error: &buck2_data::ActionError) -> buck2_error::Result<String> {
    use buck2_data::action_error::Error;

    Ok(
        match error
            .error
            .as_ref()
            .internal_error("Internal error: Missing error in action error")?
        {
            Error::MissingOutputs(missing_outputs) => {
                format!("Required outputs are missing: {}", missing_outputs.message)
            }
            Error::Unknown(error_string) => error_string.to_owned(),
            Error::CommandExecutionError(buck2_data::CommandExecutionError {}) => {
                match &error.last_command {
                    Some(c) => failure_reason_for_command_execution(c)?,
                    None => "Unexpected command status".to_owned(),
                }
            }
        },
    )
}

pub fn display_action_error(
    error: &buck2_data::ActionError,
    opts: TargetDisplayOptions,
) -> buck2_error::Result<ActionErrorDisplay<'_>> {
    let command = error.last_command.as_ref().and_then(|c| c.details.as_ref());

    let reason = get_action_error_reason(error)?;

    Ok(ActionErrorDisplay {
        action_id: display_action_identity(error.key.as_ref(), error.name.as_ref(), opts)?,
        reason,
        command,
        error_diagnostics: error.error_diagnostics.as_ref(),
    })
}

fn failure_reason_for_command_execution(
    command_execution: &buck2_data::CommandExecution,
) -> buck2_error::Result<String> {
    use buck2_data::command_execution::Cancelled;
    use buck2_data::command_execution::Error;
    use buck2_data::command_execution::Failure;
    use buck2_data::command_execution::Status;
    use buck2_data::command_execution::Success;
    use buck2_data::command_execution::Timeout;
    use buck2_data::command_execution::WorkerFailure;

    let command = command_execution
        .details
        .as_ref()
        .internal_error("CommandExecution did not include a `command`")?;

    let status = command_execution
        .status
        .as_ref()
        .internal_error("CommandExecution did not include a `status`")?;

    let locality = if let Some(command_kind) = command.command_kind.as_ref() {
        use buck2_data::command_execution_kind::Command;
        match command_kind.command {
            Some(Command::RemoteCommand(..)) => "Remote ",
            Some(Command::LocalCommand(..)) | Some(Command::OmittedLocalCommand(..)) => "Local ",
            Some(Command::WorkerInitCommand(..)) => "Local Worker Initialization ",
            Some(Command::WorkerCommand(..)) => "Local Worker ",
            None => "",
        }
    } else {
        ""
    };

    Ok(match status {
        Status::Success(Success {}) => "Unexpected command status".to_owned(),
        Status::Failure(Failure { .. }) | Status::WorkerFailure(WorkerFailure {}) => {
            struct OptionalExitCode {
                code: Option<i32>,
            }

            impl fmt::Display for OptionalExitCode {
                fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                    match self.code {
                        Some(code) => {
                            if (i16::MIN as i32) < code && code < (i16::MAX as i32) {
                                write!(f, "{code}")
                            } else {
                                let code = code as u32;
                                write!(f, "{code} ({code:#X})")
                            }
                        }
                        None => write!(f, "<no exit code>"),
                    }
                }
            }

            format!(
                "{}command returned non-zero exit code {}",
                locality,
                OptionalExitCode {
                    code: command.signed_exit_code
                }
            )
        }
        Status::Timeout(Timeout { duration }) => {
            let duration = duration
                .as_ref()
                .internal_error("Timeout did not include a `duration`")?
                .try_into_duration()
                .buck_error_context("Timeout `duration` was invalid")?;

            format!("Command timed out after {:.3}s", duration.as_secs_f64(),)
        }
        Status::Error(Error { stage, error }) => {
            format!("Internal error (stage: {stage}): {error}")
        }
        Status::Cancelled(Cancelled {}) => "Command was cancelled".to_owned(),
    })
}

pub fn success_stderr(
    action: &buck2_data::ActionExecutionEnd,
    verbosity: Verbosity,
) -> buck2_error::Result<Option<&str>> {
    if !(verbosity.print_success_stderr() || action.always_print_stderr) {
        return Ok(None);
    }

    let stderr = match action.commands.last() {
        Some(command) => {
            &command
                .details
                .as_ref()
                .internal_error("CommandExecution did not include a `command`")?
                .cmd_stderr
        }
        None => return Ok(None),
    };

    if stderr.is_empty() {
        return Ok(None);
    }

    Ok(Some(stderr))
}

pub fn sanitize_output_colors(stderr: &[u8]) -> String {
    let mut sanitized = String::with_capacity(stderr.len());
    let mut parser = termwiz::escape::parser::Parser::new();
    parser.parse(stderr, |a| match a {
        Action::Print(c) => sanitized.push(c),
        Action::Control(cc) => match cc {
            ControlCode::CarriageReturn => sanitized.push('\r'),
            ControlCode::LineFeed => sanitized.push('\n'),
            ControlCode::HorizontalTab => sanitized.push('\t'),
            _ => {}
        },
        _ => {}
    });
    sanitized
}

/// Display information extracted from a CriticalPathEntry2.
pub struct CriticalPathEntryDisplay<'a> {
    /// The kind of critical path entry (e.g., "action", "analysis", "materialization").
    pub kind: &'a str,
    /// Whether the entry's DICE value was reused from a previous command rather than computed.
    pub reused: bool,
    /// The name/label of the entry (e.g., target label, package name).
    pub name: String,
    /// Optional category (e.g., for actions).
    pub category: Option<&'a str>,
    /// Optional identifier (e.g., action identifier, file path for materializations).
    pub identifier: Option<&'a str>,
    /// Optional execution kind for actions (e.g., "local", "remote").
    pub execution_kind: Option<&'static str>,
}

impl<'a> CriticalPathEntryDisplay<'a> {
    /// Extracts display information from a CriticalPathEntry2.
    pub fn from_entry(
        entry: &'a buck2_data::CriticalPathEntry2,
        opts: TargetDisplayOptions,
    ) -> buck2_error::Result<Option<Self>> {
        use buck2_data::critical_path_entry2::Entry;

        let entry_data = match &entry.entry {
            Some(entry) => entry,
            None => return Ok(None),
        };

        let (kind, name, category, identifier, execution_kind) = match entry_data {
            Entry::Analysis(analysis) => {
                use buck2_data::critical_path_entry2::analysis::Target;

                let name = match &analysis.target {
                    Some(Target::StandardTarget(t)) => display_configured_target_label(t, opts)?,
                    None => "unknown".to_owned(),
                };
                let kind = match analysis.part {
                    Some(1) => "analysis[part1]",
                    Some(2) => "analysis[part2]",
                    _ => "analysis",
                };
                (kind, name, None, None, None)
            }
            Entry::AnonAnalysis(anon_analysis) => {
                let name = match &anon_analysis.anon_target {
                    Some(t) => display_anon_target(t)?,
                    None => "unknown".to_owned(),
                };
                let kind = match anon_analysis.part {
                    Some(1) => "anon_analysis[part1]",
                    Some(2) => "anon_analysis[part2]",
                    _ => "anon_analysis",
                };
                (kind, name, None, None, None)
            }
            Entry::DynamicAnalysis(analysis) => {
                use buck2_data::critical_path_entry2::dynamic_analysis::Target;

                let name = match &analysis.target {
                    Some(Target::StandardTarget(t)) => display_configured_target_label(t, opts)?,
                    None => "anon-unknown".to_owned(),
                };
                ("dynamic_analysis", name, None, None, None)
            }
            Entry::ActionExecution(action_execution) => {
                use buck2_data::critical_path_entry2::action_execution::Owner;

                let category = action_execution.name.as_ref().map(|n| n.category.as_str());
                let identifier = action_execution
                    .name
                    .as_ref()
                    .map(|n| n.identifier.as_str());

                let execution_kind = Some(
                    buck2_data::ActionExecutionKind::try_from(action_execution.execution_kind)
                        .unwrap_or(buck2_data::ActionExecutionKind::NotSet)
                        .as_str_name(),
                );

                let name = match &action_execution.owner {
                    Some(Owner::TargetLabel(t)) => display_configured_target_label(t, opts)?,
                    Some(Owner::BxlKey(t)) => display_bxl_key(t)?,
                    Some(Owner::AnonTarget(t)) => display_anon_target(t)?,
                    None => "unknown".to_owned(),
                };
                ("action", name, category, identifier, execution_kind)
            }
            Entry::FinalMaterialization(materialization) => {
                use buck2_data::critical_path_entry2::final_materialization::Owner;

                let identifier = Some(materialization.path.as_str());

                let name = match &materialization.owner {
                    Some(Owner::TargetLabel(t)) => display_configured_target_label(t, opts)?,
                    Some(Owner::BxlKey(t)) => display_bxl_key(t)?,
                    Some(Owner::AnonTarget(t)) => display_anon_target(t)?,
                    None => "unknown".to_owned(),
                };
                ("materialization", name, None, identifier, None)
            }
            Entry::ComputeCriticalPath(..) => {
                ("compute-critical-path", String::new(), None, None, None)
            }
            Entry::Load(load) => ("load", load.package.clone(), None, None, None),
            Entry::Listing(listing) => ("listing", listing.package.clone(), None, None, None),
            Entry::GenericEntry(generic_entry) => {
                (generic_entry.kind.as_str(), String::new(), None, None, None)
            }
            Entry::PageIn(page_in) => {
                let count = page_in.count.max(1);
                let name = if count == 1 {
                    page_in.key_type.clone()
                } else {
                    let mut key_type_counts = page_in.key_type_counts.iter().collect::<Vec<_>>();
                    key_type_counts.sort_unstable_by_key(|(key_type, _)| *key_type);
                    let key_types = key_type_counts
                        .into_iter()
                        .map(|(key_type, count)| format!("{key_type}: {count}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{count} keys ({key_types})")
                };
                ("page_in", name, None, None, None)
            }
            Entry::Waiting(entry) => {
                let name = entry.category.clone().unwrap_or_default();
                ("waiting", name, None, None, None)
            }
            Entry::TestExecution(test_execution) => {
                let name = match &test_execution.target_label {
                    Some(t) => display_configured_target_label(t, opts)?,
                    None => "unknown".to_owned(),
                };
                ("test-execution", name, None, None, None)
            }
            Entry::TestListing(test_listing) => {
                let name = match &test_listing.target_label {
                    Some(t) => display_configured_target_label(t, opts)?,
                    None => "unknown".to_owned(),
                };
                ("test-listing", name, None, None, None)
            }
            Entry::Install(install) => {
                let name = match install.target_labels.first() {
                    Some(t) => display_configured_target_label(t, opts)?,
                    None => "unknown".to_owned(),
                };
                ("install", name, None, None, None)
            }
            Entry::EnsureTransitiveSetProjection(..) => (
                "ensure-transitive-set-projection",
                String::new(),
                None,
                None,
                None,
            ),
        };

        Ok(Some(CriticalPathEntryDisplay {
            kind,
            reused: entry.was_reused(),
            name,
            category,
            identifier,
            execution_kind,
        }))
    }

    /// Returns a formatted display name combining kind, reuse marker, and name.
    pub fn display_name(&self) -> String {
        let reused = self.reused_suffix();
        if self.name.is_empty() {
            format!("{}{reused}", self.kind)
        } else {
            format!("{}{reused}: {}", self.kind, self.name)
        }
    }

    /// Returns "(reused)" for entries whose DICE value was reused, for appending to the kind in
    /// rendered output.
    pub fn reused_suffix(&self) -> &'static str {
        if self.reused { "(reused)" } else { "" }
    }
}

#[cfg(test)]
mod tests {
    use std::time::UNIX_EPOCH;

    use buck2_data::SpanStartEvent;
    use buck2_events::BuckEvent;
    use buck2_events::span::SpanId;
    use buck2_wrapper_common::invocation_id::TraceId;

    use super::*;

    fn executor_stage_event(stage: buck2_data::executor_stage_start::Stage) -> BuckEvent {
        BuckEvent::new(
            UNIX_EPOCH,
            TraceId::new(),
            Some(SpanId::next()),
            None,
            SpanStartEvent {
                data: Some(buck2_data::ExecutorStageStart { stage: Some(stage) }.into()),
            }
            .into(),
        )
    }

    #[test]
    fn test_cas_artifact_stages_are_labelled_and_not_execution() {
        use buck2_data::cas_artifact_stage::Phase;

        for (phase, expected) in [
            (Phase::CheckExpiration as i32, "cas_check_expiration"),
            (Phase::ExtendExpiration as i32, "cas_extend_expiration"),
            (Phase::RecheckExpiration as i32, "cas_recheck_expiration"),
            (Phase::TreeMetadata as i32, "cas_tree_metadata"),
            (Phase::DeclareOutputs as i32, "cas_declare_outputs"),
            (Phase::Unspecified as i32, "cas_artifact"),
            (i32::MAX, "cas_artifact"),
        ] {
            let stage = buck2_data::CasArtifactStage { phase }.into();
            assert_eq!(display_executor_stage(&stage), Some(expected));
            assert!(!is_active_execution_stage(&executor_stage_event(stage)));
        }
    }

    #[test]
    fn test_local_finalize_outputs_is_not_execution() {
        let stage = buck2_data::LocalStage {
            stage: Some(buck2_data::LocalFinalizeOutputs {}.into()),
        }
        .into();
        assert_eq!(
            display_executor_stage(&stage),
            Some("local_finalize_outputs")
        );
        assert!(!is_active_execution_stage(&executor_stage_event(stage)));
    }

    fn action_execution_event(identifier: &str) -> BuckEvent {
        BuckEvent::new(
            UNIX_EPOCH,
            TraceId::new(),
            Some(SpanId::next()),
            None,
            SpanStartEvent {
                data: Some(
                    buck2_data::ActionExecutionStart {
                        key: Some(buck2_data::ActionKey {
                            id: Default::default(),
                            owner: Some(buck2_data::action_key::Owner::TargetLabel(
                                buck2_data::ConfiguredTargetLabel {
                                    label: Some(buck2_data::TargetLabel {
                                        package: "pkg".into(),
                                        name: "target".into(),
                                    }),
                                    configuration: Some(buck2_data::Configuration {
                                        full_name: "cfg".into(),
                                    }),
                                    execution_configuration: None,
                                },
                            )),
                            key: String::new(),
                        }),
                        name: Some(buck2_data::ActionName {
                            category: "category".into(),
                            identifier: identifier.into(),
                        }),
                        kind: buck2_data::ActionKind::NotSet as i32,
                    }
                    .into(),
                ),
            }
            .into(),
        )
    }

    fn over_quota_stage(exceeded_quotas: &[&str]) -> buck2_data::executor_stage_start::Stage {
        buck2_data::executor_stage_start::Stage::Re(buck2_data::ReStage {
            stage: Some(buck2_data::re_stage::Stage::QueueOverQuota(
                buck2_data::ReQueueOverQuota {
                    queue_info: None,
                    exceeded_quotas: exceeded_quotas.iter().map(|q| (*q).to_owned()).collect(),
                },
            )),
        })
    }

    fn test_result(name: &str, status: buck2_data::TestStatus) -> buck2_data::TestResult {
        buck2_data::TestResult {
            name: name.to_owned(),
            status: status as i32,
            ..Default::default()
        }
    }

    fn rendered_lines(result: &buck2_data::TestResult) -> buck2_error::Result<Vec<String>> {
        let lines = format_test_result(result, Verbosity::default())?
            .expect("a failed, skipped or rerun test should always be displayed");
        Ok(lines.iter().map(|line| line.to_unstyled()).collect())
    }

    #[test]
    fn test_result_name_with_line_breaks_and_tabs_is_one_line() -> buck2_error::Result<()> {
        let name = "suite > names function Loading() {\n\treturn x;\r\n} > case";
        let expected = "suite > names function Loading() {  return x;  } > case";

        for (status, prefix) in [
            (buck2_data::TestStatus::Fail, "\u{2717} Fail"),
            (buck2_data::TestStatus::Skip, "\u{21b7} Skip"),
            (buck2_data::TestStatus::Rerun, "\u{21bb} Rerun"),
        ] {
            assert_eq!(
                rendered_lines(&test_result(name, status))?,
                vec![format!("{prefix}: {expected}")],
                "{prefix}: every tab and line break in the name should become a space, on a single line",
            );
        }

        Ok(())
    }

    #[test]
    fn test_result_name_with_unicode_line_separator_is_one_line() -> buck2_error::Result<()> {
        let result = test_result("a\u{2028}b", buck2_data::TestStatus::Fail);

        assert_eq!(
            rendered_lines(&result)?,
            vec!["\u{2717} Fail: a b".to_owned()]
        );

        Ok(())
    }

    #[test]
    fn test_result_name_without_special_whitespace_is_unchanged() -> buck2_error::Result<()> {
        let result = test_result("suite > keeps  two spaces", buck2_data::TestStatus::Fail);

        assert_eq!(
            rendered_lines(&result)?,
            vec!["\u{2717} Fail: suite > keeps  two spaces".to_owned()],
        );

        Ok(())
    }

    #[test]
    fn over_quota_stage_names_exceeded_quotas() {
        assert_eq!(
            display_executor_stage_detail(&over_quota_stage(&["worker_units:pool:linux"])),
            Some("re_queued(over_quota: worker_units:pool:linux)".to_owned()),
        );
        assert_eq!(
            display_executor_stage_detail(&over_quota_stage(&["a", "b"])),
            Some("re_queued(over_quota: a, b)".to_owned()),
        );
        assert_eq!(
            display_executor_stage_detail(&over_quota_stage(&[])),
            Some("re_queued(over_quota)".to_owned()),
            "Without exceeded quotas the label should fall back to the plain stage",
        );
    }

    #[test]
    fn action_event_display_hides_identifier_matching_target_leaf() -> buck2_error::Result<()> {
        let event = action_execution_event("target");
        let display = display_event(&event, TargetDisplayOptions::for_console(true))?;

        assert_eq!(display.label.as_deref(), Some("pkg:target (cfg)"));
        assert_eq!(display.detail, "category");
        assert_eq!(display.category.as_deref(), Some("category"));

        Ok(())
    }

    #[test]
    fn action_event_display_keeps_identifier_different_from_target_leaf() -> buck2_error::Result<()>
    {
        let event = action_execution_event("other");
        let display = display_event(&event, TargetDisplayOptions::for_console(true))?;

        assert_eq!(display.label.as_deref(), Some("pkg:target (cfg)"));
        assert_eq!(display.detail, "category other");
        assert_eq!(display.category.as_deref(), Some("category"));

        Ok(())
    }

    #[test]
    fn critical_path_entry_display_marks_reused_entries() -> buck2_error::Result<()> {
        let entry = |was_reused| buck2_data::CriticalPathEntry2 {
            entry: Some(
                buck2_data::critical_path_entry2::Analysis {
                    target: None,
                    target_rule_type_name: None,
                    part: None,
                }
                .into(),
            ),
            was_reused,
            ..Default::default()
        };

        let reused_entry = entry(Some(true));
        let reused =
            CriticalPathEntryDisplay::from_entry(&reused_entry, TargetDisplayOptions::for_log())?
                .expect("entry is set, so display info should be extracted");
        assert_eq!(reused.kind, "analysis");
        assert!(
            reused.reused,
            "was_reused should carry through to the display"
        );
        assert_eq!(reused.display_name(), "analysis(reused): unknown");

        let computed_entry = entry(None);
        let computed =
            CriticalPathEntryDisplay::from_entry(&computed_entry, TargetDisplayOptions::for_log())?
                .expect("entry is set, so display info should be extracted");
        assert_eq!(computed.kind, "analysis");
        assert!(!computed.reused, "unset was_reused should not mark reuse");
        assert_eq!(computed.display_name(), "analysis: unknown");

        Ok(())
    }

    #[test]
    fn removes_color_characters() {
        let message = "\x1b[0mFoo\t\x1b[34mBar\n\x1b[DBaz\r\nQuz";

        let sanitized = sanitize_output_colors(message.as_bytes());

        assert_eq!("Foo\tBar\nBaz\r\nQuz", sanitized);
    }

    #[test]
    fn strips_trailing_newline_character() {
        let stream_contents = "test\n";
        let res = strip_trailing_newline(stream_contents);
        assert_eq!(res, "test");
    }

    #[test]
    fn preserves_duplicate_newlines() {
        let stream_contents = "test\n\n";
        let res = strip_trailing_newline(stream_contents);
        assert_eq!(res, "test\n");
    }

    #[test]
    fn preserves_other_trailing_whitespace() {
        let stream_contents = "test    \t";
        let res = strip_trailing_newline(stream_contents);
        assert_eq!(res, stream_contents);
    }

    #[test]
    fn preserves_leading_whitespace() {
        let stream_contents = "\n  test";
        let res = strip_trailing_newline(stream_contents);
        assert_eq!(res, stream_contents);
    }

    #[test]
    fn correctly_handles_carriage_return() {
        let stream_contents = "test\r\n";
        let res = strip_trailing_newline(stream_contents);
        assert_eq!(res, "test");
    }
}
