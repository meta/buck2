/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::Write;

use buck2_client_ctx::client_ctx::BuckSubcommand;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::event_log_options::EventLogOptions;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_error::conversion::from_any_with_tag;
use buck2_event_log::stream_value::StreamValue;
use buck2_hash::BuckMutMap;
use buck2_hash::BuckMutSet;
use serde::Serialize;
use tokio_stream::StreamExt;

/// Print the action graph recorded in a build's event log, as JSON Lines.
///
/// Record the graph by building with `--log-action-graph`; a log recorded without it is an error.
/// One object per line, each with a `kind`:
///
/// - `header`, first: the recorded command.
/// - `action`, one per executed action, in event order: its `span` (the span id of its
///   `ActionExecution` event, which is the action's id here), `key`, `category`, `identifier`,
///   `deps` (the spans of producers that ran in this command), `reused` (the per-command ids of
///   producers this command did not run) and `tsets` (the per-command ids
///   of transitive-set projections among its inputs).
/// - `dangling`, after the actions: a `span` that some action's `deps` refer to but that no
///   action in the log has, with one action that refers to it.
/// - `stats`, last: counts of `actions`, `edges` (entries in `deps`), `reused` references and
///   `dangling` spans; the `schema` from the start event; and the `footer` the build wrote, or
///   null if the log has none (it was cut off).
#[derive(Debug, clap::Parser)]
pub struct GraphCommand {
    #[clap(flatten)]
    event_log: EventLogOptions,
}

impl BuckSubcommand for GraphCommand {
    const COMMAND_NAME: &'static str = "log-graph";

    async fn exec_impl(
        self,
        _matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
        _events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let log_path = self.event_log.get(&ctx).await?;
        let (invocation, mut events) = log_path.unpack_stream().await?;
        let command = invocation.display_command_line();

        buck2_client_ctx::stdio::print_with_writer::<buck2_error::Error, _>(async |w| {
            let mut w = std::io::BufWriter::new(w);
            let mut reader = GraphReader::default();
            reader.write(&mut w, &Record::Header { command: &command })?;
            while let Some(event) = events.try_next().await? {
                if let StreamValue::Event(event) = event {
                    reader.event(&mut w, &event)?;
                }
            }
            reader.finish(&mut w)?;
            w.flush()?;
            Ok(())
        })
        .await?;
        ExitResult::success()
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Record<'a> {
    Header {
        command: &'a str,
    },
    Action {
        span: u64,
        key: Option<&'a buck2_data::ActionKey>,
        category: &'a str,
        identifier: &'a str,
        deps: &'a [u64],
        reused: &'a [u64],
        tsets: &'a [u64],
    },
    Dangling {
        span: u64,
        referenced_by: u64,
    },
    Stats {
        actions: u64,
        edges: u64,
        reused: u64,
        dangling: u64,
        schema: Option<u32>,
        footer: Option<&'a buck2_data::ActionGraphFooter>,
    },
}

#[derive(Default)]
struct GraphReader {
    actions: u64,
    edges: u64,
    reused: u64,
    /// Spans of the actions seen so far.
    defined: BuckMutSet<u64>,
    /// Spans referred to before any action with that span was seen, with one referrer each.
    undefined: BuckMutMap<u64, u64>,
    schema: Option<u32>,
    footer: Option<buck2_data::ActionGraphFooter>,
}

impl GraphReader {
    fn write(&self, w: &mut impl Write, record: &Record<'_>) -> buck2_error::Result<()> {
        serde_json::to_writer(&mut *w, record)
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0))?;
        w.write_all(b"\n")?;
        Ok(())
    }

    fn event(
        &mut self,
        w: &mut impl Write,
        event: &buck2_data::BuckEvent,
    ) -> buck2_error::Result<()> {
        match &event.data {
            Some(buck2_data::buck_event::Data::SpanEnd(end)) => {
                if let Some(buck2_data::span_end_event::Data::ActionExecution(action)) = &end.data {
                    self.action(w, event.span_id, action)?;
                }
            }
            Some(buck2_data::buck_event::Data::Instant(instant)) => match &instant.data {
                Some(buck2_data::instant_event::Data::ActionGraphStart(start)) => {
                    self.schema = Some(start.schema);
                }
                Some(buck2_data::instant_event::Data::ActionGraphFooter(footer)) => {
                    self.footer = Some(*footer);
                }
                _ => {}
            },
            _ => {}
        }
        Ok(())
    }

    fn action(
        &mut self,
        w: &mut impl Write,
        span: u64,
        action: &buck2_data::ActionExecutionEnd,
    ) -> buck2_error::Result<()> {
        // A recording command writes the start event before any action ends.
        if self.schema.is_none() {
            return Err(not_recorded());
        }
        let (deps, reused, tsets) = producers(action.graph_producers.as_ref());
        self.actions += 1;
        self.edges += deps.len() as u64;
        self.reused += reused.len() as u64;
        for dep in deps {
            if !self.defined.contains(dep) {
                self.undefined.entry(*dep).or_insert(span);
            }
        }
        self.defined.insert(span);
        self.undefined.remove(&span);

        let name = action.name.as_ref();
        self.write(
            w,
            &Record::Action {
                span,
                key: action.key.as_ref(),
                category: name.map_or("", |n| n.category.as_str()),
                identifier: name.map_or("", |n| n.identifier.as_str()),
                deps,
                reused,
                tsets,
            },
        )
    }

    fn finish(self, w: &mut impl Write) -> buck2_error::Result<()> {
        if self.schema.is_none() {
            return Err(not_recorded());
        }
        let mut dangling: Vec<_> = self.undefined.iter().map(|(s, r)| (*s, *r)).collect();
        dangling.sort_unstable();
        for (span, referenced_by) in &dangling {
            self.write(
                w,
                &Record::Dangling {
                    span: *span,
                    referenced_by: *referenced_by,
                },
            )?;
        }
        self.write(
            w,
            &Record::Stats {
                actions: self.actions,
                edges: self.edges,
                reused: self.reused,
                dangling: dangling.len() as u64,
                schema: self.schema,
                footer: self.footer.as_ref(),
            },
        )
    }
}

