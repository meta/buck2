/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use buck2_common::cas_digest::CasDigestConfig;
use buck2_common::cas_digest::file_digest_interner;
use buck2_common::file_ops::metadata::FileDigest;
use buck2_common::file_ops::metadata::FileMetadata;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_core::execution_types::executor_config::RemoteExecutorUseCase;
use buck2_core::fs::buck_out_path::BuckOutPathKind;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project::ProjectRootTemp;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_directory::directory::entry::DirectoryEntry;
use buck2_events::dispatch::EventDispatcher;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::blob_store::BlobStore;
use buck2_execute::blob_store::FileToLink;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::directory::ActionDirectoryBuilder;
use buck2_execute::directory::ActionDirectoryEntry;
use buck2_execute::directory::ActionDirectoryMember;
use buck2_execute::directory::ActionImmutableDirectory;
use buck2_execute::directory::INTERNER;
use buck2_execute::execute::blocking::testing::DummyBlockingExecutor;
use buck2_execute::materialize::materializer::CleanStaleArtifactsArgs;
use buck2_execute::materialize::materializer::CleanStaleArtifactsPolicy;
use buck2_execute::materialize::materializer::CopiedArtifact;
use buck2_execute::materialize::materializer::DeclareArtifactPayload;
use buck2_execute::materialize::materializer::DeclareMatchOutcome;
use buck2_execute::materialize::materializer::MaterializationPurpose;
use buck2_execute::materialize::materializer::MaterializeRequest;
use buck2_execute::materialize::materializer::MaterializeResponse;
use buck2_execute::materialize::materializer::Materializer;
use buck2_execute::materialize::materializer::WriteLease;
use buck2_execute::materialize::materializer::WriteRequest;
use buck2_execute::materialize::produce::Produced;
use buck2_execute::materialize::produce::produce_locally;
use buck2_execute::materialize::produce::write_locally;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_hash::BuckMutMap;
use buck2_hash::IntentionallyStdHashMap;
use buck2_util::threads::ignore_stack_overflow_checks_for_future;
use dupe::Dupe;
use jiff::SignedDuration;
use jiff::Timestamp;
use tokio::time::sleep;
use tokio::time::timeout;

use crate::materializers::deferred::AccessTimesUpdates;
use crate::materializers::deferred::DeferredMaterializerConfigs;
use crate::materializers::deferred::TtlRefreshConfiguration;
use crate::materializers::deferred::artifact_tree::ArtifactClassification;
use crate::materializers::deferred::clean_stale::CleanStaleConfig;
use crate::materializers::deferred::tests::state_machine::Op;
use crate::materializers::deferred::tests::state_machine::StubIoHandler;
use crate::materializers::v2::core::DiskStateMaterializer;
use crate::sqlite::materializer_db::MaterializerStateSqliteDb;
use crate::sqlite::materializer_db::testing_materializer_state_sqlite_db;

fn path(s: &str) -> ProjectRelativePathBuf {
    ProjectRelativePathBuf::unchecked_new(s.to_owned())
}

fn file(content: &str) -> ArtifactValue {
    ArtifactValue::file(FileMetadata {
        digest: TrackedFileDigest::from_content(
            content.as_bytes(),
            DigestConfig::testing_default().cas_digest_config(),
        ),
        is_executable: false,
    })
}

fn file_exec(content: &str) -> ArtifactValue {
    ArtifactValue::file(FileMetadata {
        digest: TrackedFileDigest::from_content(
            content.as_bytes(),
            DigestConfig::testing_default().cas_digest_config(),
        ),
        is_executable: true,
    })
}

/// An exhaustive directory of `(relative path, content)` files.
fn dir(files: &[(&str, &str)]) -> ArtifactValue {
    let mut builder = ActionDirectoryBuilder::empty_exhaustive();
    for (p, content) in files {
        builder
            .insert(
                ForwardRelativePath::unchecked_new(p),
                file(content).entry().dupe().map_dir(|d| d.into_builder()),
            )
            .unwrap();
    }
    builder.mark_uniformly_exhaustive();
    ArtifactValue::dir(
        builder
            .fingerprint(DigestConfig::testing_default().as_directory_serializer())
            .shared(&*INTERNER),
    )
}

/// `value`, with a dep region: `dep_path` holding `dep`, under a non-exhaustive scaffold from the
/// project root, which is how symlink targets travel with a value.
fn with_dep(value: &ArtifactValue, dep_path: &str, dep: &ArtifactValue) -> ArtifactValue {
    let mut deps = ActionDirectoryBuilder::empty_non_exhaustive();
    deps.insert(
        ForwardRelativePath::unchecked_new(dep_path),
        dep.entry().dupe().map_dir(|d| d.into_builder()),
    )
    .unwrap();
    let deps = deps
        .fingerprint(DigestConfig::testing_default().as_directory_serializer())
        .shared(&*INTERNER);
    ArtifactValue::new(value.entry().dupe(), Some(deps))
}

fn configs() -> DeferredMaterializerConfigs {
    DeferredMaterializerConfigs {
        materialize_final_artifacts: true,
        defer_write_actions: false,
        ttl_refresh: TtlRefreshConfiguration {
            frequency: std::time::Duration::default(),
            min_ttl: jiff::SignedDuration::ZERO,
            enabled: false,
        },
        update_access_times: AccessTimesUpdates::Disabled,
        verbose_materializer_log: false,
        clean_stale_config: CleanStaleConfig::default(),
        readonly_outputs: false,
        strict_leases: false,
    }
}

/// A materializer under `strict_leases`, with or without a stash to put released roots back
/// from.
fn make_strict(
    fs: &ProjectRoot,
    with_stash: bool,
) -> (
    Arc<DiskStateMaterializer<StubIoHandler>>,
    Arc<StubIoHandler>,
) {
    let mut configs = configs();
    configs.strict_leases = true;
    make_with_configs_and_stash(
        fs,
        StubIoHandler::new(fs.dupe()),
        None,
        None,
        configs,
        with_stash,
    )
}

fn make(
    fs: &ProjectRoot,
    db: Option<MaterializerStateSqliteDb>,
    state: Option<crate::sqlite::materializer_db::MaterializerState>,
) -> (
    Arc<DiskStateMaterializer<StubIoHandler>>,
    Arc<StubIoHandler>,
) {
    make_with_io(fs, StubIoHandler::new(fs.dupe()), db, state)
}

/// A materializer whose downloads of the given paths take the given time.
fn make_slow(
    fs: &ProjectRoot,
    slow: &[(&ProjectRelativePathBuf, Duration)],
) -> (
    Arc<DiskStateMaterializer<StubIoHandler>>,
    Arc<StubIoHandler>,
) {
    let mut config = BuckMutMap::default();
    for (path, duration) in slow {
        config.insert((*path).clone(), *duration);
    }
    make_with_io(
        fs,
        StubIoHandler::new(fs.dupe()).with_materialization_config(config),
        None,
        None,
    )
}

fn make_with_io(
    fs: &ProjectRoot,
    io: StubIoHandler,
    db: Option<MaterializerStateSqliteDb>,
    state: Option<crate::sqlite::materializer_db::MaterializerState>,
) -> (
    Arc<DiskStateMaterializer<StubIoHandler>>,
    Arc<StubIoHandler>,
) {
    make_with_configs(fs, io, db, state, configs())
}

fn make_with_configs(
    fs: &ProjectRoot,
    io: StubIoHandler,
    db: Option<MaterializerStateSqliteDb>,
    state: Option<crate::sqlite::materializer_db::MaterializerState>,
    configs: DeferredMaterializerConfigs,
) -> (
    Arc<DiskStateMaterializer<StubIoHandler>>,
    Arc<StubIoHandler>,
) {
    make_with_configs_and_stash(fs, io, db, state, configs, true)
}

