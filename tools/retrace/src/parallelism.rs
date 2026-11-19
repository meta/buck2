/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! CPU budgets for one ingest or viewer load.
//!
//! Cap the stages that benefit from parallel work instead of starting a pool
//! the size of the machine for each stage. Independent trace/graph loads share
//! a budget, including their coordinating callers. This does not impose a
//! process-wide thread/RSS limit: the UI, other analyses and a cooperatively
//! canceling previous load can still be alive. Batch limits bound ordinary
//! in-flight work, not the retained graph or an accepted oversized record.

const DECODE_MAX: usize = 4;
const TRACE_EMIT_MAX: usize = 8;
const GRAPH_MAX: usize = 4;
const TRACE_PARSE_MAX: usize = 16;

fn available() -> usize {
    std::thread::available_parallelism().map_or(1, usize::from)
}

fn pool(cores: usize, coordinators: usize, ceiling: usize) -> usize {
    cores.max(1).saturating_sub(coordinators).min(ceiling)
}

/// Zero selects inline decoding. Parallel decode also has a framer and an
/// ordered reducer, so a one- or two-core machine keeps the serial path.
pub(crate) fn decode_workers() -> usize {
    pool(available(), 2, DECODE_MAX)
}

/// Zero selects the serial writer. A parallel writer owns one coordinator.
pub(crate) fn trace_emit_workers() -> usize {
    pool(available(), 1, TRACE_EMIT_MAX)
}

pub(crate) fn graph_emit_workers() -> usize {
    pool(available(), 1, GRAPH_MAX)
}

fn trace_limit(requested: usize) -> usize {
    if requested == 0 {
        TRACE_PARSE_MAX
    } else {
        requested
    }
}

fn trace_threads_for(cores: usize, requested: usize) -> usize {
    pool(cores, 1, trace_limit(requested)).max(1)
}

/// One parses inline. Larger values count workers in addition to the caller.
/// An explicit --threads value is an upper bound, still capped by hardware.
pub(crate) fn trace_threads(requested: usize) -> usize {
    trace_threads_for(available(), requested)
}

fn graph_workers_for(cores: usize) -> usize {
    match pool(cores, 1, GRAPH_MAX) {
        0 | 1 => 0,
        workers => workers,
    }
}

/// Zero is serial; otherwise there is one read/merge caller plus N workers.
pub(crate) fn graph_workers() -> usize {
    graph_workers_for(available())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ViewerPlan {
    pub trace_threads: usize,
    pub graph_workers: usize,
    pub overlap: bool,
}

fn viewer_plan_for(cores: usize, requested: usize, overlap_possible: bool) -> ViewerPlan {
    let cores = cores.max(1);
    if !overlap_possible || cores < 4 {
        return ViewerPlan {
            trace_threads: trace_threads_for(cores, requested),
            graph_workers: graph_workers_for(cores),
            overlap: false,
        };
    }

    // Reserve both callers, then give the graph roughly a quarter of the
    // worker budget. Benchmarks plateaued at four graph workers. The trace
    // gets the remainder, capped at sixteen by default. Its one-thread mode
    // is inline, leaving any spare cores available to the graph pool.
    let graph_share = ((cores - 2) / 4).clamp(2, GRAPH_MAX);
    let trace_threads = pool(cores, graph_share + 2, trace_limit(requested)).max(1);
    let trace_cost = if trace_threads == 1 {
        1
    } else {
        trace_threads + 1
    };
    let graph_workers = pool(cores, trace_cost + 1, GRAPH_MAX);
    ViewerPlan {
        trace_threads,
        graph_workers,
        overlap: true,
    }
}

pub(crate) fn viewer_plan(requested: usize, overlap_possible: bool) -> ViewerPlan {
    viewer_plan_for(available(), requested, overlap_possible)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_pools_reserve_their_coordinators() {
        for cores in 0..=256 {
            let budget = cores.max(1);
            let decode = pool(cores, 2, DECODE_MAX);
            assert!(decode <= DECODE_MAX);
            assert!(decode == 0 || decode + 2 <= budget);
            for cap in [TRACE_EMIT_MAX, GRAPH_MAX] {
                let emit = pool(cores, 1, cap);
                assert!(emit <= cap);
                assert!(emit == 0 || emit < budget);
            }
        }
        assert_eq!(pool(2, 2, DECODE_MAX), 0);
        assert_eq!(pool(3, 2, DECODE_MAX), 1);
        assert_eq!(pool(96, 2, DECODE_MAX), 4);
    }

    #[test]
    fn overlapping_loads_share_one_hardware_budget() {
        for cores in 0..=256 {
            for requested in [0, 1, 2, 4, 8, 16, 64, usize::MAX] {
                for possible in [false, true] {
                    let p = viewer_plan_for(cores, requested, possible);
                    let trace_cost = if p.trace_threads == 1 {
                        1
                    } else {
                        p.trace_threads + 1
                    };
                    let graph_cost = p.graph_workers + 1;
                    assert!(p.trace_threads >= 1);
                    assert!(p.trace_threads <= trace_limit(requested));
                    assert!(p.graph_workers <= GRAPH_MAX);
                    assert!(p.graph_workers != 1);
                    assert!(trace_cost <= cores.max(1));
                    assert!(graph_cost <= cores.max(1));
                    if p.overlap {
                        assert!(possible && cores >= 4);
                        assert!(trace_cost + graph_cost <= cores);
                    }
                }
            }
        }
    }

    #[test]
    fn laptop_and_many_core_defaults() {
        for (cores, trace, graph, overlap) in [
            (1, 1, 0, false),
            (2, 1, 0, false),
            (3, 2, 2, false),
            (4, 1, 2, true),
            (5, 1, 3, true),
            (8, 4, 2, true),
            (12, 8, 2, true),
            (16, 11, 3, true),
            (24, 16, 4, true),
            (96, 16, 4, true),
        ] {
            assert_eq!(
                viewer_plan_for(cores, 0, true),
                ViewerPlan {
                    trace_threads: trace,
                    graph_workers: graph,
                    overlap,
                }
            );
        }
    }

    #[test]
    fn explicit_limits_do_not_oversubscribe() {
        assert_eq!(trace_threads_for(8, usize::MAX), 7);
        assert_eq!(trace_threads_for(96, 0), 16);
        assert_eq!(trace_threads_for(96, 64), 64);
        assert_eq!(viewer_plan_for(8, 1, true).trace_threads, 1);
        assert_eq!(viewer_plan_for(8, 1, true).graph_workers, 4);
        assert_eq!(viewer_plan_for(96, 64, true).trace_threads, 64);
        assert_eq!(viewer_plan_for(96, 0, false).trace_threads, 16);
    }
}
