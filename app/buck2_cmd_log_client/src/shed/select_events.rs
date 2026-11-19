/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_client_ctx::client_ctx::BuckSubcommand;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::event_log_options::EventLogOptions;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::path_arg::PathArg;
use buck2_event_log::write::rewrite_event_log;
use regex::RegexSet;
use variant_name::VariantName;

/// Re-encode an event log, dropping events whose name matches a filter.
///
/// The output is written using the same encoding (mode + compression) as the input — the output
/// path's extension must agree with the input's, otherwise this errors. `StreamValue::Result` and
/// `StreamValue::PartialResult` are always preserved.
///
/// Event names are the PascalCase variant identifier of the inner oneof (e.g. `ActionExecution`,
/// `Snapshot`).
///
/// This can be used to get an understanding of the impact of a certain change to event log sizes in
/// a way that is more compression correct than uncompressed byte counts.
#[derive(Debug, clap::Parser)]
pub struct SelectEventsCommand {
    #[clap(flatten)]
    event_log: EventLogOptions,

    /// Output path. Must use the same extension/encoding as the input log.
    #[clap(long, short = 'o', value_name = "PATH")]
    output: PathArg,

    /// Keep only events whose name matches at least one of these regexes. Repeatable.
    #[clap(long = "include", short = 'I', value_name = "REGEX")]
    include: Vec<String>,

    /// Drop events whose name matches any of these regexes. Repeatable. Applied after
    /// `--include`.
    #[clap(long = "exclude", short = 'X', value_name = "REGEX")]
    exclude: Vec<String>,
}

impl BuckSubcommand for SelectEventsCommand {
    const COMMAND_NAME: &'static str = "log-shed-select-events";

    async fn exec_impl(
        self,
        _matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
        _events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let Self {
            event_log,
            output,
            include,
            exclude,
        } = self;

        let input = event_log.get(&ctx).await?;
        let output_path = output.resolve(&ctx.working_dir);

        let include = RegexSet::new(&include)?;
        let exclude = RegexSet::new(&exclude)?;
        let has_include = !include.is_empty();

        rewrite_event_log(&input, output_path, |event| {
            let name = event_name(event);
            (!has_include || include.is_match(name)) && !exclude.is_match(name)
        })
        .await?;

        ExitResult::success()
    }
}

