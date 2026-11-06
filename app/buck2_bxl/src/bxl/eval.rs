/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::future::Future;
use std::sync::Arc;
use std::sync::LazyLock;

use allocative::Allocative;
use buck2_build_api::bxl::result::BxlResult;
use buck2_build_api::bxl::types::BxlFunctionLabel;
use buck2_common::events::HasEvents;
use buck2_common::scope::scope_and_collect_with_dice;
use buck2_common::target_aliases::BuckConfigTargetAliasResolver;
use buck2_core::cells::CellAliasResolver;
use buck2_core::cells::CellResolver;
use buck2_core::global_cfg_options::GlobalCfgOptions;
use buck2_core::package::PackageLabel;
use buck2_data::BxlExecutionEnd;
use buck2_data::BxlExecutionStart;
use buck2_data::StarlarkFailNoStacktrace;
use buck2_error::buck2_error;
use buck2_error::conversion::from_any_with_tag;
use buck2_error::starlark_error::from_starlark_with_options;
use buck2_events::dispatch::EventDispatcher;
use buck2_events::dispatch::console_message;
use buck2_events::dispatch::get_dispatcher;
use buck2_events::dispatch::with_dispatcher;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::digest_config::HasDigestConfig;
use buck2_hash::BuckFixedInsertionMap;
use buck2_interpreter::factory::BuckStarlarkModule;
use buck2_interpreter::factory::StarlarkEvaluatorProvider;
use buck2_interpreter::file_loader::LoadedModule;
use buck2_interpreter::load_module::InterpreterCalculation;
use buck2_interpreter::paths::module::StarlarkModulePath;
use buck2_interpreter::print_handler::EventDispatcherPrintHandler;
use buck2_interpreter::soft_error::Buck2StarlarkSoftErrorHandler;
use buck2_interpreter::starlark_profiler::data::StarlarkProfileDataAndStats;
use clap::error::ErrorKind;
use dice::DiceComputations;
use dice::DiceTransaction;
use dice_futures::cancellation::CancellationObserver;
use dupe::Dupe;
use itertools::Itertools;
use starlark::eval::Evaluator;
use starlark::values::OwnedFrozenRef;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::structs::AllocStruct;
use tokio::sync::Semaphore;

use crate::bxl::key::BxlKey;
use crate::bxl::starlark_defs::bxl_function::FrozenBxlFunction;
use crate::bxl::starlark_defs::bxl_function::OwnedBxlFunction;
use crate::bxl::starlark_defs::cli_args::CliArgValue;
use crate::bxl::starlark_defs::context::BxlContext;
use crate::bxl::starlark_defs::context::BxlContextCoreData;
use crate::bxl::starlark_defs::context::output::OutputStreamOutcome;
use crate::bxl::starlark_defs::context::output::OutputStreamState;
use crate::bxl::starlark_defs::context::starlark_async::BxlDiceComputations;
use crate::bxl::starlark_defs::eval_extra::BxlEvalExtra;
use crate::bxl::starlark_defs::functions::BxlErrorWithoutStacktrace;

pub(crate) static LIMITED_EXECUTOR: LazyLock<Arc<LimitedExecutor>> = LazyLock::new(|| {
    Arc::new(LimitedExecutor::new(500)) // Default working thread of tokio is 512 threads. We set it to 500 for here to leave some room for other things.
});

/// A limited executor that can be used to limit the number of concurrent bxl execution threads.
pub(crate) struct LimitedExecutor {
    semaphore: Arc<Semaphore>,
}

impl LimitedExecutor {
    fn new(limit: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(limit)),
        }
    }

    pub(crate) async fn execute<F, T>(&self, task: F) -> T
    where
        F: Future<Output = T>,
    {
        let _permit = self.semaphore.acquire().await.unwrap();
        task.await
    }
}

