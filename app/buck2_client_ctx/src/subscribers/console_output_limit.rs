/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::buck2_env;

/// Result of attempting to emit output through the limit tracker.
pub(crate) enum EmitResult {
    /// Output is within the limit; the caller should emit it.
    Emit,
    /// The limit was just exceeded; the caller should emit the contained message.
    Exceeded(&'static str),
    /// The limit was already exceeded and the notice was already shown; skip.
    Skipped,
}

/// Tracks cumulative action/test output bytes and enforces an optional global
/// limit configured via `BUCK2_CONSOLE_OUTPUT_LIMIT` (in bytes).
pub(crate) struct ConsoleOutputLimit {
    limit: Option<u64>,
    bytes_emitted: u64,
    exceeded_printed: bool,
}

impl ConsoleOutputLimit {
    pub(crate) fn new() -> Self {
        let limit = buck2_env!("BUCK2_CONSOLE_OUTPUT_LIMIT", type=u64).unwrap_or(None);
        Self {
            limit,
            bytes_emitted: 0,
            exceeded_printed: false,
        }
    }

    /// Consume `byte_count` bytes of the output budget and return what the
    /// caller should do.
    pub(crate) fn emit(&mut self, byte_count: usize) -> EmitResult {
        let Some(limit) = self.limit else {
            return EmitResult::Emit;
        };

        self.bytes_emitted = self.bytes_emitted.saturating_add(byte_count as u64);

        if self.bytes_emitted <= limit {
            EmitResult::Emit
        } else if !self.exceeded_printed {
            self.exceeded_printed = true;
            EmitResult::Exceeded("(output limit exceeded)")
        } else {
            EmitResult::Skipped
        }
    }
}