fn not_recorded() -> buck2_error::Error {
    buck2_error::buck2_error!(
        buck2_error::ErrorTag::Input,
        "The event log has no recorded action graph: build with `--log-action-graph` to record one"
    )
}

/// The span ids, reused ids and set ids a producers message lists, all empty without one.
fn producers(p: Option<&buck2_data::ActionGraphProducers>) -> (&[u64], &[u64], &[u64]) {
    p.map_or((&[], &[], &[]), |p| {
        (&p.span_ids, &p.reused_ids, &p.tset_ids)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action_end(span: u64, deps: Vec<u64>) -> buck2_data::BuckEvent {
        buck2_data::BuckEvent {
            span_id: span,
            data: Some(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::ActionExecution(Box::new(
                        buck2_data::ActionExecutionEnd {
                            graph_producers: Some(buck2_data::ActionGraphProducers {
                                span_ids: deps,
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    ))),
                    ..Default::default()
                },
            )),
            ..Default::default()
        }
    }

    fn start() -> buck2_data::BuckEvent {
        buck2_data::BuckEvent {
            data: Some(buck2_data::buck_event::Data::Instant(
                buck2_data::InstantEvent {
                    data: Some(buck2_data::instant_event::Data::ActionGraphStart(
                        buck2_data::ActionGraphStart { schema: 1 },
                    )),
                },
            )),
            ..Default::default()
        }
    }

    /// Reads a recorded log: `events` after a start event.
    fn read(events: &[buck2_data::BuckEvent]) -> Vec<serde_json::Value> {
        let mut out = Vec::new();
        let mut reader = GraphReader::default();
        for event in std::iter::once(&start()).chain(events) {
            reader.event(&mut out, event).unwrap();
        }
        reader.finish(&mut out).unwrap();
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn test_actions_and_stats() {
        let records = read(&[action_end(1, vec![]), action_end(2, vec![1])]);
        assert_eq!(records.len(), 3, "two actions, then stats");
        assert_eq!(records[1]["kind"], "action");
        assert_eq!(records[1]["span"], 2);
        assert_eq!(records[1]["deps"], serde_json::json!([1]));
        let stats = &records[2];
        assert_eq!(
            (stats["actions"].as_u64(), stats["edges"].as_u64()),
            (Some(2), Some(1))
        );
        assert_eq!(stats["dangling"], 0);
        assert!(stats["footer"].is_null(), "no footer in this log");
    }

    #[test]
    fn test_dangling_reference() {
        let records = read(&[action_end(2, vec![9])]);
        assert_eq!(records[1]["kind"], "dangling");
        assert_eq!(
            (
                records[1]["span"].as_u64(),
                records[1]["referenced_by"].as_u64()
            ),
            (Some(9), Some(2))
        );
        assert_eq!(records[2]["dangling"], 1);
    }

    #[test]
    fn test_unrecorded_log_is_an_error() {
        assert!(
            GraphReader::default()
                .event(&mut Vec::new(), &action_end(1, vec![]))
                .is_err(),
            "an action before any start event"
        );
        assert!(
            GraphReader::default().finish(&mut Vec::new()).is_err(),
            "no start event at all"
        );
    }
}
