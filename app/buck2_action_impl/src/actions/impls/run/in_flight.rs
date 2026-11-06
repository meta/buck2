/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Merges identical `run` actions that are in flight at the same time
//! (`buck2.dedupe_in_flight_actions`).
//!
//! With content-based paths, the same command in two configurations has one action digest. The
//! first action to arrive with a given key is the leader: it looks up the caches and executes as
//! usual. An action that arrives while the leader runs is a follower: it waits, and takes the
//! leader's result instead of looking up the action cache or executing.
//!
//! - A leader that succeeds publishes its output values, whether it executed or hit a cache
//!   (the action cache, or a dep file cache after the action cache missed). Content-based
//!   outputs of leader and follower resolve to the same paths, so the follower has nothing to
//!   materialize that the leader has not already declared.
//! - A leader whose command exited with a code from 1 to 127 publishes that exit code and its
//!   output streams, and each follower fails with them. A flaky failure is shared rather than
//!   retried, as it is when the same command fails in sibling configurations without this key.
//! - A leader that times out or fails for any other reason shares nothing: an executor error, a
//!   worker failure, a missing input, or an exit code of 128 or more, which is how a command
//!   killed by a signal (an OOM kill included) is reported. Those say more about the machine than
//!   about the command. It releases its followers, and each runs as without this key, rather
//!   than one after another behind a new leader each. On RE they run at once; locally their
//!   shared placeholder still runs them one at a time.
//! - A leader that is cancelled publishes nothing. Its followers race to become the next leader,
//!   and the others follow that one.
//!
//! The registry lives for the daemon, across concurrent commands. It holds an entry while its
//! leader runs, and after a success until the leader's cache upload finishes: an action that
//! arrives during the upload would miss the action cache, so it takes the published result
//! instead.

use std::sync::Arc;
use std::sync::LazyLock;

use buck2_build_api::actions::execute::action_executor::ActionOutputs;
use buck2_core::content_hash::ContentBasedPathHash;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::execute::action_digest::ActionDigest;
use buck2_execute::execute::executor_stage_async;
use buck2_execute::execute::output::CommandStdStreams;
use buck2_execute::execute::request::CommandExecutionOutput;
use buck2_execute::execute::request::CommandExecutionRequest;
use buck2_execute::execute::result::CommandExecutionResult;
use buck2_execute::execute::result::CommandExecutionStatus;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutMap;
use dupe::Dupe;
use parking_lot::Mutex;
use tokio::sync::watch;

/// What makes two executions interchangeable. The action digest covers the command, inputs,
/// environment, output paths and RE platform; the rest is what changes a result, or how it is
/// produced, outside the digest.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct InFlightKey {
    pub(crate) digest: ActionDigest,
    /// Hash of the executor configuration: local, remote or hybrid, and its options.
    pub(crate) executor: u64,
    /// Hash of the command's options that change how it runs: strategy, cache reads and writes,
    /// uploading every action. The registry spans commands, so a `--no-remote-cache` command must
    /// not take a result its leader got from the action cache.
    pub(crate) command_options: u64,
    /// The action's `ExecutorPreference`, as displayed.
    pub(crate) executor_preference: String,
    /// Whether this action uploads its result to the action cache.
    pub(crate) allow_cache_upload: bool,
    /// The remote dep-file entry this action uploads, if any: only the leader uploads.
    pub(crate) remote_dep_file: Option<ActionDigest>,
    /// Hash of the local worker: executable, environment, concurrency, streaming and inputs.
    /// The action digest has none of these.
    pub(crate) worker: Option<u64>,
    /// Hash of the project root and isolation dir. The registry spans the daemon, and a daemon
    /// can host several of those, each with its own materializer.
    pub(crate) tenant: u64,
}

