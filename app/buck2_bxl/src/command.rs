/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use buck2_build_api::actions::artifact::get_artifact_fs::GetArtifactFs;
use buck2_build_api::actions::graph_recorder::with_graph_recording;
use buck2_build_api::artifact_groups::ArtifactGroup;
use buck2_build_api::build::build_report::BuildReportOpts;
use buck2_build_api::build::build_report::write_bxl_build_report;
use buck2_build_api::bxl::result::BxlResult;
use buck2_build_api::bxl::result::PendingStreamingOutput;
use buck2_build_api::bxl::types::BxlFunctionLabel;
use buck2_build_api::materialize::HasMaterializationQueueTracker;
use buck2_build_api::materialize::MaterializationAndUploadContext;
use buck2_build_api::materialize::materialize_and_upload_artifact_group;
use buck2_cli_proto::BxlRequest;
use buck2_cli_proto::BxlResponse;
use buck2_cli_proto::build_request::Materializations;
use buck2_cli_proto::build_request::Uploads;
use buck2_common::dice::cells::HasCellResolver;
use buck2_common::events::HasEvents;
use buck2_common::target_aliases::HasTargetAliasResolver;
use buck2_core::bxl::BxlFilePath;
use buck2_core::cells::CellAliasResolver;
use buck2_core::cells::CellResolver;
use buck2_core::cells::cell_path_with_allowed_relative_dir::CellPathWithAllowedRelativeDir;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::global_cfg_options::GlobalCfgOptions;
use buck2_core::package::PackageLabel;
use buck2_data::BxlEnsureArtifactsEnd;
use buck2_data::BxlEnsureArtifactsStart;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_events::dispatch::get_dispatcher;
use buck2_hash::BuckFixedInsertionMap;
use buck2_hash::BuckMutMap;
use buck2_interpreter::load_module::InterpreterCalculation;
use buck2_interpreter::parse_import::ParseImportOptions;
use buck2_interpreter::parse_import::RelativeImports;
use buck2_interpreter::parse_import::parse_import_with_config;
use buck2_interpreter::paths::module::StarlarkModulePath;
use buck2_server_ctx::bxl::GetBxlStreamingTracker;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::global_cfg_options::global_cfg_options_from_client_context;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;
use buck2_server_ctx::template::ServerCommandTemplate;
use buck2_server_ctx::template::run_server_command;
use dice::DiceComputations;
use dice::DiceTransaction;
use dupe::Dupe;
use dupe::IterDupedExt;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use itertools::Itertools;

use crate::bxl;
use crate::bxl::calculation::eval_bxl;
use crate::bxl::eval::BxlResolvedCliArgs;
use crate::bxl::eval::CliResolutionCtx;
use crate::bxl::eval::get_bxl_callable;
use crate::bxl::eval::resolve_cli_args;
use crate::bxl::key::BxlKey;
use crate::bxl::starlark_defs::cli_args::CliArgValue;
use crate::bxl::streaming_output_writer::StreamingOutputWriter;

pub(crate) async fn bxl_command(
    ctx: &dyn ServerCommandContextTrait,
    partial_result_dispatcher: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
    req: BxlRequest,
) -> buck2_error::Result<BxlResponse> {
    run_server_command(BxlServerCommand { req }, ctx, partial_result_dispatcher).await
}

struct BxlServerCommand {
    req: BxlRequest,
}

#[async_trait]
impl ServerCommandTemplate for BxlServerCommand {
    type StartEvent = buck2_data::BxlCommandStart;
    type EndEvent = buck2_data::BxlCommandEnd;
    type Response = buck2_cli_proto::BxlResponse;
    type PartialResult = buck2_cli_proto::StdoutBytes;

    fn start_event(&self) -> Self::StartEvent {
        let bxl_label = self.req.bxl_label.clone();
        buck2_data::BxlCommandStart { bxl_label }
    }

    fn end_event(&self, _response: &buck2_error::Result<Self::Response>) -> Self::EndEvent {
        let bxl_label = self.req.bxl_label.clone();
        buck2_data::BxlCommandEnd { bxl_label }
    }

