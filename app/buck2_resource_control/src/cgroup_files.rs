/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::os::fd::OwnedFd;
use std::sync::Arc;

use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_fs::paths::file_name::FileNameBuf;
use dupe::Dupe;
use nix::fcntl::OFlag;
use nix::sys::stat::Mode;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Environment)]
enum CgroupFileError {
    #[error("File size doesn't fit in off_t: {0}")]
    FileTooBig(std::num::TryFromIntError),
    #[error("Contents could not fit into 31 bytes: {0}")]
    FileTooLong(String),
    #[error("Unexpected format for {0}: {1}")]
    UnexpectedFormat(FileNameBuf, String),
}

/// Represents an open handle to one of the standard kernel-supplied files in the cgroup
pub(crate) struct CgroupFile(Arc<OwnedFd>, FileNameBuf);

#[derive(Clone, Copy)]
pub(crate) enum CgroupFileMode {
    ReadOnly,
    ReadWrite,
    WriteOnly,
}

impl CgroupFile {
    pub async fn open(
        d: Arc<OwnedFd>,
        name: FileNameBuf,
        mode: CgroupFileMode,
    ) -> buck2_error::Result<Self> {
        tokio::task::spawn_blocking(move || Self::sync_open(&d, name, mode)).await?
    }

    pub(crate) fn sync_open(
        d: &OwnedFd,
        name: FileNameBuf,
        mode: CgroupFileMode,
    ) -> buck2_error::Result<Self> {
        let flags = OFlag::O_CLOEXEC
            | match mode {
                CgroupFileMode::ReadOnly => OFlag::O_RDONLY,
                CgroupFileMode::ReadWrite => OFlag::O_RDWR,
                CgroupFileMode::WriteOnly => OFlag::O_WRONLY,
            };
        let file = nix::fcntl::openat(
            d,
            name.as_forward_rel_path().as_path(),
            flags,
            Mode::empty(),
        )
        .with_buck_error_context(|| format!("Failed to open cgroup file {}", name))?;
        Ok(CgroupFile(Arc::new(file), name.to_owned()))
    }

    pub(crate) async fn write(
        &self,
        data: impl AsRef<[u8]> + Send + Sync + 'static,
    ) -> buck2_error::Result<()> {
        let file = self.0.dupe();
        let name = self.1.clone();
        tokio::task::spawn_blocking(move || {
            Self::sync_write_impl(&file, data.as_ref()).map_err(buck2_error::Error::from)
        })
        .await?
        .with_buck_error_context(|| format!("Writing cgroup file {}", name))
    }

    /// Write the given buffer to the file
    ///
    /// Generally, the way that these files work is that instead of there being delimiters or
    /// terminators for values, the kernel guarantees that writes will be atomic, and if the user
    /// wants to do more than one operation, they must issue two writes. This API expects that the
    /// file being written to does indeed work that way; incomplete writes return errors.
    ///
    /// Additionally, this code is allocation-free in the happy path, which means that it's ok to
    /// use in `pre_exec` contexts
    pub(crate) fn sync_write(&self, data: &[u8]) -> std::io::Result<()> {
        Self::sync_write_impl(&self.0, data)
    }

    fn sync_write_impl(f: &OwnedFd, data: &[u8]) -> std::io::Result<()> {
        // Because of the weird semantics, we avoid std and just use libc
        let bytes_written = nix::unistd::write(f, data)?;

        if bytes_written != data.len() {
            return Err(std::io::Error::other(format!(
                "Write was incomplete: wrote {} bytes, expected {}",
                bytes_written,
                data.len()
            )));
        }

        Ok(())
    }

    /// Reads files of length at most 31 bytes
    ///
    /// Semantically like `read_to_buf` but avoids a heap allocation
    async fn read_to_short_buf(&self) -> buck2_error::Result<([u8; 32], usize)> {
        let file = self.0.dupe();
        tokio::task::spawn_blocking(move || {
            let mut data = [0u8; 32];
            let mut filled = 0;
            loop {
                if filled == data.len() {
                    return Err(CgroupFileError::FileTooLong(
                        String::from_utf8_lossy(&data).to_string(),
                    )
                    .into());
                }

                let buf = &mut data[filled..];
                let filled_trunc: nix::libc::off_t =
                    filled.try_into().map_err(CgroupFileError::FileTooBig)?;
                let read = nix::sys::uio::pread(&file, buf, filled_trunc)?;
                if read == 0 {
                    break;
                }
                filled += read;
            }
            buck2_error::Ok((data, filled))
        })
        .await?
        .with_buck_error_context(|| format!("Reading cgroup file {}", self.1))
    }

