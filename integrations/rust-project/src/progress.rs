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

use rustc_hash::FxHashMap;
use serde::Serialize;
use tracing::span;
use tracing_subscriber::Layer;
use tracing_subscriber::fmt::MakeWriter;

pub(crate) struct ProgressLayer<S, W> {
    _s: std::marker::PhantomData<S>,
    writer: W,
}

impl<S, W> ProgressLayer<S, W> {
    pub(crate) fn new(writer: W) -> Self {
        Self {
            _s: std::marker::PhantomData,
            writer,
        }
    }
}

/// Discover events that are consumed by rust-analyzer.
///
/// <https://rust-analyzer.github.io/book/configuration.html#workspace-discovery-protocol>
#[derive(Serialize, Debug, Clone, PartialEq)]
struct DiscoverProjectEvent<'a> {
    /// "progress" or "error".
    kind: serde_json::Value,
    #[serde(flatten)]
    event_fields: &'a FxHashMap<String, serde_json::Value>,
    #[serde(flatten)]
    span_fields: &'a FxHashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq)]
struct ProgressStorage {
    data: FxHashMap<String, serde_json::Value>,
}

impl<S, W> Layer<S> for ProgressLayer<S, W>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    W: for<'a> MakeWriter<'a> + 'static,
{
    fn on_new_span(
        &self,
        attrs: &span::Attributes<'_>,
        id: &span::Id,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let span = ctx.span(id).unwrap();
        let mut extensions = span.extensions_mut();

        let mut fields = FxHashMap::default();
        let mut visitor = JsonVisitor(&mut fields);
        attrs.record(&mut visitor);

        extensions.insert(ProgressStorage { data: fields });
    }

    fn on_event(&self, event: &tracing::Event<'_>, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let mut event_fields = FxHashMap::default();
        let mut visitor = JsonVisitor(&mut event_fields);
        event.record(&mut visitor);

        let Some(kind) = event_fields.remove("kind") else {
            return;
        };

        let span_fields = match ctx.lookup_current() {
            Some(span) => {
                let ext = span.extensions();
                if let Some(storage) = ext.get::<ProgressStorage>() {
                    storage.data.clone()
                } else {
                    FxHashMap::default()
                }
            }
            _ => FxHashMap::default(),
        };

        let discover_event = DiscoverProjectEvent {
            kind,
            event_fields: &event_fields,
            span_fields: &span_fields,
        };
        let discover_event = serde_json::to_string(&discover_event).unwrap();
        let mut writer = self.writer.make_writer();
        writeln!(writer, "{discover_event}").expect("unable to write");
    }
}

struct JsonVisitor<'a>(&'a mut FxHashMap<String, serde_json::Value>);

impl tracing::field::Visit for JsonVisitor<'_> {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        let value: String = if field.name() == "project" {
            serde_json::from_str(value).unwrap()
        } else {
            String::from(value)
        };
        self.0
            .insert(field.name().to_owned(), serde_json::Value::from(value));
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.insert(
            field.name().to_owned(),
            serde_json::Value::from(format!("{value:?}")),
        );
    }
}
