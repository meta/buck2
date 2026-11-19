/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::ffi::CString;
use std::io::Write;
use std::sync::Mutex;
use std::sync::OnceLock;

// Some code is adapted from buck2_util::process_stats, consider moving into shed and sharing it.

/// Returns ticks per second from sysconf(_SC_CLK_TCK).
fn sc_clk_tck() -> u64 {
    static TICKS: OnceLock<u64> = OnceLock::new();
    *TICKS.get_or_init(|| {
        // SAFETY: sysconf(_SC_CLK_TCK) is a read-only query with no preconditions.
        let rate = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        assert!(rate > 0 && rate <= 10_000, "Invalid _SC_CLK_TCK: {}", rate);
        rate as u64
    })
}

/// Process CPU usage in milliseconds, read from /proc/self/stat.
/// Returns (user_ms, system_ms).
pub fn process_cpu_ms() -> anyhow::Result<(u64, u64)> {
    let stat = std::fs::read_to_string("/proc/self/stat")?;
    let after_comm = &stat[stat
        .rfind(')')
        .ok_or_else(|| anyhow::anyhow!("no ')' in /proc/self/stat"))?
        + 2..];
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    let utime: u64 = fields[11].parse()?;
    let stime: u64 = fields[12].parse()?;
    let ticks = sc_clk_tck();
    Ok((utime * 1000 / ticks, stime * 1000 / ticks))
}

pub fn proc_rss_breakdown() -> anyhow::Result<serde_json::Value> {
    let status = std::fs::read_to_string("/proc/self/status")?;
    let mut anon = 0u64;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("RssAnon:") {
            anon = parse_proc_kb(rest)?;
        }
    }
    Ok(serde_json::json!({ "anon_bytes": anon }))
}

/// Read VmHWM (peak resident set size) from /proc/self/status.
pub fn proc_peak_rss_bytes() -> anyhow::Result<u64> {
    let status = std::fs::read_to_string("/proc/self/status")?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            return parse_proc_kb(rest);
        }
    }
    Ok(0)
}

fn parse_proc_kb(s: &str) -> anyhow::Result<u64> {
    let kb: u64 = s
        .trim()
        .strip_suffix(" kB")
        .ok_or_else(|| anyhow::anyhow!("expected ' kB' suffix in {:?}", s))?
        .trim()
        .parse()?;
    Ok(kb * 1024)
}

pub fn jemalloc_stats() -> anyhow::Result<serde_json::Value> {
    tikv_jemalloc_ctl::epoch::advance()?;
    let allocated = tikv_jemalloc_ctl::stats::allocated::read()?;
    let resident = tikv_jemalloc_ctl::stats::resident::read()?;
    Ok(serde_json::json!({
        "allocated_bytes": allocated,
        "resident_bytes": resident,
    }))
}

/// Reset the kernel's VmHWM (peak RSS) counter so the next reading
/// reflects only the current stage's peak.
pub fn reset_peak_rss() -> anyhow::Result<()> {
    std::fs::write("/proc/self/clear_refs", "5")?;
    Ok(())
}

pub fn db_size_bytes() -> u64 {
    let Ok(path) = std::env::var("BUCK2_DICE_DB_PATH") else {
        return 0;
    };
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                total += meta.len();
            }
        }
    }
    total
}

pub fn emit_stage(stage: &str, duration_secs: f64, heap_dump: Option<&str>) -> anyhow::Result<()> {
    static PREV_CPU: Mutex<(u64, u64)> = Mutex::new((0, 0));
    let (user_ms, system_ms) = process_cpu_ms()?;
    let mut prev = PREV_CPU.lock().unwrap();
    let (delta_user, delta_system) = (user_ms - prev.0, system_ms - prev.1);
    *prev = (user_ms, system_ms);

    let db_size = db_size_bytes();
    let jemalloc = jemalloc_stats()?;
    let rss = proc_rss_breakdown()?;
    let peak_rss = proc_peak_rss_bytes()?;

    let mut row = json_value::json!({
        "stage": stage,
        "duration_secs": duration_secs,
        "cpu_user_ms": delta_user, "cpu_system_ms": delta_system,
        "rss": rss,
        "peak_rss_bytes": peak_rss,
        "db_size_bytes": db_size,
        "jemalloc": jemalloc,
    });

    if let Some(path) = heap_dump {
        let dump_path = format!("{}.{}.heap", path, stage);
        dump_heap_profile(&dump_path);
        row["heap_dump"] = json_value::json!(dump_path);
    }

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    serde_json::to_writer(&mut out, &row)?;
    out.write_all(b"\n")?;
    drop(out);

    reset_peak_rss()?;
    Ok(())
}

/// Force jemalloc to return dirty pages to the OS, by default it takes 10s to free dirty pages.
pub fn jemalloc_purge() {
    let narenas = tikv_jemalloc_ctl::arenas::narenas::read().unwrap();
    for i in 0..narenas {
        // SAFETY: mallctl keys are valid null-terminated strings and the value
        // type (isize) matches the expected size for decay_ms settings.
        unsafe {
            tikv_jemalloc_ctl::raw::write(
                format!("arena.{}.dirty_decay_ms\0", i).as_bytes(),
                0_isize,
            )
            .ok();
        }
        unsafe {
            tikv_jemalloc_ctl::raw::write(
                format!("arena.{}.muzzy_decay_ms\0", i).as_bytes(),
                0_isize,
            )
            .ok();
        }
    }
}

fn dump_heap_profile(path: &str) {
    let path_c = CString::new(path).expect("heap dump path contains null byte");
    // SAFETY: path_c is a valid CString; its pointer remains valid for the
    // duration of the write call. The mallctl key "prof.dump" expects a
    // const char* path.
    let result = unsafe {
        tikv_jemalloc_ctl::raw::write(b"prof.dump\0", path_c.as_ptr() as *const libc::c_char)
    };
    match result {
        Ok(()) => eprintln!("Heap profile dumped to: {}", path),
        Err(e) => eprintln!(
            "Failed to dump heap profile (did you set MALLOC_CONF=prof:true?): {}",
            e
        ),
    }
}
