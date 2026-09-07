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

use buck2_analysis::analysis::calculation::AnalysisKey;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_artifact::artifact::source_artifact::SourceArtifact;
use buck2_build_api::actions::registry::RecordedActions;
use buck2_build_api::analysis::AnalysisResult;
use buck2_build_api::analysis::registry::RecordedAnalysisValues;
use buck2_build_api::artifact_groups::ArtifactGroup;
use buck2_build_api::artifact_groups::TransitiveSetProjectionKey;
use buck2_build_api::artifact_groups::TransitiveSetProjectionWrapper;
use buck2_build_api::artifact_groups::calculation::ArtifactGroupCalculation;
use buck2_build_api::artifact_groups::deferred::TransitiveSetKey;
use buck2_build_api::context::SetBuildContextData;
use buck2_build_api::interpreter::rule_defs::transitive_set::TransitiveSet;
use buck2_build_api::interpreter::rule_defs::transitive_set::TransitiveSetOrdering;
use buck2_build_api::keep_going::HasKeepGoing;
use buck2_common::dice::cells::SetCellResolver;
use buck2_common::dice::data::SetIsolationDir;
use buck2_common::file_ops::baseline::FileSystemBaseline;
use buck2_common::file_ops::baseline::SetFileSystemBaseline;
use buck2_common::file_ops::io::initialize_read_dir_cache;
use buck2_common::file_ops::metadata::FileMetadata;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_common::file_ops::testing::TestFileOps;
use buck2_common::legacy_configs::configs::LegacyBuckConfig;
use buck2_common::legacy_configs::dice::inject_legacy_config_for_test;
use buck2_core::cells::CellResolver;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::cell_root_path::CellRootPathBuf;
use buck2_core::cells::name::CellName;
use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_core::deferred::key::DeferredHolderKey;
use buck2_core::fs::project::ProjectRootTemp;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::package::source_path::SourcePath;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_directory::directory::entry::DirectoryEntry;
use buck2_directory::directory::find::find;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::digest_config::SetDigestConfig;
use buck2_execute::directory::ActionDirectoryMember;
use buck2_fs::paths::RelativePathBuf;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_hash::BuckFixedSortedMap;
use buck2_hash::BuckMutMap;
use buck2_node::rule_type::RuleType;
use dice::UserComputationData;
use dice::testing::DiceBuilder;
use dupe::Dupe;
use indoc::indoc;
use maplit::btreemap;
use starlark::values::OwnedFrozen;
use starlark::values::ValueTyped;

use crate::interpreter::transitive_set::testing::new_transitive_set;

