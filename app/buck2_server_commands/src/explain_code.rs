/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use core::iter::Iterator;

use buck2_build_api::query::oneshot::QUERY_FRONTEND;
use buck2_cli_proto::new_generic::ExplainRequest;
use buck2_data::CommandInvalidationInfo;
use buck2_data::FileWatcherEvent;
use buck2_data::action_key;
use buck2_event_log::read::EventLogPathBuf;
use buck2_event_log::stream_value::StreamValue;
use buck2_event_observer::display::TargetDisplayOptions;
use buck2_event_observer::display::display_anon_target;
use buck2_event_observer::display::display_bxl_key;
use buck2_event_observer::display::display_configured_target_label;
use buck2_event_observer::what_ran::CommandReproducer;
use buck2_event_observer::what_ran::WhatRanOptions;
use buck2_event_observer::what_ran::WhatRanRelevantAction;
use buck2_events::span::SpanId;
use buck2_explain::ActionEntryData;
use buck2_explain::ChangedFilesEntryData;
use buck2_hash::BuckMutMap;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_query::query::syntax::simple::eval::label_indexed::LabelIndexedSet;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::global_cfg_options::global_cfg_options_from_client_context;
use dice::DiceTransaction;
use dupe::Dupe;
use dupe::IterDupedExt;
use futures::TryStreamExt;

#[allow(clippy::vec_box)]
#[cfg(fbcode_build)]
struct ActionEntry {
    action: WhatRanRelevantAction,
    reproducers: Vec<CommandReproducer>,
}

#[cfg(fbcode_build)]
impl ActionEntry {
    fn format_action(
        &self,
        event: &buck2_data::SpanEndEvent,
    ) -> buck2_error::Result<Option<(String, ActionEntryData)>> {
        let action = &self.action;

        let action_execution = match &event.data {
                Some(buck2_data::span_end_event::Data::ActionExecution(action_exec)) => Some(action_exec),
                _ => None,
        }.expect("Should always be an ActionExecution end event because span ID must match ActionExecution start event.");
        let failed = action_execution.failed;
        let execution_kind =
            buck2_data::ActionExecutionKind::try_from(action_execution.execution_kind)
                .ok()
                .map(|v| v.as_str_name().to_owned());
        let input_files_bytes = action_execution.input_files_bytes;
        let affected_by_file_changes = matches!(
            &action_execution.invalidation_info,
            Some(CommandInvalidationInfo {
                changed_file: Some(_),
                ..
            })
        );

        let (target, mut entry) = match action {
            WhatRanRelevantAction::ActionExecution(act) => {
                let category = act.name.as_ref().map(|n| n.category.clone());
                let identifier = act.name.as_ref().map(|n| n.identifier.clone());
                let owner = match act.key.as_ref() {
                    Some(key) => key.owner.as_ref(),
                    None => return Ok(None),
                };

                let opts = TargetDisplayOptions::for_log();
                let target = match owner {
                    Some(o) => match o {
                        action_key::Owner::TargetLabel(target_label)
                        | action_key::Owner::TestTargetLabel(target_label)
                        | action_key::Owner::LocalResourceSetup(target_label) => {
                            display_configured_target_label(target_label, opts)
                        }
                        action_key::Owner::BxlKey(bxl_key) => display_bxl_key(bxl_key),
                        action_key::Owner::AnonTarget(anon_target) => {
                            display_anon_target(anon_target)
                        }
                    }?,
                    None => return Ok(None),
                };

                (
                    target,
                    ActionEntryData {
                        category,
                        failed,
                        repros: vec![],
                        execution_kind,
                        identifier,
                        input_files_bytes,
                        affected_by_file_changes,
                    },
                )
            }
            _ => return Ok(None),
        };

        for reproducer in self.reproducers.iter() {
            entry.repros.push(reproducer.to_string());
        }

        Ok(Some((target, entry)))
    }
}

