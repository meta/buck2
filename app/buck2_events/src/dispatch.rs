/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Mechanism for dispatching events. Where the rubber meets the road, for producers of events.
//!
//! The [`EventDispatcher`] is a type-erased, dupe-able container for a [`crate::EventSink`]. It is intended to be
//! liberally duplicated and passed around to the depths of buck2 so that consumers can insert events into it.

use std::cell::Cell;
use std::cell::UnsafeCell;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::pin::Pin;
use std::sync::Arc;
use std::task;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;

use buck2_core::error::SoftErrorContext;
use buck2_core::event::EventDispatch;
use buck2_data::SpanEndEvent;
use buck2_data::SpanStartEvent;
use buck2_data::buck_event;
use buck2_data::span_end_event;
use buck2_data::span_start_event;
use buck2_wrapper_common::invocation_id::TraceId;
use dupe::Dupe;
use futures::Future;
use pin_project::pin_project;
use smallvec::SmallVec;

use crate::BuckEvent;
use crate::Event;
use crate::EventSink;
use crate::daemon_id::DaemonId;
use crate::sink::null::NullEventSink;
use crate::span::SpanId;

/// A type-erased and dupe-able container for EventSinks, containing some additional metadata useful for all events
/// emitted through the dispatcher.
#[derive(Clone, Dupe, Allocative)]
pub struct EventDispatcher {
    /// The Trace ID of the current trace.
    trace_id: TraceId,
    daemon_id: DaemonId,
    /// The sink to log events to.
    #[allocative(skip)] // TODO(nga): do not skip.
    sink: Arc<dyn EventSink>,
    #[allocative(skip)]
    soft_error_context: Option<Arc<SoftErrorContext>>,
}