    async fn command(
        &self,
        server_ctx: &dyn ServerCommandContextTrait,
        mut partial_result_dispatcher: PartialResultDispatcher<Self::PartialResult>,
        ctx: DiceTransaction,
    ) -> buck2_error::Result<Self::Response> {
        let work = self.execute(
            server_ctx,
            partial_result_dispatcher.as_writer(),
            ctx.dupe(),
        );
        with_graph_recording(ctx, work, |response| {
            (!response.errors.is_empty())
                .then_some(buck2_data::action_graph_footer::Incomplete::BuildFailed)
        })
        .await
    }
}

impl BxlServerCommand {
    async fn execute(
        &self,
        server_ctx: &dyn ServerCommandContextTrait,
        _stdout: impl Write + Send,
        dice_ctx: DiceTransaction,
    ) -> buck2_error::Result<buck2_cli_proto::BxlResponse> {
        let bxl_cmd_ctx = self
            .parse_and_validate_request(server_ctx, &dice_ctx)
            .await?;

        let resolved_cli_args = self.resolve_cli_args(&bxl_cmd_ctx, &dice_ctx).await?;
        let bxl_args = match resolved_cli_args {
            BxlResolvedCliArgs::Resolved(bxl_args) => Arc::new(bxl_args),
            BxlResolvedCliArgs::Help => {
                return Ok(BxlResponse {
                    project_root: bxl_cmd_ctx.project_root,
                    errors: Vec::new(),
                    serialized_build_report: None,
                });
            }
        };

        let bxl_eval_result = self.eval_bxl(&bxl_cmd_ctx, &dice_ctx, bxl_args).await;

        // let per_transaction_data = dice_ctx.per_transaction_data();
        let dispatcher = dice_ctx.per_transaction_data().get_dispatcher().dupe();
        let mut streaming_output_writer = StreamingOutputWriter::new(dispatcher);

        // If the bxl result is cached, we need to output the streaming outputs to stdout
        let bxl_result = match bxl_eval_result {
            Ok(bxl_result) => {
                self.emit_streaming_output(
                    &dice_ctx,
                    bxl_result.streaming(),
                    &mut streaming_output_writer,
                )?;
                bxl_result
            }
            Err(e) => {
                if let Some(output) = &e.output_stream_state {
                    self.emit_streaming_output(
                        &dice_ctx,
                        &output.streaming,
                        &mut streaming_output_writer,
                    )?;
                }
                return Err(e.error);
            }
        };

        let errors = self
            .materialize_artifacts(&dice_ctx, bxl_result.dupe(), &mut streaming_output_writer)
            .await;

        self.emit_outputs(server_ctx, bxl_result, &mut streaming_output_writer)
            .await?;

        let error_reports = errors
            .iter()
            .map(buck2_data::ErrorReport::from)
            .unique_by(|e| e.message.clone())
            .collect();

        let serialized_build_report = self
            .write_build_report(&bxl_cmd_ctx, &dice_ctx, server_ctx, errors)
            .await?;

        Ok(BxlResponse {
            project_root: bxl_cmd_ctx.project_root,
            errors: error_reports,
            serialized_build_report,
        })
    }