/// How a leader's command ended, when its followers can share it.
pub(crate) enum LeaderOutcome {
    Succeeded {
        /// Output values by the placeholder path the command wrote each one to. That path is part
        /// of the action digest, so a follower's outputs resolve to the same paths.
        outputs: BuckMutMap<ProjectRelativePathBuf, ArtifactValue>,
    },
    /// The command exited with a code from 1 to 127.
    Failed {
        /// The outputs the failed command left, by placeholder path, for error handlers that
        /// read them.
        outputs: BuckMutMap<ProjectRelativePathBuf, ArtifactValue>,
        exit_code: Option<i32>,
        std_streams: CommandStdStreams,
    },
}

/// A leader's result that its followers take as their own.
pub(crate) struct LeaderResult {
    pub(crate) outcome: LeaderOutcome,
    pub(crate) leader: buck2_data::ActionKey,
    pub(crate) leader_kind: buck2_data::ActionExecutionKind,
}

impl LeaderResult {
    /// The result to publish, or `None` when the leader's followers should run themselves.
    pub(crate) fn new(
        result: &CommandExecutionResult,
        fs: &ArtifactFs,
        leader: buck2_data::ActionKey,
    ) -> buck2_error::Result<Option<Self>> {
        let outputs = || -> buck2_error::Result<_> {
            let mut outputs = BuckMutMap::default();
            for output in result.resolve_outputs(fs) {
                let (resolved, _content_path, value) = output?;
                outputs.insert(resolved.into_path(), value.dupe());
            }
            Ok(outputs)
        };
        let outcome = match &result.report.status {
            CommandExecutionStatus::Success { .. } => LeaderOutcome::Succeeded {
                outputs: outputs()?,
            },
            CommandExecutionStatus::Failure {
                missing_materialized_inputs: false,
                ..
            } if matches!(result.report.exit_code, Some(1..=127)) => LeaderOutcome::Failed {
                outputs: outputs()?,
                exit_code: result.report.exit_code,
                std_streams: result.report.std_streams.clone(),
            },
            _ => return Ok(None),
        };
        let leader_kind = result
            .report
            .status
            .execution_kind()
            .map_or(buck2_data::ActionExecutionKind::NotSet, |k| k.as_enum());
        Ok(Some(Self {
            outcome,
            leader,
            leader_kind,
        }))
    }

    /// The result of a leader served by the local dep file cache.
    pub(crate) fn from_dep_file_hit(
        outputs: &ActionOutputs,
        fs: &ArtifactFs,
        leader: buck2_data::ActionKey,
    ) -> buck2_error::Result<Self> {
        let mut by_path = BuckMutMap::default();
        for (path, value) in outputs.iter() {
            by_path.insert(
                fs.resolve_build(path, Some(&ContentBasedPathHash::OutputArtifact))?,
                value.dupe(),
            );
        }
        Ok(Self {
            outcome: LeaderOutcome::Succeeded { outputs: by_path },
            leader,
            leader_kind: buck2_data::ActionExecutionKind::LocalDepFile,
        })
    }

    /// A failed leader's values keyed by those of `request`'s outputs it left.
    pub(crate) fn failed_outputs_for(
        outputs: &BuckMutMap<ProjectRelativePathBuf, ArtifactValue>,
        request: &CommandExecutionRequest,
        fs: &ArtifactFs,
    ) -> buck2_error::Result<BuckMutInsertionMap<CommandExecutionOutput, ArtifactValue>> {
        let mut by_output = BuckMutInsertionMap::default();
        for output in request.outputs() {
            let path = output
                .resolve(fs, Some(&ContentBasedPathHash::OutputArtifact))?
                .into_path();
            if let Some(value) = outputs.get(&path) {
                by_output.insert(output.cloned(), value.dupe());
            }
        }
        Ok(by_output)
    }

