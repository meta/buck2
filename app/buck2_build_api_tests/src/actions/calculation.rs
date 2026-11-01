/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use assert_matches::assert_matches;
use buck2_analysis::analysis::calculation::AnalysisKey;
use buck2_artifact::actions::key::ActionIndex;
use buck2_artifact::actions::key::ActionKey;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_artifact::artifact::artifact_type::testing::BuildArtifactTestingExt;
use buck2_artifact::artifact::build_artifact::BuildArtifact;
use buck2_artifact::artifact::output_type::OutputType;
use buck2_artifact::artifact::source_artifact::SourceArtifact;
use buck2_build_api::actions::Action;
use buck2_build_api::actions::RegisteredAction;
use buck2_build_api::actions::calculation::ActionCalculation;
use buck2_build_api::actions::calculation::command_details;
use buck2_build_api::actions::execute::dice_data::CommandExecutorResponse;
use buck2_build_api::actions::execute::dice_data::HasCommandExecutor;
use buck2_build_api::actions::execute::dice_data::SetCommandExecutor;
use buck2_build_api::actions::execute::dice_data::SetInvalidationTrackingConfig;
use buck2_build_api::actions::execute::dice_data::SetLowPassFilter;
use buck2_build_api::actions::execute::dice_data::SetReClient;
use buck2_build_api::actions::execute::dice_data::set_fallback_executor_config;
use buck2_build_api::actions::impls::run_action_knobs::RunActionKnobs;
use buck2_build_api::actions::registry::RecordedActions;
use buck2_build_api::analysis::AnalysisResult;
use buck2_build_api::analysis::registry::RecordedAnalysisValues;
use buck2_build_api::artifact_groups::ArtifactGroup;
use buck2_build_api::artifact_groups::calculation::ArtifactGroupCalculation;
use buck2_build_api::build::detailed_aggregated_metrics::dice::SetDetailedAggregatedMetricsEventsHolder;
use buck2_build_api::build::detailed_aggregated_metrics::dice::SetDetailedAggregatedMetricsHandle;
use buck2_build_api::build::detailed_aggregated_metrics::events::DetailedAggregatedMetricsHandle;
use buck2_build_api::context::SetBuildContextData;
use buck2_build_api::keep_going::HasKeepGoing;
use buck2_build_api::spawner::BuckSpawner;
use buck2_common::dice::cells::SetCellResolver;
use buck2_common::dice::data::SetIsolationDir;
use buck2_common::dice::data::testing::SetTestingIoProvider;
use buck2_common::external_symlink::ExternalSymlink;
use buck2_common::file_ops::baseline::FileSystemBaseline;
use buck2_common::file_ops::baseline::SetFileSystemBaseline;
use buck2_common::file_ops::io::initialize_read_dir_cache;
use buck2_common::file_ops::metadata::FileMetadata;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_common::file_ops::testing::TestFileOps;
use buck2_common::http::SetHttpClient;
use buck2_common::legacy_configs::configs::LegacyBuckConfig;
use buck2_common::legacy_configs::dice::inject_legacy_config_for_test;
use buck2_core::category::CategoryRef;
use buck2_core::cells::CellResolver;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::cell_root_path::CellRootPathBuf;
use buck2_core::cells::name::CellName;
use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use buck2_core::configuration::data::ConfigurationData;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_core::deferred::key::DeferredHolderKey;
use buck2_core::execution_types::execution::ExecutionPlatformResolution;
use buck2_core::execution_types::executor_config::CommandExecutorConfig;
use buck2_core::execution_types::executor_config::RemoteExecutorUseCase;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_core::fs::project::ProjectRootTemp;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_core::package::source_path::SourcePath;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_directory::directory::entry::DirectoryEntry;
use buck2_events::dispatch::EventDispatcher;
use buck2_events::dispatch::with_dispatcher_async;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::digest_config::SetDigestConfig;
use buck2_execute::directory::ActionDirectoryMember;
use buck2_execute::execute::action_digest::ActionDigest;
use buck2_execute::execute::blocking::SetBlockingExecutor;
use buck2_execute::execute::blocking::testing::DummyBlockingExecutor;
use buck2_execute::execute::cache_uploader::NoOpCacheUploader;
use buck2_execute::execute::kind::CommandExecutionKind;
use buck2_execute::execute::low_pass_filter::LowPassFilter;
use buck2_execute::execute::output::CommandStdStreams;
use buck2_execute::execute::prepared::NoOpCommandOptionalExecutor;
use buck2_execute::execute::request::CommandExecutionOutput;
use buck2_execute::execute::result::CommandExecutionMetadata;
use buck2_execute::execute::result::CommandExecutionReport;
use buck2_execute::execute::result::CommandExecutionStatus;
use buck2_execute::execute::testing_dry_run::DryRunEntry;
use buck2_execute::execute::testing_dry_run::DryRunExecutor;
use buck2_execute::materialize::materializer::SetMaterializer;
use buck2_execute::re::invocation_re_settings::InvocationReSettings;
use buck2_execute::re::invocation_re_settings::SetInvocationReSettings;
use buck2_execute::re::manager::UnconfiguredRemoteExecutionClient;
use buck2_execute_impl::materializers::deferred::NoDiskDeferredMaterializer;
use buck2_file_watcher::dep_files::SetDepFileCache;
use buck2_file_watcher::dep_files::create_dep_file_cache;
use buck2_file_watcher::mergebase::SetMergebase;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use buck2_hash::BuckFixedSortedMap;
use buck2_hash::buck_mut_insertion_set;
use buck2_http::HttpClientBuilder;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_util::time_span::TimeSpan;
use dice::DiceTransaction;
use dice::UserComputationData;
use dice::testing::DiceBuilder;
use dupe::Dupe;
use maplit::btreemap;

