/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Optional OpenTelemetry (OTLP) export.
//!
//! When an OTLP endpoint is configured via the standard `OTEL_EXPORTER_OTLP_*` environment
//! variables, this module builds an OTLP exporter and tracer provider. Unlike a server, buck2 is a
//! short-lived CLI process, so the exported resource attributes describe *this invocation* (a fresh
//! `service.instance.id` per run, the compiled-in `service.version`, the host and pid) rather than a
//! long-running deployment.
//!
//! This module owns only the exporter lifecycle (build, activate, flush). What gets exported is up
//! to callers: anything that reaches the global provider stored here -- a `tracing` layer bridged on
//! top, or an out-of-band "wide event" span -- rides the same exporter.
//!
//! ## Environment variables
//!
//! We honor the standard OpenTelemetry environment variables so buck2 slots into an existing
//! telemetry setup without buck2-specific configuration:
//!
//! * `OTEL_EXPORTER_OTLP_*` -- endpoint, headers, timeout, compression, TLS. Read by the exporter
//!   builder itself; we only check the two endpoint variables to decide *whether* to export.
//! * `OTEL_SDK_DISABLED=true` -- the spec's global kill-switch. `opentelemetry-rust` does not honor
//!   it on its own, so we check it here and skip building the exporter entirely.
//! * `OTEL_SERVICE_NAME` / `OTEL_RESOURCE_ATTRIBUTES` -- extra/overriding resource attributes,
//!   merged in by [`resource`]. Our own authoritative attributes (pid, arch, os, host) win on
//!   conflict; `service.name` defaults to `buck2` but yields to a user-provided value.
//! * `TRACEPARENT` / `TRACESTATE` -- W3C Trace Context of the launching process (CI job, wrapper
//!   script, ...). When present, our wide-event span is parented under that trace instead of being
//!   a disconnected root. See [`export_span`].
//!
//! ## Deferred activation (important)
//!
//! The OTLP batch exporter spawns background threads (a batch-processor thread plus an HTTP client
//! runtime). The buck2 daemon daemonizes via `fork()` *without* a following `exec()`, and `fork()`
//! only copies the calling thread -- any other thread vanishes in the child but leaves the locks and
//! state it held (allocator, TLS/crypto, exporter queues) permanently wedged. A daemon that spawned
//! these threads pre-fork therefore deadlocks or aborts shortly after start. See the
//! "Do not create any threads before this point" invariant in `buck2_daemon::daemon`.
//!
//! So we do *not* build the exporter when the subscriber is installed. [`activate`] builds it later,
//! once the process is past any such `fork()`. Today only the client calls [`activate`] -- it emits
//! the invocation record and never `fork()`s-without-`exec()`. If the daemon ever exports its own
//! spans it must call [`activate`] only after it has finished daemonizing, never before.
//!
//! ## Flushing
//!
//! Spans are buffered and only flushed periodically. buck2 exits via `libc::_exit` (see
//! `ExitResult::report`), which runs no destructors, so the buffered batch would be dropped unless
//! we drain it explicitly. [`shutdown`] does that drain and must be called on every exit path. See
//! <https://github.com/open-telemetry/opentelemetry-rust/issues/1961>.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::SystemTime;

use buck2_error::conversion::from_any_with_tag;
use opentelemetry::Context;
use opentelemetry::KeyValue;
use opentelemetry::propagation::TextMapPropagator as _;
use opentelemetry::trace::Span as _;
use opentelemetry::trace::Tracer as _;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::Protocol;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::resource::EnvResourceDetector;
use opentelemetry_sdk::resource::TelemetryResourceDetector;
use opentelemetry_sdk::trace::BatchConfigBuilder;
use opentelemetry_sdk::trace::BatchSpanProcessor;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::trace::SpanLimits;
use opentelemetry_semantic_conventions::resource::HOST_ARCH;
use opentelemetry_semantic_conventions::resource::HOST_NAME;
use opentelemetry_semantic_conventions::resource::OS_TYPE;
use opentelemetry_semantic_conventions::resource::PROCESS_PID;
use opentelemetry_semantic_conventions::resource::SERVICE_INSTANCE_ID;
use opentelemetry_semantic_conventions::resource::SERVICE_VERSION;
use uuid::Uuid;

