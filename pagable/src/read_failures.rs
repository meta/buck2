/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::time::Duration;

use parking_lot::Mutex;

use crate::storage::data::DataKey;
use crate::traits::StorageState;

/// Deferred reads that failed on stored data, kept for the storage's lifetime.
///
/// A value paged back in keeps deferred fields that read storage on demand.
/// When such a read fails the value's owner is already resident and cannot be
/// repaired in place, unlike an eager page-in, which falls back to
/// recomputing. A host that finds entries here can no longer trust the values
/// it holds, and keeps finding them until it is replaced. A read that failed
/// without reaching the data (see [`is_transient_read_error`]) is not an entry:
/// the field stays unread and a retry can succeed.
#[derive(Default)]
pub struct DeferredReadFailures {
    failed: Mutex<Vec<String>>,
}

impl StorageState for DeferredReadFailures {}

impl DeferredReadFailures {
    /// Records that reading `what` failed with `error`, unless the failure is
    /// transient.
    pub fn record_error(&self, what: impl fmt::Display, error: &anyhow::Error) {
        if is_transient_read_error(error) {
            return;
        }
        self.failed.lock().push(format!("{what}: {error:#}"));
    }

    /// Every failure recorded so far.
    pub fn snapshot(&self) -> Vec<String> {
        self.failed.lock().clone()
    }
}

/// Whether `error` says the read failed rather than the data, so a retry can
/// succeed. Anything not typed as such counts as a failure of the data.
pub fn is_transient_read_error(error: &anyhow::Error) -> bool {
    error.downcast_ref::<ReadTimedOut>().is_some() || error.downcast_ref::<ReadRefused>().is_some()
}

/// Why a deferred field could not be read: the error of
/// [`DeferredValue::read`](crate::DeferredValue::read).
///
/// A type of its own, so that a caller's error type can convert from it and
/// say what failed. Converting it to [`crate::Error`] gives the underlying
/// error unchanged, so [`is_transient_read_error`] and downcasts see what the
/// read failed with.
pub struct DeferredReadError(crate::Error);

impl From<crate::Error> for DeferredReadError {
    fn from(error: crate::Error) -> Self {
        Self(error)
    }
}

impl From<ReadRefused> for DeferredReadError {
    fn from(error: ReadRefused) -> Self {
        Self(crate::Error::new(error))
    }
}

impl fmt::Debug for DeferredReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl fmt::Display for DeferredReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

// Deliberately not a `std::error::Error`. If it were, `crate::Error` would
// convert from it by wrapping it, and the underlying error would be out of
// reach of a downcast.
impl From<DeferredReadError> for crate::Error {
    fn from(error: DeferredReadError) -> Self {
        error.0
    }
}

/// A read that gave up waiting for storage.
#[derive(Debug)]
pub struct ReadTimedOut {
    pub key: DataKey,
    pub waited: Duration,
    pub attempts: u32,
    /// What the read was waiting for.
    pub waiting_for: &'static str,
}

impl fmt::Display for ReadTimedOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "read of key {:?} gave up after {} waits of {:?} for {}",
            self.key, self.attempts, self.waited, self.waiting_for
        )
    }
}

impl std::error::Error for ReadTimedOut {}

/// A read declined because of the calling thread's state, before it reached
/// storage.
#[derive(Debug)]
pub struct ReadRefused(pub &'static str);

impl fmt::Display for ReadRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for ReadRefused {}

#[cfg(test)]
mod tests {
    use super::*;

    fn timed_out() -> anyhow::Error {
        anyhow::Error::new(ReadTimedOut {
            key: DataKey::testing_new(1),
            waited: Duration::from_secs(1),
            attempts: 2,
            waiting_for: "a lock",
        })
    }

    #[test]
    fn transient_read_errors_are_not_recorded() {
        let failures = DeferredReadFailures::default();
        failures.record_error("arc a", &timed_out().context("while restoring"));
        failures.record_error("arc b", &anyhow::Error::new(ReadRefused("refused")));
        assert!(failures.snapshot().is_empty(), "{:?}", failures.snapshot());

        failures.record_error("arc c", &anyhow::anyhow!("no row"));
        assert_eq!(failures.snapshot(), ["arc c: no row"]);
    }

    #[test]
    fn deferred_read_error_shows_and_converts_to_the_underlying_error() {
        let error = DeferredReadError::from(timed_out().context("while restoring"));
        assert_eq!(error.to_string(), "while restoring");
        assert!(
            format!("{error:#}").starts_with("while restoring: read of key "),
            "{error:#}"
        );
        let error: anyhow::Error = error.into();
        assert!(
            is_transient_read_error(&error),
            "the conversion does not hide what the read failed with"
        );
    }
}