    async fn parse_and_validate_request<'d>(
        &self,
        server_ctx: &'d dyn ServerCommandContextTrait,
        dice_ctx: &'d DiceTransaction,
    ) -> buck2_error::Result<BxlCommandContext<'d>> {
        let cwd = server_ctx.working_dir();
        let cell_resolver = dice_ctx.ctx().get_cell_resolver().await?;
        let cell_alias_resolver = dice_ctx.ctx().get_cell_alias_resolver_for_dir(cwd).await?;
        let bxl_label = parse_bxl_label_from_cli(
            cwd,
            &self.req.bxl_label,
            &cell_resolver,
            &cell_alias_resolver,
        )?;
        let project_root = server_ctx.project_root().to_string();

        let global_cfg_options = global_cfg_options_from_client_context(
            self.req
                .target_cfg
                .as_ref()
                .internal_error("target_cfg must be set")?,
            server_ctx,
            &mut dice_ctx.ctx(),
        )
        .await?;

        Ok(BxlCommandContext {
            cwd,
            cell_resolver,
            bxl_label,
            project_root,
            global_cfg_options,
        })
    }

    async fn resolve_cli_args(
        &self,
        ctx: &BxlCommandContext<'_>,
        dice_ctx: &DiceTransaction,
    ) -> buck2_error::Result<BxlResolvedCliArgs> {
        let cur_package = PackageLabel::from_cell_path(ctx.cell_resolver.get_cell_path(ctx.cwd))?;
        let cell_name = ctx.cell_resolver.find(ctx.cwd);
        let cell_alias_resolver = dice_ctx.ctx().get_cell_alias_resolver(cell_name).await?;

        let target_alias_resolver = dice_ctx.ctx().target_alias_resolver().await?.dupe();

        let bxl_module = dice_ctx
            .ctx()
            .get_loaded_module(StarlarkModulePath::BxlFile(&ctx.bxl_label.bxl_path))
            .await?;

        let frozen_callable = get_bxl_callable(&ctx.bxl_label, bxl_module)?;
        let cli_ctx = CliResolutionCtx {
            target_alias_resolver,
            cell_resolver: ctx.cell_resolver,
            cell_alias_resolver,
            relative_dir: cur_package,
            dice: dice_ctx,
            global_cfg_options: ctx.global_cfg_options.dupe(),
        };

        resolve_cli_args(
            &ctx.bxl_label,
            &cli_ctx,
            &self.req.bxl_args,
            frozen_callable.as_ref(),
        )
        .await
    }

    /// Evaluate the bxl main function and return the result.
    async fn eval_bxl(
        &self,
        ctx: &BxlCommandContext<'_>,
        dice_ctx: &DiceTransaction,
        bxl_args: Arc<BuckFixedInsertionMap<String, CliArgValue>>,
    ) -> bxl::eval::Result<Arc<BxlResult>> {
        let bxl_key = BxlKey::new(
            ctx.bxl_label.clone(),
            bxl_args,
            self.req.print_stacktrace,
            ctx.global_cfg_options.dupe(),
        );

        eval_bxl(&mut dice_ctx.ctx(), bxl_key.clone()).await
    }

    /// Materializes artifacts from the BXL result
    ///
    /// Returns errors encountered during materialization.
    async fn materialize_artifacts(
        &self,
        dice_ctx: &DiceTransaction,
        bxl_result: Arc<BxlResult>,
        output: &mut (impl Write + Send),
    ) -> Vec<buck2_error::Error> {
        let materialization_context = self.create_materialization_context();

        self.ensure_all_artifacts(
            &mut dice_ctx.ctx(),
            materialization_context,
            bxl_result,
            output,
        )
        .await
    }

    /// Creates a materialization context from request parameters.
    /// This context determines how artifacts will be materialized and uploaded.
    fn create_materialization_context(&self) -> MaterializationAndUploadContext {
        let materializations = Materializations::try_from(self.req.final_artifact_materializations)
            .with_buck_error_context(|| "Invalid final_artifact_materializations")
            .unwrap();

        let uploads = Uploads::try_from(self.req.final_artifact_uploads)
            .with_buck_error_context(|| "Invalid final_artifact_uploads")
            .unwrap();

        (materializations, uploads).into()
    }

    /// Ensures that all artifacts from BXL execution are materialized.
    ///
    /// Wraps the materialization process in tracing spans.
    ///
    /// Returns errors encountered during materialization.
    async fn ensure_all_artifacts(
        &self,
        ctx: &mut DiceComputations<'_>,
        materialization_context: MaterializationAndUploadContext,
        bxl_result: Arc<BxlResult>,
        output: &mut (impl Write + Send),
    ) -> Vec<buck2_error::Error> {
        if bxl_result.artifacts().is_empty() {
            return vec![];
        }

        let result = get_dispatcher()
            .span_async(BxlEnsureArtifactsStart {}, async move {
                let result = self
                    .materialize_collected_artifacts(
                        ctx,
                        materialization_context,
                        bxl_result,
                        output,
                    )
                    .await;

                (result, BxlEnsureArtifactsEnd {})
            })
            .await;

        match result {
            Ok(_) => vec![],
            Err(errors) => errors,
        }
    }

    /// Collects and materializes artifacts
    ///
    /// Returns the aggregated errors encountered during materialization.
    async fn materialize_collected_artifacts(
        &self,
        ctx: &mut DiceComputations<'_>,
        materialization_context: MaterializationAndUploadContext,
        bxl_result: Arc<BxlResult>,
        output: &mut (impl Write + Send),
    ) -> Result<(), Vec<buck2_error::Error>> {
        let artifacts_to_materialize: Vec<_> = bxl_result.artifacts().iter().duped().collect();

        let mut futs: FuturesUnordered<_> = ctx
            .compute_many(artifacts_to_materialize.into_iter().map(|artifact| {
                DiceComputations::declare_closure(async |ctx| {
                    let res = materialize_and_upload_artifact_group(
                        ctx,
                        &artifact,
                        materialization_context,
                        &ctx.per_transaction_data()
                            .get_materialization_queue_tracker(),
                    )
                    .await;
                    match res {
                        Ok(_) => Ok(artifact),
                        Err(e) => Err(e),
                    }
                })
            }))
            .into_iter()
            .collect();

        let mut pending_streaming =
            PendingStreaming::new(bxl_result.pending_streaming_outputs().iter().cloned());

        let mut errors: Vec<buck2_error::Error> = Vec::new();

        while let Some(res) = tokio::task::unconstrained(futs.next()).await {
            match res {
                Ok(artifact) => {
                    let outputs = pending_streaming.next_outputs(&artifact);
                    for output_msg in outputs {
                        output.write_all(&output_msg).unwrap_or_else(|e| {
                            errors.push(e.into());
                        });
                    }
                    output.flush().unwrap_or_else(|e| {
                        errors.push(e.into());
                    });
                }
                Err(e) => errors.push(e),
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Output the outputs from BxlResult to stdout and stderr
    async fn emit_outputs(
        &self,
        server_ctx: &dyn ServerCommandContextTrait,
        bxl_result: Arc<BxlResult>,
        mut stdout: impl Write,
    ) -> buck2_error::Result<()> {
        stdout.write_all(bxl_result.output())?;
        server_ctx.stderr()?.write_all(bxl_result.error())?;
        Ok(())
    }

    /// Output streaming output to stdout if BxlKey is cached.
    /// Note that when cached, we do not eval the bxl, so we don't get the streaming output in the bxl script.
    fn emit_streaming_output(
        &self,
        dice_ctx: &DiceTransaction,
        streaming_output: &[u8],
        stdout: &mut impl Write,
    ) -> buck2_error::Result<()> {
        let bxl_streaming_tracker = dice_ctx
            .per_transaction_data()
            .get_bxl_streaming_tracker()
            .expect("BXL streaming tracker should be set");
        if !bxl_streaming_tracker.was_called() {
            stdout.write_all(streaming_output)?;
            stdout.flush()?;
            Ok(())
        } else {
            Ok(())
        }
    }

    async fn write_build_report(
        &self,
        ctx: &BxlCommandContext<'_>,
        dice_ctx: &DiceTransaction,
        server_ctx: &dyn ServerCommandContextTrait,
        ensured_artifact_errors: Vec<buck2_error::Error>,
    ) -> buck2_error::Result<Option<String>> {
        let bxl_opts = self
            .req
            .build_opts
            .as_ref()
            .expect("should have build options");

        let serialized_build_report = if bxl_opts.unstable_print_build_report {
            let artifact_fs = dice_ctx.ctx().get_artifact_fs().await?;
            let build_report_opts = BuildReportOpts {
                // These are all deprecated for `buck2 build`, so don't need to support them
                print_unconfigured_section: false,
                unstable_include_failures_build_report: false,
                unstable_include_package_project_relative_paths: false,
                unstable_include_artifact_hash_information: false,
                unstable_build_report_filename: bxl_opts.unstable_build_report_filename.clone(),
                graph_properties_opts: Default::default(),
                unstable_streaming_build_report_filename: bxl_opts
                    .unstable_streaming_build_report_filename
                    .clone(),
                unstable_exclude_action_error_diagnostics: false,
                unstable_truncate_error_content: false,
            };

            write_bxl_build_report(
                build_report_opts,
                artifact_fs,
                &ctx.cell_resolver,
                server_ctx.project_root(),
                ctx.cwd,
                server_ctx.events().trace_id(),
                &ctx.bxl_label,
                &ensured_artifact_errors,
                None,
                None,
                None,
            )?
        } else {
            None
        };

        Ok(serialized_build_report)
    }
}

#[derive(Debug)]
struct BxlCommandContext<'d> {
    cwd: &'d ProjectRelativePath,
    cell_resolver: &'d CellResolver,
    bxl_label: BxlFunctionLabel,
    project_root: String,
    global_cfg_options: GlobalCfgOptions,
}

pub(crate) async fn get_bxl_cli_args(
    cwd: &ProjectRelativePath,
    ctx: &DiceTransaction,
    bxl_label: &BxlFunctionLabel,
    bxl_args: &Vec<String>,
    cell_resolver: &CellResolver,
    global_cfg_options: &GlobalCfgOptions,
) -> buck2_error::Result<BxlResolvedCliArgs> {
    let cur_package = PackageLabel::from_cell_path(cell_resolver.get_cell_path(&cwd))?;
    let cell_name = cell_resolver.find(&cwd);
    let cell_alias_resolver = ctx.ctx().get_cell_alias_resolver(cell_name).await?;

    let target_alias_resolver = ctx.ctx().target_alias_resolver().await?.dupe();

    let bxl_module = ctx
        .ctx()
        .get_loaded_module(StarlarkModulePath::BxlFile(&bxl_label.bxl_path))
        .await?;

    let frozen_callable = get_bxl_callable(bxl_label, bxl_module)?;
    let cli_ctx = CliResolutionCtx {
        target_alias_resolver,
        cell_resolver,
        cell_alias_resolver,
        relative_dir: cur_package,
        dice: ctx,
        global_cfg_options: global_cfg_options.dupe(),
    };

    resolve_cli_args(bxl_label, &cli_ctx, bxl_args, frozen_callable.as_ref()).await
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum BxlLabelError {
    #[error(
        "bxl label should be of format `<cell>//path/to/file.bxl:function_name`, but got `{0}`"
    )]
    Format(String),
}