/// The active OTLP tracer provider, if any. Stored globally so that [`shutdown`] can reach it from
/// the process exit path (which lives in a different crate, and runs after any layer's type has been
/// erased into the global subscriber) and so out-of-band emitters can reuse the same exporter.
///
/// We can't use [`opentelemetry::global::tracer_provider`] here because that produces a
/// [`opentelemetry::global::GlobalTracerProvider`], which lacks the
/// [`opentelemetry_sdk::trace::SdkTracerProvider::shutdown`] method we actually need to call.
static PROVIDER: OnceLock<SdkTracerProvider> = OnceLock::new();

/// Guards [`activate`] so the exporter is built at most once, even if it is called from more than one
/// entry point (today just the client, before running a command; a future daemon exporter, which
/// would activate after daemonizing, would be a second).
static ACTIVATED: AtomicBool = AtomicBool::new(false);

/// We only enable OTLP export when an endpoint is explicitly configured. This keeps the common case
/// (no telemetry) free of overhead and avoids futile connection attempts to the default
/// `localhost:4318`. These are standard OpenTelemetry variables read by the exporter itself, not
/// buck2-owned configuration, so we check them directly rather than registering them via
/// `buck2_env!` (which would surface them misleadingly in `buck2 help-env`).
fn otlp_endpoint_configured() -> bool {
    [
        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
        "OTEL_EXPORTER_OTLP_ENDPOINT",
    ]
    .iter()
    .any(|var| std::env::var_os(var).is_some_and(|v| !v.is_empty()))
}

/// The OpenTelemetry SDK kill-switch. Per the spec, `OTEL_SDK_DISABLED=true` (case-insensitive)
/// disables the SDK; any other value, or an unset variable, leaves it enabled. `opentelemetry-rust`
/// does not read this variable itself, so we check it and skip building the exporter.
fn otel_sdk_disabled() -> bool {
    std::env::var_os("OTEL_SDK_DISABLED").is_some_and(|v| v.eq_ignore_ascii_case("true"))
}

/// Map Rust's [`std::env::consts::OS`] to the OpenTelemetry `os.type` value set, passing through any
/// value without a standardized equivalent (e.g. `ios`, `android`) as-is. Most names already match;
/// only a couple are spelled differently.
fn otel_os_type(os: &str) -> &str {
    match os {
        "macos" => "darwin",
        "dragonfly" => "dragonflybsd",
        // `linux`, `windows`, `freebsd`, `netbsd`, `openbsd`, `solaris`, `aix` already match.
        other => other,
    }
}

/// Map Rust's [`std::env::consts::ARCH`] to the OpenTelemetry `host.arch` value set, passing through
/// any value without a standardized equivalent (e.g. `riscv64`, `loongarch64`) as-is.
fn otel_host_arch(arch: &str) -> &str {
    match arch {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "arm" => "arm32",
        "powerpc" => "ppc32",
        "powerpc64" => "ppc64",
        // `x86` and `s390x` already match.
        other => other,
    }
}

