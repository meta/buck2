/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
#[cfg(target_os = "linux")]
use std::os::unix::fs::DirBuilderExt;
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::sync::atomic::AtomicU64;
#[cfg(target_os = "linux")]
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use buck2_common::kill_util::try_terminate_process_gracefully;
use buck2_common::local_resource_state::LocalResourceState;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_data::ReleaseLocalResourcesEnd;
use buck2_data::ReleaseLocalResourcesStart;
use buck2_error::BuckErrorContext;
use buck2_events::dispatch::span_async_simple;
#[cfg(target_os = "linux")]
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_hash::BuckMutMap;
use dice::DiceComputations;
use dice::UserComputationData;
use dupe::Dupe;
use tokio::sync::Mutex;

use crate::local_resource_api::CleanupRegistration;

pub(crate) const CLEANUP_CAPABILITY_ENV: &str = "BUCK2_LOCAL_RESOURCE_CLEANUP_V1";

pub(crate) enum LocalResourceEntry {
    Legacy(LocalResourceState),
    Pending {
        state: LocalResourceState,
        lease: Arc<CleanupLease>,
    },
    Registered {
        state: LocalResourceState,
        lease: Arc<CleanupLease>,
    },
}

impl LocalResourceEntry {
    pub(crate) fn state(&self) -> buck2_error::Result<LocalResourceState> {
        match self {
            Self::Legacy(state) | Self::Registered { state, .. } => Ok(state.clone()),
            Self::Pending { .. } => Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::LocalResourceSetup,
                "Local resource registration was interrupted"
            )),
        }
    }
}

pub(crate) struct PreparedLocalResource {
    pub(crate) state: LocalResourceState,
    pub(crate) ownership: Option<SetupOwnership>,
}

pub(crate) struct CleanupLease {
    #[cfg(target_os = "linux")]
    stream: tokio::net::UnixStream,
}

impl CleanupLease {
    fn close(&self) {
        #[cfg(target_os = "linux")]
        {
            // SAFETY: The stream owns this open socket; shutdown takes an integer mode.
            unsafe {
                libc::shutdown(self.stream.as_raw_fd(), libc::SHUT_RDWR);
            }
        }
    }

    #[cfg(target_os = "linux")]
    async fn read_record(&self) -> buck2_error::Result<String> {
        let mut bytes = Vec::new();
        loop {
            self.stream.readable().await?;
            let mut byte = [0];
            match self.stream.try_read(&mut byte) {
                Ok(0) => {
                    return Err(buck2_error::buck2_error!(
                        buck2_error::ErrorTag::LocalResourceSetup,
                        "Cleanup owner closed its channel"
                    ));
                }
                Ok(_) if byte[0] == b'\n' => return Ok(String::from_utf8(bytes)?),
                Ok(_) if bytes.len() < 127 => bytes.push(byte[0]),
                Ok(_) => {
                    return Err(buck2_error::buck2_error!(
                        buck2_error::ErrorTag::LocalResourceSetup,
                        "Oversized cleanup protocol record"
                    ));
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e.into()),
            }
        }
    }

