/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;

#[derive(Allocative, PartialEq, Eq, Debug)]
pub enum DiceEvent {
    /// Key evaluation started.
    Started { key_type: &'static str },

    /// Key evaluation finished.
    Finished { key_type: &'static str },

    /// Checking dependencies has started.
    CheckDepsStarted { key_type: &'static str },

    /// Checking dependencies has finished.
    CheckDepsFinished { key_type: &'static str },

    /// Compute has started.
    ComputeStarted { key_type: &'static str },

    /// Compute has finished.
    ComputeFinished { key_type: &'static str },

    /// Reading a paged-out value back from disk failed. Reported for telemetry;
    /// carries the failing key's type and the error message. `transient` is set
    /// when the read failed without reaching the data, a storage lock wait that
    /// ran out or a read refused on the calling thread, rather than on the data
    /// itself; see [`pagable::is_transient_read_error`].
    HydrationFailed {
        key_type: &'static str,
        error: String,
        transient: bool,
    },
}

pub trait DiceEventListener: Allocative + Send + Sync + 'static {
    fn event(&self, ev: DiceEvent);
}
