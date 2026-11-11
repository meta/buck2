/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use buck2_util::threads::thread_spawn;

use crate::daemon::state::TenantEviction;

#[derive(Default)]
struct WatchdogState {
    deadline: Option<Instant>,
    finished: bool,
}

#[derive(Default)]
pub(crate) struct TenantEvictionWatchdog {
    state: Mutex<WatchdogState>,
    changed: Condvar,
}

impl TenantEvictionWatchdog {
    pub fn arm(&self, deadline: Instant) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.finished {
            return false;
        }
        let first = state.deadline.is_none();
        if state.deadline.is_none_or(|previous| deadline < previous) {
            state.deadline = Some(deadline);
            self.changed.notify_all();
        }
        first
    }

    pub fn complete(&self) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .finished = true;
        self.changed.notify_all();
    }

    pub fn wait_for_expiry(&self) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        while !state.finished {
            let deadline = state.deadline.expect("watchdog is armed before waiting");
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return true;
            }
            state = self
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(|error| error.into_inner())
                .0;
        }
        false
    }
}

pub static EMERGENCY_SHUTDOWN: OnceLock<fn() -> !> = OnceLock::new();

/// Starts the deadline by which an eviction must complete before the daemon ends itself.
/// Arming an eviction again only moves its deadline earlier.
pub(crate) fn arm_tenant_eviction_watchdog(eviction: &TenantEviction, timeout: Duration) {
    let watchdog = eviction.watchdog.clone();
    // The budget is the requesting proxy's, counted from its kill; an eviction another request
    // started earlier has no less of it.
    let now = Instant::now();
    let deadline = now.checked_add(timeout).unwrap_or(now);
    if !watchdog.arm(deadline) {
        return;
    }
    let generation = eviction.generation;
    let started = eviction.started;
    // Without the thread the eviction has only the proxy's own deadline; that is better than
    // ending the daemon over a thread the host would not give it.
    let spawned = thread_spawn("buck2-tenant-eviction-watchdog", move || {
        if watchdog.wait_for_expiry() {
            tracing::warn!(
                generation,
                elapsed_ms = (Instant::now() - started).as_millis(),
                "Tenant eviction watchdog expired"
            );
            if let Some(shutdown) = EMERGENCY_SHUTDOWN.get() {
                shutdown();
            }
            #[cfg(target_os = "linux")]
            buck2_wrapper_common::kill::kill_unidaemon_tree_within(
                buck2_wrapper_common::pid::Pid::from_u32(std::process::id())
                    .expect("valid current pid"),
                0,
                Duration::from_secs(10),
            );
            // SAFETY: `_exit` is async-signal-safe and does not return. It ends the process without
            // running destructors, which may be blocked, or touching state other threads may be
            // using.
            unsafe { libc::_exit(1) }
        }
    });
    if let Err(error) = spawned {
        tracing::error!(
            generation,
            "Could not arm the tenant eviction watchdog: {error:#}"
        );
    }
}
