/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fs::File;
use std::os::unix::fs::FileExt;
use std::sync::Arc;

use dupe::Dupe;

/// Swap space of the host, in bytes.
///
/// Inside a container this still describes the machine the container runs on, not any swap limit
/// placed on the container itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostSwap {
    pub total: u64,
    pub free: u64,
}

impl HostSwap {
    pub(crate) fn used(&self) -> u64 {
        self.total.saturating_sub(self.free)
    }
}

/// Reads [`HostSwap`]; meant for callers that poll it.
pub(crate) struct HostSwapReader(Arc<File>);

impl HostSwapReader {
    /// `None` on systems that don't have a `/proc/meminfo`.
    pub(crate) fn open() -> Option<Self> {
        Some(Self(Arc::new(File::open("/proc/meminfo").ok()?)))
    }

    /// `None` if the read fails or the kernel does not report swap.
    pub(crate) async fn read(&self) -> Option<HostSwap> {
        let file = self.0.dupe();
        tokio::task::spawn_blocking(move || {
            // A read at offset zero returns as much of the file as fits. In practice that is all
            // of it, and the fields about swap are near the top regardless.
            let mut buf = [0u8; 4096];
            let len = file.read_at(&mut buf, 0).ok()?;
            parse_meminfo(&String::from_utf8_lossy(&buf[..len]))
        })
        .await
        .ok()?
    }
}

fn parse_meminfo(meminfo: &str) -> Option<HostSwap> {
    let bytes = |field: &str| {
        meminfo.lines().find_map(|line| {
            let mut parts = line.split_whitespace();
            if parts.next()?.strip_suffix(':')? != field {
                return None;
            }
            let value = parts.next()?.parse::<u64>().ok()?;
            // Despite the spelling, these are units of 1024 bytes
            (parts.next()? == "kB").then_some(value * 1024)
        })
    };
    Some(HostSwap {
        total: bytes("SwapTotal")?,
        free: bytes("SwapFree")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_meminfo() {
        let meminfo = "\
MemTotal:       131382852 kB
MemFree:         5311556 kB
SwapCached:       181488 kB
Mlocked:               0 kB
SwapTotal:      67108860 kB
SwapFree:       44739240 kB
Zswap:           1535724 kB
HugePages_Total:       0
";
        assert_eq!(
            parse_meminfo(meminfo),
            Some(HostSwap {
                total: 67108860 * 1024,
                free: 44739240 * 1024,
            })
        );
    }

    #[test]
    fn test_parse_meminfo_without_swap_fields() {
        assert_eq!(parse_meminfo("MemTotal:       131382852 kB\n"), None);
        assert_eq!(parse_meminfo(""), None);
    }

    #[tokio::test]
    async fn test_read() {
        let Some(reader) = HostSwapReader::open() else {
            return;
        };
        // Machines without swap report zero for both, so there is nothing to assert about the
        // magnitudes.
        let swap = reader.read().await.unwrap();
        assert!(swap.free <= swap.total);
    }
}