fn make_with_configs_and_stash(
    fs: &ProjectRoot,
    io: StubIoHandler,
    db: Option<MaterializerStateSqliteDb>,
    state: Option<crate::sqlite::materializer_db::MaterializerState>,
    configs: DeferredMaterializerConfigs,
    with_stash: bool,
) -> (
    Arc<DiskStateMaterializer<StubIoHandler>>,
    Arc<StubIoHandler>,
) {
    let io = Arc::new(io);
    // A store of this materializer's own, over the process's table as in the daemon, with its
    // stash under this test's buck-out. Every test in the process shares the table, and so sees
    // the records every other test's stash makes: a test whose outcome rests on whether the
    // stash holds its content stashes bytes no other test does.
    let store: &'static BlobStore = Box::leak(Box::new(BlobStore::new(file_digest_interner())));
    if with_stash {
        assert!(
            store.attach_stash(
                fs.root()
                    .join(ForwardRelativePath::unchecked_new("buck-out/v2/stash")),
                CasDigestConfig::testing_default(),
                Arc::new(DummyBlockingExecutor { fs: fs.dupe() }),
            )
        );
    }
    let dm = DiskStateMaterializer::new_with_io(
        io.dupe(),
        store,
        Arc::new(DummyBlockingExecutor { fs: fs.dupe() }),
        configs,
        db,
        state,
        EventDispatcher::null(),
    )
    .unwrap();
    (Arc::new(dm), io)
}

/// A file value's entry as a copy names it at its destination.
fn leaf(value: &ArtifactValue) -> ActionDirectoryEntry<ActionImmutableDirectory> {
    match value.entry() {
        DirectoryEntry::Leaf(leaf) => DirectoryEntry::Leaf(leaf.dupe()),
        DirectoryEntry::Dir(_) => panic!("a file"),
    }
}

/// Whether the store has `content`, by asking it to put the content at `dest`.
async fn in_store(
    dm: &DiskStateMaterializer<StubIoHandler>,
    content: &[u8],
    dest: &AbsNormPath,
) -> buck2_error::Result<bool> {
    let digest = TrackedFileDigest::from_content(content, CasDigestConfig::testing_default());
    Ok(dm
        .store()
        .materialize(vec![FileToLink {
            digest,
            executable: false,
            dest: dest.to_owned(),
        }])
        .await?[0])
}

/// The inode of the file at `path`: the same before and after only if nothing replaced it.
#[cfg(unix)]
fn inode_of(path: &AbsNormPath) -> buck2_error::Result<u64> {
    use std::os::unix::fs::MetadataExt;
    Ok(std::fs::metadata(path)?.ino())
}

/// Clean-stale arguments under which everything this daemon has not used is stale.
fn clean_unused() -> CleanStaleArtifactsArgs {
    CleanStaleArtifactsArgs {
        policy: CleanStaleArtifactsPolicy::Explicit {
            keep_since_time: Timestamp::now() + SignedDuration::from_secs(3600),
            adaptive_low_disk_threshold: None,
            adaptive_min_ttl: None,
            adaptive_unmaterialize_active: false,
        },
        dry_run: false,
        tracked_only: false,
        origin: buck2_data::clean_stale_result::Origin::CleanCommand,
    }
}

/// A materializer that loaded `roots` from a previous instance's sqlite state, with the content
/// on disk: what a daemon finds after a restart. Nothing loaded counts as used by this instance.
async fn restarted_with(
    fs: &ProjectRoot,
    roots: Vec<(ProjectRelativePathBuf, ArtifactValue)>,
) -> buck2_error::Result<(
    Arc<DiskStateMaterializer<StubIoHandler>>,
    Arc<StubIoHandler>,
)> {
    let (db, _) = testing_materializer_state_sqlite_db(fs, versions(), metadata(), None)?;
    let (dm, _io) = make(fs, Some(db), None);
    for (path, value) in &roots {
        drop(report(&dm, path, value.dupe()).await?);
        // The content on disk, as the rows will say it is.
        let size = match value.entry() {
            DirectoryEntry::Leaf(ActionDirectoryMember::File(file)) => file.digest.size(),
            _ => panic!("restarted_with takes file values"),
        };
        fs.write_file(path, "x".repeat(size as usize), false)?;
    }
    abort(dm);
    let (db, state) = testing_materializer_state_sqlite_db(fs, versions(), metadata(), None)?;
    Ok(make(fs, Some(db), Some(state?)))
}

fn abort(dm: Arc<DiskStateMaterializer<StubIoHandler>>) {
    Arc::into_inner(dm)
        .expect("every clone of the materializer is gone by the end of a test")
        .abort();
}

fn versions() -> IntentionallyStdHashMap<String, String> {
    IntentionallyStdHashMap::from([("version".to_owned(), "0".to_owned())])
}

fn metadata() -> IntentionallyStdHashMap<String, String> {
    IntentionallyStdHashMap::new()
}

fn request(artifacts: Vec<(ProjectRelativePathBuf, ArtifactValue)>) -> MaterializeRequest {
    MaterializeRequest {
        artifacts,
        outputs: Vec::new(),
        purpose: MaterializationPurpose::IntermediateOnly,
        re_use_case: RemoteExecutorUseCase::buck2_default(),
    }
}

/// Materializes and drops the lease at once.
async fn materialize_ok(
    dm: &DiskStateMaterializer<StubIoHandler>,
    artifacts: Vec<(ProjectRelativePathBuf, ArtifactValue)>,
) -> buck2_error::Result<()> {
    drop(materialize_held(dm, artifacts).await?);
    Ok(())
}

/// Materializes and returns the response, whose lease the caller holds.
async fn materialize_held(
    dm: &DiskStateMaterializer<StubIoHandler>,
    artifacts: Vec<(ProjectRelativePathBuf, ArtifactValue)>,
) -> buck2_error::Result<MaterializeResponse> {
    let mut response = dm.materialize(request(artifacts)).await?;
    for result in std::mem::take(&mut response.results) {
        result?;
    }
    Ok(response)
}

/// Reports `value` at `path` as a producer would, and returns its lease.
async fn report(
    dm: &DiskStateMaterializer<StubIoHandler>,
    path: &ProjectRelativePathBuf,
    value: ArtifactValue,
) -> buck2_error::Result<WriteLease> {
    let lease = dm.prepare_outputs(vec![path.clone()]).await?;
    dm.declare_existing(
        &lease,
        vec![DeclareArtifactPayload {
            path: path.clone(),
            artifact: value,
        }],
    )
    .await?;
    Ok(lease)
}

fn sorted(mut log: Vec<(Op, ProjectRelativePathBuf)>) -> Vec<(Op, ProjectRelativePathBuf)> {
    log.sort_by(|a, b| a.1.cmp(&b.1));
    log
}

const SLOW: Duration = Duration::from_millis(300);
const A_BIT: Duration = Duration::from_millis(50);