fn mock_analysis_for_tsets(
    mut dice_builder: DiceBuilder,
    tsets: Vec<OwnedFrozen<ValueTyped<'static, TransitiveSet<'static>>>>,
) -> DiceBuilder {
    let mut by_target: BuckMutMap<
        ConfiguredTargetLabel,
        Vec<(
            TransitiveSetKey,
            OwnedFrozen<ValueTyped<'static, TransitiveSet<'static>>>,
        )>,
    > = BuckMutMap::default();

    for value in tsets {
        let key = value.by_ref(|s| s.key().dupe());
        match key.holder_key() {
            DeferredHolderKey::Base(BaseDeferredKey::TargetLabel(target)) => {
                by_target
                    .entry(target.dupe())
                    .or_default()
                    .push((key, value.dupe()));
            }
            _ => unreachable!("we only make fake tsets with configured targets `{}`", key),
        }
    }

    for (target, tsets) in by_target.into_iter() {
        dice_builder = dice_builder.mock_and_return(
            AnalysisKey(target.dupe()),
            ResultMaybeCompatible::Compatible(
                AnalysisResult::new(
                    RecordedAnalysisValues::testing_new(
                        DeferredHolderKey::Base(BaseDeferredKey::TargetLabel(target)),
                        tsets,
                        RecordedActions::new(0),
                    ),
                    None,
                    BuckFixedSortedMap::default(),
                    0,
                    0,
                    None,
                    Vec::new(),
                    RuleType::Forward,
                )
                .expect("test analysis should have valid providers"),
            ),
        );
    }

    dice_builder
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Intentional: serializing test access via global lock
async fn test_ensure_artifact_group() -> buck2_error::Result<()> {
    // Serialize with other tests that use make_tset() and its shared global counter
    // Covers DICE tasks too: the current-thread tokio runtime polls them all on
    // this thread.
    let _stack_guard = buck2_util::threads::ignore_stack_overflow_checks_for_current_thread();

    let digest_config = DigestConfig::testing_default();

    let set = new_transitive_set(indoc!(
        r#"
            def project(value):
                return value

            TestSet = transitive_set(args_projections = {
                "project": project
            })

            foo = source_artifact("foo", "foo")
            bar = source_artifact("bar", "bar")

            def make():
                s1 = make_tset(TestSet, value = foo)
                return make_tset(TestSet, value = bar, children = [s1])
            "#
    ))?;

    let cell_resolver = CellResolver::testing_with_names_and_paths(&[
        (
            CellName::testing_new("root"),
            CellRootPathBuf::testing_new("cell-path"),
        ),
        (
            CellName::testing_new("parent"),
            CellRootPathBuf::testing_new(""),
        ),
    ]);

    let foo = CellPath::testing_new("root//foo/foo");

    let foo_artifact = Artifact::from(SourceArtifact::new(SourcePath::testing_new(
        "root//foo",
        "foo",
    )));

    let foo_meta = FileMetadata {
        digest: TrackedFileDigest::from_content(b"foo", digest_config.cas_digest_config()),
        is_executable: true,
    };

    let bar_artifact = Artifact::from(SourceArtifact::new(SourcePath::testing_new(
        "root//bar",
        "bar",
    )));

    let bar = CellPath::testing_new("root//bar/bar");

    let bar_meta = FileMetadata {
        digest: TrackedFileDigest::from_content(b"bar", digest_config.cas_digest_config()),
        is_executable: true,
    };

    let files = TestFileOps::new_with_files_metadata(btreemap![
        foo => foo_meta.dupe(),
        bar => bar_meta.dupe(),
    ]);

    let cell_parent = CellName::testing_new("parent");
    let dice_builder = files.mock_in_dice(&cell_resolver, DiceBuilder::new());
    let mut dice_builder = dice_builder.set_data(|data| {
        data.set_digest_config(DigestConfig::testing_default());
    });

    let mut all_tsets = vec![set.dupe()];
    // This is kinda clowny, but we can't upcast the TransitiveSetGen back to a Value so we
    // have to access Values from their parents.
    set.by_ref_with_reconstructor(|s, reconstructor| {
        for set in s.iter(TransitiveSetOrdering::Preorder).unwrap() {
            for child in set.unwrap().children.read().unwrap().iter() {
                all_tsets.push(reconstructor.reconstruct(ValueTyped::new(*child).unwrap()));
            }
        }
    });

    // Register all the sets as deferreds.
    dice_builder = mock_analysis_for_tsets(dice_builder, all_tsets);

    let mut extra = UserComputationData::new();
    extra.set_keep_going(true);
    initialize_read_dir_cache(&mut extra);

    let mut dice = dice_builder.build(extra).await.unwrap();
    dice.set_cell_resolver(cell_resolver)?;
    dice.set_file_system_baseline(FileSystemBaseline::unique())?;
    dice.set_buck_out_path(None)?;
    let fs = ProjectRootTemp::new()?;
    dice.set_isolation_dir(
        fs.path()
            .resolve(ProjectRelativePath::unchecked_new("buck-out/v2")),
    )?;
    inject_legacy_config_for_test(&mut dice, cell_parent, LegacyBuckConfig::empty())?;
    let dice = dice.commit().await;

    let result = dice
        .ctx()
        .ensure_artifact_group(&ArtifactGroup::TransitiveSetProjection(Arc::new(
            TransitiveSetProjectionWrapper::new(
                TransitiveSetProjectionKey {
                    key: set.by_ref(|s| s.key.dupe()),
                    projection: 0,
                },
                false,
                false,
            ),
        )))
        .await?
        .iter()
        .cloned()
        .collect::<Vec<_>>();

    assert_eq!(
        &result,
        &[
            (
                bar_artifact,
                ArtifactValue::file(FileMetadata {
                    digest: bar_meta.digest,
                    is_executable: bar_meta.is_executable,
                })
            ),
            (
                foo_artifact,
                ArtifactValue::file(FileMetadata {
                    digest: foo_meta.digest,
                    is_executable: foo_meta.is_executable,
                })
            ),
        ]
    );

    Ok(())
}

async fn source_artifact_value(
    files: &TestFileOps,
    fs: &ProjectRootTemp,
    package: &str,
    path: &str,
) -> buck2_error::Result<ArtifactValue> {
    let cell = CellName::testing_new("root");
    let cell_resolver =
        CellResolver::testing_with_name_and_path(cell, CellRootPathBuf::testing_new(""));
    let dice_builder = files
        .mock_in_dice(&cell_resolver, DiceBuilder::new())
        .set_data(|data| {
            data.set_digest_config(DigestConfig::testing_default());
        });
    let mut extra = UserComputationData::new();
    initialize_read_dir_cache(&mut extra);
    let mut dice = dice_builder.build(extra).await.unwrap();
    dice.set_cell_resolver(cell_resolver)?;
    dice.set_file_system_baseline(FileSystemBaseline::unique())?;
    dice.set_buck_out_path(None)?;
    dice.set_isolation_dir(
        fs.path()
            .resolve(ProjectRelativePath::unchecked_new("buck-out/v2")),
    )?;
    inject_legacy_config_for_test(&mut dice, cell, LegacyBuckConfig::empty())?;
    let dice = dice.commit().await;

    let artifact = Artifact::from(SourceArtifact::new(SourcePath::testing_new(package, path)));
    let values = dice
        .ctx()
        .ensure_artifact_group(&ArtifactGroup::Artifact(artifact))
        .await?;
    Ok(values.iter().next().unwrap().1.dupe())
}

fn symlink_target(value: &ArtifactValue, path: &str) -> String {
    let DirectoryEntry::Dir(dir) = value.entry() else {
        panic!("not a directory");
    };
    match find(dir, ForwardRelativePath::new(path).unwrap().iter()).unwrap() {
        Some(DirectoryEntry::Leaf(ActionDirectoryMember::Symlink(link))) => {
            link.target().as_str().to_owned()
        }
        _ => panic!("`{path}` is not a symlink"),
    }
}

#[tokio::test]
async fn test_source_dir_symlinks_to_ancestors_are_leaves() -> buck2_error::Result<()> {
    let _stack_guard = buck2_util::threads::ignore_stack_overflow_checks_for_current_thread();
    let files = TestFileOps::new_with_files_and_symlinks(
        btreemap![
            CellPath::testing_new("root//pkg/dir/file") => "file".to_owned(),
            CellPath::testing_new("root//pkg/dir/deeper/file") => "deeper".to_owned(),
        ],
        btreemap![
            CellPath::testing_new("root//pkg/dir/self") => RelativePathBuf::from("."),
            CellPath::testing_new("root//pkg/dir/deeper/up") => RelativePathBuf::from(".."),
        ],
    );
    let fs = ProjectRootTemp::new()?;

    let value = source_artifact_value(&files, &fs, "root//pkg", "dir").await?;

    assert_eq!(symlink_target(&value, "self"), ".");
    assert_eq!(symlink_target(&value, "deeper/up"), "..");
    assert!(value.deps().is_none());

    // Reading through such a link still resolves to the target.
    let value = source_artifact_value(&files, &fs, "root//pkg", "dir/self/file").await?;
    assert_eq!(
        value,
        ArtifactValue::file(FileMetadata {
            digest: TrackedFileDigest::from_content(
                b"file",
                DigestConfig::testing_default().cas_digest_config()
            ),
            is_executable: false,
        })
    );
    Ok(())
}

#[tokio::test]
async fn test_source_dir_symlink_elsewhere_keeps_deps() -> buck2_error::Result<()> {
    let _stack_guard = buck2_util::threads::ignore_stack_overflow_checks_for_current_thread();
    let files = TestFileOps::new_with_files_and_symlinks(
        btreemap![
            CellPath::testing_new("root//pkg/dir/file") => "file".to_owned(),
            CellPath::testing_new("root//pkg/other/file") => "other".to_owned(),
        ],
        btreemap![
            CellPath::testing_new("root//pkg/dir/link") => RelativePathBuf::from("../other"),
        ],
    );
    let fs = ProjectRootTemp::new()?;

    let value = source_artifact_value(&files, &fs, "root//pkg", "dir").await?;

    assert_eq!(symlink_target(&value, "link"), "../other");
    let deps = value.deps().expect("the symlink target is a dep");
    assert!(
        find(deps, ForwardRelativePath::new("pkg/other/file")?.iter())
            .unwrap()
            .is_some()
    );
    Ok(())
}