    #[cfg(target_os = "linux")]
    async fn write_record(&self, record: &str) -> buck2_error::Result<()> {
        let mut bytes = record.as_bytes();
        while !bytes.is_empty() {
            self.stream.writable().await?;
            match self.stream.try_write(bytes) {
                Ok(0) => {
                    return Err(buck2_error::buck2_error!(
                        buck2_error::ErrorTag::LocalResourceSetup,
                        "Cleanup owner write returned zero"
                    ));
                }
                Ok(n) => bytes = &bytes[n..],
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
}

pub(crate) struct SetupOwnership {
    pub(crate) lease: Arc<CleanupLease>,
    attempt_id: String,
    pid: i32,
    committed: bool,
    deadline: Option<Instant>,
}

impl Drop for SetupOwnership {
    fn drop(&mut self) {
        if !self.committed {
            self.lease.close();
        }
    }
}

impl SetupOwnership {
    pub(crate) fn validate(
        &self,
        descriptor: &CleanupRegistration,
        pid: Option<i32>,
    ) -> buck2_error::Result<()> {
        if descriptor.version != 1
            || descriptor.attempt_id != self.attempt_id
            || pid != Some(self.pid)
        {
            return Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::LocalResourceSetup,
                "Cleanup registration does not match the authenticated setup owner"
            ));
        }
        Ok(())
    }

    pub(crate) async fn register(&mut self) -> buck2_error::Result<()> {
        #[cfg(target_os = "linux")]
        {
            let registration = async {
                self.lease
                    .write_record(&format!("REGISTER 1 {}\n", self.attempt_id))
                    .await?;
                if self.lease.read_record().await? != format!("REGISTERED 1 {}", self.attempt_id) {
                    return Err(buck2_error::buck2_error!(
                        buck2_error::ErrorTag::LocalResourceSetup,
                        "Cleanup owner did not acknowledge registration"
                    ));
                }
                Ok(())
            };
            match self.deadline {
                Some(deadline) => tokio::time::timeout_at(deadline.into(), registration)
                    .await
                    .map_err(|_| {
                        buck2_error::buck2_error!(
                            buck2_error::ErrorTag::LocalResourceSetup,
                            "Local resource registration exceeded setup timeout"
                        )
                    })?,
                None => registration.await,
            }
        }
        #[cfg(not(target_os = "linux"))]
        Err(buck2_error::buck2_error!(
            buck2_error::ErrorTag::LocalResourceSetup,
            "Cleanup registration requires Linux peer credentials"
        ))
    }

    pub(crate) fn commit(mut self) -> Arc<CleanupLease> {
        self.committed = true;
        self.lease.dupe()
    }
}

pub(crate) struct CleanupSetup {
    #[cfg(target_os = "linux")]
    listener: Option<tokio::net::UnixListener>,
    #[cfg(target_os = "linux")]
    directory: Option<buck2_common::temp_path::TempPath>,
    #[cfg(target_os = "linux")]
    attempt_id: String,
    pub(crate) capability: String,
    pub(crate) ownership: Option<SetupOwnership>,
    connected: bool,
}

impl CleanupSetup {
    #[cfg(target_os = "linux")]
    pub(crate) fn create() -> buck2_error::Result<Self> {
        static NEXT_ATTEMPT: AtomicU64 = AtomicU64::new(0);
        Self::create_with_counter(&NEXT_ATTEMPT)
    }

    #[cfg(target_os = "linux")]
    fn create_with_counter(next_attempt: &AtomicU64) -> buck2_error::Result<Self> {
        for _ in 0..100 {
            let attempt_id = format!(
                "{:016x}{:016x}",
                std::process::id(),
                next_attempt.fetch_add(1, Ordering::Relaxed)
            );
            let path = AbsNormPathBuf::new(
                std::env::temp_dir().join(format!("buck2-cleanup-{attempt_id}")),
            )?;
            // Keep directory creation and guard adoption in one blocking operation.
            match std::fs::DirBuilder::new()
                .mode(0o700)
                .create(path.as_path())
            {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
            let directory = buck2_common::temp_path::TempPath::new_path(path);
            let socket = directory.path().as_path().join("owner");
            let listener = tokio::net::UnixListener::bind(&socket)?;
            // SAFETY: geteuid has no arguments or memory preconditions.
            let uid = unsafe { libc::geteuid() };
            let capability = serde_json::json!({"version": 1, "socket_path": socket, "attempt_id": attempt_id, "consumer_pid": std::process::id(), "consumer_uid": uid}).to_string();
            return Ok(Self {
                listener: Some(listener),
                directory: Some(directory),
                attempt_id,
                capability,
                ownership: None,
                connected: false,
            });
        }
        Err(buck2_error::buck2_error!(
            buck2_error::ErrorTag::LocalResourceSetup,
            "Could not create a private cleanup directory after 100 attempts"
        ))
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) fn create() -> buck2_error::Result<Self> {
        Err(buck2_error::buck2_error!(
            buck2_error::ErrorTag::LocalResourceSetup,
            "Cleanup registration requires Linux peer credentials"
        ))
    }

    #[cfg(target_os = "linux")]
    pub(crate) async fn accept(&mut self) -> buck2_error::Result<()> {
        let (stream, _) = self
            .listener
            .as_ref()
            .expect("Listener exists before accept")
            .accept()
            .await?;
        self.listener.take();
        self.directory.take();
        let peer = stream.peer_cred()?;
        let lease = Arc::new(CleanupLease { stream });
        let pid = peer.pid().filter(|pid| *pid > 0).ok_or_else(|| {
            buck2_error::buck2_error!(
                buck2_error::ErrorTag::LocalResourceSetup,
                "Cleanup peer has no valid PID"
            )
        })?;
        self.ownership = Some(SetupOwnership {
            lease,
            attempt_id: self.attempt_id.to_owned(),
            pid,
            committed: false,
            deadline: None,
        });
        // SAFETY: geteuid has no arguments or memory preconditions.
        if peer.uid() != unsafe { libc::geteuid() } {
            return Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::LocalResourceSetup,
                "Cleanup owner UID differs from consumer"
            ));
        }
        let owner = self
            .ownership
            .as_ref()
            .expect("Owner assigned before handshake");
        if owner.lease.read_record().await? != format!("HELLO 1 {} {}", self.attempt_id, pid) {
            return Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::LocalResourceSetup,
                "Invalid cleanup owner hello"
            ));
        }
        owner
            .lease
            .write_record(&format!("CONNECTED 1 {}\n", self.attempt_id))
            .await?;
        self.connected = true;
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) async fn accept(&mut self) -> buck2_error::Result<()> {
        unreachable!("Unsupported setup cannot be created")
    }

    pub(crate) fn finish(
        mut self,
        descriptor: Option<CleanupRegistration>,
        pid: Option<i32>,
        deadline: Option<Instant>,
    ) -> buck2_error::Result<Option<SetupOwnership>> {
        match (self.ownership.take(), descriptor) {
            (Some(mut owner), Some(descriptor)) if self.connected => {
                owner.validate(&descriptor, pid)?;
                owner.deadline = deadline;
                Ok(Some(owner))
            }
            (None, None) => Ok(None),
            _ => Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::LocalResourceSetup,
                "Missing authenticated cleanup session or registration descriptor"
            )),
        }
    }
}

