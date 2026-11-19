/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;
use std::time::Duration;
use std::time::Instant;

use buck2_build_signals::env::WaitingCategory;
use buck2_core::soft_error_or_panic;
use buck2_util::time_span::TimeSpan;
use variant_name::VariantName;

use crate::DetailedCriticalPathEntry;
use crate::NodeExtraData;
use crate::duration_to_proto_saturating;

/// Helper for building critical path protobuf entries.
///
/// Accumulates `CriticalPathEntry2` entries during critical path computation,
/// handling the conversion of timing data and node information into the protobuf format.
/// Automatically inserts `Waiting` entries to fill gaps between consecutive entries.
pub(crate) struct CriticalPathProtoEnhancer {
    command_start: Instant,
    /// Tracks the end time of the last added entry to detect gaps
    last_entry_end: Instant,
    entries: Vec<buck2_data::CriticalPathEntry2>,
}

impl CriticalPathProtoEnhancer {
    pub(crate) fn new(command_start: Instant, capacity: usize) -> Self {
        Self {
            command_start,
            last_entry_end: command_start,
            entries: Vec::with_capacity(capacity),
        }
    }

    pub(crate) fn add_entry(&mut self, entry: DetailedCriticalPathEntry) {
        let DetailedCriticalPathEntry {
            key,
            mut data,
            mut potential_improvement,
            deps_finished_time,
        } = entry;
        let proto_entry = key.into_critical_path_entry_data(&data.extra_data);

        let waiting_for_deps_start = self.last_entry_end;
        let waiting_for_deps_end = deps_finished_time.unwrap_or(waiting_for_deps_start);
        if !waiting_for_deps_end
            .saturating_duration_since(waiting_for_deps_start)
            .is_zero()
        {
            self.add_simple_entry(
                None,
                buck2_data::critical_path_entry2::Entry::Waiting(
                    buck2_data::critical_path_entry2::Waiting {
                        category: Some("for_deps".to_owned()),
                    },
                ),
                TimeSpan::new_saturating(waiting_for_deps_start, waiting_for_deps_end),
                false,
            );
        }

        // TODO(cjhopman): If data.duration.total.start() < waiting_for_deps_end, we have overlapping entries. That should be an error (as it indicates that we claim that
        // both A depends on B and that A started before B finished). For now, ignore the overlapping span.
        if waiting_for_deps_end > data.duration.total.start() {
            data.duration.total =
                TimeSpan::new_saturating(waiting_for_deps_end, data.duration.total.end());
            // The user and improvable parts are subsets of the duration: trim them with it.
            let trimmed = data.duration.total.duration();
            data.duration.user = data.duration.user.min(trimmed);
            potential_improvement = potential_improvement.map(|p| p.min(trimmed));
        }

        let node_start = data.duration.total.start();

        let waiting_span = TimeSpan::new_saturating(waiting_for_deps_end, node_start);

        for (time_span, category) in data
            .waiting_data
            .iter_spans(waiting_for_deps_end, node_start)
        {
            match time_span.intersection(&waiting_span) {
                Some(time_span) => {
                    // We skip adding WaitingCategory::Unknown entries if they are 0 duration. We don't skip other categories because
                    // it is useful to know that we measure that time specifically so that a user knows that the adjacent waiting spans
                    // are not that (i.e. for a local action execution, maybe there'd be an Unknown span and a 0-duration ::LocalQueued
                    // span and you'd know that the Unknown time was not being spent in the local queue).
                    if time_span.duration() > Duration::ZERO
                        || category != &WaitingCategory::Unknown
                    {
                        self.add_simple_entry(
                            None,
                            buck2_data::critical_path_entry2::Entry::Waiting(
                                buck2_data::critical_path_entry2::Waiting {
                                    category: Some(category.variant_name_lowercase().to_owned()),
                                },
                            ),
                            time_span,
                            false,
                        );
                    }
                }
                None => {
                    // this shouldn't happen, it indicates the waiting data continued until the node started or that it started
                    // before the node's deps were finished. The best we can do at this point is to ignore it (we could make this
                    // a soft error in the future, but haven't seen issues from it).
                }
            }
        }

        self.add_entry_impl(
            None,
            data.duration.total,
            buck2_data::CriticalPathEntry2 {
                span_ids: data
                    .span_ids
                    .iter()
                    .map(|span_id| (*span_id).into())
                    .collect(),
                duration: Some(duration_to_proto_saturating(
                    data.duration.critical_path_duration(),
                )),
                user_duration: Some(duration_to_proto_saturating(data.duration.user)),
                queue_duration: data.duration.queue.map(duration_to_proto_saturating),
                total_duration: Some(duration_to_proto_saturating(data.duration.total.duration())),
                potential_improvement_duration: potential_improvement
                    .map(duration_to_proto_saturating),
                entry: Some(proto_entry),
                non_critical_path_duration: None,
                start_offset_ns: Some(
                    data.duration
                        .total
                        .start()
                        .saturating_duration_since(self.command_start)
                        .as_nanos()
                        .try_into()
                        .unwrap_or(u64::MAX),
                ),
                was_reused: matches!(data.extra_data, NodeExtraData::Reused).then_some(true),
            },
        );
    }