#[tokio::test]
async fn test_materialize_downloads_once_and_then_matches() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let (dm, io) = make(temp.path(), None, None);
        let p = path("buck-out/v2/gen/out");

        materialize_ok(&dm, vec![(p.clone(), file("A"))]).await?;
        assert_eq!(io.take_log(), vec![(Op::Materialize, p.clone())]);

        // The same content again is already there; other content is not.
        materialize_ok(&dm, vec![(p.clone(), file("A"))]).await?;
        assert_eq!(io.take_log(), vec![]);
        assert!(dm.has_artifact_at(p.clone()).await?);
        assert!(matches!(
            dm.declare_match(vec![(p.clone(), file("A"))]).await?,
            DeclareMatchOutcome::Match
        ));
        assert!(matches!(
            dm.declare_match(vec![(p.clone(), file("B"))]).await?,
            DeclareMatchOutcome::NotMatch
        ));
        materialize_ok(&dm, vec![(p.clone(), file("B"))]).await?;
        assert_eq!(io.take_log(), vec![(Op::Materialize, p.clone())]);
        abort(dm);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_reported_content_is_not_fetched() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let (dm, io) = make(temp.path(), None, None);
        let p = path("buck-out/v2/gen/out");

        drop(report(&dm, &p, file("A")).await?);
        materialize_ok(&dm, vec![(p.clone(), file("A"))]).await?;
        assert_eq!(io.take_log(), vec![]);

        // Preparing the path for a new output forgets it.
        drop(dm.prepare_outputs(vec![p.clone()]).await?);
        assert!(!dm.has_artifact_at(p.clone()).await?);
        materialize_ok(&dm, vec![(p.clone(), file("A"))]).await?;
        assert_eq!(io.take_log(), vec![(Op::Materialize, p.clone())]);
        abort(dm);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_a_report_outside_the_lease_is_refused() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let (dm, _io) = make(temp.path(), None, None);
        let (a, b) = (path("buck-out/v2/gen/a"), path("buck-out/v2/gen/b"));

        let lease = dm.prepare_outputs(vec![a.clone()]).await?;
        let err = dm
            .declare_existing(
                &lease,
                vec![DeclareArtifactPayload {
                    path: b.clone(),
                    artifact: file("B"),
                }],
            )
            .await
            .expect_err("`b` is not under the lease");
        assert!(format!("{err:#}").contains("outside the write roots"));
        // A lease this materializer did not grant is refused too.
        let err = dm
            .declare_existing(
                &WriteLease::noop(),
                vec![DeclareArtifactPayload {
                    path: a.clone(),
                    artifact: file("A"),
                }],
            )
            .await
            .expect_err("a no-op lease grants nothing");
        assert!(format!("{err:#}").contains("did not grant"));
        assert!(!dm.has_artifact_at(a).await?);
        assert!(!dm.has_artifact_at(b).await?);
        drop(lease);
        abort(dm);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_a_path_inside_a_tracked_directory() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let (dm, io) = make(temp.path(), None, None);
        let d = path("buck-out/v2/gen/d");

        drop(report(&dm, &d, dir(&[("a", "A"), ("b", "B")])).await?);
        // A projection with the directory's own content is free.
        materialize_ok(
            &dm,
            vec![(d.join(ForwardRelativePath::unchecked_new("a")), file("A"))],
        )
        .await?;
        assert_eq!(io.take_log(), vec![]);
        assert!(
            dm.has_artifact_at(d.join(ForwardRelativePath::unchecked_new("b")))
                .await?
        );
        assert_eq!(
            dm.get_artifact_entries_for_materialized_paths(
                vec![d.join(ForwardRelativePath::unchecked_new("a"))],
                false
            )
            .await?
            .len(),
            1
        );

        // Other content at one file inside it writes that file and keeps the rest tracked.
        let a = d.join(ForwardRelativePath::unchecked_new("a"));
        materialize_ok(&dm, vec![(a.clone(), file("A2"))]).await?;
        assert_eq!(io.take_log(), vec![(Op::Materialize, a.clone())]);
        assert!(
            dm.has_artifact_at(d.join(ForwardRelativePath::unchecked_new("b")))
                .await?
        );
        assert!(matches!(
            dm.declare_match(vec![(a.clone(), file("A2"))]).await?,
            DeclareMatchOutcome::Match
        ));
        assert!(matches!(
            dm.declare_match(vec![(d.clone(), dir(&[("a", "A"), ("b", "B")]))])
                .await?,
            DeclareMatchOutcome::NotMatch
        ));
        abort(dm);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_a_directory_around_a_read_held_file_does_not_wait_for_it() -> buck2_error::Result<()>
{
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let (dm, io) = make(temp.path(), None, None);
        let d = path("buck-out/v2/gen/d");
        let b = d.join(ForwardRelativePath::unchecked_new("b"));

        // One action reads the projection `d/b` and keeps reading it.
        let reading = materialize_held(&dm, vec![(b.clone(), file("B"))]).await?;
        assert_eq!(io.take_log(), vec![(Op::Materialize, b.clone())]);

        // Another wants all of `d`, of which `b` is already there: it goes ahead without waiting
        // for the reader, and afterwards `d` is tracked whole.
        let whole = dir(&[("a", "A"), ("b", "B")]);
        timeout(SLOW, materialize_ok(&dm, vec![(d.clone(), whole.dupe())]))
            .await
            .expect("the write on `d` excludes `d/b` and waits for nobody")?;
        assert_eq!(io.take_log(), vec![(Op::Materialize, d.clone())]);
        assert!(matches!(
            dm.declare_match(vec![(d.clone(), whole)]).await?,
            DeclareMatchOutcome::Match
        ));
        drop(reading);
        abort(dm);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_dep_regions_are_roots_of_their_own() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let (dm, io) = make(temp.path(), None, None);
        let p = path("buck-out/v2/gen/link");
        let target = path("buck-out/v2/gen/target");

        let value = with_dep(&file("L"), "buck-out/v2/gen/target", &file("T"));
        materialize_ok(&dm, vec![(p.clone(), value.dupe())]).await?;
        assert_eq!(
            sorted(io.take_log()),
            vec![
                (Op::Materialize, p.clone()),
                (Op::Materialize, target.clone())
            ]
        );
        assert!(dm.has_artifact_at(target.clone()).await?);

        // A dep outside buck-out is not the materializer's to write.
        let value = with_dep(&file("L2"), "source/file", &file("S"));
        materialize_ok(&dm, vec![(p.clone(), value)]).await?;
        assert_eq!(io.take_log(), vec![(Op::Materialize, p.clone())]);
        abort(dm);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_each_pair_gets_its_own_result() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let (dm, io) = make(temp.path(), None, None);
        let (good, bad) = (path("buck-out/v2/gen/good"), path("buck-out/v2/gen/bad"));
        io.set_fail_on(vec![bad.clone()]);

        let response = dm
            .materialize(request(vec![
                (good.clone(), file("G")),
                (bad.clone(), file("B")),
            ]))
            .await?;
        assert!(response.results[0].is_ok());
        assert!(response.results[1].is_err());
        assert!(dm.has_artifact_at(good).await?);
        assert!(!dm.has_artifact_at(bad.clone()).await?);
        drop(response);

        // The failed root was released with the response; the next request writes it itself.
        io.set_fail_on(vec![]);
        materialize_ok(&dm, vec![(bad.clone(), file("B"))]).await?;
        assert!(dm.has_artifact_at(bad).await?);
        abort(dm);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_a_second_request_waits_for_the_writer_and_then_shares() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let p = path("buck-out/v2/gen/out");
        let (dm, io) = make_slow(temp.path(), &[(&p, SLOW)]);

        let first = tokio::spawn({
            let dm = dm.dupe();
            let p = p.clone();
            async move { materialize_held(&dm, vec![(p, file("A"))]).await }
        });
        sleep(A_BIT).await;
        let second = materialize_held(&dm, vec![(p.clone(), file("A"))]).await?;
        let first = first.await.unwrap()?;
        // One download, shared: the second was granted read once the first had reported.
        assert_eq!(io.take_log(), vec![(Op::Materialize, p.clone())]);
        drop((first, second));
        abort(dm);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_a_root_waiting_for_a_writer_holds_up_no_other_root() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let (p, q) = (path("buck-out/v2/gen/p"), path("buck-out/v2/gen/q"));
        let (dm, io) = make_slow(temp.path(), &[(&p, SLOW)]);

        let first = tokio::spawn({
            let dm = dm.dupe();
            let p = p.clone();
            async move { materialize_held(&dm, vec![(p, file("P"))]).await }
        });
        sleep(A_BIT).await;
        // `p` is being written by the first request; `q` is not on disk. The second request
        // waits for `p` but downloads `q` in the meantime.
        let second = tokio::spawn({
            let dm = dm.dupe();
            let (p, q) = (p.clone(), q.clone());
            async move { materialize_held(&dm, vec![(p, file("P")), (q, file("Q"))]).await }
        });
        sleep(A_BIT).await;
        assert!(!second.is_finished(), "`p` is still being written");
        assert_eq!(io.take_log(), vec![(Op::Materialize, q.clone())]);
        let first = first.await.unwrap()?;
        let second = second.await.unwrap()?;
        assert_eq!(
            io.take_log(),
            vec![(Op::Materialize, p.clone())],
            "`p` was shared, not downloaded again"
        );
        drop((first, second));
        abort(dm);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_a_prepared_output_blocks_readers_until_it_is_reported() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let (dm, io) = make(temp.path(), None, None);
        let p = path("buck-out/v2/gen/out");

        let lease = dm.prepare_outputs(vec![p.clone()]).await?;
        let reader = tokio::spawn({
            let dm = dm.dupe();
            let p = p.clone();
            async move { materialize_held(&dm, vec![(p, file("A"))]).await }
        });
        sleep(A_BIT).await;
        assert!(!reader.is_finished(), "the producer still holds `out`");

        // Once the producer has reported, the reader finds the content there and shares it,
        // while the producer still holds its lease.
        dm.declare_existing(
            &lease,
            vec![DeclareArtifactPayload {
                path: p.clone(),
                artifact: file("A"),
            }],
        )
        .await?;
        let held = timeout(SLOW, reader)
            .await
            .expect("reported content is readable at once")
            .unwrap()?;
        assert_eq!(io.take_log(), vec![], "nothing was downloaded");
        drop((held, lease));
        abort(dm);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_a_dropped_request_hands_its_download_to_the_next() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let p = path("buck-out/v2/gen/out");
        let (dm, io) = make_slow(temp.path(), &[(&p, SLOW)]);

        let first = tokio::spawn({
            let dm = dm.dupe();
            let p = p.clone();
            async move { materialize_held(&dm, vec![(p, file("A"))]).await }
        });
        sleep(A_BIT).await;
        let second = tokio::spawn({
            let dm = dm.dupe();
            let p = p.clone();
            async move { materialize_held(&dm, vec![(p, file("A"))]).await }
        });
        sleep(A_BIT).await;
        // The first requester goes away while its download runs and someone waits for the same
        // content: the download is handed over, not restarted.
        first.abort();
        let second = second.await.unwrap()?;
        assert_eq!(io.take_log(), vec![(Op::Materialize, p.clone())]);
        assert!(dm.has_artifact_at(p).await?);
        drop(second);
        abort(dm);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_a_dropped_request_with_no_one_waiting_parks_its_download() -> buck2_error::Result<()>
{
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let p = path("buck-out/v2/gen/out");
        let (dm, io) = make_slow(temp.path(), &[(&p, SLOW)]);

        let first = tokio::spawn({
            let dm = dm.dupe();
            let p = p.clone();
            async move { materialize_held(&dm, vec![(p, file("A"))]).await }
        });
        sleep(A_BIT).await;
        first.abort();
        sleep(A_BIT).await;
        // Nobody wanted it, so it was cancelled and parked; the next request drains it and,
        // since it cannot have been adopted, writes the content itself.
        materialize_ok(&dm, vec![(p.clone(), file("A"))]).await?;
        assert_eq!(
            io.take_log(),
            vec![(Op::Materialize, p.clone()), (Op::Materialize, p.clone())]
        );
        assert!(dm.has_artifact_at(p).await?);
        abort(dm);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_a_request_holds_its_outputs_for_writing() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let (dm, _io) = make(temp.path(), None, None);
        let (input, output) = (path("buck-out/v2/gen/in"), path("buck-out/v2/gen/out"));
        drop(report(&dm, &output, file("old")).await?);

        let response = dm
            .materialize(MaterializeRequest {
                artifacts: vec![(input.clone(), file("I"))],
                outputs: vec![output.clone()],
                purpose: MaterializationPurpose::IntermediateOnly,
                re_use_case: RemoteExecutorUseCase::buck2_default(),
            })
            .await?;
        assert!(response.results[0].is_ok());
        assert!(!dm.has_artifact_at(output.clone()).await?, "untracked");
        // Another producer of the same output waits for this one.
        assert!(
            timeout(A_BIT, dm.prepare_outputs(vec![output.clone()]))
                .await
                .is_err()
        );
        let (read, outputs) = response.into_leases()?;
        let outputs = outputs.expect("the request had outputs");
        dm.declare_existing(
            &outputs,
            vec![DeclareArtifactPayload {
                path: output.clone(),
                artifact: file("new"),
            }],
        )
        .await?;
        drop((read, outputs));
        drop(
            timeout(SLOW, dm.prepare_outputs(vec![output]))
                .await
                .unwrap()?,
        );
        abort(dm);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_publish_moves_the_content_and_tracks_it() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let fs = temp.path().dupe();
        let (dm, io) = make(&fs, None, None);
        let src = path("buck-out/v2/gen/__action__/out");
        let dest = path("buck-out/v2/gen/content/abc/out");

        let lease = dm.prepare_outputs(vec![src.clone()]).await?;
        fs.write_file(&src, "X", false)?;
        drop(
            dm.publish(&lease, src.clone(), dest.clone(), file("X"))
                .await?,
        );
        drop(lease);
        assert!(!fs.resolve(&src).exists());
        assert_eq!(std::fs::read_to_string(fs.resolve(&dest))?, "X");
        assert!(dm.has_artifact_at(dest.clone()).await?);

        // The same content published again is redundant: the source goes, the destination stays.
        let lease = dm.prepare_outputs(vec![src.clone()]).await?;
        fs.write_file(&src, "X", false)?;
        drop(
            dm.publish(&lease, src.clone(), dest.clone(), file("X"))
                .await?,
        );
        drop(lease);
        assert!(!fs.resolve(&src).exists());
        assert_eq!(std::fs::read_to_string(fs.resolve(&dest))?, "X");
        assert_eq!(io.take_log(), vec![]);
        abort(dm);
        Ok(())
    })
    .await
}

