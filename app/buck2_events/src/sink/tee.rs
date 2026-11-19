/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use async_trait::async_trait;

use crate::Event;
use crate::EventSink;

/// A Sink implementation that wraps two EventSinks and sends events to both of them.
pub struct TeeSink<A, B>(A, B);

impl<A, B> TeeSink<A, B> {
    pub fn new(a: A, b: B) -> TeeSink<A, B> {
        TeeSink(a, b)
    }
}

#[async_trait]
impl<A: EventSink, B: EventSink> EventSink for TeeSink<A, B> {
    fn send(&self, event: Event) {
        self.0.send(event.clone());
        self.1.send(event);
    }

    async fn send_now(&self, event: Event) {
        let event_clone = event.clone();
        self.0.send_now(event_clone).await;
        self.1.send_now(event).await;
    }
}