use crate::actions::testings::SimpleAction;

fn create_test_configured_target_label() -> ConfiguredTargetLabel {
    TargetLabel::testing_parse("cell//pkg:foo").configure(ConfigurationData::testing_new())
}

fn create_test_build_artifact() -> BuildArtifact {
    let configured_target_label = create_test_configured_target_label();
    let deferred_id = ActionIndex::new(0);
    BuildArtifact::testing_new(configured_target_label, "bar.out", deferred_id)
}

fn create_test_source_artifact(package_label: &str, target_name: &str) -> SourceArtifact {
    SourceArtifact::new(SourcePath::testing_new(package_label, target_name))
}

fn registered_action(
    build_artifact: BuildArtifact,
    action: Box<dyn Action>,
) -> Arc<RegisteredAction> {
    let registered_action = RegisteredAction::new(
        build_artifact.key().dupe(),
        action,
        CommandExecutorConfig::testing_local(),
    );
    Arc::new(registered_action)
}

fn mock_analysis_for_action_resolution(
    dice_builder: DiceBuilder,
    action_key: &ActionKey,
    registered_action_arc: Arc<RegisteredAction>,
) -> DiceBuilder {
    let configured_target_label = create_test_configured_target_label();
    let node = ConfiguredTargetNode::testing_new(
        configured_target_label.dupe(),
        "foo_lib",
        ExecutionPlatformResolution::new_for_testing(None, Vec::new()),
        vec![],
        None,
    );

    assert_eq!(
        &DeferredHolderKey::Base(BaseDeferredKey::TargetLabel(configured_target_label.dupe())),
        action_key.holder_key()
    );

    let mut actions = RecordedActions::new(1);
    actions.insert(action_key.dupe(), registered_action_arc);

    // Action execution must obtain telemetry from its analysis holder. Leave the
    // configured-node key unmocked so an accidental graph lookup fails this test.
    dice_builder.mock_and_return(
        AnalysisKey(configured_target_label.dupe()),
        ResultMaybeCompatible::Compatible(
            AnalysisResult::new(
                RecordedAnalysisValues::testing_new(
                    action_key.holder_key().dupe(),
                    Vec::new(),
                    actions,
                ),
                None,
                BuckFixedSortedMap::default(),
                0,
                0,
                None,
                Vec::new(),
                node.rule_type().dupe(),
            )
            .expect("test analysis should have valid providers"),
        ),
    )
}