#[derive(Debug, Allocative, Clone, Dupe)]
pub(crate) struct BxlEvalError {
    pub(crate) output_stream_state: Option<Arc<OutputStreamOutcome>>,
    pub(crate) error: buck2_error::Error,
}

pub(crate) type Result<T> = std::result::Result<T, BxlEvalError>;

impl From<buck2_error::Error> for BxlEvalError {
    fn from(value: buck2_error::Error) -> Self {
        Self {
            output_stream_state: None,
            error: value,
        }
    }
}

impl From<starlark::Error> for BxlEvalError {
    fn from(value: starlark::Error) -> Self {
        Self {
            output_stream_state: None,
            error: value.into(),
        }
    }
}

impl From<tokio::task::JoinError> for BxlEvalError {
    fn from(value: tokio::task::JoinError) -> Self {
        Self {
            output_stream_state: None,
            error: value.into(),
        }
    }
}

pub(crate) async fn eval(
    ctx: &mut DiceComputations<'_>,
    key: BxlKey,
    liveness: CancellationObserver,
) -> Result<(BxlResult, Option<Arc<StarlarkProfileDataAndStats>>)> {
    // Note: because we use `block_in_place`, that will prevent the inner future from being polled
    // and yielded. So, for cancellation observers to work properly within the dice cancellable
    // future context, we need the future that it's attached to the cancellation context can
    // yield and be polled. To ensure that, we have to spawn the future that then enters block_in_place

    let dispatcher = ctx.per_transaction_data().get_dispatcher().dupe();

    let limited_executor = LIMITED_EXECUTOR.clone();

    let (_, futs) = unsafe {
        // SAFETY: as long as we don't `forget` the return object from `scope_and_collect`, it is safe

        // Additional cancellation notes:
        // the `scope_and_collect` will block on drop, but it will move the blocking to a tokio
        // blocking thread, freeing up the main worker threads. Additionally, the `spawn_cancellable`
        // on the scope will be dropped at the earliest await point. If we are within the blocking
        // section of bxl, the cancellation observer will be notified and cause the blocking calls
        // to terminate.
        scope_and_collect_with_dice(ctx, |ctx, s| {
            s.spawn_cancellable(
                limited_executor.execute(eval_bxl_inner(ctx, dispatcher, key, liveness)),
                || Err(buck2_error!(buck2_error::ErrorTag::Tier0, "cancelled").into()),
            )
        })
    }
    .await;

    match futs.into_iter().exactly_one() {
        Ok(res) => res?,
        Err(_) => panic!("only spawned one task"),
    }
}

struct BxlInnerEvaluator {
    data: BxlContextCoreData,
    module: LoadedModule,
    liveness: CancellationObserver,
    digest_config: DigestConfig,
    dispatcher: EventDispatcher,
}