/// Parse the bxl function label out of cli pattern
pub(crate) fn parse_bxl_label_from_cli(
    cwd: &ProjectRelativePath,
    bxl_label: &str,
    cell_resolver: &CellResolver,
    cell_alias_resolver: &CellAliasResolver,
) -> buck2_error::Result<BxlFunctionLabel> {
    let current_cell = cell_resolver.get_cell_path(cwd);

    let (bxl_path, bxl_fn) = bxl_label
        .rsplit_once(':')
        .ok_or_else(|| BxlLabelError::Format(bxl_label.to_owned()))?;

    let opts: ParseImportOptions = ParseImportOptions {
        allow_missing_at_symbol: true,
        relative_import_option: RelativeImports::Allow {
            current_dir_with_allowed_relative: &CellPathWithAllowedRelativeDir::new(
                current_cell,
                None,
            ),
        },
    };
    let import_path = parse_import_with_config(cell_alias_resolver, bxl_path, &opts)?;

    // FIXME(JakobDegen): A bxl path that resolves into a cell other than the one it names should
    // be an error; as it is, changes to the file may go unnoticed.
    Ok(BxlFunctionLabel {
        bxl_path: BxlFilePath::new(import_path)?,
        name: bxl_fn.to_owned(),
    })
}

#[derive(Debug)]
struct PendingStreaming {
    indexes: BuckMutMap<ArtifactGroup, Vec<Arc<Mutex<PendingStreamingOutput>>>>,
}

