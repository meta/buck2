/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use crate::threads::available_parallelism;

pub struct UnixSystemStats {
    pub load1: f64,
    pub load5: f64,
    pub load15: f64,
}

impl UnixSystemStats {
    #[cfg(unix)]
    pub fn get() -> Option<Self> {
        let mut loadavg: [f64; 3] = [0.0, 0.0, 0.0];
        if unsafe { libc::getloadavg(&mut loadavg[0], 3) } != 3 {
            // This doesn't seem to set errno (or at least it's not documented to do so).
            return None;
        }
        Some(Self {
            load1: loadavg[0],
            load5: loadavg[1],
            load15: loadavg[2],
        })
    }

    #[cfg(not(unix))]
    pub fn get() -> Option<Self> {
        None
    }
}

/// Returns the number of CPU cores on the system.
#[cfg(unix)]
pub fn num_cores() -> usize {
    use std::sync::OnceLock;
    static NUM_CORES: OnceLock<usize> = OnceLock::new();
    *NUM_CORES.get_or_init(|| {
        let n = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
        if n < 1 {
            available_parallelism()
        } else {
            n as usize
        }
    })
}

#[cfg(not(unix))]
pub fn num_cores() -> usize {
    available_parallelism()
}

pub fn system_memory_stats() -> u64 {
    if let Ok(Some(bytes)) = buck2_env::env::buck2_env!("BUCK2_TEST_FAKE_SYSTEM_TOTAL_MEMORY", type=u64, applicability=testing)
    {
        return bytes;
    }

    use sysinfo::MemoryRefreshKind;
    use sysinfo::RefreshKind;
    use sysinfo::System;

    let system = System::new_with_specifics(
        RefreshKind::nothing().with_memory(MemoryRefreshKind::nothing().with_ram()),
    );
    system.total_memory()
}

/// The memory limit of the container this process runs in, as published by the container's
/// launcher, or `None` outside such containers. Inside one, [`system_memory_stats`] reports the
/// host's memory instead.
pub fn container_memory_limit_bytes() -> Option<u64> {
    buck2_env::env::buck2_env!("FAAS_CONTAINER_MEM_SOFT_LIMIT", type=u64, applicability=internal)
        .ok()
        .flatten()
}

// `sysinfo` calls `sysconf`s that Miri does not implement.
#[cfg(all(test, not(miri)))]
mod tests {
    use super::system_memory_stats;

    #[test]
    fn get_system_memory_stats() {
        let total_mem = system_memory_stats();
        // sysinfo returns zero when fails to retrieve data
        assert!(total_mem > 0);
    }
}