    /// Add a simple critical path entry for generic build phases.
    ///
    /// These entries have zero user_duration and potential_improvement_duration.
    ///
    /// # Parameters
    /// - `waiting_category`: Optional category label for any automatically-inserted waiting entries
    /// - `entry`: The critical path entry to add
    /// - `time_span`: Time span for this entry
    /// - `is_critical`: Whether this time is on the critical path (affects duration vs non_critical_duration)
    pub(crate) fn add_simple_entry(
        &mut self,
        waiting_category: Option<&str>,
        entry: buck2_data::critical_path_entry2::Entry,
        time_span: TimeSpan,
        is_critical: bool,
    ) {
        let (duration, non_critical_duration) = if is_critical {
            (time_span.duration(), Duration::ZERO)
        } else {
            (Duration::ZERO, time_span.duration())
        };
        self.add_entry_impl(
            waiting_category,
            time_span,
            self.create_simple_entry(entry, time_span.start(), duration, non_critical_duration),
        )
    }

    pub(crate) fn into_entries(self) -> Vec<buck2_data::CriticalPathEntry2> {
        group_page_in_entries(self.entries)
    }

    /// Internal implementation for adding entries with automatic waiting entry insertion.
    ///
    /// This method detects gaps between `last_entry_end` and the new entry's start time.
    /// If a gap exceeding 1ms is detected, it automatically inserts a `Waiting` entry
    /// to account for the missing time.
    fn add_entry_impl(
        &mut self,
        waiting_category: Option<&str>,
        time_span: TimeSpan,
        entry: buck2_data::CriticalPathEntry2,
    ) {
        let entry_start = time_span.start();
        if let Some(overlap) = self.last_entry_end.checked_duration_since(entry_start)
            && overlap > Duration::ZERO
        {
            soft_error_or_panic!(
                "critical_path_entry_overlap",
                buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Tier0,
                    "Critical path nodes had overlapping time spans (overlap duration {}us). \
                    This indicates that the work of the latter node does not actually \
                    depend on the work of the previous node. This leads to incorrect critical \
                    path calculations and double counting of time.

                    Previous node:
                    {:?}
                    Current node:
                    {:?}",
                    overlap.as_micros(),
                    self.entries.last(),
                    entry
                )
                .into()
            );
        }

        let missing_duration = entry_start.saturating_duration_since(self.last_entry_end);
        if missing_duration.as_millis() > 0 {
            self.entries.push(
                self.create_simple_entry(
                    buck2_data::critical_path_entry2::Waiting {
                        category: waiting_category.map(|v| v.to_owned()),
                    }
                    .into(),
                    self.last_entry_end,
                    Duration::ZERO,
                    missing_duration,
                ),
            );
        }
        self.entries.push(entry);
        self.last_entry_end = time_span.end();
    }

    fn create_simple_entry(
        &self,
        entry: buck2_data::critical_path_entry2::Entry,
        start_time: Instant,
        duration: Duration,
        non_critical_duration: Duration,
    ) -> buck2_data::CriticalPathEntry2 {
        let duration_proto = duration_to_proto_saturating(duration);
        buck2_data::CriticalPathEntry2 {
            span_ids: Vec::new(),
            duration: Some(duration_proto),
            user_duration: Some(prost_types::Duration::default()),
            queue_duration: None,
            total_duration: Some(duration_proto),
            potential_improvement_duration: Some(prost_types::Duration::default()),
            non_critical_path_duration: Some(duration_to_proto_saturating(non_critical_duration)),
            entry: Some(entry),
            start_offset_ns: Some(
                start_time
                    .saturating_duration_since(self.command_start)
                    .as_nanos()
                    .try_into()
                    .unwrap_or(u64::MAX),
            ),
            was_reused: None,
        }
    }
}