/// A materializer over a sqlite db, persisting access times as `updates` says.
fn make_persisting(
    fs: &ProjectRoot,
    updates: AccessTimesUpdates,
) -> buck2_error::Result<Arc<DiskStateMaterializer<StubIoHandler>>> {
    let (db, _) = testing_materializer_state_sqlite_db(fs, versions(), metadata(), None)?;
    let mut configs = configs();
    configs.update_access_times = updates;
    let (dm, _io) = make_with_configs(fs, StubIoHandler::new(fs.dupe()), Some(db), None, configs);
    Ok(dm)
}

/// A read of a tracked root bumps its access time, which waits for a flush to sqlite.
#[tokio::test]
async fn test_a_read_buffers_an_access_time_bump() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let dm = make_persisting(temp.path(), AccessTimesUpdates::Partial)?;
        let p = path("buck-out/v2/gen/read");
        drop(report(&dm, &p, file("read")).await?);
        assert_eq!(dm.pending_access_times().await?, 0);
        materialize_ok(&dm, vec![(p.clone(), file("read"))]).await?;
        assert_eq!(dm.pending_access_times().await?, 1);
        dm.flush_all_access_times().await?;
        assert_eq!(dm.pending_access_times().await?, 0);
        abort(dm);
        Ok(())
    })
    .await
}

#[tokio::test(start_paused = true)]
async fn test_access_times_are_flushed_on_the_period() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let dm = make_persisting(temp.path(), AccessTimesUpdates::Full)?;
        let p = path("buck-out/v2/gen/read");
        drop(report(&dm, &p, file("read")).await?);
        materialize_ok(&dm, vec![(p.clone(), file("read"))]).await?;
        // Paused time passes as the test waits; the flush comes on its period.
        let mut pending = usize::MAX;
        for _ in 0..30 {
            pending = dm.pending_access_times().await?;
            if pending == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        assert_eq!(pending, 0);
        abort(dm);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_access_times_are_not_kept_when_disabled() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let dm = make_persisting(temp.path(), AccessTimesUpdates::Disabled)?;
        let p = path("buck-out/v2/gen/read");
        drop(report(&dm, &p, file("read")).await?);
        materialize_ok(&dm, vec![(p.clone(), file("read"))]).await?;
        assert_eq!(dm.pending_access_times().await?, 0);
        abort(dm);
        Ok(())
    })
    .await
}