/// Resource attributes identifying this build invocation, following OpenTelemetry semantic
/// conventions (<https://opentelemetry.io/docs/specs/semconv/resource/>).
///
/// Attributes are layered in ascending priority -- each step below overrides earlier ones on
/// conflict -- so `service.name` falls back to `buck2` but yields to whatever the user configured,
/// while the values buck2 measures itself always win. Expressing precedence by ordering lets the
/// user's `service.name` simply override our default instead of us having to detect whether they set
/// one. (We can't use the all-in-one `Resource::builder()`: it runs its detectors *first*, i.e. at
/// the lowest priority, leaving no way to slip our `buck2` default underneath them.)
fn resource(version: &'static str) -> Resource {
    let mut authoritative = vec![
        // buck2 is not a deployed service, so `service.version` is just this binary's build version.
        // The caller passes it in (`BuckVersion::get_version()`, the same string `buck2 --version`
        // prints) because the richer version -- the source revision stamped at build time via
        // `BUCK2_SET_EXPLICIT_VERSION` -- is only resolvable in the `buck2` bin crate, not here. It
        // falls back to the binary's build-id when no revision is stamped.
        KeyValue::new(SERVICE_VERSION, version),
        // Every buck2 process is its own "instance"; a fresh v4 UUID keeps invocations distinct.
        //
        // NB: This is not the build ID / trace ID, which is _also_ written as a v4 UUID.
        KeyValue::new(SERVICE_INSTANCE_ID, Uuid::new_v4().to_string()),
        KeyValue::new(HOST_ARCH, otel_host_arch(std::env::consts::ARCH)),
        KeyValue::new(OS_TYPE, otel_os_type(std::env::consts::OS)),
        KeyValue::new(PROCESS_PID, i64::from(std::process::id())),
    ];
    if let Ok(Some(hostname)) = hostname::get().map(|h| h.into_string().ok()) {
        authoritative.push(KeyValue::new(HOST_NAME, hostname));
    }

    let mut builder = Resource::builder_empty()
        // Lowest priority: our default `service.name`, overridden by anything the user sets below.
        .with_service_name("buck2")
        // `telemetry.sdk.*`, plus any attributes from `OTEL_RESOURCE_ATTRIBUTES` (which may carry a
        // `service.name` that then wins over our default, and other attributes like
        // `deployment.environment` that flow through untouched).
        .with_detectors(&[
            Box::new(TelemetryResourceDetector),
            Box::new(EnvResourceDetector::new()),
        ]);
    // `OTEL_SERVICE_NAME` is the dedicated variable and outranks a `service.name` in
    // `OTEL_RESOURCE_ATTRIBUTES`; `EnvResourceDetector` only reads the latter, so apply it ourselves.
    if let Some(name) = std::env::var_os("OTEL_SERVICE_NAME")
        .and_then(|v| v.into_string().ok())
        .filter(|v| !v.is_empty())
    {
        builder = builder.with_service_name(name);
    }
    // Highest priority: the attributes buck2 measures itself (pid, arch, os, host) win over anything
    // the environment supplies.
    builder.with_attributes(authoritative).build()
}

/// Build the OTLP exporter and tracer provider (spawning the exporter's background threads) and store
/// the provider in [`PROVIDER`]. No-op when no endpoint is configured.
fn build_provider(version: &'static str) -> buck2_error::Result<()> {
    if otel_sdk_disabled() || !otlp_endpoint_configured() {
        return Ok(());
    }

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .build()
        .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0))?;

    let batch_config = BatchConfigBuilder::default().build();
    let processor = BatchSpanProcessor::builder(exporter)
        .with_batch_config(batch_config)
        .build();

    let provider = SdkTracerProvider::builder()
        // The default settings limit all of these values at 128, which is small enough to start
        // dropping data on our `InvocationRecord`s!
        .with_span_limits(SpanLimits {
            max_events_per_span: 1024,
            max_attributes_per_span: 2048,
            max_links_per_span: 512,
            max_attributes_per_event: 2048,
            max_attributes_per_link: 1024,
        })
        .with_resource(resource(version))
        .with_span_processor(processor)
        .build();

    // `activate` runs once, so ignore an already-set slot.
    let _ = PROVIDER.set(provider);

    Ok(())
}