fn group_page_in_entries(
    entries: Vec<buck2_data::CriticalPathEntry2>,
) -> Vec<buck2_data::CriticalPathEntry2> {
    use buck2_data::critical_path_entry2::Entry;

    let mut result = Vec::with_capacity(entries.len());
    let mut entries = entries.into_iter().peekable();

    while let Some(entry) = entries.next() {
        if !matches!(entry.entry, Some(Entry::PageIn(..))) {
            result.push(entry);
            continue;
        }

        let mut run = vec![entry];
        loop {
            let mut bridge = Vec::new();
            while entries.peek().is_some_and(|entry| {
                matches!(entry.entry, Some(Entry::Waiting(..))) || is_zero_duration_connector(entry)
            }) {
                bridge.push(entries.next().unwrap());
            }

            if matches!(
                entries.peek().and_then(|entry| entry.entry.as_ref()),
                Some(Entry::PageIn(..))
            ) {
                run.extend(bridge);
                run.push(entries.next().unwrap());
            } else {
                result.push(group_page_in_run(run));
                result.extend(bridge);
                break;
            }
        }
    }

    result
}

fn is_zero_duration_connector(entry: &buck2_data::CriticalPathEntry2) -> bool {
    use buck2_data::critical_path_entry2::Entry;

    !matches!(entry.entry, Some(Entry::PageIn(..) | Entry::Waiting(..)))
        && entry.span_ids.is_empty()
        && proto_duration(entry.duration.as_ref()).is_zero()
        && proto_duration(entry.user_duration.as_ref()).is_zero()
        && proto_duration(entry.total_duration.as_ref()).is_zero()
        && proto_duration(entry.potential_improvement_duration.as_ref()).is_zero()
        && proto_duration(entry.queue_duration.as_ref()).is_zero()
        && proto_duration(entry.non_critical_path_duration.as_ref()).is_zero()
}