/// A final-output materialization of a root tracked as an intermediate makes it a final output,
/// which a restart sees.
#[tokio::test]
async fn test_a_final_output_materialization_promotes_a_tracked_root() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let fs = temp.path().dupe();
        let (db, _) = testing_materializer_state_sqlite_db(&fs, versions(), metadata(), None)?;
        let (dm, _io) = make(&fs, Some(db), None);
        let p = path("buck-out/v2/gen/promoted");
        drop(report(&dm, &p, file("promoted")).await?);
        let mut final_output = request(vec![(p.clone(), file("promoted"))]);
        final_output.purpose = MaterializationPurpose::FinalOutput { required: true };
        let mut response = dm.materialize(final_output).await?;
        for result in std::mem::take(&mut response.results) {
            result?;
        }
        drop(response);
        abort(dm);
        let (_, state) = testing_materializer_state_sqlite_db(&fs, versions(), metadata(), None)?;
        let entry = state?
            .into_iter()
            .find(|entry| entry.path == p)
            .expect("the root is persisted");
        assert_eq!(entry.classification, ArtifactClassification::FinalOutput);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_a_published_output_enters_the_store() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let fs = temp.path().dupe();
        let (dm, _io) = make(&fs, None, None);
        let src = path("buck-out/v2/gen/__action__/out");
        let dest = path("buck-out/v2/gen/content/def/out");
        let lease = dm.prepare_outputs(vec![src.clone()]).await?;
        fs.write_file(&src, "published here", false)?;
        // Held as the action's value holds it; the store keeps nothing past the last holder.
        let published = file("published here");
        drop(dm.publish(&lease, src, dest, published.dupe()).await?);
        drop(lease);
        let copy = fs.resolve(path("buck-out/v2/gen/copy"));
        assert!(in_store(&dm, b"published here", &copy).await?);
        drop(published);
        abort(dm);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_ensuring_paths_without_values_asks_nothing() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let (dm, io) = make(temp.path(), None, None);
        let tracked = path("buck-out/v2/gen/incremental/out");
        drop(report(&dm, &tracked, file("previous")).await?);
        dm.ensure_materialized(
            vec![tracked, path("buck-out/v2/gen/incremental/never")],
            MaterializationPurpose::IntermediateOnly,
        )
        .await?;
        assert_eq!(io.take_log(), vec![]);
        abort(dm);
        Ok(())
    })
    .await
}
#[tokio::test]
async fn test_tracked_state_survives_a_restart() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let fs = temp.path().dupe();
        let (db, _) = testing_materializer_state_sqlite_db(&fs, versions(), metadata(), None)?;
        let (dm, io) = make(&fs, Some(db), None);
        let (p, d) = (path("buck-out/v2/gen/out"), path("buck-out/v2/gen/d"));
        materialize_ok(
            &dm,
            vec![
                (p.clone(), file("A")),
                (d.clone(), dir(&[("a", "A"), ("b", "B")])),
            ],
        )
        .await?;
        io.take_log();
        // Expanding a directory rewrites its rows too.
        let a = d.join(ForwardRelativePath::unchecked_new("a"));
        materialize_ok(&dm, vec![(a.clone(), file("A2"))]).await?;
        io.take_log();
        abort(dm);
        // The stub downloads nothing; what the rows say is on disk has to be there for the next
        // daemon's check of them.
        let b = d.join(ForwardRelativePath::unchecked_new("b"));
        fs.write_file(&p, "A", false)?;
        fs.write_file(&a, "A2", false)?;
        fs.write_file(&b, "B", false)?;

        let (db, state) = testing_materializer_state_sqlite_db(&fs, versions(), metadata(), None)?;
        let state = state?;
        let (dm, io) = make(&fs, Some(db), Some(state));
        materialize_ok(
            &dm,
            vec![
                (p.clone(), file("A")),
                (a.clone(), file("A2")),
                (b, file("B")),
            ],
        )
        .await?;
        assert_eq!(io.take_log(), vec![], "everything came back from sqlite");
        abort(dm);
        Ok(())
    })
    .await
}

/// A root loaded from sqlite is checked against disk, kind and size, the first time it is read:
/// one that is not there as the rows said is forgotten and put there again, and its rows with it.
#[tokio::test]
async fn test_a_loaded_root_is_trusted_as_the_rows_say() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let fs = temp.path().dupe();
        let (db, _) = testing_materializer_state_sqlite_db(&fs, versions(), metadata(), None)?;
        let (dm, io) = make(&fs, Some(db), None);
        let (gone, changed, d) = (
            path("buck-out/v2/gen/gone"),
            path("buck-out/v2/gen/changed"),
            path("buck-out/v2/gen/dir"),
        );
        materialize_ok(
            &dm,
            vec![
                (gone.clone(), file("A")),
                (changed.clone(), file("A")),
                (d.clone(), dir(&[("a", "A"), ("b", "B")])),
            ],
        )
        .await?;
        io.take_log();
        abort(dm);
        // Disk disagrees with the rows: one file gone, one of another size, one directory with a
        // directory where a file was. The rows are trusted all the same, as the deferred core
        // trusts its own: nothing is checked, nothing is put there again.
        fs.write_file(&changed, "longer", false)?;
        fs.write_file(d.join(ForwardRelativePath::unchecked_new("a")), "A", false)?;
        std::fs::create_dir_all(fs.resolve(d.join(ForwardRelativePath::unchecked_new("b"))))?;

        let (db, state) = testing_materializer_state_sqlite_db(&fs, versions(), metadata(), None)?;
        let (dm, io) = make(&fs, Some(db), Some(state?));
        materialize_ok(
            &dm,
            vec![
                (gone.clone(), file("A")),
                (changed.clone(), file("A")),
                (d.clone(), dir(&[("a", "A"), ("b", "B")])),
            ],
        )
        .await?;
        assert_eq!(io.take_log(), vec![]);
        abort(dm);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_clean_stale_deletes_what_this_daemon_has_not_used() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let fs = temp.path().dupe();
        let (p, d) = (path("buck-out/v2/gen/p"), path("buck-out/v2/gen/d"));
        let (dm, _io) =
            restarted_with(&fs, vec![(p.clone(), file("P")), (d.clone(), file("D"))]).await?;
        fs.write_file(path("buck-out/v2/gen/junk"), "J", false)?;
        // Using `p` makes it this daemon's; `d` is only what a previous one left.
        materialize_ok(&dm, vec![(p.clone(), file("P"))]).await?;

        let stats = dm
            .clean_stale_artifacts(clean_unused())
            .await?
            .stats
            .expect("stats");
        assert!(stats.used_disk_space_bytes_start.is_some());
        assert!(stats.used_disk_space_bytes_end.is_some());
        assert_eq!(
            (
                stats.stale_artifact_count,
                stats.retained_artifact_count,
                stats.untracked_artifact_count,
                stats.cleaned_artifact_count,
            ),
            (1, 1, 1, 2)
        );
        assert!(dm.has_artifact_at(p).await?);
        assert!(!dm.has_artifact_at(d).await?);
        abort(dm);
        Ok(())
    })
    .await
}

