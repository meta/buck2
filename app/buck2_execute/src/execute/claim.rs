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
use std::sync::Arc;

use async_trait::async_trait;
use derivative::Derivative;
use derive_more::Display;
use dupe::Dupe;
use tokio::sync::Mutex;
use tokio::sync::OwnedMutexGuard;

#[async_trait]
pub trait ClaimManager: Send + Sync + 'static {
    /// Acquire a claim. the Claim mamanger is normally internally mutable and will likely lock out
    /// other executions from acquiring a claim.
    ///
    /// Actions *must* acquire a claim before writing to outputs for an action or declaring
    /// output artifacts in the materializer.
    ///
    /// The expectation is that multiple actions that attempt to write to the same output directory
    /// should not be allowed to obtain claims concurrently.
    async fn claim(self: Box<Self>) -> Box<dyn Claim>;

    /// Inform the manager that producing a result might take longer than expected.
    ///
    /// The manager may use this information to notify other parts of the system. We use this notably
    /// to unblock local execution if RE reports long queueing.
    fn on_result_delayed(&mut self);
}

pub trait Claim: Send + Sync + fmt::Debug + 'static {
    /// Release a claim. This can be done to make it available again after acquiring it. Once the
    /// claim is released, the claim holder should no longer write any output.
    fn release(self: Box<Self>) -> buck2_error::Result<()>;
}

#[derive(Debug)]
pub enum ClaimStatus {
    // No claim has been attempted, or one was attempted but abandoned.
    NotClaimed,
    // A claim has been made.
    Claimed,
}

#[derive(Dupe, Clone)]
pub struct MutexClaimManager {
    mutex: Arc<Mutex<ClaimStatus>>,
}

impl MutexClaimManager {
    pub fn new() -> Self {
        Self {
            mutex: Arc::new(Mutex::new(ClaimStatus::NotClaimed)),
        }
    }
}

#[async_trait]
impl ClaimManager for MutexClaimManager {
    async fn claim(self: Box<Self>) -> Box<dyn Claim> {
        let mut guard = self.mutex.dupe().lock_owned().await;
        match *guard {
            ClaimStatus::NotClaimed => {}
            ClaimStatus::Claimed => futures::future::pending().await,
        }
        *guard = ClaimStatus::Claimed;
        Box::new(MutexClaim { guard })
    }

    fn on_result_delayed(&mut self) {}
}

#[derive(Display, Derivative)]
#[display("MutexClaim")]
#[derivative(Debug)]
pub struct MutexClaim {
    // No point in printing this as it will *always* be Claimed.
    #[derivative(Debug = "ignore")]
    guard: OwnedMutexGuard<ClaimStatus>,
}

impl Claim for MutexClaim {
    fn release(mut self: Box<Self>) -> buck2_error::Result<()> {
        *self.guard = ClaimStatus::NotClaimed;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use std::task::Poll;

    use super::*;

    #[tokio::test]
    async fn test_mutex_claim_release() {
        let claim_manager = MutexClaimManager::new();
        let claim = Box::new(claim_manager.dupe()).claim().await;

        let claim2 = Box::new(claim_manager.dupe()).claim();
        futures::pin_mut!(claim2);
        assert_matches!(futures::poll!(claim2.as_mut()), Poll::Pending);

        claim.release().expect("Can release claim");
        assert_matches!(futures::poll!(claim2.as_mut()), Poll::Ready(..));
    }
}
