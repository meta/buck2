/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! An event sink that exports the end-of-invocation [`InvocationRecord`](buck2_data::InvocationRecord)
//! to OpenTelemetry (OTLP) as a single "wide event" span.
//!
//! This is a peer of the Scribe [`RemoteEventSink`](crate::sink::remote::RemoteEventSink): it has
//! the same shape (`send_now` / `send_messages_now` / `offer` / [`EventSink`]) so callers route the
//! record to it exactly as they route to Scribe, with no special-casing. Unlike Scribe it is not
//! Meta-specific, so it is the wide-event export path in open-source builds too.
//!
//! The actual proto -> attribute mapping lives in [`crate::sink::otel_record`]; spans are emitted
//! through [`buck2_core::logging::otel`], which owns the exporter lifecycle (activation after the
//! daemon fork, flushing on exit).

use std::sync::Arc;
use std::time::Duration;

use opentelemetry::KeyValue;

use crate::BuckEvent;
use crate::Event;
use crate::EventSink;
use crate::EventSinkStats;
use crate::EventSinkWithStats;
use crate::sink::otel_record::invocation_record_attributes;

/// Forwards the invocation record to the process-wide OTLP exporter as a span.
pub struct OtelEventSink;

impl OtelEventSink {
    pub fn new() -> OtelEventSink {
        OtelEventSink
    }

    /// Export the event now. The OTLP batch processor accepts spans synchronously and is drained by
    /// `buck2_core::logging::otel::shutdown` on exit, so there is nothing to await -- the `async`
    /// signature exists only to match [`RemoteEventSink`](crate::sink::remote::RemoteEventSink).
    pub async fn send_now(&self, event: BuckEvent) -> buck2_error::Result<()> {
        Self::export(&event);
        Ok(())
    }

    pub async fn send_messages_now(&self, events: Vec<BuckEvent>) -> buck2_error::Result<()> {
        for event in &events {
            Self::export(event);
        }
        Ok(())
    }

    pub fn offer(&self, event: BuckEvent) {
        Self::export(&event);
    }

    /// Translate an invocation-record event into an OTLP span. Events that are not an
    /// `InvocationRecord` are ignored -- this sink only produces the wide event.
    fn export(event: &BuckEvent) {
        let buck2_data::buck_event::Data::Record(record_event) = event.data() else {
            return;
        };
        let Some(buck2_data::record_event::Data::InvocationRecord(record)) = &record_event.data
        else {
            return;
        };

        // The span brackets the invocation: it ends when the record was emitted and started
        // `client_walltime` earlier (the walltime the client measured for the whole command).
        let end = event.timestamp();
        let walltime = record
            .client_walltime
            .as_ref()
            .map(|d| Duration::new(d.seconds.max(0) as u64, d.nanos.max(0) as u32))
            .unwrap_or_default();
        let start = end.checked_sub(walltime).unwrap_or(end);

        let mut attributes = invocation_record_attributes(record);
        // The invocation's trace UUID lives on the event envelope, not in the record payload, but
        // it is the primary correlation key (it ties this wide event to the event log and to any
        // other telemetry for the same invocation), so attach it explicitly.
        if let Ok(trace_id) = event.trace_id() {
            attributes.push(KeyValue::new("buck2.trace_id", trace_id.to_string()));
        }
        // NB: The event isn't necessarily a 'build' in the strict sense (this also fires for tests,
        // queries, etc.), but this matches Meta's `buck2_builds` table upstream.
        buck2_core::logging::otel::export_span("buck2_build", start, end, attributes);
    }
}

impl EventSink for OtelEventSink {
    fn send(&self, event: Event) {
        if let Event::Buck(event) = event {
            Self::export(&event);
        }
    }
}

impl EventSinkWithStats for OtelEventSink {
    fn to_event_sync(self: Arc<Self>) -> Arc<dyn EventSink> {
        self as _
    }

    fn stats(&self) -> EventSinkStats {
        // The exporter owns its own buffering/retry accounting; this sink keeps no counters.
        EventSinkStats {
            successes: 0,
            failures_invalid_request: 0,
            failures_unauthorized: 0,
            failures_rate_limited: 0,
            failures_pushed_back: 0,
            failures_enqueue_failed: 0,
            failures_internal_error: 0,
            failures_timed_out: 0,
            failures_unknown: 0,
            buffered: 0,
            dropped: 0,
            bytes_written: 0,
        }
    }
}

/// Construct an OTLP sink, or `None` when OTLP export is not active (no endpoint configured, or the
/// exporter was never activated). Mirrors
/// [`new_remote_event_sink_if_enabled`](crate::sink::remote::new_remote_event_sink_if_enabled) so
/// the two remote sinks are constructed and used identically.
pub fn new_otel_event_sink_if_enabled() -> Option<OtelEventSink> {
    if buck2_core::logging::otel::is_active() {
        Some(OtelEventSink::new())
    } else {
        None
    }
}
