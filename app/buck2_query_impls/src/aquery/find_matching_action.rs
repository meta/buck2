/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;

use buck2_artifact::actions::key::ActionKey;
use buck2_artifact::artifact::build_artifact::BuildArtifact;
use buck2_build_api::actions::query::ActionQueryNode;
use buck2_build_api::actions::query::FIND_MATCHING_ACTION;
use buck2_build_api::analysis::AnalysisResult;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::global_cfg_options::GlobalCfgOptions;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use dice::DiceComputations;
use dupe::Dupe;
use dupe::IterDupedExt;
use either::Either;
use futures::FutureExt;
use tracing::debug;

use crate::aquery::evaluator::get_dice_aquery_delegate;

// Given the buckout path, how do we search actions?
enum ActionKeyMatch<'v> {
    // This action key exactly produces the output path.
    Exact(&'v ActionKey),
    // Builds an output that is in the path.
    OutputsOf(&'v BuildArtifact),
}

fn check_output_path<'v>(
    build_artifact: &'v BuildArtifact,
    path_to_check: &ForwardRelativePathBuf,
) -> buck2_error::Result<Option<ActionKeyMatch<'v>>> {
    let path = build_artifact.get_path().path();

    debug!(
        "Checking action's output path: {}. Path to check: {}",
        path, path_to_check
    );

    let key = build_artifact.key();

    if path_to_check == path {
        Ok(Some(ActionKeyMatch::Exact(key)))
    } else if path_to_check.starts_with(path) {
        Ok(Some(ActionKeyMatch::OutputsOf(build_artifact)))
    } else {
        Ok(None)
    }
}

async fn find_matching_action(
    ctx: &mut DiceComputations<'_>,
    working_dir: &ProjectRelativePath,
    global_cfg_options: &GlobalCfgOptions,
    analysis: &AnalysisResult,
    short_path: ForwardRelativePathBuf,
) -> buck2_error::Result<Option<ActionQueryNode>> {
    ctx.with_linear_recompute(|ctx| {
        async move {
            let dice_aquery_delegate =
                get_dice_aquery_delegate(ctx, working_dir, global_cfg_options.dupe()).await?;

            // Try to find exact path match first. If there are no exact matches, try to find an action
            // that starts with the relevant part of the output path (this case is for targets that declare
            // directories as outputs).
            //
            // FIXME(@wendyy): If we've iterated over all build artifacts and still haven't found an exact
            // action key match, then return a possible action key with the shortest path. This can happen
            // if a target declared an output directory instead of an artifact. As a best effort, we keep
            // track of the possible build artifact with the shortest path to try find the action that produced
            // the top-most directory. To fix this properly, we would need to let the action key or build
            // artifact itself know if the output was a directory, which is nontrivial.

            // TODO(cjhopman): We should probably just support iterating over declared artifacts rather than
            // this more complex approach.
            let mut maybe_match: Option<BuildArtifact> = None;
            let analysis_values = analysis.analysis_values()?;
            let action_outputs = analysis_values
                .iter_actions()
                .flat_map(|action| match action {
                    Ok(action) => Either::Left(
                        match action.action().outputs() {
                            Cow::Borrowed(outputs) => Either::Left(outputs.iter().duped()),
                            Cow::Owned(outputs) => Either::Right(outputs.into_iter()),
                        }
                        .map(Ok),
                    ),
                    Err(error) => Either::Right(std::iter::once(Err(error))),
                });

            for build_artifact in analysis_values
                .iter_dynamic_lambda_outputs()?
                .map(Ok)
                .chain(action_outputs)
            {
                let build_artifact = build_artifact?;
                if let Some(action_key_match) = check_output_path(&build_artifact, &short_path)? {
                    match action_key_match {
                        ActionKeyMatch::Exact(key) => {
                            return Ok(Some(dice_aquery_delegate.get_action_node(key).await?));
                        }
                        ActionKeyMatch::OutputsOf(artifact) => {
                            if let Some(maybe) = &maybe_match {
                                if artifact.get_path().path().as_str().len()
                                    < maybe.get_path().path().as_str().len()
                                {
                                    maybe_match = Some(artifact.dupe());
                                }
                            } else {
                                maybe_match = Some(artifact.dupe())
                            }
                        }
                    }
                }
            }

            match maybe_match {
                Some(maybe) => Ok(Some(
                    dice_aquery_delegate.get_action_node(maybe.key()).await?,
                )),
                None => Ok(None),
            }
        }
        .boxed()
    })
    .await
}

pub(crate) fn init_find_matching_action() {
    FIND_MATCHING_ACTION.init(
        |ctx, working_dir, global_cfg_options, analysis, short_path| {
            Box::pin(find_matching_action(
                ctx,
                working_dir,
                global_cfg_options,
                analysis,
                short_path,
            ))
        },
    );
}