impl EventDispatcher {
    /// Creates a new Event Dispatcher using the given TraceId and logging events to the given sink.
    pub fn new<T: EventSink + 'static>(
        trace_id: TraceId,
        daemon_id: DaemonId,
        sink: T,
    ) -> EventDispatcher {
        EventDispatcher {
            trace_id,
            daemon_id,
            sink: Arc::new(sink),
            soft_error_context: None,
        }
    }

    /// Associates command-scoped soft-error policy and rate-limit state with this dispatcher.
    pub fn with_soft_error_context(mut self, context: Arc<SoftErrorContext>) -> Self {
        self.soft_error_context = Some(context);
        self
    }

    /// Returns the command-scoped soft-error context, if this dispatcher has one.
    pub fn soft_error_context(&self) -> Option<Arc<SoftErrorContext>> {
        self.soft_error_context.clone()
    }

    pub fn sink(&self) -> Arc<dyn EventSink> {
        self.sink.dupe()
    }

    /// Creates a new null Event Dispatcher that accepts events but does not write them anywhere.
    pub fn null() -> EventDispatcher {
        EventDispatcher {
            trace_id: TraceId::null(),
            daemon_id: DaemonId::null(),
            sink: Arc::new(NullEventSink::new()),
            soft_error_context: None,
        }
    }

    /// Creates an Event Dispatcher that reports a soft_error if any event is
    /// dispatched through it. Used as a sentinel for code paths that should not
    /// emit events; if they do, it means a real EventDispatcher needs to be
    /// threaded through from the caller/requestor.
    pub fn error_on_event() -> EventDispatcher {
        use crate::sink::error_on_event::ErrorOnEventSink;
        EventDispatcher {
            trace_id: TraceId::null(),
            daemon_id: DaemonId::null(),
            sink: Arc::new(ErrorOnEventSink),
            soft_error_context: None,
        }
    }

    /// Emits an event annotated with the current trace ID.
    pub fn buck_event(&self, data: buck_event::Data) {
        self.event_with_span_id(data, None, current_span());
    }

    /// Emits an InstantEvent annotated with the current trace ID
    pub fn instant_event<E: Into<buck2_data::instant_event::Data>>(&self, data: E) {
        let instant = buck2_data::InstantEvent {
            data: Some(data.into()),
        };
        self.event_with_span_id(instant, None, current_span());
    }

    pub fn console_message(&self, message: String) {
        self.instant_event(buck2_data::ConsoleMessage { message })
    }

    pub fn console_warning(&self, message: String) {
        self.instant_event(buck2_data::ConsoleWarning { message })
    }

    pub fn streaming_output(&self, message: String) {
        self.instant_event(buck2_data::StdoutStreamingOutput { message })
    }

    /// Like `instant_event`, but bypasses internal buffering. See [`EventSink::send_now`].
    pub async fn instant_event_send_now<E: Into<buck2_data::instant_event::Data>>(&self, data: E) {
        let instant = buck2_data::InstantEvent {
            data: Some(data.into()),
        };
        let now = SystemTime::now();
        let event = BuckEvent::new(
            now,
            self.trace_id.dupe(),
            None,
            current_span(),
            instant.into(),
        );
        self.sink.send_now(Event::Buck(event)).await;
    }

    fn event_with_span_id<E: Into<buck_event::Data>>(
        &self,
        data: E,
        span_id: Option<SpanId>,
        parent_id: Option<SpanId>,
    ) {
        let now = SystemTime::now();

        let event = BuckEvent::new(now, self.trace_id.dupe(), span_id, parent_id, data.into());
        self.sink.send(Event::Buck(event));
    }

    pub fn partial_result(&self, data: buck2_cli_proto::PartialResult) {
        self.sink.send(Event::PartialResult(data));
    }

    pub fn command_result(&self, data: buck2_cli_proto::CommandResult) {
        self.sink.send(Event::CommandResult(Box::new(data)));
    }

    /// Introduces a new span and immediately fires the given start event. When the given synchronous function returns,
    /// the span is closed and the end event emitted.
    pub fn span<Start, End, F, R>(&self, start: Start, func: F) -> R
    where
        Start: Into<span_start_event::Data>,
        End: Into<span_end_event::Data>,
        F: FnOnce() -> (R, End),
    {
        let mut span = Span::start(self.dupe(), start);
        let (result, end) = span.call_in_span(func);
        span.end(end);
        result
    }

    /// Introduces a new span and immediately fires the given start event. When the given future resolves,  the span is
    /// closed and the event is emitted. This span is a "suspending span"; it is intended to suspend and resume whenever
    /// the future itself is suspended and resumed, respectively.
    ///
    /// TODO(swgillespie) actually implement suspend/resume
    pub fn span_async<Start, End, Fut, R>(
        &self,
        start: Start,
        fut: Fut,
    ) -> impl Future<Output = R> + use<Start, End, Fut, R>
    where
        Start: Into<span_start_event::Data>,
        End: Into<span_end_event::Data>,
        Fut: Future<Output = (R, End)>,
    {
        Span::start(self.dupe(), start).wrap_future(fut)
    }

    /// Creates a new span but does not enter it. Use `wrap_future` or `wrap_closure` to enter it,
    /// or create children spans via `create_child` and enter those.
    pub fn create_span<Start>(&self, start: Start) -> Span
    where
        Start: Into<span_start_event::Data>,
    {
        Span::start(self.dupe(), start)
    }

    /// Returns the traceid for this event dispatcher.
    pub fn trace_id(&self) -> &TraceId {
        &self.trace_id
    }

    pub fn daemon_id(&self) -> &DaemonId {
        &self.daemon_id
    }
}

pub struct EventDispatcherLateBinding;

impl EventDispatch for EventDispatcherLateBinding {
    fn emit_instant_event_for_data(&self, data: buck2_data::instant_event::Data) {
        get_dispatcher().instant_event(data);
    }
}

// While this may be simpler to implement via futures::poll_fn or some kind of async fn/block or similar, we need
// to ensure that we don't explode the size of our futures and so we implement the future explicitly.
#[pin_project]
struct SpanAsync<End, Fut, R>
where
    End: Into<span_end_event::Data>,
    Fut: Future<Output = (R, End)>,
{
    #[pin]
    fut: Fut,
    span: Span,
}

impl<End, Fut, R> Future for SpanAsync<End, Fut, R>
where
    End: Into<span_end_event::Data>,
    Fut: Future<Output = (R, End)>,
{
    type Output = R;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        let this = self.project();
        let span: &mut _ = this.span;
        match span.call_in_span(|| this.fut.poll(cx)) {
            task::Poll::Ready((v, end)) => {
                span.send(end.into());
                task::Poll::Ready(v)
            }
            task::Poll::Pending => task::Poll::Pending,
        }
    }
}

/// A [`Future`] wrapper that captures the current [`SpanId`] at construction
/// time and proxies it into each [`Future::poll`] call via
/// [`maybe_proxy_current_span`]. This is the async counterpart to the
/// synchronous `maybe_proxy_current_span`, allowing span context to follow
/// futures that may be polled on a different tokio task or thread.
#[pin_project]
pub struct SpanProxyAsync<Fut, R>
where
    Fut: Future<Output = R>,
{
    #[pin]
    fut: Fut,
    span_id: Option<SpanId>,
}