fn group_page_in_run(
    entries: Vec<buck2_data::CriticalPathEntry2>,
) -> buck2_data::CriticalPathEntry2 {
    use buck2_data::critical_path_entry2::Entry;

    let page_in_count = entries
        .iter()
        .filter(|entry| matches!(entry.entry, Some(Entry::PageIn(..))))
        .count();
    if page_in_count == 1 {
        return entries.into_iter().next().unwrap();
    }

    let mut count = 0u64;
    let mut key_type_counts = BTreeMap::<String, u64>::new();
    for entry in &entries {
        let Some(Entry::PageIn(page_in)) = &entry.entry else {
            continue;
        };
        let entry_count = page_in.count.max(1);
        count = count.saturating_add(entry_count);
        if page_in.key_type_counts.is_empty() {
            let type_count = key_type_counts.entry(page_in.key_type.clone()).or_default();
            *type_count = type_count.saturating_add(entry_count);
        } else {
            for (key_type, count) in &page_in.key_type_counts {
                let type_count = key_type_counts.entry(key_type.clone()).or_default();
                *type_count = type_count.saturating_add(*count);
            }
        }
    }

    let key_type = match key_type_counts.keys().next() {
        Some(key_type) if key_type_counts.len() == 1 => key_type.clone(),
        _ => "multiple".to_owned(),
    };
    let total_duration = sum_proto_durations(entries.iter().map(|entry| &entry.total_duration));
    let start_offset_ns = entries.first().and_then(|entry| entry.start_offset_ns);
    let end_offset_ns = entries
        .iter()
        .filter_map(|entry| {
            let start_offset_ns = entry.start_offset_ns?;
            let duration = proto_duration(entry.total_duration.as_ref())
                .saturating_add(proto_duration(entry.non_critical_path_duration.as_ref()));
            Some(start_offset_ns.saturating_add(duration.as_nanos().try_into().unwrap_or(u64::MAX)))
        })
        .max();
    let non_critical_path_duration = match (start_offset_ns, end_offset_ns) {
        (Some(start), Some(end)) => Some(duration_to_proto_saturating(
            Duration::from_nanos(end.saturating_sub(start))
                .saturating_sub(proto_duration(total_duration.as_ref())),
        )),
        _ => sum_proto_durations(
            entries
                .iter()
                .map(|entry| &entry.non_critical_path_duration),
        ),
    };

    buck2_data::CriticalPathEntry2 {
        span_ids: entries
            .iter()
            .flat_map(|entry| entry.span_ids.iter().copied())
            .collect(),
        duration: sum_proto_durations(entries.iter().map(|entry| &entry.duration)),
        user_duration: sum_proto_durations(entries.iter().map(|entry| &entry.user_duration)),
        total_duration,
        potential_improvement_duration: sum_proto_durations(entries.iter().filter_map(|entry| {
            matches!(entry.entry, Some(Entry::PageIn(..)))
                .then_some(&entry.potential_improvement_duration)
        })),
        queue_duration: sum_proto_durations(entries.iter().filter_map(|entry| {
            matches!(entry.entry, Some(Entry::PageIn(..))).then_some(&entry.queue_duration)
        })),
        non_critical_path_duration,
        start_offset_ns,
        entry: Some(Entry::PageIn(buck2_data::critical_path_entry2::PageIn {
            key_type,
            count,
            key_type_counts: key_type_counts.into_iter().collect(),
        })),
        was_reused: None,
    }
}

fn sum_proto_durations<'a>(
    durations: impl IntoIterator<Item = &'a Option<prost_types::Duration>>,
) -> Option<prost_types::Duration> {
    let mut found = false;
    let mut total = Duration::ZERO;
    for duration in durations.into_iter().filter_map(Option::as_ref) {
        found = true;
        total = total.saturating_add(proto_duration(Some(duration)));
    }
    found.then(|| duration_to_proto_saturating(total))
}

fn proto_duration(duration: Option<&prost_types::Duration>) -> Duration {
    duration
        .and_then(|duration| (*duration).try_into().ok())
        .unwrap_or(Duration::ZERO)
}

#[cfg(test)]
mod tests {
    use buck2_data::critical_path_entry2;

    use super::*;

    fn entry(
        entry: critical_path_entry2::Entry,
        start_ms: u64,
        critical_ms: u64,
        non_critical_ms: u64,
    ) -> buck2_data::CriticalPathEntry2 {
        buck2_data::CriticalPathEntry2 {
            span_ids: Vec::new(),
            duration: Some(duration_to_proto_saturating(Duration::from_millis(
                critical_ms,
            ))),
            user_duration: Some(duration_to_proto_saturating(Duration::from_millis(
                critical_ms,
            ))),
            total_duration: Some(duration_to_proto_saturating(Duration::from_millis(
                critical_ms,
            ))),
            potential_improvement_duration: None,
            queue_duration: None,
            non_critical_path_duration: Some(duration_to_proto_saturating(Duration::from_millis(
                non_critical_ms,
            ))),
            start_offset_ns: Some(Duration::from_millis(start_ms).as_nanos() as u64),
            entry: Some(entry),
            was_reused: None,
        }
    }

    fn page_in(key_type: &str, start_ms: u64, duration_ms: u64) -> buck2_data::CriticalPathEntry2 {
        entry(
            critical_path_entry2::PageIn {
                key_type: key_type.to_owned(),
                count: 1,
                key_type_counts: Default::default(),
            }
            .into(),
            start_ms,
            duration_ms,
            0,
        )
    }

    fn waiting(start_ms: u64, duration_ms: u64) -> buck2_data::CriticalPathEntry2 {
        entry(
            critical_path_entry2::Waiting {
                category: Some("for_deps".to_owned()),
            }
            .into(),
            start_ms,
            0,
            duration_ms,
        )
    }