/// What a dep-file hit matches against stays active for the cleaner, as it does under the
/// deferred core, where a match re-declares the outputs.
#[tokio::test]
async fn test_a_match_keeps_a_loaded_root_from_the_cleaner() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let fs = temp.path().dupe();
        let (matched, unused) = (
            path("buck-out/v2/gen/matched"),
            path("buck-out/v2/gen/unused"),
        );
        let (dm, _io) = restarted_with(
            &fs,
            vec![(matched.clone(), file("M")), (unused.clone(), file("U"))],
        )
        .await?;
        assert!(
            dm.declare_match(vec![(matched.clone(), file("M"))])
                .await?
                .is_match()
        );
        assert!(
            !dm.declare_match(vec![(unused.clone(), file("other"))])
                .await?
                .is_match()
        );

        dm.clean_stale_artifacts(clean_unused()).await?;
        assert!(dm.has_artifact_at(matched).await?);
        assert!(!dm.has_artifact_at(unused).await?);
        abort(dm);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_clean_stale_leaves_what_someone_holds() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let fs = temp.path().dupe();
        let (dm, _io) = make(&fs, None, None);
        // An output being produced is untracked on disk and held for writing.
        let out = path("buck-out/v2/gen/out");
        let lease = dm.prepare_outputs(vec![out.clone()]).await?;
        fs.write_file(&out, "half", false)?;

        let stats = dm
            .clean_stale_artifacts(clean_unused())
            .await?
            .stats
            .expect("stats");
        assert_eq!(
            (stats.untracked_artifact_count, stats.cleaned_artifact_count),
            (1, 0)
        );
        drop(lease);
        let stats = dm
            .clean_stale_artifacts(clean_unused())
            .await?
            .stats
            .expect("stats");
        assert_eq!(
            (stats.untracked_artifact_count, stats.cleaned_artifact_count),
            (1, 1)
        );
        abort(dm);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_clean_stale_dry_run_deletes_nothing() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let fs = temp.path().dupe();
        let d = path("buck-out/v2/gen/d");
        let (dm, _io) = restarted_with(&fs, vec![(d.clone(), file("D"))]).await?;
        let stats = dm
            .clean_stale_artifacts(CleanStaleArtifactsArgs {
                dry_run: true,
                ..clean_unused()
            })
            .await?
            .stats
            .expect("stats");
        assert!(stats.used_disk_space_bytes_start.is_some());
        assert!(stats.used_disk_space_bytes_end.is_some());
        assert_eq!(
            (stats.stale_artifact_count, stats.cleaned_artifact_count),
            (1, 0)
        );
        assert!(dm.has_artifact_at(d).await?);
        abort(dm);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_an_active_root_stays_when_its_upload_fails() -> buck2_error::Result<()> {
    use crate::materializers::deferred::clean_stale::LowDiskCleanConfig;
    use crate::materializers::deferred::clean_stale::LowDiskCleanMode;
    use crate::materializers::deferred::clean_stale::UnmaterializationUploadConfig;

    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let fs = temp.path().dupe();
        // The configured low-disk policy at a threshold nothing is above: every retained root
        // is promoted, and active intermediates are unmaterialized, uploaded first when the CAS
        // is not known to hold them. The stub cannot upload.
        let mut configs = configs();
        configs.ttl_refresh.enabled = true;
        configs.ttl_refresh.frequency = Duration::from_secs(3600);
        configs.clean_stale_config = CleanStaleConfig {
            low_disk: Some(LowDiskCleanConfig {
                threshold_percent: 100.0,
                mode: LowDiskCleanMode::Adaptive {
                    min_ttl: Duration::ZERO,
                    delete_intermediate_within_min_ttl: true,
                    unmaterialize_active: true,
                    unmaterialization_threshold_percent: 100.0,
                },
            }),
            unmaterialize_upload: Some(UnmaterializationUploadConfig {
                re_use_case: RemoteExecutorUseCase::buck2_default(),
                max_bytes: 1 << 30,
            }),
            ..CleanStaleConfig::default()
        };
        let (dm, _io) = make_with_configs(&fs, StubIoHandler::new(fs.dupe()), None, None, configs);
        let p = path("buck-out/v2/gen/p");
        drop(report(&dm, &p, file("P")).await?);
        fs.write_file(&p, "P", false)?;

        let stats = dm
            .clean_stale_artifacts(CleanStaleArtifactsArgs {
                policy: CleanStaleArtifactsPolicy::Configured,
                dry_run: false,
                tracked_only: false,
                origin: buck2_data::clean_stale_result::Origin::CleanCommand,
            })
            .await?
            .stats
            .expect("stats");
        // Promoted by the policy, so counted as selected rather than as active retained.
        assert_eq!(stats.retained_artifact_count, 1);
        assert_eq!(stats.adaptive_unmaterialization_selected_artifact_count, 1);
        assert_eq!(stats.unmaterialization_upload_attempted_artifact_count, 1);
        assert_eq!(stats.unmaterialization_upload_failed_artifact_count, 1);
        assert_eq!(stats.cleaned_artifact_count, 0);
        assert!(dm.has_artifact_at(p).await?, "still tracked, still there");
        abort(dm);
        Ok(())
    })
    .await
}

/// The destination's value names the same digests as the source's, so nothing is copied, held or
/// tracked: whoever needs the destination materializes it from them.
#[tokio::test]
async fn test_a_copy_of_buck_out_content_is_nothing_to_the_core() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let (dm, io) = make(temp.path(), None, None);
        let (src, dest) = (path("buck-out/v2/gen/src"), path("buck-out/v2/gen/copy"));
        drop(report(&dm, &src, file("S")).await?);
        dm.declare_copy_impl(
            dest.clone(),
            file("S"),
            vec![CopiedArtifact::new(
                src.clone(),
                dest.clone(),
                leaf(&file("S")),
                None,
            )],
        )
        .await?;
        // So is a copy with no sources at all, the symlink at a configuration path.
        dm.declare_copy_impl(path("buck-out/v2/gen/link"), file("L"), vec![])
            .await?;
        assert_eq!(io.take_log(), vec![]);
        assert!(!dm.has_artifact_at(dest).await?);
        // The source is not held: it can be taken for writing right away.
        drop(
            timeout(A_BIT, dm.prepare_outputs(vec![src]))
                .await
                .expect("nothing holds the source")?,
        );
        abort(dm);
        Ok(())
    })
    .await
}

/// A copy of a source file is the action's to make under `hold_or_take`: declared to this core,
/// it is refused.
#[tokio::test]
async fn test_a_copy_from_outside_buck_out_is_refused() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let (dm, io) = make(temp.path(), None, None);
        let (src, dest) = (path("src/b.txt"), path("buck-out/v2/gen/copy"));
        assert!(
            dm.declare_copy_impl(
                dest.clone(),
                file("B"),
                vec![CopiedArtifact::new(src, dest, leaf(&file("B")), None)],
            )
            .await
            .is_err()
        );
        assert_eq!(io.take_log(), vec![]);
        abort(dm);
        Ok(())
    })
    .await
}

/// Under `readonly_outputs` a reported root's files lose their write bits, a write's as a local
/// action's, directories keep theirs, and an incremental action's run gets the bits back.
#[tokio::test]
async fn test_tracked_files_are_read_only_under_the_knob() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let fs = temp.path().dupe();
        let mut configs = configs();
        configs.readonly_outputs = true;
        let (dm, _io) = make_with_configs(&fs, StubIoHandler::new(fs.dupe()), None, None, configs);
        let readonly = |p: &ProjectRelativePathBuf| -> buck2_error::Result<bool> {
            Ok(std::fs::metadata(fs.resolve(p))?.permissions().readonly())
        };

        let w = path("buck-out/v2/gen/written");
        let blocking = DummyBlockingExecutor { fs: fs.dupe() };
        write_locally(
            &*dm,
            &blocking,
            &fs,
            DigestConfig::testing_default(),
            WriteRequest {
                path: w.clone(),
                content: b"written".to_vec(),
                is_executable: false,
                path_kind: BuckOutPathKind::Configuration,
            },
        )
        .await?;
        assert!(readonly(&w)?, "a write's output");

        // A local action's outputs, with the modes the tool left.
        let d = path("buck-out/v2/gen/out");
        let (a, b) = (
            d.join(ForwardRelativePath::unchecked_new("a")),
            d.join(ForwardRelativePath::unchecked_new("b")),
        );
        fs.write_file(&a, "A", true)?;
        fs.write_file(&b, "B", false)?;
        let lease = report(&dm, &d, dir(&[("a", "A"), ("b", "B")])).await?;
        assert!(readonly(&a)? && readonly(&b)?, "a reported root's files");
        assert!(!readonly(&d)?, "directories stay writable");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(fs.resolve(&a))?.permissions().mode() & 0o777;
            assert_eq!(mode, 0o555, "the executable bits stay");
        }
        drop(lease);

        // The run of an incremental action may write them again.
        let lease = dm.prepare_outputs(vec![d.clone()]).await?;
        dm.make_outputs_writable(vec![d.clone()]).await?;
        assert!(!readonly(&a)? && !readonly(&b)?);
        drop(lease);
        abort(dm);
        Ok(())
    })
    .await
}