/// Emit a fully-assembled "wide event" as a single span, out-of-band from the `tracing` subscriber.
///
/// Most spans are accumulated from nested `tracing` spans, but some records -- notably the
/// end-of-invocation `InvocationRecord` -- are assembled once as a flat field set. OTLP backends
/// (Honeycomb and friends) ingest a span as a single wide row, so we ship such a record as one span
/// whose `attributes` are its fields and whose start/end bracket the invocation.
///
/// This is intentionally dumb: the caller hands us the finished attribute set (it owns the mapping
/// from its domain type to keys/values), and we just attach it to a span. The span is enqueued on
/// the same batch processor as every other span and flushed by [`shutdown`], so it must be emitted
/// before the process exits. No-op when telemetry was never activated (no endpoint configured, or
/// [`activate`] not yet called). Because it reuses the global provider, it must only be called after
/// any `fork()`-without-`exec()` -- in practice it is only emitted by the client at
/// end-of-invocation, which never forks that way.
pub fn export_span(
    name: &'static str,
    start: SystemTime,
    end: SystemTime,
    attributes: Vec<KeyValue>,
) {
    let Some(provider) = PROVIDER.get() else {
        return;
    };

    let tracer = provider.tracer("buck2");
    let mut span = tracer
        .span_builder(name)
        .with_start_time(start)
        .with_attributes(attributes)
        .start_with_context(&tracer, &parent_context_from_env());
    span.end_with_timestamp(end);
}

/// Extract a parent trace context from the environment, following the W3C Trace Context convention
/// that CI systems and wrappers (`otel-cli`, the Jenkins / GitHub Actions OpenTelemetry plugins,
/// Buildkite, ...) use to hand an in-progress trace to child processes via `$TRACEPARENT` (and its
/// companion `$TRACESTATE`). When buck2 is launched under such a trace, this makes our wide-event
/// span a child of the launching trace instead of a disconnected root.
///
/// `opentelemetry-rust` only ever extracts trace context from HTTP-style carriers, never from the
/// process environment, so we assemble a carrier from the env ourselves and run the standard
/// `TraceContextPropagator` over it. Returns an empty (root) context when `$TRACEPARENT` is absent
/// or malformed -- i.e. the previous always-root behavior.
fn parent_context_from_env() -> Context {
    let env = |var| std::env::var_os(var).and_then(|v| v.into_string().ok());
    parent_context_from_carrier(
        env("TRACEPARENT").as_deref(),
        env("TRACESTATE").as_deref(),
    )
}

/// The environment-independent core of [`parent_context_from_env`], split out so it can be tested
/// without mutating process-global environment state.
fn parent_context_from_carrier(traceparent: Option<&str>, tracestate: Option<&str>) -> Context {
    let mut carrier: HashMap<String, String> = HashMap::new();
    // The propagator keys off the lowercased header names, matching how these would arrive over HTTP.
    for (header, value) in [("traceparent", traceparent), ("tracestate", tracestate)] {
        if let Some(value) = value.filter(|v| !v.is_empty()) {
            carrier.insert(header.to_owned(), value.to_owned());
        }
    }
    TraceContextPropagator::new().extract(&carrier)
}

/// Build the OTLP exporter and start exporting, if telemetry is configured.
///
/// This spawns the exporter's background threads, so it MUST be called only after the process has
/// finished any `fork()`-without-`exec()`. Today only the client calls it, before running a command
/// (the client never `fork()`s-without-`exec()`, so any time is fine); a future daemon exporter would
/// have to call it only after daemonizing. Idempotent: the exporter is built at most once. No-op if
/// telemetry is not configured.
/// Errors are logged rather than propagated -- telemetry must never fail a command.
///
/// `version` is recorded as the `service.version` resource attribute; pass
/// `BuckVersion::get_version()` (the string `buck2 --version` prints).
pub fn activate(version: &'static str) {
    if ACTIVATED.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Err(e) = build_provider(version) {
        tracing::warn!("Failed to start OpenTelemetry exporter: {e}");
    }
}

/// Whether the OTLP exporter is built and exporting (i.e. [`activate`] ran and an endpoint was
/// configured). Lets callers skip building span attributes when nothing would consume them --
/// [`export_span`] is itself a no-op in that case, but assembling its attributes is not free.
pub fn is_active() -> bool {
    PROVIDER.get().is_some()
}