async fn make_default_dice_state(
    dry_run_tracker: Arc<Mutex<Vec<DryRunEntry>>>,
    temp_fs: &ProjectRootTemp,
    mocks: Vec<Box<dyn FnOnce(DiceBuilder) -> DiceBuilder>>,
) -> buck2_error::Result<DiceTransaction> {
    let fs = temp_fs.path().dupe();

    let cell_resolver = CellResolver::testing_with_name_and_path(
        CellName::testing_new("cell"),
        CellRootPathBuf::new(ProjectRelativePathBuf::unchecked_new("cell-path".into())),
    );
    let output_path = ProjectRelativePathBuf::unchecked_new("buck-out/v2".into());

    let mut dice_builder = DiceBuilder::new();
    dice_builder = dice_builder.set_data(|data| {
        data.set_testing_io_provider(temp_fs);
        data.set_digest_config(DigestConfig::testing_default());
        data.set_invalidation_tracking_config(true);
        data.set_detailed_aggregated_metrics_handle(DetailedAggregatedMetricsHandle::new());
    });

    for mock in mocks.into_iter() {
        dice_builder = mock(dice_builder);
    }

    let mut extra = UserComputationData::new();
    extra.set_keep_going(true);
    struct CommandExecutorProvider {
        dry_run_tracker: Arc<Mutex<Vec<DryRunEntry>>>,
    }
    impl HasCommandExecutor for CommandExecutorProvider {
        fn get_command_executor(
            &self,
            artifact_fs: &ArtifactFs,
            _config: &CommandExecutorConfig,
        ) -> buck2_error::Result<CommandExecutorResponse> {
            let executor = Arc::new(DryRunExecutor::new(
                self.dry_run_tracker.dupe(),
                artifact_fs.dupe(),
            ));
            Ok(CommandExecutorResponse {
                executor,
                action_cache_checker: Arc::new(NoOpCommandOptionalExecutor {}),
                remote_dep_file_cache_checker: Arc::new(NoOpCommandOptionalExecutor {}),
                platform: Default::default(),
                cache_uploader: Arc::new(NoOpCacheUploader {}),
                output_trees_download_config:
                    buck2_execute::re::output_trees_download_config::OutputTreesDownloadConfig::new(
                        None, true,
                    ),
                write_action_cache: None,
            })
        }
    }

    set_fallback_executor_config(&mut extra.data, CommandExecutorConfig::testing_local());
    extra.set_command_executor(Box::new(CommandExecutorProvider { dry_run_tracker }));
    extra.set_detailed_aggregated_metrics_events_holder();
    extra.set_blocking_executor(Arc::new(DummyBlockingExecutor { fs: fs.dupe() }));
    extra.set_materializer(Arc::new(NoDiskDeferredMaterializer::testing_new_no_disk(
        fs,
    )?));
    extra.set_re_client(UnconfiguredRemoteExecutionClient::testing_new_dummy());
    extra.set_invocation_re_settings(InvocationReSettings {
        use_case: RemoteExecutorUseCase::buck2_default(),
        cas_configured: false,
        may_probe_cas: false,
    });
    extra.set_http_client(HttpClientBuilder::https_with_system_roots().await?.build());
    extra.set_dep_file_cache(create_dep_file_cache());
    extra.set_mergebase(Default::default());
    extra.data.set(EventDispatcher::null());
    extra.data.set(RunActionKnobs::default());
    extra.set_low_pass_filter(Arc::new(LowPassFilter::new(1)));
    extra.spawner = Arc::new(BuckSpawner::current_runtime().unwrap());

    let mut computations = dice_builder.build(extra).await.unwrap();
    inject_legacy_config_for_test(
        &mut computations,
        CellName::testing_new("root"),
        LegacyBuckConfig::empty(),
    )?;
    computations.set_isolation_dir(temp_fs.path().resolve(&output_path))?;
    computations.set_buck_out_path(Some(output_path))?;
    computations.set_cell_resolver(cell_resolver)?;
    computations.set_file_system_baseline(FileSystemBaseline::unique())?;

    Ok(computations.commit().await)
}

