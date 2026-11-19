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
use std::iter;
use std::sync::Arc;

use allocative::Allocative;
use anyhow::Context;
use futures_intrusive::sync::SharedSemaphore;
use futures_intrusive::sync::SharedSemaphoreReleaser;
use pagable::Pagable;
use starlark_map::sorted_vec::SortedVec;

use crate::NamedSemaphores;

const SINGLE_RUN: usize = 1;

/// This class is intended to represent the resources required by each test. This is then used to
/// map onto resources available on the machine where the tests are run on in order to not saturate the
/// machine and adversely impact testrunning performance and reliability.
/// Testpilot Classic uses the concept of machine cores to manage resources. Testpilot has two types of
/// tests: Normal (1 core) and Heavy (4 cores) where this would run on a machine with typically 24 cores
/// (managed through a semaphore with size 24). Since Testpilot was introduced we have moved to run
/// on Sandcastle machines with 56 cores so we want to move away from the core-analogy and instead use
/// the term "permits" to describe the limited resources available on each machine.
/// More long term we want improve this to also take into account memory usage, cpu usage etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Allocative, Hash, Pagable)]
pub enum WeightClass {
    /// Tests can require any number of permits and this can be used to mimic resource utilization like
    /// memory or cpu. For now, we map the Testpilot behaviour as Normal->Permits(1) and Heavy->Permits(4).
    Permits(usize),
    /// A percentage of available resources.
    Percentage(WeightPercentage),
}

impl fmt::Display for WeightClass {
    fn fmt(&self, w: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Permits(p) => write!(w, "{p}"),
            Self::Percentage(p) => write!(w, "{}%", p.into_value()),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Allocative, Hash, Pagable)]
pub struct WeightPercentage {
    value: u8, // Between 0 and 100
}

impl WeightPercentage {
    pub fn try_new<T, E>(value: T) -> anyhow::Result<Self>
    where
        u8: TryFrom<T, Error = E>,
        E: Into<anyhow::Error>,
    {
        let value = u8::try_from(value)
            .map_err(|e| e.into())
            .context("WeightPercentage value must convert to u8")?;

        if value > 100 {
            return Err(anyhow::anyhow!(
                "WeightPercentage value cannot exceed 100: {}",
                value
            ));
        }

        Ok(Self { value })
    }

    pub fn into_value(self) -> u8 {
        self.value
    }
}

/// Some commands require that we only run one instance of this binary (using an identifier)
/// to check for other instances of the same binary.
/// Some commands required the full host to run, others just dont care.
/// This enum encapsulates all the different scenarios.
#[derive(Debug, Clone, PartialEq, Eq, Allocative, Hash, Pagable)]
pub enum HostSharingRequirements {
    /// Needs exclusive access to the host. No other processes should run.
    ExclusiveAccess,
    /// Can share with other processes, but not with others requiring the same token
    OnePerToken(String, WeightClass),
    /// Can share with other processes, but not with any others requiring any of the same tokens
    OnePerTokens(SortedVec<String>, WeightClass),
    /// Run with any other processes within reasonable limits.
    Shared(WeightClass),
}

impl fmt::Display for HostSharingRequirements {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self {
            HostSharingRequirements::ExclusiveAccess => write!(f, "ExclusiveAccess"),
            HostSharingRequirements::OnePerToken(name, class) => {
                write!(f, "OnePerToken({name},{class})")
            }
            HostSharingRequirements::OnePerTokens(names, class) => {
                write!(f, "OnePerTokens({names:?},{class})")
            }
            HostSharingRequirements::Shared(class) => write!(f, "Shared({class})"),
        }
    }
}

impl Default for HostSharingRequirements {
    fn default() -> HostSharingRequirements {
        HostSharingRequirements::Shared(WeightClass::Permits(1))
    }
}

