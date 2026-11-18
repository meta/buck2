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
use std::time::SystemTime;

use async_trait::async_trait;
use buck2_cli_proto::command_result;
use buck2_common::invocation_paths::InvocationPaths;
use buck2_events::sink::remote::RemoteEventConfig;
use buck2_events::sink::remote::RemoteEventSink;
#[cfg(fbcode_build)]
use buck2_events::sink::remote::new_remote_event_sink_if_enabled;
use buck2_wrapper_common::invocation_id::TraceId;
use dupe::Dupe;
use fbinit::FacebookInit;

use crate::remote_sink_config;
use crate::subscribers::subscriber::EventSubscriber;

pub struct BuildGraphStats {
    trace_id: TraceId,
    sink: Option<RemoteEventSink>,
}

impl BuildGraphStats {
    pub fn new_with_paths(
        fb: FacebookInit,
        trace_id: TraceId,
        paths: Option<&InvocationPaths>,
    ) -> Self {
        let config = remote_sink_config::with_buckconfig_overrides(
            paths,
            RemoteEventConfig {
                buffer_size: 1,
                retry_backoff: Duration::from_millis(100),
                retry_attempts: 2,
                #[cfg(not(fbcode_build))]
                retry_window: Duration::from_secs(
                    buck2_events::sink::remote::DEFAULT_RETRY_WINDOW_SECS,
                ),
                #[cfg(not(fbcode_build))]
                replay_spill_dir: None,
                #[cfg(not(fbcode_build))]
                replay_spill_max_bytes: buck2_events::sink::remote::DEFAULT_REPLAY_SPILL_MAX_BYTES,
                message_batch_size: None,
                #[cfg(fbcode_build)]
                thrift_timeout: Duration::from_secs(1),
                #[cfg(not(fbcode_build))]
                grpc_timeout: Duration::from_secs(10),
                #[cfg(not(fbcode_build))]
                bes_backend: None,
                #[cfg(not(fbcode_build))]
                bes_headers: Vec::new(),
                bes_tls: Default::default(),
                #[cfg(not(fbcode_build))]
                build_metadata: Vec::new(),
                #[cfg(not(fbcode_build))]
                event_format: Default::default(),
                #[cfg(not(fbcode_build))]
                bazel_artifact_upload: true,
                #[cfg(not(fbcode_build))]
                upload_successful_action_events: true,
                #[cfg(not(fbcode_build))]
                bazel_artifact_upload_backend: None,
                #[cfg(not(fbcode_build))]
                re_client_cas_address: None,
                #[cfg(not(fbcode_build))]
                bazel_artifact_upload_instance_name: None,
                #[cfg(not(fbcode_build))]
                re_client_instance_name: None,
                #[cfg(not(fbcode_build))]
                bazel_artifact_uri_authority: None,
                #[cfg(not(fbcode_build))]
                bazel_artifact_upload_max_bytes: 10 * 1024 * 1024,
                #[cfg(not(fbcode_build))]
                channel: Default::default(),
            },
        );

        let sink = {
            #[cfg(fbcode_build)]
            {
                match new_remote_event_sink_if_enabled(fb, config) {
                    Ok(sink) => sink,
                    Err(e) => {
                        tracing::warn!("Failed to create remote sink for BuildGraphStats: {:#}", e);
                        None
                    }
                }
            }
            #[cfg(not(fbcode_build))]
            {
                let _ = fb;
                drop(config);
                // OSS BES uploads are owned by the daemon-side sink.
                // Opening a second client-side stream with the same invocation
                // ID causes BuildBuddy to cancel the original channel and mark
                // the invocation disconnected.
                None
            }
        };

        Self { trace_id, sink }
    }

    async fn handle_build_response(
        &self,
        res: &buck2_cli_proto::BuildResponse,
    ) -> buck2_error::Result<()> {
        let events = self.build_graph_stats_from_build_response(res);
        self.send_events(events).await;

        Ok(())
    }

    fn build_graph_stats_from_build_response(
        &self,
        res: &buck2_cli_proto::BuildResponse,
    ) -> Vec<buck2_events::BuckEvent> {
        const MAX_BUILD_TARGETS_LEN: usize = 3000;

        res.build_targets
            .chunks(MAX_BUILD_TARGETS_LEN)
            .map(|ts| {
                buck2_events::BuckEvent::new(
                    SystemTime::now(),
                    self.trace_id.dupe(),
                    None,
                    None,
                    buck2_data::RecordEvent {
                        data: Some(
                            buck2_data::BuildGraphStats {
                                build_targets: ts
                                    .iter()
                                    .map(|t| buck2_data::BuildTarget {
                                        target: t.target.clone(),
                                        configuration: t.configuration.clone(),
                                        configured_graph_size: t.configured_graph_size,
                                    })
                                    .collect(),
                            }
                            .into(),
                        ),
                    }
                    .into(),
                )
            })
            .collect()
    }