#[tokio::test]
async fn test_get_action_for_artifact() -> buck2_error::Result<()> {
    buck2_certs::certs::maybe_setup_cryptography();
    let build_artifact = create_test_build_artifact();
    let registered_action = registered_action(
        build_artifact.dupe(),
        Box::new(SimpleAction::new(
            buck_mut_insertion_set![],
            buck_mut_insertion_set![build_artifact.dupe()],
            vec![],
            CategoryRef::new("fake_action").unwrap().to_owned(),
            None,
        )),
    );

    let mut dice_builder = DiceBuilder::new();
    dice_builder = mock_analysis_for_action_resolution(
        dice_builder,
        build_artifact.key(),
        registered_action.dupe(),
    );
    let dice_computations = dice_builder
        .build(UserComputationData::new())
        .await
        .unwrap()
        .commit()
        .await;

    let result = with_dispatcher_async(
        EventDispatcher::null(),
        ActionCalculation::get_action(&mut dice_computations.ctx(), build_artifact.key()),
    )
    .await;
    assert_eq!(result?, registered_action);
    Ok(())
}

#[tokio::test]
async fn test_build_action() -> buck2_error::Result<()> {
    buck2_certs::certs::maybe_setup_cryptography();
    let temp_fs = ProjectRootTemp::new()?;
    let build_artifact = create_test_build_artifact();
    let registered_action = registered_action(
        build_artifact.dupe(),
        Box::new(SimpleAction::new(
            buck_mut_insertion_set![],
            buck_mut_insertion_set![build_artifact.dupe()],
            vec!["foo".to_owned(), "cmd".to_owned()],
            CategoryRef::new("fake_action").unwrap().to_owned(),
            None,
        )),
    );

    let dry_run_tracker = Arc::new(Mutex::new(vec![]));
    let dice_computations = make_default_dice_state(
        dry_run_tracker.dupe(),
        &temp_fs,
        vec![{
            let action = registered_action.dupe();
            let action_key = build_artifact.key().dupe();
            Box::new(move |builder| {
                mock_analysis_for_action_resolution(builder, &action_key, action)
            })
        }],
    )
    .await?;

    let result =
        ActionCalculation::build_action(&mut dice_computations.ctx(), registered_action.key())
            .await;

    result.unwrap();

    assert_eq!(
        dry_run_tracker.lock().unwrap()[0],
        DryRunEntry {
            args: vec!["foo".to_owned(), "cmd".to_owned()],
            outputs: vec![CommandExecutionOutput::BuildArtifact {
                path: build_artifact.get_path().dupe(),
                output_type: OutputType::File,
            }],
            env: BuckFixedSortedMap::new()
        }
    );

    Ok(())
}

#[tokio::test]
async fn test_build_artifact() -> buck2_error::Result<()> {
    buck2_certs::certs::maybe_setup_cryptography();
    let temp_fs = ProjectRootTemp::new()?;
    let build_artifact = create_test_build_artifact();
    let registered_action = registered_action(
        build_artifact.dupe(),
        Box::new(SimpleAction::new(
            buck_mut_insertion_set![],
            buck_mut_insertion_set![build_artifact.dupe()],
            vec!["bar".to_owned(), "cmd".to_owned()],
            CategoryRef::new("fake_action").unwrap().to_owned(),
            None,
        )),
    );

    let dry_run_tracker = Arc::new(Mutex::new(vec![]));
    let dice_computations = make_default_dice_state(dry_run_tracker.dupe(), &temp_fs, {
        let registered_action = registered_action.dupe();
        let action_key = build_artifact.key().dupe();
        vec![Box::new(move |builder| {
            mock_analysis_for_action_resolution(builder, &action_key, registered_action)
        })]
    })
    .await?;

    let result = with_dispatcher_async(
        EventDispatcher::null(),
        ActionCalculation::build_artifact(&mut dice_computations.ctx(), &build_artifact),
    )
    .await;

    result.unwrap();

    assert_eq!(
        dry_run_tracker.lock().unwrap()[0],
        DryRunEntry {
            args: vec!["bar".to_owned(), "cmd".to_owned()],
            outputs: vec![CommandExecutionOutput::BuildArtifact {
                path: build_artifact.get_path().dupe(),
                output_type: OutputType::File,
            }],
            env: BuckFixedSortedMap::new()
        }
    );
    Ok(())
}

