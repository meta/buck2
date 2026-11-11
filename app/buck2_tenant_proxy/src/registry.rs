/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The shared daemon's registry of the proxies it launched, one per tenant generation, and the
//! per-tenant budget for replacing a proxy that was lost.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use buck2_cli_proto::DaemonProcessInfo;
use buck2_common::daemon_compatibility::TenantProxySpec;
use buck2_error::ErrorTag;
use buck2_error::buck2_error;
use buck2_hash::BuckMutMap;
use buck2_wrapper_common::kill;
use buck2_wrapper_common::pid::Pid;

pub(crate) type ProxyKey = (String, String, u64);

pub(crate) struct ProxyRecord {
    pub(crate) info: DaemonProcessInfo,
    pub(crate) lost: Arc<AtomicBool>,
    /// Set once the launch has committed: the proxy has published itself in the slot.
    pub(crate) published: bool,
    /// The daemon's end of the proxy's liveness socket, for telling the proxy to exit.
    pub(crate) retire: Option<UnixStream>,
    /// Set when the generation was retired before its proxy was spawned, which the launch then
    /// gives up instead of publishing a proxy nothing would retire.
    pub(crate) retired: bool,
    /// Signalled by the supervisor once the proxy's liveness link has ended.
    pub(crate) exit: Arc<ProxyExit>,
}

static PROXIES: OnceLock<Mutex<BuckMutMap<ProxyKey, ProxyRecord>>> = OnceLock::new();

fn proxies() -> &'static Mutex<BuckMutMap<ProxyKey, ProxyRecord>> {
    PROXIES.get_or_init(|| Mutex::new(BuckMutMap::default()))
}

/// The end of a proxy's liveness link, which its exit closes.
#[derive(Default)]
pub(crate) struct ProxyExit {
    exited: Mutex<bool>,
    changed: Condvar,
}

impl ProxyExit {
    pub(crate) fn signal(&self) {
        *self
            .exited
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = true;
        self.changed.notify_all();
    }

    /// Waits until the link has ended, or `timeout` passes; returns whether it ended.
    fn wait(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut exited = self
            .exited
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        while !*exited {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            exited = self
                .changed
                .wait_timeout(exited, remaining)
                .unwrap_or_else(|error| error.into_inner())
                .0;
        }
        *exited
    }
}

pub(crate) struct ProxyReservation {
    lost: Arc<AtomicBool>,
    pub(crate) committed: bool,
}

impl Drop for ProxyReservation {
    fn drop(&mut self) {
        // The entry stays, marked lost, rather than being removed: the daemon retires a
        // generation whose launch failed, and a concurrent registration must then fail rather
        // than launch a proxy for it. Generations are never reused, so nothing else needs the key.
        if !self.committed {
            self.lost.store(true, Ordering::Release);
        }
    }
}

pub(crate) enum Reservation {
    /// Another launch of this generation has published its proxy.
    Published(DaemonProcessInfo),
    Reserved {
        lost: Arc<AtomicBool>,
        exit: Arc<ProxyExit>,
        reservation: ProxyReservation,
    },
}