impl<Fut, R> SpanProxyAsync<Fut, R>
where
    Fut: Future<Output = R>,
{
    pub fn new(fut: Fut) -> Self {
        SpanProxyAsync {
            fut,
            span_id: current_span(),
        }
    }

    pub fn new_with_span(fut: Fut, span_id: Option<SpanId>) -> Self {
        SpanProxyAsync { fut, span_id }
    }
}

impl<Fut, R> Future for SpanProxyAsync<Fut, R>
where
    Fut: Future<Output = R>,
{
    type Output = R;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        let this = self.project();
        let span_id = *this.span_id;
        maybe_proxy_current_span(span_id, || this.fut.poll(cx))
    }
}

impl Debug for EventDispatcher {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "EventDispatcher(TraceId = {})", self.trace_id().dupe())
    }
}

#[derive(Default, Allocative)]
struct SpanStats {
    max_poll_time: Duration,
    total_poll_time: Duration,
}

/// Utility to track the start & end of a span
#[must_use = "You must end a Span you started"]
#[derive(Allocative)]
pub struct Span {
    dispatcher: EventDispatcher,
    start_instant: Instant,
    span_id: SpanId,
    stats: SpanStats,
    parent_id: Option<SpanId>,
    sent: bool,
    /// Whether we recorded (in any active span recorder) that this span was entered.
    recorded: bool,
}

impl Span {
    pub fn start<D>(dispatcher: EventDispatcher, data: D) -> Self
    where
        D: Into<span_start_event::Data>,
    {
        Self::start_impl(dispatcher, data, current_span())
    }

    fn start_impl<D>(dispatcher: EventDispatcher, data: D, parent_id: Option<SpanId>) -> Self
    where
        D: Into<span_start_event::Data>,
    {
        let span_id = SpanId::next();
        let start_instant = Instant::now();

        dispatcher.event_with_span_id(
            SpanStartEvent {
                data: Some(data.into()),
            },
            Some(span_id),
            parent_id,
        );

        Self {
            dispatcher,
            start_instant,
            span_id,
            parent_id,
            stats: Default::default(),
            sent: false,
            recorded: false,
        }
    }

    pub fn end<D>(mut self, data: D)
    where
        D: Into<span_end_event::Data>,
    {
        self.send(data.into());
    }

    pub fn span_id(&self) -> SpanId {
        self.span_id
    }

    pub fn wrap_future<End, Fut, R>(self, fut: Fut) -> impl Future<Output = R>
    where
        End: Into<span_end_event::Data>,
        Fut: Future<Output = (R, End)>,
    {
        SpanAsync { fut, span: self }
    }

    pub fn wrap_closure<End, Fun, R>(mut self, fun: Fun) -> R
    where
        End: Into<span_end_event::Data>,
        Fun: FnOnce() -> (R, End),
    {
        let (r, end) = self.call_in_span(fun);
        self.send(end.into());
        r
    }

    pub fn create_child(&self, data: impl Into<span_start_event::Data>) -> Span {
        Span::start_impl(self.dispatcher.dupe(), data, Some(self.span_id))
    }

    fn call_in_span<T, F>(&mut self, f: F) -> T
    where
        F: FnOnce() -> T,
    {
        let now = Instant::now();

        if !self.recorded {
            with_thread_local_recorder(|tl_recorder| {
                if let Some(tl_recorder) = tl_recorder.as_mut() {
                    tl_recorder.push(self.span_id);
                }
            });

            self.recorded = true;
        }

        let ret = unset_thread_local_recorder(|| {
            CURRENT_SPAN.with(|tl_span| {
                let previous_span = tl_span.replace(Some(self.span_id));
                let ret = f();
                tl_span.set(previous_span);
                ret
            })
        });

        let elapsed = Instant::now() - now;
        self.stats.max_poll_time = std::cmp::max(elapsed, self.stats.max_poll_time);
        self.stats.total_poll_time += elapsed;

        ret
    }