impl PendingStreaming {
    fn new(pending_streaming_outputs: impl Iterator<Item = PendingStreamingOutput>) -> Self {
        let mut indexes: BuckMutMap<ArtifactGroup, Vec<Arc<Mutex<PendingStreamingOutput>>>> =
            BuckMutMap::default();

        let pending_streaming_outputs = pending_streaming_outputs
            .into_iter()
            .map(|p| Arc::new(Mutex::new(p)));

        for pending_streaming_output in pending_streaming_outputs {
            let guard = pending_streaming_output.lock().unwrap();
            let waits_on = guard.waits_on();
            for wait_on in waits_on {
                indexes
                    .entry(wait_on.dupe())
                    .or_default()
                    .push(pending_streaming_output.dupe())
            }
        }

        Self { indexes }
    }

    fn next_outputs(&mut self, artifact: &ArtifactGroup) -> Vec<Vec<u8>> {
        let mut outputs = vec![];
        if let Some(pendings) = self.indexes.remove(artifact) {
            for pending in pendings.iter() {
                let mut pending_guard = pending.lock().unwrap();
                pending_guard.remove_wait_on(artifact);
                if !pending_guard.is_pending() {
                    outputs.push(pending_guard.output().to_owned());
                }
            }
        }
        outputs
    }
}

