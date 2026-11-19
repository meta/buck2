/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::time::Duration;
use std::time::SystemTime;

use buck2_data::SpanEndEvent;
use buck2_events::BuckEvent;
use variant_name::VariantName;

use crate::unpack_event::UnpackedBuckEvent;
use crate::unpack_event::unpack_event;

static NUM_DELAYS_FOR_AVERAGE: usize = 10;

#[derive(Default)]
pub struct SpanData {
    pub started: u64,
    pub finished: u64,
    pub total_duration: Duration,
    pub total_poll_time: Duration,
    // We display this as average max poll time, but easiest to track it as total.
    pub total_max_poll_time: Duration,
}

#[derive(Default)]
pub struct InstantData {
    pub count: u64,
}

pub struct DebugEventsState {
    pub event_count: u64,
    pub spans: BTreeMap<String, SpanData>,
    pub instants: BTreeMap<String, InstantData>,
    /// events we receive have SystemTime timestamps and may be based on a different time than
    /// we are running in. This is used to adjust them to our local Instant-based time.
    pub events_start_time: Option<SystemTime>,
    pub recent_delays: VecDeque<Duration>,
    pub max_delay: Duration,
}

impl DebugEventsState {
    pub fn new() -> Self {
        Self {
            event_count: 0,
            spans: BTreeMap::new(),
            instants: BTreeMap::new(),
            events_start_time: None,
            recent_delays: VecDeque::new(),
            max_delay: Duration::ZERO,
        }
    }

    pub fn handle_event(&mut self, event: &BuckEvent) -> buck2_error::Result<()> {
        self.event_count += 1;

        let delay = event
            .timestamp()
            .duration_since(SystemTime::now())
            .unwrap_or(Duration::ZERO);

        self.max_delay = std::cmp::max(delay, self.max_delay);
        self.recent_delays.push_back(delay);
        while self.recent_delays.len() > NUM_DELAYS_FOR_AVERAGE {
            self.recent_delays.pop_front();
        }

        match unpack_event(event)? {
            UnpackedBuckEvent::SpanStart(_, _, data) => self.span_started(Some(data)),
            UnpackedBuckEvent::UnrecognizedSpanStart(_, _) => self.span_started(None),
            UnpackedBuckEvent::SpanEnd(_, span_end, data) => self.span_end(span_end, Some(data)),
            UnpackedBuckEvent::UnrecognizedSpanEnd(_, span_end) => self.span_end(span_end, None),
            UnpackedBuckEvent::Instant(_, _, data) => self.instant(Some(data)),
            UnpackedBuckEvent::UnrecognizedInstant(_, _) => self.instant(None),
        }

        Ok(())
    }

    fn span_started(&mut self, data: Option<&buck2_data::span_start_event::Data>) {
        let name = data.map_or("Unrecognized", |d| d.variant_name());
        let entry = {
            match self.spans.get_mut(name) {
                Some(v) => v,
                None => {
                    self.spans.insert(name.to_owned(), SpanData::default());
                    self.spans.get_mut(name).unwrap()
                }
            }
        };
        entry.started += 1;
    }

    fn span_end(
        &mut self,
        span_end: &SpanEndEvent,
        data: Option<&buck2_data::span_end_event::Data>,
    ) {
        // Right now, matching these end events to the start events depends on the field names in the protobufs
        // matching. That works but is fragile, if it breaks at some point we can do the match and explicitly
        // match them ourselves.
        let name = data.map_or("Unrecognized", |d| d.variant_name());

        let entry = {
            match self.spans.get_mut(name) {
                Some(v) => v,
                None => {
                    self.spans.insert(name.to_owned(), SpanData::default());
                    self.spans.get_mut(name).unwrap()
                }
            }
        };
        entry.finished += 1;
        if let Some(v) = &span_end.stats {
            entry.total_poll_time += Duration::from_micros(v.max_poll_time_us);
            entry.total_max_poll_time += Duration::from_micros(v.max_poll_time_us);
        }
    }

    fn instant(&mut self, data: Option<&buck2_data::instant_event::Data>) {
        let name = data.map_or("Unrecognized", |d| d.variant_name());

        let entry = {
            match self.instants.get_mut(name) {
                Some(v) => v,
                None => {
                    self.instants
                        .insert(name.to_owned(), InstantData::default());
                    self.instants.get_mut(name).unwrap()
                }
            }
        };

        entry.count += 1;
    }
}
