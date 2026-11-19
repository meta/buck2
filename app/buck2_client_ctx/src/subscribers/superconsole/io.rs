/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::io_counters::IoCounterKey;
use buck2_event_observer::humanized::HumanizedBytes;
use buck2_event_observer::two_snapshots::TwoSnapshots;
use superconsole::Component;
use superconsole::Dimensions;
use superconsole::DrawMode;
use superconsole::Line;
use superconsole::Lines;

use crate::subscribers::superconsole::SuperConsoleConfig;

pub(crate) struct IoHeader<'s> {
    pub(crate) super_console_config: &'s SuperConsoleConfig,
    pub(crate) two_snapshots: &'s TwoSnapshots,
}

impl Component for IoHeader<'_> {
    type Error = buck2_error::Error;

    fn draw_unchecked(&self, dimensions: Dimensions, mode: DrawMode) -> buck2_error::Result<Lines> {
        render(
            self.two_snapshots,
            mode,
            dimensions.width,
            self.super_console_config.enable_io,
        )
    }
}

/// Place space-separated words on lines.
fn words_to_lines(words: Vec<String>, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current_line = String::new();
    for word in words {
        if current_line.is_empty() {
            current_line = word;
            continue;
        }
        // This works correctly only for ASCII strings.
        if current_line.len() + 1 + word.len() > width {
            lines.push(current_line);
            current_line = word;
        } else {
            current_line.push(' ');
            current_line.push_str(&word);
        }
    }
    if !current_line.is_empty() {
        lines.push(current_line);
    }
    lines
}

pub fn io_in_flight_non_zero_counters(
    snapshot: &buck2_data::Snapshot,
) -> impl Iterator<Item = (IoCounterKey, u32)> + '_ {
    IoCounterKey::ALL
        .iter()
        .map(|key| {
            let value = match key {
                IoCounterKey::Stat => snapshot.io_in_flight_stat,
                IoCounterKey::Copy => snapshot.io_in_flight_copy,
                IoCounterKey::Symlink => snapshot.io_in_flight_symlink,
                IoCounterKey::Hardlink => snapshot.io_in_flight_hardlink,
                IoCounterKey::MkDir => snapshot.io_in_flight_mk_dir,
                IoCounterKey::ReadDir => snapshot.io_in_flight_read_dir,
                IoCounterKey::ReadDirEden => snapshot.io_in_flight_read_dir_eden,
                IoCounterKey::RmDir => snapshot.io_in_flight_rm_dir,
                IoCounterKey::RmDirAll => snapshot.io_in_flight_rm_dir_all,
                IoCounterKey::StatEden => snapshot.io_in_flight_stat_eden,
                IoCounterKey::Chmod => snapshot.io_in_flight_chmod,
                IoCounterKey::ReadLink => snapshot.io_in_flight_read_link,
                IoCounterKey::Remove => snapshot.io_in_flight_remove,
                IoCounterKey::Rename => snapshot.io_in_flight_rename,
                IoCounterKey::Read => snapshot.io_in_flight_read,
                IoCounterKey::Write => snapshot.io_in_flight_write,
                IoCounterKey::Canonicalize => snapshot.io_in_flight_canonicalize,
                IoCounterKey::EdenSettle => snapshot.io_in_flight_eden_settle,
            };
            (*key, value)
        })
        .filter(|(_, value)| *value > 0)
}