    fn send(&mut self, data: buck2_data::span_end_event::Data) {
        self.sent = true;

        self.dispatcher.event_with_span_id(
            SpanEndEvent {
                duration: (Instant::now() - self.start_instant).try_into().ok(),
                data: Some(data),
                stats: Some(buck2_data::SpanStats {
                    max_poll_time_us: self.stats.max_poll_time.as_micros() as u64,
                    total_poll_time_us: self.stats.total_poll_time.as_micros() as u64,
                }),
            },
            Some(self.span_id),
            self.parent_id,
        );
    }
}

impl Drop for Span {
    /// When dropping a Span, send a SpanEndEvent. We send SpanCancelled in Drop to ensure that
    /// even if we never `end()` a Span, we notify clients (if any exist).
    fn drop(&mut self) {
        if !self.sent {
            self.send(buck2_data::SpanCancelled {}.into())
        }
    }
}

struct RestoreSpan<'a> {
    tl_span: &'a Cell<Option<SpanId>>,
    previous_span: Option<SpanId>,
}

impl Drop for RestoreSpan<'_> {
    fn drop(&mut self) {
        self.tl_span.set(self.previous_span);
    }
}

/// Temporarily installs a captured `SpanId` as the `CURRENT_SPAN` on the
/// current thread while executing `fun`. This is used to propagate span
/// context from a parent thread into worker threads (e.g. the IO executor),
/// so that actions executed on those threads appear under the correct span.
///
/// If the current thread already has its own span set, the proxied span is
/// **not** installed (don't-clobber semantics). The thread-local
/// `RootSpansRecorder` is also cleared for the duration to prevent the worker
/// thread from accidentally recording spans belonging to the caller.
///
/// No-op when `span_id` is `None`.
pub fn maybe_proxy_current_span<T, Fun>(span_id: Option<SpanId>, fun: Fun) -> T
where
    Fun: FnOnce() -> T,
{
    match span_id {
        Some(span_id) => unset_thread_local_recorder(|| {
            CURRENT_SPAN.with(|tl_span| {
                let previous_span = tl_span.replace(Some(span_id));
                if previous_span.is_some() {
                    // This thread already has its own span; don't override it.
                    tl_span.set(previous_span);
                }
                let _guard = RestoreSpan {
                    tl_span,
                    previous_span,
                };
                fun()
            })
        }),
        None => fun(),
    }
}

thread_local! {
    static CURRENT_SPAN: Cell<Option<SpanId>> = const { Cell::new(None) };
}

use allocative::Allocative;
use buck2_core::buck2_env;

tokio::task_local! {
    pub static EVENTS: EventDispatcher;
}

/// Invokes function func, setting the dispatcher to the task_local for the duration
/// of the call (and for any downstream events).
pub fn with_dispatcher<R, F>(dispatcher: EventDispatcher, func: F) -> R
where
    F: FnOnce() -> R,
{
    EVENTS.sync_scope(dispatcher, func)
}

/// Invokes `func` with an optional captured dispatcher, preserving the absence of command context.
pub fn with_dispatcher_opt<R, F>(dispatcher: Option<EventDispatcher>, func: F) -> R
where
    F: FnOnce() -> R,
{
    match dispatcher {
        Some(dispatcher) => with_dispatcher(dispatcher, func),
        None => func(),
    }
}

// Wraps the Future fut with a TaskLocalFuture that sets the task_local dispatcher and the current_span (if any) before polling fut.
pub fn with_dispatcher_async<F, R>(dispatcher: EventDispatcher, fut: F) -> impl Future<Output = R>
where
    F: Future<Output = R>,
{
    EVENTS.scope(dispatcher, SpanProxyAsync::new(fut))
}

/// Polls `fut` with an optional captured dispatcher, preserving absent command context.
pub async fn with_dispatcher_opt_async<F, R>(dispatcher: Option<EventDispatcher>, fut: F) -> R
where
    F: Future<Output = R>,
{
    match dispatcher {
        Some(dispatcher) => with_dispatcher_async(dispatcher, fut).await,
        None => fut.await,
    }
}

/// Get the ambient dispatcher, if one is available (and None otherwise). In contexts that aren't
/// tied to a specific command (e.g. materializer command loop), an ambient dispatcher may not be
/// available.
pub fn get_dispatcher_opt() -> Option<EventDispatcher> {
    EVENTS.try_with(|dispatcher| dispatcher.dupe()).ok()
}