impl BxlInnerEvaluator {
    fn do_eval(
        self,
        provider: StarlarkEvaluatorProvider,
        dice: &mut DiceComputations,
    ) -> Result<(BxlResult, Option<Arc<StarlarkProfileDataAndStats>>)> {
        let BxlInnerEvaluator {
            data,
            module,
            liveness,
            digest_config,
            dispatcher,
        } = self;

        BuckStarlarkModule::with_profiling(|env| {
            let key = data.key().dupe();

            let bxl_dice = BxlDiceComputations::new(dice, liveness.dupe());
            let data = Arc::new(data);

            let (finished_eval, (actions, output_stream_outcome)) = {
                let stream_state = OutputStreamState::new();

                let resolved_args = ValueOf::alloc_struct(
                    env.heap(),
                    AllocStruct(
                        key.cli_args()
                            .iter()
                            .map(|(k, v)| (k, v.as_starlark(env.heap()))),
                    ),
                );

                let print = EventDispatcherPrintHandler(dispatcher.clone());
                let mut extra = BxlEvalExtra::new(bxl_dice, data.dupe(), stream_state.dupe());

                provider
                    .with_evaluator(&env, liveness.into(), |eval, _| {
                        let bxl_function_name = key.label().name.clone();
                        let frozen_callable = get_bxl_callable(key.label(), &module)?;
                        eval.set_print_handler(&print);
                        eval.set_soft_error_handler(&Buck2StarlarkSoftErrorHandler);

                        eval.extra_mut = Some(&mut extra);

                        let force_print_stacktrace = key.force_print_stacktrace();
                        let bxl_ctx = BxlContext::new(
                            eval.heap(),
                            data,
                            stream_state.dupe(),
                            resolved_args,
                            digest_config,
                        )?;

                        let bxl_ctx = ValueTyped::<BxlContext>::new_err(env.heap().alloc(bxl_ctx))?;

                        tokio::task::block_in_place(|| {
                            with_dispatcher(dispatcher.clone(), || {
                                dispatcher.clone().span(
                                    BxlExecutionStart {
                                        name: bxl_function_name,
                                    },
                                    || {
                                        (
                                            eval_bxl(
                                                eval,
                                                frozen_callable,
                                                bxl_ctx,
                                                force_print_stacktrace,
                                            ),
                                            BxlExecutionEnd {},
                                        )
                                    },
                                )
                            })
                        })?;

                        BxlContext::take_state(bxl_ctx)
                    })
                    // When eval fails, we want to include the streaming cache file in Error, so
                    // that we can still print out the streaming content even if the bxl is cached.
                    .map_err(|e| match stream_state.take_state() {
                        Ok(stream_outcome) => BxlEvalError {
                            output_stream_state: Some(Arc::new(stream_outcome)),
                            error: e,
                        },
                        Err(_) => BxlEvalError {
                            output_stream_state: None,
                            error: e,
                        },
                    })?
            };

            let actions_finalizer = actions.finalize(&env)?;

            // TODO(cjhopman): Why is there so much divergence in code here for whether we created actions or
            // not? It seems to just make this unnecessarily complex.

            let (token, frozen_module, profile_data) = finished_eval.freeze_and_finish(env)?;
            let recorded_values = actions_finalizer(&frozen_module)?;

            let bxl_result = BxlResult::new(
                output_stream_outcome.output,
                output_stream_outcome.error,
                output_stream_outcome.streaming,
                output_stream_outcome.ensured_artifacts,
                output_stream_outcome.pending_streaming_outputs,
                recorded_values,
            );

            Ok((token, (bxl_result, profile_data)))
        })
    }
}

async fn eval_bxl_inner(
    ctx: &mut DiceComputations<'_>,
    dispatcher: EventDispatcher,
    key: BxlKey,
    liveness: CancellationObserver,
) -> Result<(BxlResult, Option<Arc<StarlarkProfileDataAndStats>>)> {
    let bxl_module = ctx
        .get_loaded_module(StarlarkModulePath::BxlFile(&key.label().bxl_path))
        .await?
        .dupe();

    let digest_config = ctx.global_data().get_digest_config();
    let core_data = BxlContextCoreData::new(key.dupe(), ctx).await?;

    // The bxl function may trigger async operations like builds, analysis, parsing etc, but those
    // will be blocking calls so that starlark can remain synchronous.
    // So indicate to tokio that this may block in place to avoid starvation. Ideally we use
    // spawn_blocking but that requires a static lifetime. There is no `join`s of multiple
    // futures that requires work to be done on the current thread, so using block_in_place
    // should have no noticeable different compared to spawn_blocking

    let eval_ctx = BxlInnerEvaluator {
        data: core_data,
        module: bxl_module,
        liveness,
        digest_config,
        dispatcher,
    };

    let eval_kind = key.as_starlark_eval_kind();
    let eval_provider = StarlarkEvaluatorProvider::new(ctx, eval_kind).await?;
    eval_ctx.do_eval(eval_provider, ctx)
}

