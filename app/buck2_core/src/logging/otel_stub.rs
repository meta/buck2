//! Stub of [`crate::logging::otel`] for Meta, with empty implementations.
//!
//! Note that this module provides no `export_span` equivalent, because that function takes an
//! [`opentelemetry::KeyValue`] as a parameter.

pub fn activate(version: &'static str) {}

pub fn is_active() -> bool {
    false
}

pub fn shutdown() {}