pub fn get_dispatcher() -> EventDispatcher {
    let enforce_event_dispatcher_set = buck2_env!("ENFORCE_DISPATCHER_SET", bool).unwrap();

    match get_dispatcher_opt() {
        Some(dispatcher) => dispatcher,
        None => {
            if enforce_event_dispatcher_set {
                panic!("dispatcher is not set")
            } else {
                // TODO: This is firing millions of times, needs to fix this up before it's made a soft error.
                // soft_error!(buck2_error::buck2_error!([], "Task local event dispatcher not set."));
                EventDispatcher::null()
            }
        }
    }
}

pub fn current_span() -> Option<SpanId> {
    CURRENT_SPAN.with(|tl_span| tl_span.get())
}

/// Introduces a new span and immediately fires the given start event. When the given synchronous function returns,
/// the span is closed and the end event emitted.
pub fn span<Start, End, F, R>(start: Start, func: F) -> R
where
    Start: Into<span_start_event::Data>,
    End: Into<span_end_event::Data>,
    F: FnOnce() -> (R, End),
{
    get_dispatcher().span(start, func)
}

/// Simpler version of `span` where the end event
/// can be constructed without requiring the result of the function.
pub fn span_simple<Start, End, F, R>(start: Start, func: F, end: End) -> R
where
    Start: Into<span_start_event::Data>,
    End: Into<span_end_event::Data>,
    F: FnOnce() -> R,
{
    span(start, || (func(), end))
}

/// Emits an InstantEvent annotated with the current trace ID
pub fn instant_event<E: Into<buck2_data::instant_event::Data>>(data: E) {
    get_dispatcher().instant_event(data)
}

/// Send console message from the server.
pub fn console_message(message: String) {
    get_dispatcher().console_message(message)
}

/// Send console warning from the server.
pub fn console_warning(message: String) {
    get_dispatcher().console_warning(message)
}

/// Introduces a new span and immediately fires the given start event. When the given future resolves,  the span is
/// closed and the event is emitted. This span is a "suspending span"; it is intended to suspend and resume whenever
/// the future itself is suspended and resumed, respectively.
///
/// TODO(swgillespie) actually implement suspend/resume
pub fn span_async<Start, End, Fut, R>(start: Start, fut: Fut) -> impl Future<Output = R>
where
    Start: Into<span_start_event::Data>,
    End: Into<span_end_event::Data>,
    Fut: Future<Output = (R, End)>,
{
    get_dispatcher().span_async(start, fut)
}

/// Simpler version of `span_async` where the end event
/// can be constructed without requiring the result of the future.
pub fn span_async_simple<Start, End, Fut, R>(
    start: Start,
    fut: Fut,
    end: End,
) -> impl Future<Output = R>
where
    Start: Into<span_start_event::Data>,
    End: Into<span_end_event::Data>,
    Fut: Future<Output = R>,
{
    span_async(start, async { (fut.await, end) })
}

/// To use when wrapping via span() and span_async is not convenient. This produces a Span guard
/// that must be ended. The span is not automatically entered.
pub fn create_span(start: impl Into<span_start_event::Data>) -> Span {
    get_dispatcher().create_span(start)
}

/// Allows the caller to record root spans being created in their current context.
#[derive(Default)]
struct RootSpansRecorder {
    spans: SmallVec<[SpanId; 1]>,
}

impl RootSpansRecorder {
    fn new() -> Self {
        Self {
            spans: SmallVec::new(),
        }
    }

    fn push(&mut self, span_id: SpanId) {
        self.spans.push(span_id);
    }

    fn call_with_recorder<T, F>(&mut self, f: F) -> T
    where
        F: FnOnce() -> T,
    {
        let this_recorder = std::mem::take(self);

        unset_thread_local_recorder(|| {
            with_thread_local_recorder(|tl_recorder| *tl_recorder = Some(this_recorder));
            let v = f();
            // Unwrap safety: only this unset_thread_local_recorder ever unsets this, and it
            // promises to put it back.
            *self = with_thread_local_recorder(|tl_recorder| tl_recorder.take().unwrap());
            v
        })
    }
}

fn unset_thread_local_recorder<F, O>(f: F) -> O
where
    F: FnOnce() -> O,
{
    struct RestoreRecorder {
        previous_recorder: Option<RootSpansRecorder>,
    }

    impl Drop for RestoreRecorder {
        fn drop(&mut self) {
            with_thread_local_recorder(|tl_recorder| {
                *tl_recorder = std::mem::take(&mut self.previous_recorder);
            })
        }
    }

    let previous_recorder = with_thread_local_recorder(std::mem::take);
    let _guard = RestoreRecorder { previous_recorder };
    f()
}