    async fn send_events(&self, events: Vec<buck2_events::BuckEvent>) {
        if let Some(sink) = self.sink.as_ref() {
            tracing::info!("Sending events to Scribe: {:?}", &events);
            let _res = sink.send_messages_now(events).await;
        } else {
            tracing::info!("Events were not sent to Scribe: {:?}", &events);
        }
    }
}

#[async_trait]
impl EventSubscriber for BuildGraphStats {
    async fn handle_command_result(
        &mut self,
        result: &buck2_cli_proto::CommandResult,
    ) -> buck2_error::Result<()> {
        match &result.result {
            Some(command_result::Result::BuildResponse(res)) => {
                self.handle_build_response(res).await
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fbinit::test]
    fn build_graph_stats_normal(fb: FacebookInit) {
        let res = buck2_cli_proto::BuildResponse {
            build_targets: vec![
                buck2_cli_proto::BuildTarget {
                    target: "//some/target:A".to_owned(),
                    configuration: "//some/conf:A".to_owned(),
                    configured_graph_size: Some(123),
                    ..Default::default()
                },
                buck2_cli_proto::BuildTarget {
                    target: "//some/target:B".to_owned(),
                    configuration: "//some/conf:B".to_owned(),
                    configured_graph_size: None,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let uuid = TraceId::new();
        let handler = BuildGraphStats::new_with_paths(fb, uuid.dupe(), None);
        let events = handler.build_graph_stats_from_build_response(&res);

        let event_expected = buck2_data::BuckEvent {
            data: Some(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::BuildGraphStats(
                        buck2_data::BuildGraphStats {
                            build_targets: vec![
                                buck2_data::BuildTarget {
                                    target: "//some/target:A".to_owned(),
                                    configuration: "//some/conf:A".to_owned(),
                                    configured_graph_size: Some(123),
                                },
                                buck2_data::BuildTarget {
                                    target: "//some/target:B".to_owned(),
                                    configuration: "//some/conf:B".to_owned(),
                                    configured_graph_size: None,
                                },
                            ],
                        },
                    )),
                },
            )),
            ..Default::default()
        };

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].trace_id().unwrap(), uuid);
        assert_eq!(events[0].data(), event_expected.data.as_ref().unwrap());
    }

    #[fbinit::test]
    fn build_graph_stats_empty_target(fb: FacebookInit) {
        let res = buck2_cli_proto::BuildResponse {
            build_targets: vec![],
            ..Default::default()
        };

        let uuid = TraceId::new();
        let handler = BuildGraphStats::new_with_paths(fb, uuid.dupe(), None);
        let events = handler.build_graph_stats_from_build_response(&res);

        assert_eq!(events.len(), 0);
    }

    #[fbinit::test]
    fn build_graph_stats_too_long_targets(fb: FacebookInit) {
        let build_target = buck2_cli_proto::BuildTarget {
            target: "T".to_owned(),
            configuration: "C".to_owned(),
            configured_graph_size: Some(1),
            ..Default::default()
        };

        // Testing if [6002] becomes [[3000], [3000], [2]]
        let mut input_build_targets = vec![];
        for _ in 0..6002 {
            input_build_targets.push(build_target.clone());
        }

        let res = buck2_cli_proto::BuildResponse {
            build_targets: input_build_targets,
            ..Default::default()
        };

        let uuid = TraceId::new();
        let handler = BuildGraphStats::new_with_paths(fb, uuid.dupe(), None);
        let events = handler.build_graph_stats_from_build_response(&res);

        let build_target = buck2_data::BuildTarget {
            target: "T".to_owned(),
            configuration: "C".to_owned(),
            configured_graph_size: Some(1),
        };

        let mut output_build_targets_3000 = vec![];
        for _ in 0..3000 {
            output_build_targets_3000.push(build_target.clone());
        }
        let output_build_targets_2 = vec![build_target.clone(), build_target.clone()];

        let event_expected_3000 = buck2_data::BuckEvent {
            data: Some(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::BuildGraphStats(
                        buck2_data::BuildGraphStats {
                            build_targets: output_build_targets_3000,
                        },
                    )),
                },
            )),
            ..Default::default()
        };
        let event_expected_2 = buck2_data::BuckEvent {
            data: Some(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::BuildGraphStats(
                        buck2_data::BuildGraphStats {
                            build_targets: output_build_targets_2,
                        },
                    )),
                },
            )),
            ..Default::default()
        };

        assert_eq!(events.len(), 3);
        assert_eq!(events[0].data(), event_expected_3000.data.as_ref().unwrap());
        assert_eq!(events[1].data(), event_expected_3000.data.as_ref().unwrap());
        assert_eq!(events[2].data(), event_expected_2.data.as_ref().unwrap());
    }
}