fn event_name(event: &buck2_data::BuckEvent) -> &'static str {
    use buck2_data::buck_event::Data;

    let Some(data) = event.data.as_ref() else {
        return "Unknown";
    };
    match data {
        Data::SpanStart(s) => match s.data.as_ref() {
            Some(d) => d.variant_name(),
            None => "Unknown",
        },
        Data::SpanEnd(s) => match s.data.as_ref() {
            Some(d) => d.variant_name(),
            None => "Unknown",
        },
        Data::Instant(i) => i.data.as_ref().map_or("Unknown", VariantName::variant_name),
        Data::Record(_) => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use buck2_data::ActionExecutionStart;
    use buck2_data::SpanEndEvent;
    use buck2_data::SpanStartEvent;
    use buck2_data::buck_event::Data;
    use buck2_event_log::read::EventLogPathBuf;
    use buck2_event_log::stream_value::StreamValue;
    use buck2_event_log::utils::Invocation;
    use buck2_event_log::write::rewrite_event_log;
    use buck2_events::BuckEvent;
    use buck2_events::span::SpanId;
    use buck2_fs::paths::abs_path::AbsPathBuf;
    use buck2_wrapper_common::invocation_id::TraceId;
    use futures::TryStreamExt;
    use tempfile::TempDir;

    use super::*;

    fn make_event(data: Data) -> BuckEvent {
        BuckEvent::new(
            SystemTime::now(),
            TraceId::new(),
            Some(SpanId::next()),
            None,
            data,
        )
    }

    fn span_start_action() -> Data {
        Data::SpanStart(SpanStartEvent {
            data: Some(buck2_data::span_start_event::Data::ActionExecution(
                ActionExecutionStart::default(),
            )),
        })
    }

    fn span_end_action() -> Data {
        Data::SpanEnd(SpanEndEvent {
            data: Some(buck2_data::span_end_event::Data::ActionExecution(
                Box::default(),
            )),
            ..Default::default()
        })
    }

    fn instant_snapshot() -> Data {
        Data::Instant(buck2_data::InstantEvent {
            data: Some(buck2_data::instant_event::Data::Snapshot(Box::default())),
        })
    }

    /// Build a `.pb.zst` event log on disk containing a default invocation header plus the
    /// given events, by writing the raw protobuf framing + zstd compression directly. This
    /// avoids depending on `NamedEventLogWriter` / `WriteEventLog`, neither of which is
    /// publicly constructible from outside `buck2_event_log`.
    async fn build_input_log(
        path: AbsPathBuf,
        events: Vec<BuckEvent>,
    ) -> buck2_error::Result<EventLogPathBuf> {
        use std::io::Write;

        use prost::Message;

        let invocation = buck2_data::Invocation {
            command_line_args: vec!["buck2".to_owned()],
            expanded_command_line_args: vec!["buck2".to_owned()],
            working_dir: "/tmp".to_owned(),
            trace_id: Some(TraceId::null().to_string()),
            start_time: None,
        };

        let mut raw = Vec::new();
        invocation.encode_length_delimited(&mut raw).unwrap();
        for e in events {
            let cp = buck2_cli_proto::CommandProgress {
                progress: Some(buck2_cli_proto::command_progress::Progress::Event(e.into())),
            };
            cp.encode_length_delimited(&mut raw).unwrap();
        }

        let compressed = {
            let mut enc = zstd::Encoder::new(Vec::new(), 0).unwrap();
            enc.write_all(&raw).unwrap();
            enc.finish().unwrap()
        };
        std::fs::write(&path, compressed).unwrap();
        EventLogPathBuf::infer(path)
    }

    async fn collect(path: &EventLogPathBuf) -> (Invocation, Vec<StreamValue>) {
        let (inv, mut s) = path.unpack_stream().await.unwrap();
        let mut out = Vec::new();
        while let Some(v) = s.try_next().await.unwrap() {
            out.push(v);
        }
        (inv, out)
    }

    #[tokio::test]
    async fn round_trip_no_filter_preserves_events() -> buck2_error::Result<()> {
        let tmp = TempDir::new()?;
        let input = AbsPathBuf::try_from(tmp.path().join("in.pb.zst")).unwrap();
        let output = AbsPathBuf::try_from(tmp.path().join("out.pb.zst")).unwrap();

        let events = vec![
            make_event(span_start_action()),
            make_event(span_end_action()),
            make_event(instant_snapshot()),
        ];
        let input_log = build_input_log(input, events).await?;

        rewrite_event_log(&input_log, output.clone(), |_| true).await?;

        let output_log = EventLogPathBuf::infer(output)?;
        let (_inv_in, vals_in) = collect(&input_log).await;
        let (_inv_out, vals_out) = collect(&output_log).await;
        assert_eq!(
            vals_in.len(),
            vals_out.len(),
            "All events should round-trip"
        );
        Ok(())
    }

    #[tokio::test]
    async fn exclude_action_execution_drops_both_halves() -> buck2_error::Result<()> {
        let tmp = TempDir::new()?;
        let input = AbsPathBuf::try_from(tmp.path().join("in.pb.zst")).unwrap();
        let output = AbsPathBuf::try_from(tmp.path().join("out.pb.zst")).unwrap();

        let events = vec![
            make_event(span_start_action()),
            make_event(span_end_action()),
            make_event(instant_snapshot()),
        ];
        let input_log = build_input_log(input, events).await?;

        let exclude = RegexSet::new(["^ActionExecution$"]).unwrap();
        rewrite_event_log(&input_log, output.clone(), |e| {
            !exclude.is_match(event_name(e))
        })
        .await?;

        let output_log = EventLogPathBuf::infer(output)?;
        let (_inv, vals) = collect(&output_log).await;
        assert_eq!(
            vals.len(),
            1,
            "Only the snapshot event should survive (start + end of ActionExecution dropped)"
        );
        match &vals[0] {
            StreamValue::Event(e) => assert_eq!(event_name(e), "Snapshot"),
            _ => panic!("Expected Snapshot event"),
        }
        Ok(())
    }

    #[tokio::test]
    async fn extension_mismatch_errors() -> buck2_error::Result<()> {
        let tmp = TempDir::new()?;
        let input = AbsPathBuf::try_from(tmp.path().join("in.pb.zst")).unwrap();
        let output = AbsPathBuf::try_from(tmp.path().join("out.json-lines")).unwrap();

        let input_log = build_input_log(input, vec![make_event(instant_snapshot())]).await?;

        let err = rewrite_event_log(&input_log, output, |_| true)
            .await
            .expect_err("Mismatched extensions should error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains(".pb.zst"),
            "Error should mention the expected extension, got: {msg}"
        );
        Ok(())
    }
}
