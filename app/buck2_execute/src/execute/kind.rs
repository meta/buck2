/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::time::Duration;

use allocative::Allocative;
use buck2_core::execution_types::executor_config::RemoteExecutorUseCase;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_data::RePlatform;
use buck2_hash::BuckFixedSortedMap;
use derive_more::Display;
use dupe::Dupe;
use remote_execution as RE;

use crate::execute::action_digest::ActionDigest;
use crate::execute::dep_file_digest::DepFileDigest;
use crate::re::convert::platform_to_proto;

/// How the remote execution service produced the result of a command submitted for execution.
#[derive(Debug, Clone, Copy, Dupe, PartialEq, Eq, Allocative)]
pub enum RemoteExecutionOrigin {
    Executed,
    /// Served from the service's own action cache instead of running.
    ServedFromCache,
    /// Deduplicated against an identical execution already in flight instead of running.
    Deduplicated,
}

impl From<RemoteExecutionOrigin> for buck2_data::RemoteCommandOrigin {
    fn from(origin: RemoteExecutionOrigin) -> Self {
        match origin {
            RemoteExecutionOrigin::Executed => Self::Executed,
            RemoteExecutionOrigin::ServedFromCache => Self::ReServedFromCache,
            RemoteExecutionOrigin::Deduplicated => Self::ReDeduplicated,
        }
    }
}

#[derive(Debug, Display, Clone, Allocative)]
pub enum CommandExecutionKind {
    /// This action was executed locally.
    #[display("local")]
    Local {
        // Even though this did not run on RE, we still produced this, so we might as well report
        // it.
        digest: ActionDigest,
        command: Vec<String>,
        env: BuckFixedSortedMap<String, String>,
    },
    /// This action was submitted to a remote executor.
    #[display("remote")]
    Remote {
        details: RemoteCommandExecutionDetails,

        /// How long this command queued in RE. This value excludes execution time, i.e. for action cache hit,
        /// this value represents how long a request has to wait for server to handle.
        queue_time: Duration,
        /// Local paths to the materialized inputs for failed actions, if `--materialize-failed-inputs`
        /// was passed to build options
        materialized_inputs_for_failed: Option<Vec<ProjectRelativePathBuf>>,
        /// Local paths to the materialized outputs for failed actions, if `--unstable-materialize-failed-action-outputs`
        /// was passed to build options
        materialized_outputs_for_failed_actions: Option<Vec<ProjectRelativePathBuf>>,
        origin: RemoteExecutionOrigin,
    },
    /// This action was served by the action cache and not executed.
    #[display("action_cache")]
    ActionCache {
        details: RemoteCommandExecutionDetails,
    },
    /// This action was served by the action cache (remote dep file) and not executed.
    #[display("remote_dep_file_cache")]
    RemoteDepFileCache {
        details: RemoteCommandExecutionDetails,
    },
    /// This action would have executed via a local worker but failed during worker initialization.
    #[display("worker_init")]
    LocalWorkerInit {
        command: Vec<String>,
        env: BuckFixedSortedMap<String, String>,
    },
    /// This action was executed via a local worker.
    #[display("worker")]
    LocalWorker {
        digest: ActionDigest,
        command: Vec<String>,
        env: BuckFixedSortedMap<String, String>,
        fallback_exe: Vec<String>,
    },
    /// This action took the result of an identical action already running in this daemon, and
    /// was neither looked up in a cache nor executed.
    #[display("in_flight")]
    InFlight {
        digest: ActionDigest,
        /// The action whose execution this one shared.
        leader: buck2_data::ActionKey,
        /// How the leader's result was produced.
        leader_kind: buck2_data::ActionExecutionKind,
    },
}

impl CommandExecutionKind {
    pub fn as_enum(&self) -> buck2_data::ActionExecutionKind {
        match self {
            Self::Local { .. } => buck2_data::ActionExecutionKind::Local,
            Self::LocalWorker { .. } | Self::LocalWorkerInit { .. } => {
                buck2_data::ActionExecutionKind::LocalWorker
            }
            Self::Remote {
                details,
                origin: RemoteExecutionOrigin::Executed,
                ..
            } => match details.persistent_worker {
                false => buck2_data::ActionExecutionKind::Remote,
                true => buck2_data::ActionExecutionKind::RemoteWorker,
            },
            Self::Remote {
                origin: RemoteExecutionOrigin::ServedFromCache,
                ..
            } => buck2_data::ActionExecutionKind::RemoteServedFromCache,
            Self::Remote {
                origin: RemoteExecutionOrigin::Deduplicated,
                ..
            } => buck2_data::ActionExecutionKind::RemoteDeduplicated,
            Self::ActionCache { .. } => buck2_data::ActionExecutionKind::ActionCache,
            Self::RemoteDepFileCache { .. } => buck2_data::ActionExecutionKind::RemoteDepFileCache,
            Self::InFlight { .. } => buck2_data::ActionExecutionKind::InFlight,
        }
    }