    fn duration(duration: &Option<prost_types::Duration>) -> Duration {
        (*duration).unwrap().try_into().unwrap()
    }

    #[test]
    fn groups_page_ins_separated_only_by_waiting() {
        let trailing_waiting = waiting(10, 1);
        let result = group_page_in_entries(vec![
            page_in("analysis_key", 0, 2),
            waiting(2, 3),
            page_in("package_listing", 5, 5),
            trailing_waiting.clone(),
        ]);

        assert_eq!(2, result.len());
        assert_eq!(trailing_waiting, result[1]);

        let grouped = &result[0];
        let Some(critical_path_entry2::Entry::PageIn(page_in)) = &grouped.entry else {
            panic!("expected grouped page-in entry");
        };
        assert_eq!(2, page_in.count);
        assert_eq!(Some(&1), page_in.key_type_counts.get("analysis_key"));
        assert_eq!(Some(&1), page_in.key_type_counts.get("package_listing"));
        assert_eq!(Duration::from_millis(7), duration(&grouped.duration));
        assert_eq!(Duration::from_millis(7), duration(&grouped.total_duration));
        assert_eq!(
            Duration::from_millis(3),
            duration(&grouped.non_critical_path_duration)
        );
        assert_eq!(Some(0), grouped.start_offset_ns);
    }

    #[test]
    fn does_not_group_page_ins_across_other_work() {
        let other = entry(
            critical_path_entry2::GenericEntry {
                kind: "other".to_owned(),
            }
            .into(),
            2,
            1,
            0,
        );
        let result = group_page_in_entries(vec![
            page_in("analysis_key", 0, 2),
            other,
            page_in("analysis_key", 3, 2),
        ]);

        assert_eq!(3, result.len());
    }

    #[test]
    fn groups_page_ins_across_zero_duration_connectors() {
        let connector = entry(
            critical_path_entry2::GenericEntry {
                kind: "connector".to_owned(),
            }
            .into(),
            2,
            0,
            0,
        );
        let result = group_page_in_entries(vec![
            page_in("analysis_key", 0, 2),
            connector,
            page_in("analysis_key", 2, 2),
        ]);

        assert_eq!(1, result.len());
        let Some(critical_path_entry2::Entry::PageIn(page_in)) = &result[0].entry else {
            panic!("expected grouped page-in entry");
        };
        assert_eq!(2, page_in.count);
    }

    #[test]
    fn includes_unrepresented_gaps_in_group_envelope() {
        let mut second = page_in("analysis_key", 0, 1);
        second.start_offset_ns = Some(Duration::from_micros(2500).as_nanos() as u64);
        let result = group_page_in_entries(vec![page_in("analysis_key", 0, 2), second]);

        assert_eq!(1, result.len());
        assert_eq!(
            Duration::from_micros(500),
            duration(&result[0].non_critical_path_duration)
        );
    }

    fn trimmed_entry() -> buck2_data::CriticalPathEntry2 {
        use buck2_build_signals::env::NodeDuration;
        use buck2_build_signals::env::WaitingData;
        use buck2_common::package_listing::dice::PackageListingKey;
        use buck2_core::package::PackageLabel;
        use buck2_util::time_span::TimeSpan;

        let t0 = Instant::now();
        let mut enhancer = CriticalPathProtoEnhancer::new(t0, 4);
        // A node that started 2 s before its last dependency finished.
        enhancer.add_entry(DetailedCriticalPathEntry {
            key: crate::NodeKey::PackageListingKey(PackageListingKey(PackageLabel::testing_parse(
                "cell//n",
            ))),
            data: crate::NodeData {
                extra_data: NodeExtraData::None,
                duration: NodeDuration {
                    user: Duration::from_secs(3),
                    total: TimeSpan::from_start_and_duration(t0, Duration::from_secs(3)),
                    queue: None,
                },
                waiting_data: WaitingData::new(),
                span_ids: Default::default(),
            },
            potential_improvement: Some(Duration::from_secs(3)),
            deps_finished_time: Some(t0 + Duration::from_secs(2)),
        });
        enhancer.into_entries().pop().unwrap()
    }