    /// A succeeded leader's values keyed by `request`'s outputs, or `None` if one of them has no
    /// value.
    pub(crate) fn outputs_for(
        outputs: &BuckMutMap<ProjectRelativePathBuf, ArtifactValue>,
        request: &CommandExecutionRequest,
        fs: &ArtifactFs,
    ) -> buck2_error::Result<Option<BuckMutInsertionMap<CommandExecutionOutput, ArtifactValue>>>
    {
        let mut by_output = BuckMutInsertionMap::default();
        for output in request.outputs() {
            let path = output
                .resolve(fs, Some(&ContentBasedPathHash::OutputArtifact))?
                .into_path();
            let Some(value) = outputs.get(&path) else {
                return Ok(None);
            };
            by_output.insert(output.cloned(), value.dupe());
        }
        Ok(Some(by_output))
    }
}

/// What a leader hands its followers.
#[derive(Clone)]
enum Published {
    Result(Arc<LeaderResult>),
    /// Each follower runs itself.
    Release,
}

type Slot = Option<Published>;

/// A running leader: who it is, and where its result will appear.
struct Entry {
    leader: buck2_data::ActionKey,
    rx: watch::Receiver<Slot>,
}

static IN_FLIGHT: LazyLock<Mutex<BuckMutMap<InFlightKey, Entry>>> =
    LazyLock::new(|| Mutex::new(BuckMutMap::default()));

pub(crate) enum Gate {
    /// No identical action is running: run this one, and publish its result if it succeeds.
    Leader(LeaderGuard),
    /// An identical action's result can be shared: it succeeded, or its command exited with a code
    /// from 1 to 127, while this one waited or before its leader finished uploading.
    Follower(Arc<LeaderResult>),
    /// The identical action this one waited for ended in a way that cannot be shared: run this
    /// one, as without the key.
    Alone,
}

/// Waits until this action, `action`, may run as a leader or can take a leader's result.
pub(crate) async fn enter(key: InFlightKey, action: buck2_data::ActionKey) -> Gate {
    loop {
        let (leader, mut rx) = {
            let mut in_flight = IN_FLIGHT.lock();
            match in_flight.get(&key) {
                Some(entry) => (entry.leader.clone(), entry.rx.clone()),
                None => {
                    let (tx, rx) = watch::channel(None);
                    in_flight.insert(key.clone(), Entry { leader: action, rx });
                    return Gate::Leader(LeaderGuard { key, tx });
                }
            }
        };
        // The channel closes without a value when the leader is cancelled. Go round again: one
        // of the waiters becomes the next leader.
        let stage = buck2_data::InFlightWait {
            leader_action_key: Some(leader),
        };
        let published = executor_stage_async(stage, async {
            rx.wait_for(Option::is_some)
                .await
                .ok()
                .and_then(|slot| slot.clone())
        })
        .await;
        match published {
            Some(Published::Result(result)) => return Gate::Follower(result),
            Some(Published::Release) => return Gate::Alone,
            None => {}
        }
    }
}

/// Held by a leader while it runs and uploads. Dropping it removes the leader's entry and closes
/// the channel, whether or not a result was published.
pub(crate) struct LeaderGuard {
    key: InFlightKey,
    tx: watch::Sender<Slot>,
}

impl LeaderGuard {
    /// Hands `result` to the followers waiting now and to any that arrive before the guard is
    /// dropped.
    pub(crate) fn publish(&self, result: LeaderResult) {
        // No receivers just means nobody waited.
        let _ignored = self.tx.send(Some(Published::Result(Arc::new(result))));
    }

    /// Lets the followers waiting now, and any that arrive before the entry is gone, run
    /// themselves: this leader ended in a way they cannot share.
    pub(crate) fn release(self) {
        let _ignored = self.tx.send(Some(Published::Release));
    }
}

