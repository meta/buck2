/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! A guard that `SIGKILL`s a process's detached descendants when dropped, so that a kill abandoned
//! midway, for example by dropping its future during a grace period, still leaves nothing behind.
//! The process group itself has [`buck2_process_kill::KillGroupOnDrop`].

#[cfg(target_os = "linux")]
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::sync::Mutex;
#[cfg(target_os = "linux")]
use std::sync::MutexGuard;

#[cfg(target_os = "linux")]
use buck2_process_kill::ProcessId;

/// Kills the descendants of a process that left its process group, such as daemons, as listed
/// when the guard was created and when it was last refreshed. Only Linux can list them; elsewhere
/// the guard kills nothing.
#[derive(Default)]
pub(crate) struct KillDetachedDescendantsOnDrop {
    #[cfg(target_os = "linux")]
    listing: Option<Arc<Mutex<Listing>>>,
    /// The group leader whose descendants were listed, while it is unreaped.
    #[cfg(target_os = "linux")]
    leader: Option<i32>,
}

/// Shared between a guard and the blocking task listing its processes, so that whichever of them
/// finishes last kills them: a guard dropped while a list is still being made does not leave the
/// processes that list finds behind.
#[cfg(target_os = "linux")]
enum Listing {
    /// A listing is running; holds what earlier ones found.
    Pending(Vec<ProcessId>),
    Done(Vec<ProcessId>),
    Abandoned,
}

#[cfg(target_os = "linux")]
fn lock(listing: &Mutex<Listing>) -> MutexGuard<'_, Listing> {
    listing.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(target_os = "linux")]
fn kill_all(found: &[ProcessId]) {
    for p in found {
        if let Err(e) = p.kill() {
            tracing::warn!("Failed to kill detached descendant {}: {}", p.pid(), e);
        }
    }
}

/// With `group`, a listing abandoned before it finished also kills that process group: its leader
/// was kept alive until then, since descendants it adopted are reparented to init once it dies.
#[cfg(target_os = "linux")]
fn finish_listing(listing: &Mutex<Listing>, found: Vec<ProcessId>, group: Option<i32>) {
    let mut listing = lock(listing);
    match &mut *listing {
        Listing::Abandoned => {
            if let Some(pgid) = group {
                let _ignored = buck2_process_kill::kill_group(pgid);
            }
            kill_all(&found);
        }
        Listing::Pending(known) | Listing::Done(known) => {
            let mut known = std::mem::take(known);
            for p in found {
                if !known.contains(&p) {
                    known.push(p);
                }
            }
            *listing = Listing::Done(known);
        }
    }
}

impl KillDetachedDescendantsOnDrop {
    /// Lists the detached descendants of `pgid`'s leader. Must run before the leader exits: its
    /// detached descendants are then reparented to init and can no longer be found. If this future
    /// is dropped first, the listing kills the group as well as what it finds, once it is done.
    /// Best effort: failing to list them only leaves them alive.
    #[cfg(target_os = "linux")]
    pub(crate) async fn of(pgid: i32) -> Self {
        let listing = Arc::new(Mutex::new(Listing::Pending(Vec::new())));
        let guard = Self {
            listing: Some(Arc::clone(&listing)),
            leader: Some(pgid),
        };
        // Reads all of `/proc`, so it runs off the async runtime.
        let task = tokio::task::spawn_blocking(move || {
            let found = buck2_process_kill::descendants(pgid);
            finish_listing(&listing, found, Some(pgid))
        });
        if let Err(e) = task.await {
            tracing::warn!("Failed to list descendants of {}: {}", pgid, e);
        }
        guard
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) async fn of(_pgid: i32) -> Self {
        Self::default()
    }