fn eval_bxl<'v>(
    eval: &mut Evaluator<'v, '_, '_>,
    frozen_callable: OwnedBxlFunction,
    ctx: ValueTyped<'v, BxlContext<'v>>,
    force_print_stacktrace: bool,
) -> buck2_error::Result<()> {
    let bxl_impl = frozen_callable
        .as_ref()
        .add_to_heap(eval.heap())
        .as_ref()
        .implementation();
    let result = eval.eval_function(bxl_impl, &[ctx.to_value()], &[]);

    let e = match result {
        Ok(v) => {
            if !v.is_none() {
                return Err(NotAValidReturnType(v.get_type()).into());
            }

            return Ok(());
        }
        Err(e) => e,
    };

    let should_skip_backtrace = !force_print_stacktrace
        && match e.kind() {
            starlark::ErrorKind::Native(e) => {
                e.downcast_ref::<BxlErrorWithoutStacktrace>().is_some()
            }
            _ => false,
        };

    let e = from_starlark_with_options(
        e,
        buck2_error::starlark_error::NativeErrorHandling::Unknown,
        should_skip_backtrace,
    );
    if should_skip_backtrace {
        let dispatcher = get_dispatcher();
        dispatcher.instant_event(StarlarkFailNoStacktrace {
            trace: format!("{e}"),
        });
        dispatcher
            .console_message("Re-run the script with `-v5` to show the full stacktrace".to_owned());
    }

    Err(e)
}

pub(crate) fn get_bxl_callable(
    spec: &BxlFunctionLabel,
    bxl_module: &LoadedModule,
) -> buck2_error::Result<OwnedBxlFunction> {
    let callable = bxl_module
        .env()
        .get_any_visibility(&spec.name)
        .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0))?
        .0;

    Ok(callable.downcast_starlark::<FrozenBxlFunction<'static>>()?)
}

pub(crate) struct CliResolutionCtx<'d> {
    pub(crate) target_alias_resolver: BuckConfigTargetAliasResolver,
    pub(crate) cell_resolver: &'d CellResolver,
    pub(crate) cell_alias_resolver: &'d CellAliasResolver,
    pub(crate) relative_dir: PackageLabel,
    pub(crate) dice: &'d DiceTransaction,
    pub(crate) global_cfg_options: GlobalCfgOptions,
}

pub(crate) enum BxlResolvedCliArgs {
    Resolved(BuckFixedInsertionMap<String, CliArgValue>),
    Help,
}

pub(crate) async fn resolve_cli_args<'a>(
    spec: &BxlFunctionLabel,
    cli_ctx: &CliResolutionCtx<'a>,
    bxl_args: &Vec<String>,
    frozen_callable: OwnedFrozenRef<'a, ValueTyped<'static, FrozenBxlFunction<'static>>>,
) -> buck2_error::Result<BxlResolvedCliArgs> {
    let cli_spec = frozen_callable.value().as_ref().cli_spec();
    match cli_spec
        // ast-grep-ignore: rust/buck2-no-command-new
        .to_clap(clap::Command::new(&spec.name).no_binary_name(true))
        .try_get_matches_from(bxl_args)
    {
        Ok(args) => Ok(BxlResolvedCliArgs::Resolved(
            cli_spec.parse_clap(args, cli_ctx).await?,
        )),
        Err(e) => match e.kind() {
            ErrorKind::DisplayHelp => {
                let mut help_out = Vec::new();

                cli_spec
                    // ast-grep-ignore: rust/buck2-no-command-new
                    .to_clap(clap::Command::new(&spec.name).no_binary_name(true))
                    .write_long_help(&mut help_out)
                    .unwrap();
                let help_msg = String::from_utf8(help_out)?;

                console_message(help_msg);

                Ok(BxlResolvedCliArgs::Help)
            }
            _ => Err(e.into()),
        },
    }
}

#[derive(Debug, buck2_error::Error)]
#[error("Expected `NoneType` to be returned from bxl. Got return value `{0}`")]
#[buck2(tag = Input)]
struct NotAValidReturnType(&'static str);
