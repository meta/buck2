/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_cli_proto::new_generic::CompleteRequest;
use buck2_cli_proto::new_generic::CompleteResponse;
use buck2_common::pattern::parse_from_cli::parse_patterns_from_cli_args;
use buck2_core::pattern::pattern_type::TargetPatternExtra;
use buck2_events::dispatch::with_dispatcher_async;
use buck2_node::load_patterns::MissingTargetBehavior;
use buck2_node::load_patterns::load_patterns;
use buck2_node::nodes::eval_result::is_generated_target;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::partial_result_dispatcher::NoPartialResult;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;
use buck2_server_ctx::template::ServerCommandTemplate;
use buck2_server_ctx::template::run_server_command;
use dice::DiceTransaction;

pub(crate) async fn complete_command(
    ctx: &dyn ServerCommandContextTrait,
    partial_result_dispatcher: PartialResultDispatcher<NoPartialResult>,
    req: CompleteRequest,
) -> buck2_error::Result<CompleteResponse> {
    run_server_command(
        CompleteServerCommand { req },
        ctx,
        partial_result_dispatcher,
    )
    .await
}

struct CompleteServerCommand {
    req: CompleteRequest,
}

#[async_trait::async_trait]
impl ServerCommandTemplate for CompleteServerCommand {
    type StartEvent = buck2_data::CompleteCommandStart;
    type EndEvent = buck2_data::CompleteCommandEnd;
    type Response = buck2_cli_proto::new_generic::CompleteResponse;
    type PartialResult = NoPartialResult;

    async fn command(
        &self,
        server_ctx: &dyn ServerCommandContextTrait,
        _partial_result_dispatcher: PartialResultDispatcher<Self::PartialResult>,
        dice: DiceTransaction,
    ) -> buck2_error::Result<Self::Response> {
        let cwd = server_ctx.working_dir().to_buf();
        let partial_target = self.req.partial_target.clone();
        let dispatcher = server_ctx.events().clone();

        // Put the actual work behind a spawned task - we do this so that if the client hits the
        // timeout and cancels the request, the actual load itself will not be cancelled and will
        // continue to run in the background. This way we give the load a chance to complete and
        // the next time the user hits tab, completions might be available. Otherwise, it would
        // never be possible to get completions for a buildfile that takes more than 500ms to load.
        tokio::spawn(with_dispatcher_async(dispatcher, async move {
            let parsed_target_patterns = parse_patterns_from_cli_args::<TargetPatternExtra>(
                &mut dice.ctx(),
                &[partial_target],
                &cwd,
            )
            .await?;

            let results = &load_patterns(
                &mut dice.ctx(),
                parsed_target_patterns,
                MissingTargetBehavior::Fail,
            )
            .await?;

            let mut output: Vec<String> = vec![];
            for node in results.iter_loaded_targets() {
                let node = node?;

                if is_generated_target(node) {
                    continue;
                }
                output.push(format!("{}", node.label()));
            }
            Ok(CompleteResponse {
                completions: output,
            })
        }))
        .await
        .unwrap()
    }

    fn exclusive_command_name(&self) -> Option<String> {
        Some("complete".to_owned())
    }
}