fn with_thread_local_recorder<F, O>(f: F) -> O
where
    for<'a> F: FnOnce(&'a mut Option<RootSpansRecorder>) -> O,
{
    thread_local! {
        static ROOT_SPAN_RECORDER: UnsafeCell<Option<RootSpansRecorder>> = const { UnsafeCell::new(None) };
    }

    // SAFETY: Nobody can possibly hold a reference to the contents of this cell, since the thread
    // local is defined within this function (so nobody besies this function can hold a reference
    // to *that*), and the reference we pass ends when this function ends.
    ROOT_SPAN_RECORDER.with(|tl_recorder| {
        let recorder = unsafe { &mut *tl_recorder.get() };
        f(recorder)
    })
}

pub fn record_root_spans<F, R>(f: F) -> (R, SmallVec<[SpanId; 1]>)
where
    F: FnOnce() -> R,
{
    let mut recorder = RootSpansRecorder::new();
    let v = recorder.call_with_recorder(f);
    (v, recorder.spans)
}

#[pin_project]
pub struct RootSpansRecordingFuture<Fut> {
    recorder: RootSpansRecorder,
    #[pin]
    fut: Fut,
}

impl<Fut> Future for RootSpansRecordingFuture<Fut>
where
    Fut: Future,
{
    type Output = (<Fut as Future>::Output, SmallVec<[SpanId; 1]>);

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        let this = self.project();
        let v = futures::ready!(this.recorder.call_with_recorder(|| this.fut.poll(cx)));
        task::Poll::Ready((v, std::mem::take(&mut this.recorder.spans)))
    }
}

pub fn async_record_root_spans<Fut>(fut: Fut) -> RootSpansRecordingFuture<Fut> {
    RootSpansRecordingFuture {
        recorder: RootSpansRecorder::new(),
        fut,
    }
}

#[cfg(test)]
mod tests {
    use buck2_core::error::SoftErrorContext;
    use buck2_data::CommandEnd;
    use buck2_data::CommandStart;
    use tokio::task::JoinHandle;

    use super::*;
    use crate::sink::channel::ChannelEventSink;
    use crate::source::ChannelEventSource;

    async fn next_event(source: &mut ChannelEventSource) -> BuckEvent {
        source.receive().unwrap().unpack_buck().unwrap().clone()
    }

    fn create_dispatcher() -> (EventDispatcher, ChannelEventSource, TraceId) {
        let (send, recv) = crossbeam_channel::unbounded();
        let source = ChannelEventSource::new(recv);

        let sink = ChannelEventSink::new(send);
        let trace_id = TraceId::new();
        let dispatcher = EventDispatcher::new(trace_id.dupe(), DaemonId::new(), sink);

        (dispatcher, source, trace_id)
    }

    fn create_start_end_events() -> (CommandStart, CommandEnd) {
        let start = CommandStart {
            ..Default::default()
        };
        let end = CommandEnd {
            ..Default::default()
        };

        (start, end)
    }

    #[tokio::test]
    async fn soft_error_context_propagates_through_spawn() -> buck2_error::Result<()> {
        let context = Arc::new(SoftErrorContext::new("", "")?);
        let dispatcher = EventDispatcher::null().with_soft_error_context(context.dupe());

        let observed = tokio::spawn(with_dispatcher_async(dispatcher, async {
            get_dispatcher_opt().and_then(|dispatcher| dispatcher.soft_error_context())
        }))
        .await?;

        assert!(observed.is_some_and(|observed| Arc::ptr_eq(&context, &observed)));
        Ok(())
    }

    #[tokio::test]
    async fn send_event_smoke() {
        let (dispatcher, mut source, trace_id) = create_dispatcher();
        let (start, _) = create_start_end_events();

        dispatcher.buck_event(
            SpanStartEvent {
                data: Some(start.into()),
            }
            .into(),
        );

        let event = next_event(&mut source).await;
        assert_eq!(event.trace_id().unwrap(), trace_id);
        assert!(event.parent_id.is_none());
        assert!(event.span_id.is_none());
    }

    #[tokio::test]
    async fn send_event_with_span() {
        let (dispatcher, mut source, trace_id) = create_dispatcher();
        let (start, end) = create_start_end_events();

        dispatcher.span(start, || ((), end));

        let event = next_event(&mut source).await;
        let span_id = event.span_id.unwrap();
        assert_eq!(event.trace_id().unwrap(), trace_id);
        assert!(event.parent_id.is_none());

        let end = next_event(&mut source).await;
        assert_eq!(end.span_id.unwrap(), span_id);
    }

