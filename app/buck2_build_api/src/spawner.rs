/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::any::Any;

use allocative::Allocative;
use buck2_common::events::HasEvents;
use buck2_events::dispatch::with_dispatcher_async;
use dice_futures::spawner::Spawner;
use dupe::Dupe;
use futures::future::BoxFuture;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;

#[derive(Allocative)]
pub struct BuckSpawner {
    #[allocative(skip)]
    rt: Handle,
}

impl BuckSpawner {
    pub fn new(rt: Handle) -> Self {
        Self { rt }
    }

    pub fn current_runtime() -> Option<Self> {
        Some(Self {
            rt: Handle::try_current().ok()?,
        })
    }
}

impl<T: HasEvents> Spawner<T> for BuckSpawner {
    fn spawn(
        &self,
        ctx: &T,
        fut: BoxFuture<'static, Box<dyn Any + Send + 'static>>,
    ) -> JoinHandle<Box<dyn Any + Send + 'static>> {
        let dispatcher = ctx.get_dispatcher().dupe();
        let task = async move { with_dispatcher_async(dispatcher, fut).await };
        self.rt.spawn(task)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use buck2_data::CommandEnd;
    use buck2_data::CommandStart;
    use buck2_events::BuckEvent;
    use buck2_events::create_source_sink_pair;
    use buck2_events::daemon_id::DaemonId;
    use buck2_events::dispatch::EventDispatcher;
    use buck2_events::dispatch::span;
    use buck2_events::source::ChannelEventSource;
    use buck2_wrapper_common::invocation_id::TraceId;
    use dice::DiceData;
    use dice::UserComputationData;
    use dice_futures::spawn::spawn_dropcancel;
    use futures::future::FutureExt;

    use super::*;

    async fn next_event(source: &mut ChannelEventSource) -> BuckEvent {
        source.receive().unwrap().unpack_buck().unwrap().clone()
    }

    fn create_ctx(dispatcher: EventDispatcher) -> UserComputationData {
        let dice_data = {
            let mut data = DiceData::new();
            data.set(dispatcher);
            data
        };
        UserComputationData {
            data: dice_data,
            ..Default::default()
        }
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
    async fn test_spawn() {
        let sp = BuckSpawner::current_runtime().unwrap();

        // Create dispatcher
        let (mut events, sink) = create_source_sink_pair();
        let trace_id = TraceId::new();
        let dispatcher = EventDispatcher::new(trace_id.dupe(), DaemonId::new(), sink);

        let ctx = create_ctx(dispatcher);
        let (start, end) = create_start_end_events();

        let task = async { span(start, || (Box::new(()) as _, end)) }.boxed();

        sp.spawn(&ctx, task).await.expect("Task panicked");

        // Check that the events are received successfully & correctly.
        let event = next_event(&mut events).await;
        let span_id = event.span_id().unwrap();
        assert_eq!(event.trace_id().unwrap(), trace_id);
        assert!(event.parent_id().is_none());

        let end = next_event(&mut events).await;
        assert_eq!(end.span_id().unwrap(), span_id);
    }

    #[tokio::test]
    async fn test_spawn_task() {
        let sp = Arc::new(BuckSpawner::current_runtime().unwrap());

        // Create dispatchers
        let (mut events1, sink1) = create_source_sink_pair();
        let trace_id1 = TraceId::new();
        let dispatcher1 = EventDispatcher::new(trace_id1.dupe(), DaemonId::new(), sink1);

        let (mut events2, sink2) = create_source_sink_pair();
        let trace_id2 = TraceId::new();
        let dispatcher2 = EventDispatcher::new(trace_id2.dupe(), DaemonId::new(), sink2);

        let ctx1 = create_ctx(dispatcher1);
        let ctx2 = create_ctx(dispatcher2);
        let (start1, end1) = create_start_end_events();
        let (start2, end2) = create_start_end_events();

        // Create tasks with separate dispatchers
        let task1 = async {
            span(start1, || ((), end1));
            "Hello!"
        }
        .boxed();

        let task2 = async {
            span(start2, || ((), end2));
            "World!"
        }
        .boxed();

        let poll1 = spawn_dropcancel(|_| task1.boxed(), sp.as_ref(), &ctx1);
        let poll2 = spawn_dropcancel(|_| task2.boxed(), sp.as_ref(), &ctx2);
        let joins = vec![poll1, poll2];

        assert_eq!(
            buck2_util::future::join_all(joins).await,
            ["Hello!", "World!"]
        );

        // Check that the events are received successfully from the correct event sources.
        let event = next_event(&mut events1).await;
        let span_id = event.span_id().unwrap();
        assert_eq!(event.trace_id().unwrap(), trace_id1);
        assert!(event.parent_id().is_none());

        let end = next_event(&mut events1).await;
        assert_eq!(end.span_id().unwrap(), span_id);

        let event = next_event(&mut events2).await;
        let span_id = event.span_id().unwrap();
        assert_eq!(event.trace_id().unwrap(), trace_id2);
        assert!(event.parent_id().is_none());

        let end = next_event(&mut events2).await;
        assert_eq!(end.span_id().unwrap(), span_id);
    }
}