#[tokio::test]
async fn test_ensure_artifact_build_artifact() -> buck2_error::Result<()> {
    buck2_certs::certs::maybe_setup_cryptography();
    let temp_fs = ProjectRootTemp::new()?;
    let build_artifact = create_test_build_artifact();
    let registered_action = registered_action(
        build_artifact.dupe(),
        Box::new(SimpleAction::new(
            buck_mut_insertion_set![],
            buck_mut_insertion_set![build_artifact.dupe()],
            vec!["ensure".to_owned(), "cmd".to_owned()],
            CategoryRef::new("fake_action").unwrap().to_owned(),
            None,
        )),
    );

    let dry_run_tracker = Arc::new(Mutex::new(vec![]));
    let dice_computations = make_default_dice_state(dry_run_tracker.dupe(), &temp_fs, {
        let registered_action = registered_action.dupe();
        let action_key = build_artifact.key().dupe();
        vec![Box::new(move |builder| {
            mock_analysis_for_action_resolution(builder, &action_key, registered_action)
        })]
    })
    .await?;

    let result = with_dispatcher_async(
        EventDispatcher::null(),
        dice_computations
            .ctx()
            .ensure_artifact_group(&ArtifactGroup::Artifact(build_artifact.dupe().into())),
    )
    .await;

    result.unwrap();

    assert_eq!(
        dry_run_tracker.lock().unwrap()[0],
        DryRunEntry {
            args: vec!["ensure".to_owned(), "cmd".to_owned()],
            outputs: vec![CommandExecutionOutput::BuildArtifact {
                path: build_artifact.get_path().dupe(),
                output_type: OutputType::File,
            }],
            env: BuckFixedSortedMap::new()
        }
    );

    Ok(())
}

/// DICE over `file_ops`, with the `cell` cell at the project root.
async fn dice_with_test_file_ops(file_ops: &TestFileOps) -> buck2_error::Result<DiceTransaction> {
    let cells = CellResolver::testing_with_name_and_path(
        CellName::testing_new("cell"),
        CellRootPathBuf::new(ProjectRelativePathBuf::unchecked_new("".into())),
    );
    let dice_builder = DiceBuilder::new().set_data(|data| {
        data.set_digest_config(DigestConfig::testing_default());
    });
    let mut extra = UserComputationData::new();
    initialize_read_dir_cache(&mut extra);
    let mut dice = file_ops
        .mock_in_dice(&cells, dice_builder)
        .build(extra)
        .await
        .unwrap();
    dice.set_cell_resolver(cells)?;
    dice.set_file_system_baseline(FileSystemBaseline::unique())?;
    Ok(dice.commit().await)
}

#[tokio::test]
async fn test_ensure_artifact_source_artifact() -> buck2_error::Result<()> {
    buck2_certs::certs::maybe_setup_cryptography();
    let digest_config = DigestConfig::testing_default();

    let path = CellPath::testing_new("cell//pkg/src.cpp");
    let source_artifact = create_test_source_artifact("cell//pkg", "src.cpp");
    let metadata = FileMetadata {
        digest: TrackedFileDigest::from_content(b"content", digest_config.cas_digest_config()),
        is_executable: true,
    };

    let file_ops = TestFileOps::new_with_files_metadata(btreemap![path => metadata.dupe()]);
    let dice_computations = dice_with_test_file_ops(&file_ops).await?;

    let source_artifact = Artifact::from(source_artifact);
    let input = ArtifactGroup::Artifact(source_artifact.dupe());
    let result = with_dispatcher_async(
        EventDispatcher::null(),
        dice_computations.ctx().ensure_artifact_group(&input),
    )
    .await?
    .iter()
    .cloned()
    .collect::<Vec<_>>();

    assert_eq!(
        &result,
        &[(
            source_artifact,
            ArtifactValue::file(FileMetadata {
                digest: metadata.digest,
                is_executable: metadata.is_executable,
            })
        )],
    );
    Ok(())
}