    #[tokio::test]
    async fn send_event_with_span_async() {
        let (dispatcher, mut source, trace_id) = create_dispatcher();
        let (start, end) = create_start_end_events();

        dispatcher.span_async(start, async { ((), end) }).await;

        let event = next_event(&mut source).await;
        let span_id = event.span_id.unwrap();
        assert_eq!(event.trace_id().unwrap(), trace_id);
        assert!(event.parent_id.is_none());

        let end = next_event(&mut source).await;
        assert_eq!(end.span_id.unwrap(), span_id);
    }

    #[tokio::test]
    async fn send_event_with_nested_span() {
        let (dispatcher, mut source, _) = create_dispatcher();
        let (start, end) = create_start_end_events();

        dispatcher.span(start.clone(), || {
            dispatcher.span(start.clone(), || ((), end.clone()));
            ((), end.clone())
        });

        let e1 = next_event(&mut source).await;
        let e2 = next_event(&mut source).await;
        let e3 = next_event(&mut source).await;
        let e4 = next_event(&mut source).await;

        assert_eq!(e1.span_id, e4.span_id);
        assert_eq!(e2.span_id, e3.span_id);
        assert_eq!(e2.parent_id, e1.span_id);
    }

    #[tokio::test]
    async fn send_event_with_nested_span_async() {
        let (dispatcher, mut source, _) = create_dispatcher();
        let (start, end) = create_start_end_events();

        dispatcher
            .span_async(start.clone(), async {
                dispatcher
                    .span_async(start.clone(), async { ((), end.clone()) })
                    .await;
                ((), end.clone())
            })
            .await;

        let e1 = next_event(&mut source).await;
        let e2 = next_event(&mut source).await;
        let e3 = next_event(&mut source).await;
        let e4 = next_event(&mut source).await;

        assert_eq!(e1.span_id, e4.span_id);
        assert_eq!(e2.span_id, e3.span_id);
        assert_eq!(e2.parent_id, e1.span_id);
    }

    #[tokio::test]
    async fn send_event_with_nested_span_explicit() {
        let (dispatcher, mut source, _trace_id) = create_dispatcher();
        let (start, end) = create_start_end_events();

        let top = dispatcher.create_span(start.clone());
        let middle = top.create_child(start.clone());
        middle.wrap_closure(|| {
            (
                dispatcher.span(start.clone(), || ((), end.clone())),
                end.clone(),
            )
        });
        top.end(end);

        let enter_top = next_event(&mut source).await;
        let enter_middle = next_event(&mut source).await;
        let enter_bottom = next_event(&mut source).await;
        let exit_bottom = next_event(&mut source).await;
        let exit_middle = next_event(&mut source).await;
        let exit_top = next_event(&mut source).await;

        assert_eq!(enter_top.span_id, exit_top.span_id);
        assert_eq!(enter_middle.span_id, exit_middle.span_id);
        assert_eq!(enter_bottom.span_id, exit_bottom.span_id);

        assert_eq!(enter_middle.parent_id.unwrap(), enter_top.span_id.unwrap());
        assert_eq!(
            enter_bottom.parent_id.unwrap(),
            enter_middle.span_id.unwrap()
        );
    }

    #[test]
    fn test_span_stats() {
        let (dispatcher, _, _) = create_dispatcher();
        let (start, _) = create_start_end_events();

        let mut span = Span::start(dispatcher.dupe(), start);
        span.call_in_span(|| std::thread::sleep(Duration::from_millis(10)));
        assert_eq!(span.stats.max_poll_time, span.stats.total_poll_time);

        span.call_in_span(|| std::thread::sleep(Duration::from_millis(10)));
        assert!(span.stats.max_poll_time < span.stats.total_poll_time);
    }