/// Reserves the registry entry of a generation for a launch. Clients of one tenant may register
/// concurrently and share a generation; the second waits until the first one's launch has
/// published the proxy, rather than failing it. A spawned proxy may still fail its handshake or
/// refuse the slot, so a live pid is not enough.
pub(crate) fn reserve(key: &ProxyKey) -> buck2_error::Result<Reservation> {
    let wait_until = Instant::now() + Duration::from_secs(20);
    loop {
        let mut entries = proxies().lock().unwrap_or_else(|error| error.into_inner());
        match entries.get(key) {
            Some(record) if record.lost.load(Ordering::Acquire) => {
                return Err(buck2_error!(
                    ErrorTag::Input,
                    "Tenant proxy liveness was lost; its generation is retiring"
                ));
            }
            Some(record) if record.published => {
                if kill::process_exists(Pid::from_i64(record.info.pid)?)? {
                    return Ok(Reservation::Published(record.info.clone()));
                }
                return Err(buck2_error!(
                    ErrorTag::Input,
                    "Tenant proxy liveness was lost; its generation is retiring"
                ));
            }
            Some(_) => {
                drop(entries);
                if Instant::now() >= wait_until {
                    return Err(buck2_error!(
                        ErrorTag::Input,
                        "Timed out waiting for a concurrent launch of the tenant proxy"
                    ));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            None => {
                let lost = Arc::new(AtomicBool::new(false));
                let exit = Arc::new(ProxyExit::default());
                entries.insert(
                    key.clone(),
                    ProxyRecord {
                        info: DaemonProcessInfo::default(),
                        lost: lost.clone(),
                        published: false,
                        retire: None,
                        retired: false,
                        exit: exit.clone(),
                    },
                );
                return Ok(Reservation::Reserved {
                    lost: lost.clone(),
                    exit,
                    reservation: ProxyReservation {
                        lost,
                        committed: false,
                    },
                });
            }
        }
    }
}

/// Records the process a reserved launch spawned, and the link for retiring it. Returns whether
/// the generation was retired meanwhile, in which case the launch must not go on.
pub(crate) fn spawned(key: &ProxyKey, pid: i64, retire: UnixStream) -> bool {
    let mut entries = proxies().lock().unwrap_or_else(|error| error.into_inner());
    let record = entries.get_mut(key).expect("reserved proxy");
    record.info.pid = pid;
    record.retire = Some(retire);
    record.retired
}

/// Drops the retirement link of a proxy whose liveness has ended; there is no one to retire.
pub(crate) fn forget_retire(key: &ProxyKey) {
    if let Some(record) = proxies()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get_mut(key)
    {
        record.retire = None;
    }
}

pub(crate) fn record_info(key: &ProxyKey, info: &DaemonProcessInfo) {
    if let Some(record) = proxies()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get_mut(key)
    {
        record.info = info.clone();
    }
}

pub(crate) fn mark_published(key: &ProxyKey) {
    if let Some(record) = proxies()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get_mut(key)
    {
        record.published = true;
    }
}

pub(crate) fn proxy_key(spec: &TenantProxySpec) -> ProxyKey {
    (
        spec.tenant.project_root.clone(),
        spec.tenant.isolation.clone(),
        spec.reference.generation,
    )
}

/// How long a retired proxy gets to exit before the eviction completes without it. A proxy told
/// to exit does so at once; one that has not after this long is wedged, and the client that
/// asked for the eviction stops it when its own grace runs out, so waiting longer would only
/// delay the eviction's completion, not change the proxy's fate.
const PROXY_RETIREMENT_TIMEOUT: Duration = Duration::from_secs(3);

/// Tells the proxy of an evicted generation to exit and waits, briefly, until it has. The proxy
/// removes its own connection files on the way out, so the next generation's proxy finds the
/// tenant directory free; a client that asked for the eviction waits for the same rather than
/// stopping the proxy itself. Blocks.
pub fn retire(project_root: &str, isolation: &str, generation: u64) {
    let Some(proxies) = PROXIES.get() else {
        return;
    };
    let exit = {
        let mut entries = proxies.lock().unwrap_or_else(|error| error.into_inner());
        let Some(record) =
            entries.get_mut(&(project_root.to_owned(), isolation.to_owned(), generation))
        else {
            return;
        };
        // The link serves once: the entry outlives the proxy, its descriptor need not.
        let Some(stream) = record.retire.take() else {
            // Retired before its proxy was spawned, between the launch's reservation and its
            // spawn: the launch sees the mark and gives up, as nothing would retire that proxy.
            if !record.published {
                record.retired = true;
            }
            return;
        };
        // A proxy whose liveness is lost cannot hear this; its supervisor stops it once the
        // eviction completes, and the eviction has a kill's budget to complete in.
        if record.lost.load(Ordering::Acquire) || (&stream).write_all(b"E").is_err() {
            return;
        }
        let _ignored = stream.shutdown(std::net::Shutdown::Write);
        record.exit.clone()
    };
    // The proxy's exit closes its end of the liveness link, which the supervisor reports here.
    exit.wait(PROXY_RETIREMENT_TIMEOUT);
}

/// Recoveries allowed per tenant within `RECOVERY_WINDOW`. A proxy that keeps failing would
/// otherwise make the daemon re-initialize its tenant indefinitely with no client waiting; past
/// the budget the tenant retires, and the next command registers it again.
const RECOVERY_BUDGET: usize = 3;
const RECOVERY_WINDOW: Duration = Duration::from_secs(60);

static RECOVERIES: OnceLock<Mutex<BuckMutMap<(String, String), Vec<Instant>>>> = OnceLock::new();

/// Takes one recovery from the tenant's budget, if any is left.
pub(crate) fn take_recovery_budget(key: &ProxyKey) -> bool {
    let now = Instant::now();
    let mut recoveries = RECOVERIES
        .get_or_init(|| Mutex::new(BuckMutMap::default()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let recent = recoveries
        .entry((key.0.clone(), key.1.clone()))
        .or_default();
    recent.retain(|at| now.duration_since(*at) < RECOVERY_WINDOW);
    if recent.len() >= RECOVERY_BUDGET {
        return false;
    }
    recent.push(now);
    true
}

#[cfg(test)]
mod retire_tests {
    use std::os::unix::net::UnixStream;

    use super::Reservation;
    use super::reserve;
    use super::retire;
    use super::spawned;

    #[test]
    fn retire_before_spawn_marks_the_launch() -> buck2_error::Result<()> {
        let key = ("/retire/before-spawn".to_owned(), "v2".to_owned(), 1);
        let Reservation::Reserved { .. } = reserve(&key)? else {
            panic!("a fresh key is reserved");
        };
        retire(&key.0, &key.1, key.2);
        let (link, _proxy_end) = UnixStream::pair()?;
        assert!(
            spawned(&key, 1, link),
            "the launch learns that its generation was retired"
        );

        let key = ("/retire/after-spawn".to_owned(), "v2".to_owned(), 1);
        let Reservation::Reserved { .. } = reserve(&key)? else {
            panic!("a fresh key is reserved");
        };
        let (link, _proxy_end) = UnixStream::pair()?;
        assert!(!spawned(&key, 1, link), "nothing retired this generation");
        Ok(())
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::take_recovery_budget;

    #[test]
    fn recovery_budget_is_per_tenant_and_limited() {
        let tenant = ("/recovery/one".to_owned(), "v2".to_owned(), 1);
        let other = ("/recovery/two".to_owned(), "v2".to_owned(), 1);
        for _ in 0..super::RECOVERY_BUDGET {
            assert!(take_recovery_budget(&tenant), "within the budget");
        }
        assert!(
            !take_recovery_budget(&tenant),
            "past the budget within the window"
        );
        assert!(
            take_recovery_budget(&other),
            "another tenant has its own budget"
        );
    }
}
