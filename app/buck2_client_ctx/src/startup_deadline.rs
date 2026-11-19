/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cmp;
use std::future::Future;
use std::time::Duration;
use std::time::Instant;

use buck2_common::client_utils::retrying;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;

/// Utility to time out properly with context during buck2 client startup.
///
/// The problem is this. Consider three stacked operations: `A -> B -> C`.
/// And we need to apply timeout to the whole operation.
/// If we apply timeout to `A`, we time out properly, but won't be able to provide
/// information that failure happened during `B -> C`.
///
/// So we should timeout in each of `A`, `B`, `C` separately.
/// Additionally, when going down, we should decrease timeout a bit,
/// to make sure inner operation times out before outer one.
#[derive(Debug)]
pub struct StartupDeadline {
    deadline: Instant,
}

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = ClientStartupTimeout)]
enum StartupDeadlineError {
    #[error("timed out before {op}")]
    TimeoutBefore { op: String },
    #[error("{} timed out after {:.3}s", op, rem_duration.as_secs_f32())]
    TimeoutDuring { op: String, rem_duration: Duration },
}

impl StartupDeadline {
    pub fn duration_from_now(duration: Duration) -> buck2_error::Result<StartupDeadline> {
        Ok(StartupDeadline {
            deadline: Instant::now()
                .checked_add(duration)
                .internal_error("overflow")?,
        })
    }

    pub(crate) fn deadline(&self) -> Instant {
        self.deadline
    }

    /// Deadline for a nested operation.
    ///
    /// Must be lower than outer deadline to make sure inner operation times out before outer one.
    pub(crate) fn down_deadline(&self) -> buck2_error::Result<StartupDeadline> {
        let new_deadline = self
            .deadline
            .checked_sub(Duration::from_millis(100))
            .internal_error("deadline underflow")?;
        Ok(StartupDeadline {
            deadline: new_deadline,
        })
    }

    /// How much time is left for the current operation.
    ///
    /// A deadline exactly equal to the current time counts as expired. Coarse monotonic
    /// clock granularity (notably on Windows) otherwise makes a zero timeout nondeterministic:
    /// whether the outermost or an inner operation reports the timeout would depend on
    /// whether the clock ticked between constructing the deadline and checking it.
    pub(crate) fn rem_duration(&self, op: &str) -> buck2_error::Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|rem| !rem.is_zero())
            .ok_or(StartupDeadlineError::TimeoutBefore { op: op.to_owned() }.into())
    }

    /// Decrease the deadline by 100ms and invoke the given function with the new deadline.
    pub(crate) async fn down<R, Fut, F>(&self, op: &str, f: F) -> buck2_error::Result<R>
    where
        F: FnOnce(StartupDeadline) -> Fut,
        Fut: Future<Output = buck2_error::Result<R>>,
    {
        let rem_duration = self.rem_duration(op)?;
        let res = tokio::time::timeout_at(
            tokio::time::Instant::from_std(self.deadline),
            f(self.down_deadline()?),
        )
        .await;

        match res {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(e.context(op.to_owned())),
            Err(_elapsed) => Err(StartupDeadlineError::TimeoutDuring {
                op: op.to_owned(),
                rem_duration,
            }
            .into()),
        }
    }

    pub async fn run<R, Fut>(&self, op: &str, f: Fut) -> buck2_error::Result<R>
    where
        Fut: Future<Output = buck2_error::Result<R>>,
    {
        self.down(op, |_| f).await
    }

    /// Round current deadline down to the given duration added to the current time.
    pub(crate) fn min(&self, other: Duration) -> buck2_error::Result<StartupDeadline> {
        let other = Instant::now()
            .checked_add(other)
            .internal_error("overflow")?;
        Ok(StartupDeadline {
            deadline: cmp::min(self.deadline, other),
        })
    }

    /// Create a deadline object for the half of remaining time.
    pub(crate) fn half(&self) -> buck2_error::Result<StartupDeadline> {
        let now = Instant::now();
        let duration = self.deadline.duration_since(now) / 2;
        let deadline = now
            .checked_add(duration)
            .internal_error("duration overflow")?;
        Ok(StartupDeadline { deadline })
    }

    pub(crate) async fn retrying<R, F, Fut>(
        &self,
        op: &str,
        initial_delay: Duration,
        max_delay: Duration,
        f: F,
    ) -> buck2_error::Result<R>
    where
        Fut: Future<Output = buck2_error::Result<R>>,
        F: Fn() -> Fut,
    {
        retrying(initial_delay, max_delay, self.rem_duration(op)?, f)
            .await
            .with_buck_error_context(|| op.to_owned())
    }
}