/// A guard for all permits and resources acquired for a HostSharingBroker.acquire request.
/// Keeps the data structures received from semaphores after acquiring.
/// Semaphores are held until this struct is dropped.
pub struct HostSharingGuard {
    _run_guard: SharedSemaphoreReleaser,
    _name_guards: Vec<SharedSemaphoreReleaser>,
}

/// Used to ensure that host resources are properly reserved before executing a command spec.
pub struct HostSharingBroker {
    permits: SharedSemaphore,
    num_machine_permits: usize,
    named_semaphores: Arc<NamedSemaphores>,
}

pub struct RequestedPermits {
    count: usize,
    cap: usize,
}

impl RequestedPermits {
    pub fn into_count(self) -> usize {
        self.count.min(self.cap)
    }

    pub fn into_count_uncapped(self) -> usize {
        self.count
    }
}

impl HostSharingBroker {
    // If a test requires Permits(4) permits but the machine only has 3 permits then we cap the
    // test's required permits to 3. Otherwise the test would never be allowed to run.
    pub fn requested_permits(&self, weight_class: &WeightClass) -> RequestedPermits {
        let count = match weight_class {
            WeightClass::Permits(required_permits) => *required_permits,
            WeightClass::Percentage(percentage) => {
                let percentage: usize = percentage.into_value().into();
                (self.num_machine_permits * percentage).div_ceil(100)
            }
        };

        RequestedPermits {
            count,
            cap: self.num_machine_permits,
        }
    }

    pub fn new_with_named_semaphores(
        host_sharing_strategy: HostSharingStrategy,
        num_machine_permits: usize,
        named_semaphores: Arc<NamedSemaphores>,
    ) -> Self {
        let permits = match host_sharing_strategy {
            HostSharingStrategy::Fifo => SharedSemaphore::new(true, num_machine_permits),
            HostSharingStrategy::SmallerTasksFirst => {
                SharedSemaphore::new(false, num_machine_permits)
            }
        };

        Self {
            permits,
            num_machine_permits,
            named_semaphores,
        }
    }

    pub fn new(host_sharing_strategy: HostSharingStrategy, num_machine_permits: usize) -> Self {
        Self::new_with_named_semaphores(
            host_sharing_strategy,
            num_machine_permits,
            Arc::new(NamedSemaphores::new()),
        )
    }

    pub fn num_machine_permits(&self) -> usize {
        self.num_machine_permits
    }

    pub async fn acquire(
        &self,
        host_sharing_requirements: &HostSharingRequirements,
    ) -> HostSharingGuard {
        match host_sharing_requirements {
            HostSharingRequirements::Shared(weight_class) => {
                let permits = self.requested_permits(weight_class).into_count();
                self.acquire_from_permits_and_identifiers(permits, iter::empty())
                    .await
            }
            HostSharingRequirements::ExclusiveAccess => {
                self.acquire_from_permits_and_identifiers(self.num_machine_permits, iter::empty())
                    .await
            }
            HostSharingRequirements::OnePerToken(identifier, weight_class) => {
                let permits = self.requested_permits(weight_class).into_count();
                self.acquire_from_permits_and_identifiers(permits, iter::once(identifier))
                    .await
            }
            HostSharingRequirements::OnePerTokens(sorted_identifiers, weight_class) => {
                let permits = self.requested_permits(weight_class).into_count();
                self.acquire_from_permits_and_identifiers(permits, sorted_identifiers.iter())
                    .await
            }
        }
    }