    /// An entry that started before one of its dependencies finished is trimmed to start at
    /// that point, and so are its `user_duration` and `potential_improvement_duration`.
    #[test]
    fn trimmed_entry_trims_user_and_potential() {
        let e = trimmed_entry();
        assert_eq!(duration(&e.duration), Duration::from_secs(1));
        assert_eq!(duration(&e.user_duration), Duration::from_secs(1));
        assert_eq!(
            duration(&e.potential_improvement_duration),
            Duration::from_secs(1)
        );
    }

    /// Lays out an install node whose installed artifacts finished building at `deps_done_s` and
    /// whose installer ran from `installer_ready_s` to `installer_finished_s`, all from `t0`.
    fn install_entries(
        deps_done_s: u64,
        installer_ready_s: u64,
        installer_finished_s: u64,
    ) -> Vec<buck2_data::CriticalPathEntry2> {
        use std::sync::Arc;

        use buck2_build_signals::env::NodeDuration;
        use buck2_build_signals::env::WaitingData;
        use buck2_core::configuration::data::ConfigurationData;
        use buck2_core::provider::label::ConfiguredProvidersLabel;
        use buck2_core::provider::label::ProvidersName;
        use buck2_core::target::configured_target_label::ConfiguredTargetLabel;

        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let label =
            |l: &str| ConfiguredTargetLabel::testing_parse(l, ConfigurationData::testing_new());
        let total = TimeSpan::new_saturating(at(installer_ready_s), at(installer_finished_s));
        let mut enhancer = CriticalPathProtoEnhancer::new(t0, 4);
        enhancer.add_entry(DetailedCriticalPathEntry {
            key: crate::NodeKey::Install(crate::test_signals::InstallBuildSignalKey {
                installer: ConfiguredProvidersLabel::new(
                    label("cell//tools:installer"),
                    ProvidersName::Default,
                ),
                targets: Arc::new(vec![label("cell//app:app")]),
            }),
            data: crate::NodeData {
                extra_data: NodeExtraData::None,
                duration: NodeDuration {
                    user: total.duration(),
                    total,
                    queue: None,
                },
                waiting_data: WaitingData::with_initial_category(WaitingCategory::InstallerStartup),
                span_ids: Default::default(),
            },
            potential_improvement: None,
            deps_finished_time: Some(at(deps_done_s)),
        });
        enhancer.into_entries()
    }

    fn waiting_category(entry: &buck2_data::CriticalPathEntry2) -> Option<&str> {
        match &entry.entry {
            Some(critical_path_entry2::Entry::Waiting(w)) => w.category.as_deref(),
            _ => None,
        }
    }

    /// When the installer is ready after the artifacts are built, the wait for it is attributed
    /// to installer startup and the whole installer run is critical.
    #[test]
    fn install_waits_for_late_installer() {
        let entries = install_entries(1, 3, 5);
        let [for_deps, startup, install] = entries.as_slice() else {
            panic!("unexpected entries: {entries:?}");
        };
        assert_eq!(waiting_category(for_deps), Some("for_deps"));
        assert_eq!(
            waiting_category(startup),
            Some(WaitingCategory::InstallerStartup.variant_name_lowercase())
        );
        assert_eq!(
            duration(&startup.non_critical_path_duration),
            Duration::from_secs(2)
        );
        assert!(matches!(
            install.entry,
            Some(critical_path_entry2::Entry::Install(_))
        ));
        assert_eq!(duration(&install.duration), Duration::from_secs(2));
    }

    /// When the installer is ready before the artifacts are built, only the installer time after
    /// the last artifact is critical, and no time is attributed to installer startup.
    #[test]
    fn install_overlapping_build_is_trimmed() {
        let entries = install_entries(4, 1, 6);
        let install = entries.last().unwrap();
        assert!(matches!(
            install.entry,
            Some(critical_path_entry2::Entry::Install(_))
        ));
        assert_eq!(duration(&install.duration), Duration::from_secs(2));
        for e in &entries {
            if waiting_category(e)
                == Some(WaitingCategory::InstallerStartup.variant_name_lowercase())
            {
                assert_eq!(duration(&e.non_critical_path_duration), Duration::ZERO);
            }
        }
    }
}
