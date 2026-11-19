/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::num::NonZeroU64;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use allocative::Allocative;
use buck2_error::BuckErrorOptionContext;
use dupe::Dupe;
use serde::Serialize;

/// A SpanId is a unique identifier for a span, which is a pair of events that represent a conceptual start and stop
/// of a particular operation.
#[derive(
    Debug,
    Copy,
    Clone,
    Dupe,
    Serialize,
    PartialEq,
    Eq,
    Hash,
    derive_more::Display,
    Allocative
)]
pub struct SpanId(pub NonZeroU64);

impl SpanId {
    pub fn from_u64(span_id: u64) -> buck2_error::Result<SpanId> {
        SpanId::from_u64_opt(span_id).internal_error("zero span id")
    }

    pub fn from_u64_opt(span_id: u64) -> Option<SpanId> {
        NonZeroU64::new(span_id).map(SpanId)
    }

    /// Generates a new SpanId, suitable for identifying a particular span within the context of a trace. Span IDs are
    /// increasing nonzero 64-bit integers.
    pub fn next() -> SpanId {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        loop {
            let next_id = NEXT_ID.fetch_add(1, Ordering::AcqRel);
            if let Some(id) = NonZeroU64::new(next_id) {
                return SpanId(id);
            }
            // 64-bit wrap around; continue the loop to generate the next non-zero ID.
        }
    }
}

impl From<SpanId> for u64 {
    fn from(span_id: SpanId) -> Self {
        span_id.0.into()
    }
}
