/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use dashmap::DashMap;
use futures_intrusive::sync::SharedSemaphore;

const SINGLE_WORKER: usize = 1;

#[derive(Default)]
pub struct NamedSemaphores {
    buckets: DashMap<String, SharedSemaphore>,
}

impl NamedSemaphores {
    pub fn new() -> NamedSemaphores {
        Default::default()
    }

    pub fn get(&self, name: &str) -> SharedSemaphore {
        // Fast path avoids allocating a `String` key on the hot path.
        if let Some(bucket_semaphore) = self.buckets.get(name) {
            return bucket_semaphore.clone();
        }
        // Fair semaphore: slightly faster wakeups for single-permit requests.
        self.buckets
            .entry(name.to_owned())
            .or_insert_with(|| SharedSemaphore::new(true, SINGLE_WORKER))
            .value()
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::NamedSemaphores;

    #[test]
    fn test_named_semaphore_works() {
        let named_semaphore = NamedSemaphores::new();
        let identifier = "testing".to_owned();
        {
            let first_permit = named_semaphore.get(&identifier);
            assert!(first_permit.try_acquire(1).is_some());
        }
        {
            let second_permit = named_semaphore.get(&identifier);
            assert!(second_permit.try_acquire(1).is_some());
        }
    }

    #[test]
    fn test_multiple_locks_fail() {
        let named_semaphore = NamedSemaphores::new();
        let identifier = "testing".to_owned();
        let first_permit = named_semaphore.get(&identifier);
        let first_permit_acquire = first_permit.try_acquire(1);
        assert!(first_permit_acquire.is_some());
        let second_permit = named_semaphore.get(&identifier);
        assert!(second_permit.try_acquire(1).is_none());
    }
}