/// Content an action makes goes to disk under `hold_or_take` and a report, as a local action's
/// output does, and the store holds it as it holds theirs; asked for again, the path is found
/// holding it and nothing is made.
#[tokio::test]
async fn test_produced_content_is_tracked_and_enters_the_store() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let (dm, io) = make(temp.path(), None, None);
        let blocking = DummyBlockingExecutor {
            fs: temp.path().dupe(),
        };
        let p = path("buck-out/v2/gen/written");
        let request = || WriteRequest {
            path: p.clone(),
            content: b"made under hold_or_take".to_vec(),
            is_executable: true,
            path_kind: BuckOutPathKind::Configuration,
        };
        let (value, produced) = write_locally(
            &*dm,
            &blocking,
            temp.path(),
            DigestConfig::testing_default(),
            request(),
        )
        .await?;
        assert_eq!(
            (value.dupe(), produced),
            (file_exec("made under hold_or_take"), Produced::Made)
        );
        assert_eq!(io.take_log(), vec![]);
        assert_eq!(
            std::fs::read_to_string(temp.path().resolve(&p))?,
            "made under hold_or_take"
        );
        assert!(dm.has_artifact_at(p.clone()).await?);
        let DirectoryEntry::Leaf(ActionDirectoryMember::File(file)) = value.entry() else {
            panic!("a write's value is a file");
        };
        assert!(
            file.digest.stash().is_some(),
            "the store holds what the CAS is not known to"
        );

        // Made again, the path is found holding it: nothing is written.
        #[cfg(unix)]
        let inode = inode_of(&temp.path().resolve(&p))?;
        let (_, produced) = write_locally(
            &*dm,
            &blocking,
            temp.path(),
            DigestConfig::testing_default(),
            request(),
        )
        .await?;
        assert_eq!(produced, Produced::Found);
        #[cfg(unix)]
        assert_eq!(
            inode_of(&temp.path().resolve(&p))?,
            inode,
            "the file was not replaced"
        );
        // Asked for as a reader, it is found tracked as wanted.
        materialize_ok(&dm, vec![(p.clone(), file_exec("made under hold_or_take"))]).await?;
        assert_eq!(io.take_log(), vec![]);
        abort(dm);
        Ok(())
    })
    .await
}

/// Content a previous daemon left on disk, in the rows and not in this daemon's store, is found
/// under `hold_or_take` and enters the store from its path; nothing is made.
#[tokio::test]
async fn test_content_a_previous_daemon_left_is_found_and_enters_the_store()
-> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let p = path("buck-out/v2/gen/written");
        let (dm, io) = restarted_with(
            temp.path(),
            vec![(p.clone(), file("left by a previous daemon"))],
        )
        .await?;
        temp.path()
            .write_file(&p, "left by a previous daemon", false)?;
        let blocking = DummyBlockingExecutor {
            fs: temp.path().dupe(),
        };
        #[cfg(unix)]
        let inode = inode_of(&temp.path().resolve(&p))?;
        // Held as a value would hold it; the store keeps content only while something does.
        let value = file("left by a previous daemon");
        let produced = produce_locally(&*dm, &blocking, p.clone(), value.dupe(), || {
            unreachable!("the path holds the content")
        })
        .await?;
        assert_eq!(produced, Produced::Found);
        assert_eq!(io.take_log(), vec![]);
        #[cfg(unix)]
        assert_eq!(
            inode_of(&temp.path().resolve(&p))?,
            inode,
            "the file was not replaced"
        );
        let out = temp.path().resolve(path("buck-out/v2/gen/out"));
        assert!(
            in_store(&dm, b"left by a previous daemon", &out).await?,
            "the store took the content from the path"
        );
        abort(dm);
        Ok(())
    })
    .await
}

/// What the store has is linked out of it; only the rest is asked of the CAS.
#[tokio::test]
async fn test_content_the_store_has_is_linked_rather_than_downloaded() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let (dm, io) = make(temp.path(), None, None);
        let content = b"from the store";
        // Held, as a value in a build would hold it: the store keeps what something holds.
        let held = TrackedFileDigest::from_content(content, CasDigestConfig::testing_default());
        dm.store()
            .insert_bytes(content.to_vec(), &held, false)
            .await?;

        // A file root the store serves needs nothing from the CAS.
        let p = path("buck-out/v2/gen/whole");
        materialize_ok(&dm, vec![(p.clone(), file("from the store"))]).await?;
        assert_eq!(io.take_log(), vec![]);
        assert_eq!(
            std::fs::read_to_string(temp.path().resolve(&p))?,
            "from the store"
        );
        assert!(dm.has_artifact_at(p).await?);

        // A directory with one file the store has and one it lacks: the served file is linked,
        // and the structure and the other file are the CAS's.
        let d = path("buck-out/v2/gen/dir");
        materialize_ok(
            &dm,
            vec![(d.clone(), dir(&[("a", "from the store"), ("b", "not")]))],
        )
        .await?;
        assert_eq!(io.take_log(), vec![(Op::Materialize, d.clone())]);
        assert_eq!(
            std::fs::read_to_string(
                temp.path()
                    .resolve(d.join(ForwardRelativePath::unchecked_new("a")))
            )?,
            "from the store"
        );
        assert!(
            !temp
                .path()
                .resolve(d.join(ForwardRelativePath::unchecked_new("b")))
                .exists()
        );
        abort(dm);
        Ok(())
    })
    .await
}

/// Content tracked at another path is copied from there before the CAS is asked, when the stash
/// does not have it: content the CAS holds, say, which the stash lets go of.
#[tokio::test]
async fn test_content_tracked_elsewhere_is_copied_rather_than_downloaded() -> buck2_error::Result<()>
{
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let (dm, io) = make(temp.path(), None, None);
        let config = CasDigestConfig::testing_default();
        // In the CAS as far as the digest knows, so that the report puts nothing in the stash.
        let digest = TrackedFileDigest::new_expires(
            FileDigest::from_content(b"on disk already", config),
            Timestamp::now() + SignedDuration::from_hours(4),
            config,
        );
        let value = ArtifactValue::file(FileMetadata {
            digest: digest.dupe(),
            is_executable: false,
        });
        let first = path("buck-out/v2/gen/first");
        let on_disk = temp.path().resolve(&first);
        std::fs::create_dir_all(on_disk.parent().unwrap())?;
        std::fs::write(&on_disk, b"on disk already")?;
        drop(report(&dm, &first, value.dupe()).await?);
        let nowhere = temp.path().resolve(path("buck-out/v2/gen/nowhere"));
        assert!(!in_store(&dm, b"on disk already", &nowhere).await?);

        // A file root: copied from the first, nothing from the CAS.
        let second = path("buck-out/v2/gen/second");
        materialize_ok(&dm, vec![(second.clone(), value.dupe())]).await?;
        assert_eq!(io.take_log(), vec![]);
        assert_eq!(
            std::fs::read_to_string(temp.path().resolve(&second))?,
            "on disk already"
        );
        assert!(dm.has_artifact_at(second.clone()).await?);

        // Inside a directory: the copy, with the mode asked for; the rest is the CAS's.
        let d = path("buck-out/v2/gen/dir");
        let mut builder = ActionDirectoryBuilder::empty_exhaustive();
        builder.insert(
            ForwardRelativePath::unchecked_new("a"),
            DirectoryEntry::Leaf(ActionDirectoryMember::File(FileMetadata {
                digest: digest.dupe(),
                is_executable: true,
            })),
        )?;
        builder.insert(
            ForwardRelativePath::unchecked_new("b"),
            file("not").entry().dupe().map_dir(|d| d.into_builder()),
        )?;
        builder.mark_uniformly_exhaustive();
        let with_both = ArtifactValue::dir(
            builder
                .fingerprint(DigestConfig::testing_default().as_directory_serializer())
                .shared(&*INTERNER),
        );
        materialize_ok(&dm, vec![(d.clone(), with_both)]).await?;
        assert_eq!(io.take_log(), vec![(Op::Materialize, d.clone())]);
        let a = temp
            .path()
            .resolve(d.join(ForwardRelativePath::unchecked_new("a")));
        assert_eq!(std::fs::read_to_string(&a)?, "on disk already");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert!(std::fs::metadata(&a)?.permissions().mode() & 0o111 != 0);
        }
        assert!(
            !temp
                .path()
                .resolve(d.join(ForwardRelativePath::unchecked_new("b")))
                .exists()
        );
        abort(dm);
        Ok(())
    })
    .await
}