    async fn read_to_buf(&self) -> buck2_error::Result<Vec<u8>> {
        let file = self.0.dupe();
        tokio::task::spawn_blocking(move || {
            let mut data = vec![0u8; 2048]; // Enough in practice
            let mut filled = 0;
            loop {
                if filled == data.len() {
                    data.resize(data.len() * 2, 0);
                }

                let buf = &mut data[filled..];
                let filled_trunc: nix::libc::off_t =
                    filled.try_into().map_err(CgroupFileError::FileTooBig)?;
                let read = nix::sys::uio::pread(&file, buf, filled_trunc)?;
                if read == 0 {
                    break;
                }
                filled += read;
            }
            data.truncate(filled);
            buck2_error::Ok(data)
        })
        .await?
        .with_buck_error_context(|| format!("Reading cgroup file {}", self.1))
    }

    pub(crate) async fn read_to_string(&self) -> buck2_error::Result<String> {
        let buf = self.read_to_buf().await?;
        Ok(String::from_utf8(buf)?)
    }

    // FIXME(JakobDegen): Ought probably to have some types to represent the files
    pub(crate) async fn read_memory_stat(&self) -> buck2_error::Result<MemoryStat> {
        MemoryStat::parse(&self.read_to_string().await?)
            .buck_error_context("Failed to parse memory.stat")
    }

    pub(crate) async fn read_swap_events(&self) -> buck2_error::Result<SwapEvents> {
        SwapEvents::parse(&self.read_to_string().await?)
            .buck_error_context("Failed to parse memory.swap.events")
    }

    pub(crate) async fn read_max_or_int(&self) -> buck2_error::Result<Option<u64>> {
        let (data, filled) = self.read_to_short_buf().await?;
        let data = &data[..filled];
        std::str::from_utf8(data)
            .ok()
            .map(|d| d.trim_end())
            .and_then(|d| {
                if d == "max" {
                    Some(None)
                } else {
                    d.parse::<u64>().ok().map(Some)
                }
            })
            .ok_or_else(|| {
                CgroupFileError::UnexpectedFormat(
                    self.1.clone(),
                    String::from_utf8_lossy(data).to_string(),
                )
                .into()
            })
    }

    pub(crate) async fn read_int(&self) -> buck2_error::Result<u64> {
        let (data, filled) = self.read_to_short_buf().await?;
        let data = &data[..filled];
        std::str::from_utf8(data)
            .ok()
            .map(|d| d.trim_end())
            .and_then(|d| d.parse::<u64>().ok())
            .ok_or_else(|| {
                CgroupFileError::UnexpectedFormat(
                    self.1.clone(),
                    String::from_utf8_lossy(data).to_string(),
                )
                .into()
            })
    }

    pub(crate) async fn read_resource_pressure(&self) -> buck2_error::Result<ResourcePressure> {
        ResourcePressure::parse(&self.read_to_string().await?)
    }
}

/// Calls `entry` with each key and value of a file in what the kernel calls the flat keyed format:
/// one `KEY VALUE` pair per line.
fn parse_flat_keyed(content: &str, mut entry: impl FnMut(&str, u64)) -> buck2_error::Result<()> {
    for line in content.lines() {
        let mut parts = line.split_whitespace();
        let key = parts
            .next()
            .with_internal_error(|| format!("Invalid line: '{}' (no key)", line))?;
        let value = parts
            .next()
            .with_internal_error(|| format!("Invalid line: '{}' (no value)", line))?
            .parse::<u64>()
            .with_buck_error_context(|| format!("Invalid line: '{}' (invalid value)", line))?;
        if parts.next().is_some() {
            return Err(buck2_error::internal_error!(
                "Invalid line: '{}' (too many parts)",
                line
            ));
        }
        entry(key, value);
    }
    Ok(())
}

/// A few interesting values from memory.stat
#[derive(Debug, Default)]
pub struct MemoryStat {
    /// Anonymous memory, inclusive of swap.
    pub anon: u64,
    pub inactive_anon: u64,
    pub active_anon: u64,
    /// File-backed memory.
    pub file: u64,
    pub active_file: u64,
    pub inactive_file: u64,
    /// Kernel memory.
    pub kernel: u64,
}

impl MemoryStat {
    fn parse(content: &str) -> buck2_error::Result<Self> {
        let mut res = MemoryStat::default();
        parse_flat_keyed(content, |key, value| match key {
            "anon" => res.anon = value,
            "inactive_anon" => res.inactive_anon = value,
            "active_anon" => res.active_anon = value,
            "file" => res.file = value,
            "active_file" => res.active_file = value,
            "inactive_file" => res.inactive_file = value,
            "kernel" => res.kernel = value,
            // Ignore other keys
            _ => {}
        })?;
        Ok(res)
    }