pub(crate) async fn explain(
    server_ctx: &dyn ServerCommandContextTrait,
    ctx: DiceTransaction,
    req: &ExplainRequest,
) -> buck2_error::Result<()> {
    let build_log = EventLogPathBuf::infer(req.log_path.clone())?;
    let (_, mut events) = build_log.unpack_stream().await?;

    let options = WhatRanOptions {
        skip_cache_hits: true,
        emit_cache_queries: false,
        ..Default::default()
    };
    let mut known_actions: BuckMutMap<SpanId, ActionEntry> = Default::default();

    let mut executed_actions = vec![];
    let mut changed_files = vec![];

    while let Some(event) = events.try_next().await? {
        if let StreamValue::Event(event) = event {
            // TODO iguridi: deduplicate this from whatran code
            if let Some(data) = event.data {
                if let Some(action) = WhatRanRelevantAction::from_buck_data(&data) {
                    known_actions.insert(
                        SpanId::from_u64(event.span_id)?,
                        ActionEntry {
                            action,
                            reproducers: Default::default(),
                        },
                    );
                }
                if let Some(repro) = CommandReproducer::from_buck_data(&data, &options)
                    && let Some(parent_id) = SpanId::from_u64_opt(event.parent_id)
                    && let Some(entry) = known_actions.get_mut(&parent_id)
                {
                    entry.reproducers.push(repro);
                }

                if let buck2_data::buck_event::Data::SpanEnd(span) = data {
                    if let Some(entry) = known_actions.remove(&SpanId::from_u64(event.span_id)?)
                        && let Some(entry) = entry.format_action(&span)?
                    {
                        executed_actions.push(entry);
                    }
                    if let Some(buck2_data::span_end_event::Data::FileWatcher(end)) = &span.data {
                        let events: &[FileWatcherEvent] =
                            end.stats.as_ref().expect("of source eh").events.as_ref();
                        for event in events {
                            let path = event.path.clone();
                            changed_files.push(path);
                        }
                    }
                }
            }
        }
    }

    let target_universe: Option<&[String]> = if req.target_universe.is_empty() {
        None
    } else {
        Some(&req.target_universe)
    };

    let global_cfg_options =
        global_cfg_options_from_client_context(&req.target_cfg, server_ctx, &mut ctx.ctx()).await?;

    let targets = {
        let (query_result, _universes) = QUERY_FRONTEND
            .get()?
            .eval_cquery(
                &mut ctx.ctx(),
                server_ctx.working_dir(),
                &req.target,
                &[],
                global_cfg_options.dupe(),
                target_universe,
                false, // collect universes
            )
            .await?;

        query_result
            .targets()
            .map(|v| v.map(|v| v.dupe()))
            .collect::<Result<Vec<ConfiguredTargetNode>, _>>()?
    };

    let file_update_entries = {
        let mut file_update_entries = vec![];
        // TODO iguridi: one by one and serially is not very smart
        for file_change in changed_files {
            let (targets_with_file_updates, _universes) = QUERY_FRONTEND
                .get()?
                .eval_cquery(
                    &mut ctx.ctx(),
                    server_ctx.working_dir(),
                    &format!("owner(\"{file_change}\")"),
                    &[],
                    global_cfg_options.dupe(),
                    Some(std::slice::from_ref(&req.target)), // target universe
                    false,
                )
                .await?;

            let targets = targets_with_file_updates
                .targets()
                .map(|v| v.map(|v| v.dupe()))
                .collect::<Result<Vec<ConfiguredTargetNode>, _>>()?;

            file_update_entries.push(ChangedFilesEntryData {
                path: file_change,
                targets: targets.into_iter().map(|t| t.label().to_string()).collect(),
            });
        }
        file_update_entries
    };

    let all_deps = {
        let mut stack = targets;
        let mut visited = LabelIndexedSet::new();
        while let Some(node) = stack.pop() {
            if visited.insert(node.dupe()) {
                stack.extend(node.deps().iter().duped());
            }
        }
        visited.into_iter().collect::<Vec<ConfiguredTargetNode>>()
    };

    buck2_explain::main(
        all_deps,
        executed_actions,
        file_update_entries,
        req.output.as_ref(),
        req.fbs_dump.as_ref(),
        req.manifold_path.as_deref(),
    )
    .await?;

    Ok(())
}