#[tokio::test]
async fn test_ensure_artifact_external_symlink() -> buck2_error::Result<()> {
    buck2_certs::certs::maybe_setup_cryptography();
    let path = CellPath::testing_new("cell//proj/to_gvfs/include");
    let source_artifact = create_test_source_artifact("cell//proj/to_gvfs", "include");
    let symlink = Arc::new(
        ExternalSymlink::new(
            PathBuf::from("/mnt/gvfs"),
            ForwardRelativePathBuf::new("include".to_owned()).unwrap(),
        )
        .unwrap(),
    );

    let file_ops = TestFileOps::new_with_symlinks(btreemap![path => symlink.dupe()]);
    let dice_computations = dice_with_test_file_ops(&file_ops).await?;

    let source_artifact = Artifact::from(source_artifact);
    let input = ArtifactGroup::Artifact(source_artifact.dupe());
    let result = with_dispatcher_async(
        EventDispatcher::null(),
        dice_computations.ctx().ensure_artifact_group(&input),
    )
    .await?
    .iter()
    .cloned()
    .collect::<Vec<_>>();

    assert_eq!(
        &result,
        &[(
            source_artifact,
            ArtifactValue::new(
                DirectoryEntry::Leaf(ActionDirectoryMember::ExternalSymlink(symlink)),
                None
            )
        )]
    );
    Ok(())
}

#[tokio::test]
async fn test_command_details_omission() {
    use buck2_data::command_execution_kind::Command;

    buck2_certs::certs::maybe_setup_cryptography();
    let digest_config = DigestConfig::testing_default();

    let mut report = CommandExecutionReport {
        claim: None,
        status: CommandExecutionStatus::Success {
            execution_kind: CommandExecutionKind::Local {
                digest: ActionDigest::empty(digest_config.cas_digest_config()),
                command: vec![],
                env: BuckFixedSortedMap::new(),
            },
        },
        timing: CommandExecutionMetadata::empty(TimeSpan::empty_now()),
        std_streams: CommandStdStreams::Local {
            stdout: "stdout".to_owned().into_bytes(),
            stderr: "stderr".to_owned().into_bytes(),
        },
        exit_code: Some(1),
        additional_message: None,
        inline_environment_metadata: buck2_data::InlineCommandExecutionEnvironmentMetadata {
            sandcastle_instance_id: Some(123),
        },
    };

    let proto = command_details(&report, false).await;
    let command_kind = proto.command_kind.unwrap();
    assert_matches!(command_kind.command, Some(Command::LocalCommand(..)));
    assert_eq!(&proto.cmd_stdout, "stdout");
    assert_eq!(&proto.cmd_stderr, "stderr");

    let proto = command_details(&report, true).await;
    let command_kind = proto.command_kind.unwrap();
    assert_matches!(command_kind.command, Some(Command::OmittedLocalCommand(..)));
    assert_eq!(&proto.cmd_stdout, "");
    assert_eq!(&proto.cmd_stderr, "stderr");

    report.status = CommandExecutionStatus::Failure {
        execution_kind: CommandExecutionKind::Local {
            digest: ActionDigest::empty(digest_config.cas_digest_config()),
            command: vec![],
            env: BuckFixedSortedMap::new(),
        },
        missing_materialized_inputs: false,
    };
    let proto = command_details(&report, true).await;
    let command_kind = proto.command_kind.unwrap();
    assert_matches!(command_kind.command, Some(Command::LocalCommand(..)));
    assert_eq!(&proto.cmd_stdout, "stdout");
    assert_eq!(&proto.cmd_stderr, "stderr");
}