    /// The page cache that the kernel can drop to make room.
    ///
    /// Less than `file`, which also counts tmpfs and shared memory. Those can only make room by
    /// going to swap, like anonymous memory.
    pub fn page_cache(&self) -> u64 {
        self.active_file + self.inactive_file
    }
}

/// A few interesting values from memory.swap.events
#[derive(Debug, Default)]
pub struct SwapEvents {
    /// How many times the kernel wanted to move memory to swap and couldn't, be it because of a
    /// swap limit on this cgroup or one above it, or because the system is out of swap.
    pub fail: u64,
}

impl SwapEvents {
    fn parse(content: &str) -> buck2_error::Result<Self> {
        let mut res = SwapEvents::default();
        parse_flat_keyed(content, |key, value| {
            if key == "fail" {
                res.fail = value;
            }
        })?;
        Ok(res)
    }
}

pub struct ResourcePressurePart {
    pub avg10: f64,
    pub avg60: f64,
    pub total: u64,
}

pub struct ResourcePressure {
    pub full: ResourcePressurePart,
}

impl ResourcePressure {
    fn parse(s: &str) -> buck2_error::Result<Self> {
        Self::parse_inner(s).ok_or_else(|| {
            buck2_error::Error::from(CgroupFileError::UnexpectedFormat(
                FileNameBuf::unchecked_new("memory.pressure"),
                s.to_owned(),
            ))
        })
    }

    fn parse_inner(s: &str) -> Option<Self> {
        let mut parts = s.lines();
        let some = parts.next()?;
        let full = parts.next()?;
        if parts.next().is_some() {
            return None;
        }

        let parse_part = |s: &str, expected: &str| {
            let (name, mut rest) = s.split_once(' ')?;
            if name != expected {
                return None;
            }
            let mut getitem = |expected_name: &str| {
                let (item, new_rest) = rest.split_once(' ').unwrap_or((rest, ""));
                rest = new_rest;
                let (name, value) = item.split_once('=')?;
                if name != expected_name {
                    None
                } else {
                    Some(value)
                }
            };
            let avg10: f64 = getitem("avg10")?.parse().ok()?;
            let avg60: f64 = getitem("avg60")?.parse().ok()?;
            let _avg300 = getitem("avg300")?;
            let total = getitem("total")?;
            if !rest.is_empty() {
                return None;
            }
            Some(ResourcePressurePart {
                avg10,
                avg60,
                total: total.parse().ok()?,
            })
        };

        let _some = parse_part(some, "some")?;

        Some(Self {
            full: parse_part(full, "full")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::cgroup_files::MemoryStat;
    use crate::cgroup_files::SwapEvents;

    #[test]
    fn test_parse_memory_stat() {
        let sample_stat = r#"anon 1048576
file 2097152
kernel 524288
kernel_stack 8192
pagetables 16384
percpu 32768
sock 4096
vmalloc 65536
shmem 131072
file_mapped 262144
file_dirty 8192
file_writeback 0
swapcached 0
anon_thp 0
file_thp 0
shmem_thp 0
inactive_anon 524288
active_anon 524288
inactive_file 1048576
active_file 1048576
unevictable 0
slab_reclaimable 196608
slab_unreclaimable 65536
slab 262144"#;

        let stat = MemoryStat::parse(sample_stat).unwrap();
        assert_eq!(stat.anon, 1048576);
        assert_eq!(stat.file, 2097152);
        assert_eq!(stat.kernel, 524288);
        assert_eq!(stat.inactive_anon, 524288);
        assert_eq!(stat.active_anon, 524288);
        assert_eq!(stat.inactive_file, 1048576);
        assert_eq!(stat.active_file, 1048576);
        assert_eq!(stat.page_cache(), 2 * 1048576);
    }

    #[test]
    fn test_parse_swap_events() {
        let events = SwapEvents::parse("high 0\nmax 12\nfail 34\n").unwrap();
        assert_eq!(events.fail, 34);
    }

    #[test]
    fn test_parse_resource_pressure() {
        let sample_pressure = r#"some avg10=1.01 avg60=2.02 avg300=3.03 total=45904150
full avg10=1.10 avg60=2.20 avg300=3.30 total=45781727"#;

        let pressure = crate::cgroup_files::ResourcePressure::parse(sample_pressure).unwrap();
        assert_eq!(pressure.full.total, 45781727);
        assert_eq!(pressure.full.avg10, 1.10);
        assert_eq!(pressure.full.avg60, 2.20);
    }
}