#[cfg(test)]
mod tests {
    use buck2_artifact::actions::key::ActionIndex;
    use buck2_artifact::artifact::artifact_type::Artifact;
    use buck2_artifact::artifact::artifact_type::testing::BuildArtifactTestingExt;
    use buck2_artifact::artifact::build_artifact::BuildArtifact;
    use buck2_core::configuration::data::ConfigurationData;
    use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
    use buck2_hash::BuckMutInsertionSet;

    use super::*;

    fn new_test_artifact_group(id: u32) -> ArtifactGroup {
        let path = format!("a{id}");
        let target =
            ConfiguredTargetLabel::testing_parse("cell//pkg:foo", ConfigurationData::testing_new());
        let build_artifact = BuildArtifact::testing_new(target.dupe(), &path, ActionIndex::new(id));
        let artifact = Artifact::from(build_artifact);

        ArtifactGroup::Artifact(artifact)
    }

    #[test]
    fn test_pending_streaming_next_outputs() {
        let a1 = new_test_artifact_group(1);
        let a2 = new_test_artifact_group(2);
        let a3 = new_test_artifact_group(3);
        let p1 = PendingStreamingOutput::new(
            BuckMutInsertionSet::from([a1.dupe(), a2.dupe()]),
            b"output1".to_vec(),
        );
        let p2 = PendingStreamingOutput::new(
            BuckMutInsertionSet::from([a1.dupe()]),
            b"output2".to_vec(),
        );
        let p3 = PendingStreamingOutput::new(
            BuckMutInsertionSet::from([a2.dupe(), a3.dupe()]),
            b"output3".to_vec(),
        );

        let mut pending_streaming = PendingStreaming::new(vec![p1, p2, p3].into_iter());

        let a4 = new_test_artifact_group(4);
        let results_after_a4 = pending_streaming.next_outputs(&a4);
        // a4 is not tracked under any artifact, so it does not produce output.
        assert_eq!(results_after_a4, Vec::<Vec<u8>>::new());

        // Trigger a1
        let results_after_a1 = pending_streaming.next_outputs(&a1);
        // p1 still depends on a2, so it does not produce output yet.
        // p2 no longer depends on any other artifact, so it produces "output2".
        // p3 is not tracked under a1, so it is not processed at this point.
        // Therefore, only "output2" should be returned.
        assert_eq!(results_after_a1, vec![b"output2".to_vec()]);

        // Trigger a2
        let results_after_a2 = pending_streaming.next_outputs(&a2);
        // p1 no longer depends on any other artifact, so it produces "output1".
        // p3 still needs a3, so it will not produce anything yet.
        // Therefore, "output1" should be returned.
        assert_eq!(results_after_a2, vec![b"output1".to_vec()]);

        // Trigger a3
        let results_after_a3 = pending_streaming.next_outputs(&a3);
        // p3 no longer depends on any other artifact, so it produces "output3".
        // Therefore, "output3" should be returned.
        assert_eq!(results_after_a3, vec![b"output3".to_vec()]);

        // Triggering the same artifact again should produce no additional output.
        let empty_result = pending_streaming.next_outputs(&a2);
        assert_eq!(empty_result, Vec::<Vec<u8>>::new());
    }
}