type ResourceEntries = BuckMutMap<ConfiguredTargetLabel, buck2_error::Result<LocalResourceEntry>>;

#[derive(Default)]
pub struct LocalResourceRegistry(pub(crate) Arc<Mutex<ResourceEntries>>);

impl LocalResourceRegistry {
    pub(crate) async fn release_all_resources(&self) -> buck2_error::Result<()> {
        let resources = {
            let mut lock = self.0.lock().await;
            lock.drain()
                .flat_map(|(_, v)| v)
                .filter_map(|entry| match entry {
                    LocalResourceEntry::Legacy(state) => Some(state),
                    LocalResourceEntry::Pending { lease, .. }
                    | LocalResourceEntry::Registered { lease, .. } => {
                        lease.close();
                        None
                    }
                })
                .collect::<Vec<_>>()
        };

        if resources.is_empty() {
            return Ok(());
        }

        let cleanup = || async move {
            let resource_futs = resources
                .into_iter()
                .filter(|s| s.owning_pid().is_some())
                .map(|s| async move {
                    let pid = s.owning_pid().unwrap();
                    try_terminate_process_gracefully(pid, Duration::from_secs(20))
                        .await
                        .buck_error_context(format!(
                            "Failed to kill a process with `{}` PID to release local resource `{}`",
                            pid,
                            s.source_target()
                        ))
                });

            buck2_util::future::join_all(resource_futs.collect::<Vec<_>>())
                .await
                .into_iter()
                .collect::<Result<(), _>>()?;

            Ok::<(), buck2_error::Error>(())
        };

        let start = ReleaseLocalResourcesStart {};
        let end = ReleaseLocalResourcesEnd {};

        span_async_simple(start, cleanup(), end).await?;

        Ok::<(), buck2_error::Error>(())
    }
}

pub trait InitLocalResourceRegistry {
    fn init_local_resource_registry(&mut self);
}

pub trait HasLocalResourceRegistry {
    fn get_local_resource_registry(&self) -> buck2_error::Result<Arc<LocalResourceRegistry>>;
}

impl InitLocalResourceRegistry for UserComputationData {
    fn init_local_resource_registry(&mut self) {
        self.data.set(Arc::new(LocalResourceRegistry::default()));
    }
}