/// Flush and shut down the OTLP exporter, draining any spans still buffered in the batch processor.
///
/// This must be called before the process exits. buck2 exits via `libc::_exit`, which runs no
/// destructors, so without this the final batch of spans is silently lost. No-op when OTLP export
/// was never activated.
pub fn shutdown() {
    if let Some(provider) = PROVIDER.get() {
        // Best-effort: we are on the way out regardless, so a failed flush only costs us the last
        // batch of spans.
        if let Err(e) = provider.shutdown() {
            tracing::warn!("Failed to shut down OpenTelemetry exporter on exit: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use opentelemetry::trace::TraceContextExt as _;

    use super::*;

    #[test]
    fn traceparent_becomes_remote_parent() {
        // A well-formed W3C traceparent yields a valid, remote parent span context whose trace id
        // our span will inherit.
        let cx = parent_context_from_carrier(
            Some("00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"),
            None,
        );
        let span_context = cx.span().span_context().clone();
        assert!(span_context.is_valid());
        assert!(span_context.is_remote());
        assert_eq!(
            span_context.trace_id().to_string(),
            "0af7651916cd43dd8448eb211c80319c"
        );
    }

    #[test]
    fn no_or_malformed_traceparent_is_root() {
        // No traceparent -> no parent (a fresh root span, as before).
        assert!(
            !parent_context_from_carrier(None, None)
                .span()
                .span_context()
                .is_valid()
        );
        // An empty value is treated as absent, not as a malformed header.
        assert!(
            !parent_context_from_carrier(Some(""), Some("foo=bar"))
                .span()
                .span_context()
                .is_valid()
        );
        // Garbage is ignored rather than propagated.
        assert!(
            !parent_context_from_carrier(Some("not-a-traceparent"), None)
                .span()
                .span_context()
                .is_valid()
        );
    }

    #[test]
    fn os_type_remaps_and_passes_through() {
        // Rust spellings that differ from the OpenTelemetry value set.
        assert_eq!(otel_os_type("macos"), "darwin");
        assert_eq!(otel_os_type("dragonfly"), "dragonflybsd");
        // Already-conformant values are unchanged.
        assert_eq!(otel_os_type("linux"), "linux");
        assert_eq!(otel_os_type("windows"), "windows");
        // No standardized equivalent: passed through as-is.
        assert_eq!(otel_os_type("ios"), "ios");
        assert_eq!(otel_os_type("android"), "android");
    }

    #[test]
    fn host_arch_remaps_and_passes_through() {
        assert_eq!(otel_host_arch("x86_64"), "amd64");
        assert_eq!(otel_host_arch("aarch64"), "arm64");
        assert_eq!(otel_host_arch("arm"), "arm32");
        assert_eq!(otel_host_arch("powerpc"), "ppc32");
        assert_eq!(otel_host_arch("powerpc64"), "ppc64");
        // Already-conformant values are unchanged.
        assert_eq!(otel_host_arch("x86"), "x86");
        assert_eq!(otel_host_arch("s390x"), "s390x");
        // No standardized equivalent: passed through as-is.
        assert_eq!(otel_host_arch("riscv64"), "riscv64");
    }

    /// The live host's arch/os must map to a value the OpenTelemetry spec actually defines (this
    /// catches a Rust target whose spelling we have not remapped).
    #[test]
    fn current_host_is_conformant() {
        const OS_TYPES: &[&str] = &[
            "windows",
            "linux",
            "darwin",
            "freebsd",
            "netbsd",
            "openbsd",
            "dragonflybsd",
            "hpux",
            "aix",
            "solaris",
            "z_os",
            "zos",
        ];
        const HOST_ARCHES: &[&str] = &[
            "amd64", "arm32", "arm64", "ia64", "ppc32", "ppc64", "s390x", "x86",
        ];

        // These hold on every platform buck2 currently builds for; a new target that needs a remap
        // would trip one of them rather than silently emitting a non-conformant value.
        assert!(
            OS_TYPES.contains(&otel_os_type(std::env::consts::OS)),
            "os.type {:?} is not an OpenTelemetry-defined value",
            otel_os_type(std::env::consts::OS)
        );
        assert!(
            HOST_ARCHES.contains(&otel_host_arch(std::env::consts::ARCH)),
            "host.arch {:?} is not an OpenTelemetry-defined value",
            otel_host_arch(std::env::consts::ARCH)
        );
    }
}