    /// Returns the RE platform identifier if executed remotely.
    ///
    /// Format: `"platform"`, or `"platform.subplatform"` when a non-empty
    /// `subplatform` property is also set. Returns `None` if the action did
    /// not run remotely or no `platform` property is set.
    pub fn re_platform_name(&self) -> Option<String> {
        let details = match self {
            Self::Remote { details, .. }
            | Self::ActionCache { details }
            | Self::RemoteDepFileCache { details } => details,
            _ => return None,
        };
        re_platform_name_from_properties(&details.platform.properties)
    }

    pub fn to_proto(&self, omit_details: bool) -> buck2_data::CommandExecutionKind {
        use buck2_data::command_execution_kind::Command;

        let command = Some(match self {
            Self::Local {
                command,
                env,
                digest,
            } => {
                if omit_details {
                    Command::OmittedLocalCommand(buck2_data::OmittedLocalCommand {
                        action_digest: digest.to_string(),
                    })
                } else {
                    Command::LocalCommand(buck2_data::LocalCommand {
                        action_digest: digest.to_string(),
                        argv: command.to_owned(),
                        env: env
                            .iter()
                            .map(|(key, value)| buck2_data::EnvironmentEntry {
                                key: key.clone(),
                                value: value.clone(),
                            })
                            .collect(),
                        ..Default::default()
                    })
                }
            }
            Self::Remote {
                details,
                queue_time,
                materialized_inputs_for_failed,
                materialized_outputs_for_failed_actions,
                origin,
            } => Command::RemoteCommand(buck2_data::RemoteCommand {
                action_digest: details.action_digest.to_string(),
                cache_hit: false,
                cache_hit_type: buck2_data::RemoteCommandOrigin::from(*origin).into(),
                remote_dep_file_key: None,
                queue_time: (*queue_time).try_into().ok(),
                details: details.to_proto(omit_details),
                materialized_inputs_for_failed: materialized_inputs_for_failed
                    .as_ref()
                    .map(|paths| {
                        paths
                            .clone()
                            .iter()
                            .map(|p| format!("{p}"))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
                materialized_outputs_for_failed_actions: materialized_outputs_for_failed_actions
                    .as_ref()
                    .map(|paths| {
                        paths
                            .clone()
                            .iter()
                            .map(|p| format!("{p}"))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
                re_worker: None,
            }),
            Self::ActionCache { details } => Command::RemoteCommand(buck2_data::RemoteCommand {
                action_digest: details.action_digest.to_string(),
                cache_hit: true,
                cache_hit_type: buck2_data::RemoteCommandOrigin::ActionCache.into(),
                queue_time: None,
                details: details.to_proto(omit_details),
                remote_dep_file_key: None,
                materialized_inputs_for_failed: Vec::new(),
                materialized_outputs_for_failed_actions: Vec::new(),
                re_worker: None,
            }),

            Self::RemoteDepFileCache { details } => {
                Command::RemoteCommand(buck2_data::RemoteCommand {
                    action_digest: details.action_digest.to_string(),
                    cache_hit: true,
                    cache_hit_type: buck2_data::RemoteCommandOrigin::RemoteDepFileCache.into(),
                    queue_time: None,
                    details: details.to_proto(omit_details),
                    remote_dep_file_key: details
                        .remote_dep_file_key
                        .as_ref()
                        .map(|k| k.to_string()),
                    materialized_inputs_for_failed: Vec::new(),
                    materialized_outputs_for_failed_actions: Vec::new(),
                    re_worker: None,
                })
            }

            Self::LocalWorkerInit { command, env } => {
                Command::WorkerInitCommand(buck2_data::WorkerInitCommand {
                    argv: command.to_owned(),
                    env: env
                        .iter()
                        .map(|(key, value)| buck2_data::EnvironmentEntry {
                            key: key.clone(),
                            value: value.clone(),
                        })
                        .collect(),
                })
            }

            Self::LocalWorker {
                command,
                env,
                digest,
                fallback_exe,
            } => Command::WorkerCommand(buck2_data::WorkerCommand {
                action_digest: digest.to_string(),
                argv: command.to_owned(),
                env: env
                    .iter()
                    .map(|(key, value)| buck2_data::EnvironmentEntry {
                        key: key.clone(),
                        value: value.clone(),
                    })
                    .collect(),
                fallback_exe: fallback_exe.to_owned(),
            }),
            Self::InFlight {
                digest,
                leader,
                leader_kind,
            } => Command::InFlightCommand(buck2_data::InFlightCommand {
                action_digest: digest.to_string(),
                leader_action_key: Some(leader.clone()),
                leader_execution_kind: (*leader_kind).into(),
            }),
        });

        buck2_data::CommandExecutionKind { command }
    }
}

/// Structured data for a RE request.
#[derive(Debug, Clone, Allocative)]
pub struct RemoteCommandExecutionDetails {
    pub action_digest: ActionDigest,
    pub remote_dep_file_key: Option<DepFileDigest>,
    pub session_id: Option<String>,
    pub use_case: RemoteExecutorUseCase,
    pub platform: RePlatform,
    pub persistent_worker: bool,
}

impl RemoteCommandExecutionDetails {
    pub fn new(
        action_digest: ActionDigest,
        remote_dep_file_key: Option<DepFileDigest>,
        session_id: Option<String>,
        use_case: RemoteExecutorUseCase,
        platform: &RE::Platform,
        persistent_worker: bool,
    ) -> Self {
        Self {
            action_digest,
            remote_dep_file_key,
            session_id,
            use_case,
            platform: platform_to_proto(platform),
            persistent_worker,
        }
    }

    fn to_proto(&self, omit_details: bool) -> Option<buck2_data::RemoteCommandDetails> {
        Some(buck2_data::RemoteCommandDetails {
            session_id: if omit_details {
                None
            } else {
                self.session_id.clone()
            },
            use_case: self.use_case.to_string(),
            platform: Some(self.platform.clone()),
            persistent_worker: self.persistent_worker,
        })
    }
}

fn re_platform_name_from_properties(
    properties: &[buck2_data::re_platform::Property],
) -> Option<String> {
    let mut platform: Option<&str> = None;
    let mut subplatform: Option<&str> = None;
    for p in properties {
        match p.name.as_str() {
            "platform" => platform = Some(p.value.as_str()),
            "subplatform" => subplatform = Some(p.value.as_str()),
            _ => {}
        }
    }
    platform.map(|p| match subplatform {
        Some(s) if !s.is_empty() => format!("{p}.{s}"),
        _ => p.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use buck2_data::re_platform::Property;

    use super::*;

    fn prop(name: &str, value: &str) -> Property {
        Property {
            name: name.to_owned(),
            value: value.to_owned(),
        }
    }

    #[test]
    fn platform_only() {
        let props = vec![prop("platform", "linux-remote-execution")];
        assert_eq!(
            re_platform_name_from_properties(&props),
            Some("linux-remote-execution".to_owned()),
        );
    }

    #[test]
    fn platform_and_subplatform_joined_with_dot() {
        let props = vec![
            prop("platform", "gpu-remote-execution"),
            prop("subplatform", "H100"),
        ];
        assert_eq!(
            re_platform_name_from_properties(&props),
            Some("gpu-remote-execution.H100".to_owned()),
        );
    }

    #[test]
    fn empty_subplatform_omits_suffix() {
        let props = vec![
            prop("platform", "linux-remote-execution"),
            prop("subplatform", ""),
        ];
        assert_eq!(
            re_platform_name_from_properties(&props),
            Some("linux-remote-execution".to_owned()),
        );
    }

    #[test]
    fn missing_platform_returns_none() {
        let props = vec![prop("subplatform", "H100")];
        assert_eq!(re_platform_name_from_properties(&props), None);
    }

    #[test]
    fn ignores_unrelated_properties() {
        let props = vec![
            prop("container-image", "docker://foo"),
            prop("platform", "mac"),
            prop("OSFamily", "Darwin"),
        ];
        assert_eq!(
            re_platform_name_from_properties(&props),
            Some("mac".to_owned()),
        );
    }
}
