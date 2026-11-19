/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use crate::host_swap::HostSwap;

/// The fraction of the host's swap that has to stay free.
///
/// oomd does not kill for the host being low on swap until less than this is free.
const HOST_SWAP_FREE_FLOOR: f64 = 0.1;

/// How far the host's swap usage is from the point at which oomd may kill buck for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SwapHeadroom {
    /// The amount of swap that can be in use before that point is reached.
    pub(crate) capacity: u64,
    /// How much more swap can be used before that point is reached.
    pub(crate) available: u64,
}

impl SwapHeadroom {
    /// `None` if the host has no swap, and can therefore not run out of it.
    pub(crate) fn new(host_swap: HostSwap) -> Option<Self> {
        let capacity = ((1.0 - HOST_SWAP_FREE_FLOOR) * host_swap.total as f64) as u64;
        (capacity > 0).then_some(Self {
            capacity,
            available: capacity.saturating_sub(host_swap.used()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

    /// A host with 100GB of swap, and therefore a capacity of 90GB
    fn host(used_gb: u64) -> Option<HostSwap> {
        Some(HostSwap {
            total: 100 * GB,
            free: (100 - used_gb) * GB,
        })
    }

    #[test]
    fn test_headroom() {
        assert_eq!(
            host(60).and_then(SwapHeadroom::new),
            Some(SwapHeadroom {
                capacity: 90 * GB,
                available: 30 * GB,
            })
        );
        assert_eq!(
            host(95).and_then(SwapHeadroom::new),
            Some(SwapHeadroom {
                capacity: 90 * GB,
                available: 0,
            })
        );
    }

    #[test]
    fn test_headroom_of_host_without_swap() {
        assert_eq!(SwapHeadroom::new(HostSwap { total: 0, free: 0 }), None);
    }
}