    async fn acquire_from_permits_and_identifiers<'a>(
        &self,
        num_requested_permits: usize,
        sorted_identifiers: impl Iterator<Item = &'a String>,
    ) -> HostSharingGuard {
        // Ensure that there is only one active run per identifier. The identifiers must be sorted
        // to avoid a potential deadlock. A repeated identifier is skipped: its semaphore has a
        // single permit, and this request already holds it.
        //
        // Acquire the identifier semaphores first, then acquire the permits needed to actually run.
        // This is so that no permits (which map to system resources / cores) are held while waiting
        // for the previous runs on these identifiers to finish.
        let mut name_guards = Vec::new();
        let mut previous_identifier: Option<&String> = None;
        for identifier in sorted_identifiers {
            if previous_identifier == Some(identifier) {
                continue;
            }
            previous_identifier = Some(identifier);
            let run_semaphore = self.named_semaphores.get(identifier);
            let name_guard = run_semaphore.acquire(SINGLE_RUN).await;
            name_guards.push(name_guard);
        }
        let run_guard = self.permits.acquire(num_requested_permits).await;

        HostSharingGuard {
            _run_guard: run_guard,
            _name_guards: name_guards,
        }
    }
}

/// Determines whether a fair or unfair semaphore is used to manage host sharing
pub enum HostSharingStrategy {
    SmallerTasksFirst,
    Fifo,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    // if we only have 2 machine permits then even a test requiring 4 permits will be capped to only require 2 permits
    // (otherwise it would not run)
    fn test_heavyweight_capped_to_machine_permits() {
        let broker = HostSharingBroker::new(HostSharingStrategy::SmallerTasksFirst, 2);

        let permits = broker
            .requested_permits(&WeightClass::Permits(4))
            .into_count();
        assert_eq!(2, permits);

        let permits = broker
            .requested_permits(&WeightClass::Permits(4))
            .into_count_uncapped();
        assert_eq!(4, permits);
    }

    #[test]
    fn test_one_per_tokens_with_repeated_token() {
        use std::future::Future;
        use std::task::Context;
        use std::task::Waker;

        let broker = HostSharingBroker::new(HostSharingStrategy::Fifo, 1);
        let mut cx = Context::from_waker(Waker::noop());
        let requirements = |tokens: &[&str]| {
            HostSharingRequirements::OnePerTokens(
                SortedVec::from(tokens.iter().map(|t| (*t).to_owned()).collect::<Vec<_>>()),
                WeightClass::Permits(1),
            )
        };

        // Nothing is held, so distinct tokens are acquired on the first poll.
        let distinct = requirements(&["a", "b"]);
        assert!(
            std::pin::pin!(broker.acquire(&distinct))
                .as_mut()
                .poll(&mut cx)
                .is_ready()
        );

        // A repeated token is acquired once, so the acquire completes on the first poll.
        let repeated = requirements(&["a", "b", "a"]);
        let guard = match std::pin::pin!(broker.acquire(&repeated))
            .as_mut()
            .poll(&mut cx)
        {
            std::task::Poll::Ready(guard) => guard,
            std::task::Poll::Pending => panic!("acquire with a repeated token must complete"),
        };

        // The token is held exactly once: another acquire waits until the guard is dropped.
        let other = requirements(&["a"]);
        let mut other_acquire = std::pin::pin!(broker.acquire(&other));
        assert!(other_acquire.as_mut().poll(&mut cx).is_pending());
        drop(guard);
        assert!(other_acquire.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn test_percentage() {
        let broker = HostSharingBroker::new(HostSharingStrategy::SmallerTasksFirst, 10);

        assert_eq!(
            broker
                .requested_permits(&WeightClass::Percentage(WeightPercentage { value: 0 }))
                .into_count(),
            0,
        );

        assert_eq!(
            broker
                .requested_permits(&WeightClass::Percentage(WeightPercentage { value: 40 }))
                .into_count(),
            4,
        );

        // This rounds up.
        assert_eq!(
            broker
                .requested_permits(&WeightClass::Percentage(WeightPercentage { value: 15 }))
                .into_count(),
            2,
        );

        assert_eq!(
            broker
                .requested_permits(&WeightClass::Percentage(WeightPercentage { value: 99 }))
                .into_count(),
            10,
        );

        assert_eq!(
            broker
                .requested_permits(&WeightClass::Percentage(WeightPercentage { value: 100 }))
                .into_count(),
            10,
        );
    }
}