    async fn dispatcher_task(
        dispatcher: EventDispatcher,
        start: CommandStart,
        end: CommandEnd,
    ) -> Result<u32, ()> {
        let mut err = false;
        let mut events = 0u32;

        async fn event(
            start: CommandStart,
            end: CommandEnd,
            events: &mut u32,
            trace_id: &TraceId,
            err: &mut bool,
        ) {
            async fn yield_and_check(trace_id: &TraceId, err: &mut bool) {
                tokio::task::yield_now().await;
                *err |= get_dispatcher().trace_id != *trace_id;
            }

            // 2*2 for Start and End events, for 2 spans
            *events += 4;

            span_async(start.clone(), async {
                for _ in 0..4 {
                    yield_and_check(trace_id, err).await;
                }
                // TODO(mufeez): re-consider adding recursive calls to event
                span_async(start.clone(), async {
                    for _ in 0..4 {
                        yield_and_check(trace_id, err).await;
                    }
                    ((), end.clone())
                })
                .await;
                ((), end.clone())
            })
            .await
        }

        with_dispatcher_async(
            dispatcher.dupe(),
            event(
                start.clone(),
                end.clone(),
                &mut events,
                &dispatcher.trace_id,
                &mut err,
            ),
        )
        .await;

        if err { Err(()) } else { Ok(events) }
    }

    // Test function used in test_concurrent_single_thread and
    // test_concurrent_multi_thread.
    async fn test_concurrent() {
        let (events1, source1, trace_id1) = create_dispatcher();
        let (events2, source2, trace_id2) = create_dispatcher();

        let mut event_sources: [ChannelEventSource; 2] = [source1, source2];
        let trace_ids: [TraceId; 2] = [trace_id1, trace_id2];

        let (start1, end1) = create_start_end_events();
        let (start2, end2) = create_start_end_events();

        let handles: [JoinHandle<Result<u32, ()>>; 2] = [
            tokio::spawn(dispatcher_task(events1, start1, end1)),
            tokio::spawn(dispatcher_task(events2, start2, end2)),
        ];

        let mut sent_counts = Vec::new();

        for handle in handles {
            match handle.await {
                Ok(result) => {
                    sent_counts.push(result.unwrap());
                }
                Err(e) => assert!(!e.is_panic()),
            }
        }

        for (i, sent) in sent_counts.iter().enumerate() {
            for _ in 0..*sent {
                let e = next_event(&mut event_sources[i]).await;
                assert!(e.trace_id().unwrap() == trace_ids[i]);
            }
        }
    }

    #[tokio::test]
    async fn test_concurrent_single_thread() {
        test_concurrent().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_concurrent_multi_thread() {
        test_concurrent().await;
    }

    fn with_span(f: impl FnOnce()) -> SpanId {
        span(CommandStart::default(), || {
            let id = current_span().unwrap();
            f();
            (id, CommandEnd::default())
        })
    }

    #[test]
    fn test_record_root_spans() {
        let nop = || ();

        // Test a basic case.
        let (id, spans) = record_root_spans(|| with_span(nop));
        assert_eq!(&*spans, &[id]);

        // Test multiple spans.
        let (ids, spans) = record_root_spans(|| {
            let id1 = with_span(nop);
            let id2 = with_span(nop);
            vec![id1, id2]
        });
        assert_eq!(&*spans, &ids);

        // Test we don't record nested spans.
        let (id, spans) = record_root_spans(|| {
            with_span(|| {
                span(CommandStart::default(), || {
                    (with_span(nop), CommandEnd::default())
                });
            })
        });
        assert_eq!(&*spans, &[id]);

        // Test nested calls to record_root_spans work.
        let (id, spans) = record_root_spans(|| {
            with_span(|| {
                let (id, spans) = record_root_spans(|| with_span(nop));
                assert_eq!(&*spans, &[id]);
            })
        });
        assert_eq!(&*spans, &[id]);
    }

    #[tokio::test]
    async fn test_async_record_root_spans() {
        let nop = || ();

        // Easy case.
        let (id, spans) = async_record_root_spans(async { with_span(nop) }).await;
        assert_eq!(&*spans, &[id]);

        // Easy case (2).
        let (id, spans) = async_record_root_spans(span_async(CommandStart::default(), async {
            (current_span().unwrap(), CommandEnd::default())
        }))
        .await;
        assert_eq!(&*spans, &[id]);

        // With yields, check we don't re-enter.
        let (ids, spans) = async_record_root_spans(async {
            let id1 = span_async(CommandStart::default(), async {
                tokio::task::yield_now().await;
                (current_span().unwrap(), CommandEnd::default())
            })
            .await;

            tokio::task::yield_now().await;

            let id2 = span_async(CommandStart::default(), async {
                tokio::task::yield_now().await;
                (current_span().unwrap(), CommandEnd::default())
            })
            .await;

            vec![id1, id2]
        })
        .await;
        assert_eq!(&*spans, &ids);
    }
}
