/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::soft_error_or_panic;
use buck2_error::buck2_error;

use crate::Event;
use crate::EventSink;

/// An EventSink that reports a soft_error if any event is dispatched through it.
///
/// Used as a sentinel for code paths that should not be emitting events. If an
/// event is dispatched, it means a caller failed to thread through a real
/// EventDispatcher.
pub struct ErrorOnEventSink;

impl EventSink for ErrorOnEventSink {
    fn send(&self, event: Event) {
        // Report via soft_error so the missing dispatcher is logged in
        // production and caught as a hard failure in tests with
        // soft_error enforcement.
        soft_error_or_panic!(
            "event_dispatched_without_dispatcher",
            buck2_error!(
                buck2_error::ErrorTag::InternalError,
                "Event dispatched through a code path that lacks a real EventDispatcher. \
                 An EventDispatcher needs to be threaded through from the caller. \
                 Event: {:?}",
                event
            )
        );
    }
}