    /// Adds what the leader and the listed processes that are still running started since they
    /// were listed, for example during a grace period. A leader that has exited by then is no
    /// help: its children were reparented to init when it exited.
    #[cfg(target_os = "linux")]
    pub(crate) async fn refresh(&mut self) {
        let Some(listing) = &self.listing else {
            return;
        };
        let roots = {
            let mut state = lock(listing);
            match std::mem::replace(&mut *state, Listing::Abandoned) {
                Listing::Done(known) => {
                    let roots = known.clone();
                    *state = Listing::Pending(known);
                    roots
                }
                other => {
                    *state = other;
                    return;
                }
            }
        };
        let listing = Arc::clone(listing);
        let leader = self.leader;
        let task = tokio::task::spawn_blocking(move || {
            let found = buck2_process_kill::descendants_of(leader, &roots);
            finish_listing(&listing, found, None)
        });
        if let Err(e) = task.await {
            tracing::warn!("Failed to list descendants: {}", e);
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) async fn refresh(&mut self) {}
}

#[cfg(target_os = "linux")]
impl Drop for KillDetachedDescendantsOnDrop {
    fn drop(&mut self) {
        if let Some(listing) = self.listing.take() {
            match std::mem::replace(&mut *lock(&listing), Listing::Abandoned) {
                Listing::Pending(known) | Listing::Done(known) => kill_all(&known),
                Listing::Abandoned => {}
            }
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::io::BufRead;
    use std::io::BufReader;
    use std::os::unix::process::CommandExt;
    use std::os::unix::process::ExitStatusExt;
    use std::process::Child;
    use std::process::Command;
    use std::process::Stdio;

    use super::*;

    fn spawn_sh(script: &str) -> std::io::Result<Child> {
        // ast-grep-ignore: rust/buck2-no-command-new
        Command::new("sh")
            .arg("-c")
            .arg(script)
            .process_group(0)
            .stdout(Stdio::piped())
            .spawn()
    }

    /// This process's child `child`, as `descendants` lists it. Guards in these tests only ever
    /// hold processes the test started, never the other descendants of the test binary.
    fn process_id(child: &Child) -> ProcessId {
        buck2_process_kill::descendants(std::process::id() as i32)
            .into_iter()
            .find(|p| p.pid() == child.id() as i32)
            .expect("a child should be a descendant")
    }

    fn guard_with(listing: Listing) -> (KillDetachedDescendantsOnDrop, Arc<Mutex<Listing>>) {
        let listing = Arc::new(Mutex::new(listing));
        let guard = KillDetachedDescendantsOnDrop {
            listing: Some(Arc::clone(&listing)),
            leader: None,
        };
        (guard, listing)
    }

    fn assert_sigkilled(child: &mut Child) -> std::io::Result<()> {
        assert_eq!(child.wait()?.signal(), Some(libc::SIGKILL));
        Ok(())
    }

    #[test]
    fn test_dropping_kills_listed_processes() -> std::io::Result<()> {
        let mut child = spawn_sh("exec sleep 1000")?;
        let (guard, _listing) = guard_with(Listing::Done(vec![process_id(&child)]));
        drop(guard);
        assert_sigkilled(&mut child)
    }

    #[test]
    fn test_a_listing_finished_after_drop_kills_the_group_and_what_it_found() -> std::io::Result<()>
    {
        let mut leader = spawn_sh("exec sleep 1000")?;
        let mut found = spawn_sh("exec sleep 1000")?;
        let (guard, listing) = guard_with(Listing::Pending(Vec::new()));
        drop(guard);
        finish_listing(&listing, vec![process_id(&found)], Some(leader.id() as i32));
        assert_sigkilled(&mut leader)?;
        assert_sigkilled(&mut found)
    }

    /// Asserts that `pid`, which is not this process's child, is gone or a zombie within 30 s. If
    /// not, kills it before failing, so that a failing test leaves nothing behind.
    fn assert_killed_soon(pid: i32) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !gone_or_zombie(pid) {
            if std::time::Instant::now() >= deadline {
                // SAFETY: `kill` has no memory safety requirements.
                unsafe { libc::kill(pid, libc::SIGKILL) };
                panic!("process {pid} should have been killed");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// Whether `pid`, which is not this process's child, is gone or a zombie.
    fn gone_or_zombie(pid: i32) -> bool {
        std::fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |stat| {
            stat.rsplit_once(')')
                .is_some_and(|(_, rest)| rest.trim_start().starts_with('Z'))
        })
    }

    #[tokio::test]
    async fn test_refresh_adds_processes_started_since_the_listing() -> std::io::Result<()> {
        use std::io::Write;

        // ast-grep-ignore: rust/buck2-no-command-new
        let mut parent = Command::new("sh")
            .arg("-c")
            .arg("read _; sleep 1000 </dev/null >/dev/null 2>&1 & echo $!; wait")
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let (mut guard, _listing) = guard_with(Listing::Done(vec![process_id(&parent)]));

        // The parent starts its child only once it has been listed.
        writeln!(parent.stdin.take().unwrap())?;
        let mut line = String::new();
        BufReader::new(parent.stdout.take().unwrap()).read_line(&mut line)?;
        let late_child: i32 = line.trim().parse().unwrap();

        guard.refresh().await;
        drop(guard);
        assert_sigkilled(&mut parent)?;
        assert_killed_soon(late_child);
        Ok(())
    }

    #[tokio::test]
    async fn test_refresh_adds_processes_the_leader_detached_since_the_listing()
    -> std::io::Result<()> {
        use std::io::Write;

        // ast-grep-ignore: rust/buck2-no-command-new
        let mut leader = Command::new("sh")
            .arg("-c")
            .arg("read _; setsid sleep 1000 </dev/null >/dev/null 2>&1 & echo $!; wait")
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let mut guard = KillDetachedDescendantsOnDrop::of(leader.id() as i32).await;

        // The leader detaches its child only once it has been listed.
        writeln!(leader.stdin.take().unwrap())?;
        let mut line = String::new();
        BufReader::new(leader.stdout.take().unwrap()).read_line(&mut line)?;
        let detached: i32 = line.trim().parse().unwrap();

        guard.refresh().await;
        drop(guard);
        assert_killed_soon(detached);
        leader.kill()?;
        leader.wait()?;
        Ok(())
    }
}