fn do_render(
    two_snapshots: &TwoSnapshots,
    snapshot: &buck2_data::Snapshot,
    width: usize,
) -> buck2_error::Result<Lines> {
    let mut lines = Vec::new();
    const RSS_FIELD_WIDTH: usize = "RSS = ".len() + HumanizedBytes::FIXED_WIDTH_WIDTH;

    let mut allocator = Vec::new();
    if let Some(rss) = snapshot.buck2_rss {
        allocator.push(format!("RSS = {}", HumanizedBytes::fixed_width(rss)));
    } else if snapshot.buck2_max_rss > 0
        && (snapshot.malloc_bytes_active.is_some() || snapshot.malloc_bytes_allocated.is_some())
    {
        allocator.push(" ".repeat(RSS_FIELD_WIDTH));
    }
    if let Some(active) = snapshot.malloc_bytes_active {
        allocator.push(format!("Active = {}", HumanizedBytes::fixed_width(active)));
    }
    if let Some(allocated) = snapshot.malloc_bytes_allocated {
        allocator.push(format!(
            "Allocated = {}",
            HumanizedBytes::fixed_width(allocated)
        ));
    }
    if let (Some(active), Some(allocated)) = (
        snapshot.malloc_bytes_active,
        snapshot.malloc_bytes_allocated,
    ) {
        let slack = active.saturating_sub(allocated);
        let percent = if allocated == 0 {
            String::new()
        } else {
            format!(" ({:.1}%)", 100.0 * slack as f64 / allocated as f64)
        };
        allocator.push(format!("Slack = {}{}", HumanizedBytes::new(slack), percent));
    }
    if let Some(cgroup) = &snapshot.allprocs_cgroup {
        allocator.push(format!(
            "Cgroup swap = {}",
            HumanizedBytes::new(cgroup.swap_bytes)
        ));
    }
    if !allocator.is_empty() {
        lines.push(Line::unstyled(&format!(
            "Memory    : {}",
            allocator.join("  ")
        ))?);
    }

    let mut allocator_max = Vec::new();
    // Current RSS is unavailable on non-Linux Unix platforms, so keep max RSS independent of it.
    if snapshot.buck2_max_rss > 0 {
        allocator_max.push(format!(
            "RSS = {}",
            HumanizedBytes::fixed_width(snapshot.buck2_max_rss)
        ));
    } else if snapshot.buck2_rss.is_some()
        && (two_snapshots.max_malloc_bytes_active.is_some()
            || two_snapshots.max_malloc_bytes_allocated.is_some())
    {
        allocator_max.push(" ".repeat(RSS_FIELD_WIDTH));
    }
    if let Some(max_active) = two_snapshots.max_malloc_bytes_active {
        allocator_max.push(format!(
            "Active = {}",
            HumanizedBytes::fixed_width(max_active)
        ));
    }
    if let Some(max_allocated) = two_snapshots.max_malloc_bytes_allocated {
        allocator_max.push(format!(
            "Allocated = {}",
            HumanizedBytes::fixed_width(max_allocated)
        ));
    }
    if !allocator_max.is_empty() {
        lines.push(Line::unstyled(&format!(
            "Memory Max: {}",
            allocator_max.join("  ")
        ))?);
    }

    let mut parts = Vec::new();
    let user_cpu_percents = two_snapshots.user_cpu_percents();
    let system_cpu_percents = two_snapshots.system_cpu_percents();
    if user_cpu_percents.is_some() || system_cpu_percents.is_some() {
        let mut cpu_str_parts = vec!["buckd CPU".to_owned()];
        if let Some(p) = user_cpu_percents {
            cpu_str_parts.push(format!("user = {p}%"));
        }
        if let Some(p) = system_cpu_percents {
            cpu_str_parts.push(format!("system = {p}%"));
        }
        let cpu_str = cpu_str_parts.join("  ");
        parts.push(cpu_str);
    }

    // Show Tokio IO metrics in compact format: busy/total+queue
    parts.push(format!(
        "Tokio IO = {}/{}+{}",
        snapshot.tokio_num_blocking_threads - snapshot.tokio_num_idle_blocking_threads,
        snapshot.tokio_num_blocking_threads,
        snapshot.tokio_blocking_queue_depth
    ));

    if snapshot.deferred_materializer_queue_size > 0 {
        parts.push(format!(
            "DM Queue = {}",
            snapshot.deferred_materializer_queue_size
        ));
    }
    if snapshot.blocking_executor_io_queue_size > 0 {
        parts.push(format!(
            "IO Queue = {}",
            snapshot.blocking_executor_io_queue_size
        ));
    }
    if snapshot.blocking_executor_io_inline_in_flight > 0
        || snapshot.blocking_executor_io_inline_waiting > 0
    {
        parts.push(format!(
            "IO Inline = {} ({} waiting)",
            snapshot.blocking_executor_io_inline_in_flight,
            snapshot.blocking_executor_io_inline_waiting
        ));
    }
    if !parts.is_empty() {
        lines.push(Line::from_iter([superconsole::Span::new_unstyled(
            parts.join("  "),
        )?]));
    }

    let mut counters = Vec::new();
    for (key, value) in io_in_flight_non_zero_counters(snapshot) {
        counters.push(format!("{key:?} = {value}"));
    }
    lines.extend(
        words_to_lines(counters, width)
            .into_iter()
            .map(|s| Line::unstyled(&s))
            .collect::<Result<Vec<_>, _>>()?,
    );

    Ok(Lines(lines))
}

fn render(
    two_snapshots: &TwoSnapshots,
    draw_mode: DrawMode,
    width: usize,
    enabled: bool,
) -> buck2_error::Result<Lines> {
    if !enabled {
        return Ok(Lines::new());
    }
    if let DrawMode::Final = draw_mode {
        return Ok(Lines::new());
    }
    if let Some((_, snapshot)) = &two_snapshots.last {
        do_render(two_snapshots, snapshot, width)
    } else {
        Ok(Lines::new())
    }
}

#[cfg(test)]
mod tests {
    use super::words_to_lines;

    #[test]
    fn test_words_to_lines() {
        assert_eq!(Vec::<String>::new(), words_to_lines(vec![], 5));
        assert_eq!(
            vec!["ab".to_owned()],
            words_to_lines(vec!["ab".to_owned()], 5)
        );
        assert_eq!(
            vec!["ab cd".to_owned()],
            words_to_lines(vec!["ab".to_owned(), "cd".to_owned()], 5)
        );
        assert_eq!(
            vec!["ab".to_owned(), "cd".to_owned()],
            words_to_lines(vec!["ab".to_owned(), "cd".to_owned()], 4)
        );
        assert_eq!(
            vec!["abcd".to_owned()],
            words_to_lines(vec!["abcd".to_owned()], 3)
        );
    }
}