impl HasLocalResourceRegistry for DiceComputations<'_> {
    fn get_local_resource_registry(&self) -> buck2_error::Result<Arc<LocalResourceRegistry>> {
        let data = self
            .per_transaction_data()
            .data
            .get::<Arc<LocalResourceRegistry>>()
            .expect("LocalResourceRegistry should be set");

        Ok(data.dupe())
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;

    use buck2_common::local_resource_state::LocalResourceState;
    use buck2_core::configuration::data::ConfigurationData;
    use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
    use buck2_util::process::background_command;
    use tokio::net::UnixStream;

    use crate::local_resource_registry::CleanupLease;
    use crate::local_resource_registry::CleanupSetup;
    use crate::local_resource_registry::LocalResourceEntry;
    use crate::local_resource_registry::LocalResourceRegistry;
    use crate::local_resource_registry::SetupOwnership;

    fn owner() -> (SetupOwnership, Arc<CleanupLease>) {
        let (stream, peer) = UnixStream::pair().expect("Socket pair");
        (
            SetupOwnership {
                lease: Arc::new(CleanupLease { stream }),
                attempt_id: "attempt".to_owned(),
                pid: 42,
                committed: false,
                deadline: None,
            },
            Arc::new(CleanupLease { stream: peer }),
        )
    }

    #[tokio::test]
    async fn stale_cleanup_directory_is_preserved_and_setup_retries() {
        let first = 1_u64 << 63;
        let attempt = format!("{:016x}{first:016x}", std::process::id());
        let path = buck2_fs::paths::abs_norm_path::AbsNormPathBuf::new(
            std::env::temp_dir().join(format!("buck2-cleanup-{attempt}")),
        )
        .expect("Stale path");
        std::fs::create_dir(path.as_path()).expect("Precreate collision");
        let stale = buck2_common::temp_path::TempPath::new_path(path);
        let marker = stale.path().as_path().join("owner");
        std::fs::write(&marker, b"stale owner").expect("Stale marker");

        let counter = AtomicU64::new(first);
        let setup = CleanupSetup::create_with_counter(&counter).expect("Retry collision");
        let config: serde_json::Value =
            serde_json::from_str(&setup.capability).expect("Capability");
        let socket = std::path::PathBuf::from(config["socket_path"].as_str().expect("Path"));
        let directory = socket.parent().expect("Private directory").to_owned();
        assert_eq!(counter.load(Ordering::Relaxed), first + 2);
        assert_eq!(
            config["attempt_id"],
            format!("{:016x}{:016x}", std::process::id(), first + 1)
        );
        assert_eq!(
            std::fs::metadata(&directory)
                .expect("Directory")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert!(socket.exists());
        drop(setup);
        assert!(!directory.exists());
        assert_eq!(
            std::fs::read(marker).expect("Stale owner untouched"),
            b"stale owner"
        );
    }

    #[tokio::test]
    async fn cleanup_directory_collision_retries_are_bounded() {
        let first = (1_u64 << 63) + 1000;
        let mut stale = Vec::new();
        for counter in first..first + 100 {
            let attempt = format!("{:016x}{counter:016x}", std::process::id());
            let path = buck2_fs::paths::abs_norm_path::AbsNormPathBuf::new(
                std::env::temp_dir().join(format!("buck2-cleanup-{attempt}")),
            )
            .expect("Stale path");
            std::fs::create_dir(path.as_path()).expect("Precreate collision");
            stale.push(buck2_common::temp_path::TempPath::new_path(path));
        }
        let counter = AtomicU64::new(first);
        assert!(CleanupSetup::create_with_counter(&counter).is_err());
        assert_eq!(counter.load(Ordering::Relaxed), first + 100);
        assert!(
            stale
                .iter()
                .all(|directory| directory.path().as_path().exists())
        );
    }

    #[tokio::test]
    async fn drop_closes_lease_retained_by_pending_registry_entry() {
        let (owner, peer) = owner();
        let retained = owner.lease.clone();
        drop(owner);
        assert!(
            peer.read_record().await.is_err(),
            "Rollback must close even a retained lease"
        );
        drop(retained);
    }

    #[tokio::test]
    async fn lost_registration_ack_rolls_back() {
        let (mut owner, peer) = owner();
        let registration = async {
            assert_eq!(
                peer.read_record().await.expect("Register"),
                "REGISTER 1 attempt"
            );
            peer.close();
        };
        let (result, ()) = tokio::join!(owner.register(), registration);
        assert!(result.is_err());
        drop(owner);
    }

    #[tokio::test]
    async fn cancellation_during_ack_closes_pending_lease() {
        let (mut owner, peer) = owner();
        let retained = owner.lease.clone();
        let future = async move { owner.register().await };
        let mut future = Box::pin(future);
        tokio::select! {
            result = &mut future => panic!("Registration ended without ack: {result:?}"),
            record = peer.read_record() => assert_eq!(record.expect("Register"), "REGISTER 1 attempt"),
        }
        drop(future);
        assert!(peer.read_record().await.is_err());
        drop(retained);
    }

    #[tokio::test]
    async fn protocol_release_uses_channel_not_pid_kill() {
        let (mut owner, peer) = owner();
        let ack = async {
            peer.read_record().await.expect("Register");
            peer.write_record("REGISTERED 1 attempt\n")
                .await
                .expect("Ack");
        };
        let (result, ()) = tokio::join!(owner.register(), ack);
        result.expect("Registration");
        let registry = LocalResourceRegistry::default();
        let target = ConfiguredTargetLabel::testing_parse(
            "cell//pkg:owner",
            ConfigurationData::testing_new(),
        );
        let mut child = background_command("/bin/sleep")
            .arg("60")
            .spawn()
            .expect("Task-owned child");
        let state = LocalResourceState::new(target, Some(child.id() as i32), vec![]);
        registry.0.lock().await.insert(
            target,
            Ok(LocalResourceEntry::Registered {
                state,
                lease: owner.commit(),
            }),
        );
        registry
            .release_all_resources()
            .await
            .expect("Channel release");
        let alive = child.try_wait().expect("Child status").is_none();
        drop(child.kill());
        child.wait().expect("Reap child");
        assert!(alive, "Protocol release must not signal the owned PID");
        assert!(peer.read_record().await.is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_blocking_setup_drops_unclaimed_listener() {
        struct DropSignal(Option<tokio::sync::oneshot::Sender<()>>);
        impl Drop for DropSignal {
            fn drop(&mut self) {
                let _ = self.0.take().expect("Drop sender").send(());
            }
        }

        let (created_tx, created_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (dropped_tx, dropped_rx) = tokio::sync::oneshot::channel();
        let task = buck2_fs::async_fs_util::spawn_blocking(move || {
            let setup = CleanupSetup::create().expect("Blocking listener creation");
            let config: serde_json::Value =
                serde_json::from_str(&setup.capability).expect("Capability");
            let socket = std::path::PathBuf::from(config["socket_path"].as_str().expect("Path"));
            created_tx.send(socket).expect("Created listener");
            release_rx.recv().expect("Release construction");
            (setup, DropSignal(Some(dropped_tx)))
        });
        let socket = tokio::time::timeout(std::time::Duration::from_secs(5), created_rx)
            .await
            .expect("Blocking constructor completed")
            .expect("Listener path");
        let directory = socket.parent().expect("Private directory").to_owned();
        assert!(tokio::fs::metadata(&socket).await.is_ok());
        drop(task);
        release_tx.send(()).expect("Finish detached constructor");
        tokio::time::timeout(std::time::Duration::from_secs(5), dropped_rx)
            .await
            .expect("Unclaimed setup was dropped")
            .expect("Drop notification");
        for path in [socket, directory] {
            assert_eq!(
                tokio::fs::metadata(path)
                    .await
                    .expect_err("Setup removed")
                    .kind(),
                std::io::ErrorKind::NotFound
            );
        }
    }

    #[tokio::test]
    async fn setup_authenticates_peer_and_sockets_are_cloexec() {
        let mut setup = CleanupSetup::create().expect("Listener");
        let config: serde_json::Value =
            serde_json::from_str(&setup.capability).expect("Capability");
        let attempt = config["attempt_id"].as_str().expect("Attempt").to_owned();
        let client = async {
            let stream = UnixStream::connect(config["socket_path"].as_str().expect("Path"))
                .await
                .expect("Connect");
            // SAFETY: F_GETFD takes no variadic argument; stream owns its descriptor.
            assert_ne!(
                unsafe { libc::fcntl(stream.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
                0
            );
            let peer = CleanupLease { stream };
            peer.write_record(&format!("HELLO 1 {attempt} {}\n", std::process::id()))
                .await
                .expect("Hello");
            assert_eq!(
                peer.read_record().await.expect("Connected"),
                format!("CONNECTED 1 {attempt}")
            );
            peer
        };
        let (result, peer) = tokio::join!(setup.accept(), client);
        result.expect("Authenticated handshake");
        let stream = &setup.ownership.as_ref().expect("Owner").lease.stream;
        // SAFETY: F_GETFD takes no variadic argument; stream owns its descriptor.
        assert_ne!(
            unsafe { libc::fcntl(stream.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        drop(setup);
        assert!(peer.read_record().await.is_err());
    }
}