impl Drop for LeaderGuard {
    fn drop(&mut self) {
        let ours = self.tx.subscribe();
        let mut in_flight = IN_FLIGHT.lock();
        if in_flight
            .get(&self.key)
            .is_some_and(|entry| entry.rx.same_channel(&ours))
        {
            in_flight.remove(&self.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn key(n: u8) -> InFlightKey {
        InFlightKey {
            digest: ActionDigest::new_sha1([n; 20], 1),
            executor: 0,
            command_options: 0,
            executor_preference: "default".to_owned(),
            allow_cache_upload: false,
            remote_dep_file: None,
            worker: None,
            tenant: 0,
        }
    }

    fn result(kind: buck2_data::ActionExecutionKind) -> LeaderResult {
        LeaderResult {
            outcome: LeaderOutcome::Succeeded {
                outputs: BuckMutMap::default(),
            },
            leader: buck2_data::ActionKey::default(),
            leader_kind: kind,
        }
    }

    fn leader(gate: Gate) -> LeaderGuard {
        match gate {
            Gate::Leader(guard) => guard,
            Gate::Follower(_) | Gate::Alone => panic!("expected to lead"),
        }
    }

    #[tokio::test]
    async fn follower_takes_published_result() {
        let guard = leader(enter(key(1), buck2_data::ActionKey::default()).await);
        let follower = tokio::spawn(enter(key(1), buck2_data::ActionKey::default()));
        tokio::time::sleep(Duration::from_millis(50)).await;
        guard.publish(result(buck2_data::ActionExecutionKind::Remote));
        match follower.await.unwrap() {
            Gate::Follower(r) => assert_eq!(r.leader_kind, buck2_data::ActionExecutionKind::Remote),
            Gate::Leader(_) | Gate::Alone => panic!("expected to follow"),
        }
    }

    #[tokio::test]
    async fn followers_run_alone_after_a_release() {
        let guard = leader(enter(key(5), buck2_data::ActionKey::default()).await);
        let followers = [
            tokio::spawn(enter(key(5), buck2_data::ActionKey::default())),
            tokio::spawn(enter(key(5), buck2_data::ActionKey::default())),
        ];
        tokio::time::sleep(Duration::from_millis(50)).await;
        guard.release();
        // Both run now; neither waits for the other as a new leader.
        for follower in followers {
            assert!(matches!(follower.await.unwrap(), Gate::Alone));
        }
        assert!(!IN_FLIGHT.lock().contains_key(&key(5)));
    }

    #[tokio::test]
    async fn follower_leads_when_leader_gives_up() {
        let guard = leader(enter(key(2), buck2_data::ActionKey::default()).await);
        let follower = tokio::spawn(enter(key(2), buck2_data::ActionKey::default()));
        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(guard);
        let next = leader(follower.await.unwrap());
        // The new leader's entry is the one in the registry now.
        let late = tokio::spawn(enter(key(2), buck2_data::ActionKey::default()));
        tokio::time::sleep(Duration::from_millis(50)).await;
        next.publish(result(buck2_data::ActionExecutionKind::Local));
        assert!(matches!(late.await.unwrap(), Gate::Follower(_)));
    }

    #[tokio::test]
    async fn arrival_after_publish_follows_until_guard_drops() {
        let guard = leader(enter(key(4), buck2_data::ActionKey::default()).await);
        guard.publish(result(buck2_data::ActionExecutionKind::Local));
        // The leader is uploading: a new arrival takes the published result.
        match enter(key(4), buck2_data::ActionKey::default()).await {
            Gate::Follower(r) => assert_eq!(r.leader_kind, buck2_data::ActionExecutionKind::Local),
            Gate::Leader(_) | Gate::Alone => panic!("expected to follow"),
        }
        drop(guard);
        assert!(!IN_FLIGHT.lock().contains_key(&key(4)));
    }

    #[tokio::test]
    async fn entry_ends_with_its_leader() {
        leader(enter(key(3), buck2_data::ActionKey::default()).await)
            .publish(result(buck2_data::ActionExecutionKind::Local));
        leader(enter(key(3), buck2_data::ActionKey::default()).await);
        assert!(!IN_FLIGHT.lock().contains_key(&key(3)));
    }
}