/// A reported output enters the store as it is reported; one that is not on disk, which is a bug
/// upstream, is reported as a soft error and costs the store's copy only.
#[tokio::test]
async fn test_a_reported_output_enters_the_store() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let (dm, _io) = make(temp.path(), None, None);
        let out = path("buck-out/v2/gen/out");
        let on_disk = temp.path().resolve(&out);
        std::fs::create_dir_all(on_disk.parent().unwrap())?;
        std::fs::write(&on_disk, b"built here")?;
        // Held as the action's value holds it; the store keeps nothing past the last holder.
        let built = file("built here");
        drop(report(&dm, &out, built.dupe()).await?);

        let copy = temp.path().resolve(path("buck-out/v2/gen/copy"));
        assert!(in_store(&dm, b"built here", &copy).await?);
        assert_eq!(std::fs::read_to_string(&copy)?, "built here");
        drop(built);

        drop(report(&dm, &path("buck-out/v2/gen/missing"), file("never written")).await?);
        let nowhere = temp.path().resolve(path("buck-out/v2/gen/nowhere"));
        assert!(!in_store(&dm, b"never written", &nowhere).await?);
        abort(dm);
        Ok(())
    })
    .await
}

/// What a live value holds stays in the store; what nothing holds leaves as its last handle
/// drops. The tree holds nothing (`entry.rs`), so a tracked root is no holder.
#[tokio::test]
async fn test_what_nothing_holds_leaves_the_store() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let (dm, _io) = make(temp.path(), None, None);
        let out = path("buck-out/v2/gen/out");
        let on_disk = temp.path().resolve(&out);
        std::fs::create_dir_all(on_disk.parent().unwrap())?;
        std::fs::write(&on_disk, b"held by a root")?;
        let held = file("held by a root");
        drop(report(&dm, &out, held.dupe()).await?);
        // Inserted and then held by nothing, as a value DICE has dropped.
        let orphan = TrackedFileDigest::from_content(b"orphan", CasDigestConfig::testing_default());
        dm.store()
            .insert_bytes(b"orphan".to_vec(), &orphan, false)
            .await?;
        drop(orphan);
        dm.store().flush_collector().await;

        let dest = temp.path().resolve(path("buck-out/v2/gen/dest"));
        assert!(in_store(&dm, b"held by a root", &dest).await?);
        let nowhere = temp.path().resolve(path("buck-out/v2/gen/nowhere"));
        assert!(!in_store(&dm, b"orphan", &nowhere).await?);
        drop(held);
        abort(dm);
        Ok(())
    })
    .await
}

/// Polls `check` until it holds. The deadline is far beyond what the remover needs, since a
/// loaded test runner, the OSS cargo test run among them, stretches the handful of thread and
/// task hops a removal takes.
async fn eventually(
    mut check: impl AsyncFnMut() -> buck2_error::Result<bool>,
) -> buck2_error::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if check().await? {
            return Ok(());
        }
        assert!(Instant::now() < deadline, "the condition never held");
        sleep(Duration::from_millis(5)).await;
    }
}

/// Writes `content` at `p` and reports it as `value` under a lease over it, as a producer does.
/// The caller keeps `value`, as the action's dice value would: the stash holds a blob only
/// while some tracked digest of it is held.
async fn write_and_report(
    dm: &DiskStateMaterializer<StubIoHandler>,
    fs: &ProjectRoot,
    p: &ProjectRelativePathBuf,
    content: &str,
    value: &ArtifactValue,
) -> buck2_error::Result<WriteLease> {
    let lease = dm.prepare_outputs(vec![p.clone()]).await?;
    fs.write_file(p, content, false)?;
    dm.declare_existing(
        &lease,
        vec![DeclareArtifactPayload {
            path: p.clone(),
            artifact: value.dupe(),
        }],
    )
    .await?;
    Ok(lease)
}

#[tokio::test]
async fn test_strict_leases_remove_a_released_root_and_serve_it_again() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let fs = temp.path();
        let (dm, _io) = make_strict(fs, true);
        let p = path("buck-out/v2/gen/out");
        // Content no other test stashes: the digest table is the process's, and a record another
        // test's stash made for the same bytes would stand in for this one's.
        let value = file("removed and served again");

        let lease = write_and_report(&dm, fs, &p, "removed and served again", &value).await?;
        assert!(dm.has_artifact_at(p.clone()).await?);
        drop(lease);
        // Released: the root goes, from the tree and from disk.
        eventually(async || Ok(!dm.has_artifact_at(p.clone()).await?)).await?;
        assert!(!fs.resolve(&p).exists());

        // And comes back from the stash when asked for.
        let response = dm
            .materialize(request(vec![(p.clone(), value.dupe())]))
            .await?;
        let lease = response.ensure_results_ok()?;
        assert_eq!(
            std::fs::read_to_string(fs.resolve(&p))?,
            "removed and served again"
        );
        drop(lease);
        abort(dm);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_strict_leases_keep_final_outputs_and_what_is_read_after_the_command()
-> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let fs = temp.path();
        let (dm, _io) = make_strict(fs, true);

        // A final output stays: the user reads it after the command.
        let out = path("buck-out/v2/gen/out");
        let x = file("a final output");
        drop(write_and_report(&dm, fs, &out, "a final output", &x).await?);
        eventually(async || Ok(!dm.has_artifact_at(out.clone()).await?)).await?;
        let mut final_output = request(vec![(out.clone(), x.dupe())]);
        final_output.purpose = MaterializationPurpose::FinalOutput { required: true };
        drop(dm.materialize(final_output).await?.ensure_results_ok()?);

        // So does a test's output, which the runner reads after the command.
        let test_output = path("buck-out/v2/test/execution/out");
        let t = file("a test's output");
        drop(write_and_report(&dm, fs, &test_output, "a test's output", &t).await?);

        sleep(A_BIT).await;
        assert!(dm.has_artifact_at(out.clone()).await?);
        assert!(fs.resolve(&out).exists());
        assert!(dm.has_artifact_at(test_output.clone()).await?);
        assert!(fs.resolve(&test_output).exists());
        abort(dm);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_strict_leases_remove_a_released_root_nothing_could_put_back()
-> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let fs = temp.path();
        // No stash: a report's content exists nowhere but at its path.
        let (dm, _io) = make_strict(fs, false);
        let p = path("buck-out/v2/gen/out");
        let value = file("nothing could put this back");

        drop(write_and_report(&dm, fs, &p, "nothing could put this back", &value).await?);
        // The root goes all the same: a reader without a lease must not find it, and a request
        // for the content fails at the read instead.
        eventually(async || Ok(!dm.has_artifact_at(p.clone()).await?)).await?;
        assert!(!fs.resolve(&p).exists());
        abort(dm);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_strict_leases_remove_a_released_root_nothing_holds() -> buck2_error::Result<()> {
    ignore_stack_overflow_checks_for_future(async {
        let temp = ProjectRootTemp::new()?;
        let fs = temp.path();
        let (dm, _io) = make_strict(fs, true);
        let p = path("buck-out/v2/gen/out");
        let content = "held by its command alone";
        let value = file(content);

        let lease = write_and_report(&dm, fs, &p, content, &value).await?;
        // The last handle to the content goes before the lease, as an action's metadata file's
        // does; the stash drops the blob with it.
        drop(value);
        let digest = FileDigest::from_content(
            content.as_bytes(),
            DigestConfig::testing_default().cas_digest_config(),
        );
        assert!(file_digest_interner().peek(&digest).is_none());
        // The root goes with its lease as any freed root does.
        drop(lease);
        eventually(async || Ok(!dm.has_artifact_at(p.clone()).await?)).await?;
        assert!(!fs.resolve(&p).exists());
        abort(dm);
        Ok(())
    })
    .await
}
