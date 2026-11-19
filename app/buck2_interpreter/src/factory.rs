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

use buck2_common::legacy_configs::dice::HasLegacyConfigs;
use buck2_error::conversion::from_any_with_tag;
use buck2_settings::Starlark;
use buck2_settings::dice::HasDiceBuckSettings;
use dice::DiceComputations;
use dice::UserComputationData;
use dice_futures::cancellation::CancellationPoller;
use dupe::Dupe;
use starlark::environment::FrozenModule;
use starlark::environment::Module;
use starlark::eval::Evaluator;
use starlark::values::FrozenHeapName;

use crate::dice::starlark_debug::HasStarlarkDebugger;
use crate::dice::starlark_provider::StarlarkEvalKind;
use crate::from_freeze::from_freeze_error;
use crate::starlark_debug::StarlarkDebugController;
use crate::starlark_profiler::config::GetStarlarkProfilerInstrumentation;
use crate::starlark_profiler::data::StarlarkProfileDataAndStats;
use crate::starlark_profiler::profiler::ProfilerData;

pub struct StarlarkEvaluatorProvider {
    pub(crate) profiler_data: ProfilerData,
    eval_kind: StarlarkEvalKind,
    debugger: Option<Box<dyn StarlarkDebugController>>,
    profile_listener: Option<Arc<dyn ProfileEventListener>>,
    starlark_max_callstack_size: Option<usize>,
}

/// Holds profiler data associated with completed Starlark evaluation.
///
/// The caller is responsible for calling finish() to ensure that profile data collection is completed.
#[must_use]
pub struct FinishedStarlarkEvaluation {
    pub(crate) profiler_data: ProfilerData,
    eval_kind: StarlarkEvalKind,
    listener: Option<Arc<dyn ProfileEventListener>>,
}

impl FinishedStarlarkEvaluation {
    fn finish_impl(
        self,
        frozen_module: Option<&FrozenModule>,
    ) -> buck2_error::Result<(
        ProfilingReportedToken,
        Option<Arc<StarlarkProfileDataAndStats>>,
    )> {
        let res = self.profiler_data.finish(frozen_module, self.eval_kind);
        if let Ok(Some(res)) = &res {
            if let Some(listener) = &self.listener {
                listener.profile_collected(res.targets[0].clone(), res);
            }
        }
        res.map(|res| (ProfilingReportedToken(()), res))
    }

    /// Collect all profiling data.
    pub fn finish(
        self,
    ) -> buck2_error::Result<(
        ProfilingReportedToken,
        Option<Arc<StarlarkProfileDataAndStats>>,
    )> {
        self.finish_impl(None)
    }

    pub fn freeze_and_finish(
        self,
        env: BuckStarlarkModule,
    ) -> buck2_error::Result<(
        ProfilingReportedToken,
        FrozenModule,
        Option<Arc<StarlarkProfileDataAndStats>>,
    )> {
        let frozen = env
            .0
            .freeze_named(FrozenHeapName::User(Box::new(self.eval_kind.dupe())))
            .map_err(from_freeze_error)?;
        let (token, profile_data) = self.finish_impl(Some(&frozen))?;
        Ok((token, frozen, profile_data))
    }
}

impl StarlarkEvaluatorProvider {
    /// Trivial provider that just constructs an Evaluator. Useful for tests (but not necessarily limited to them).
    pub fn passthrough(eval_kind: StarlarkEvalKind) -> Self {
        Self {
            profiler_data: ProfilerData::new(None),
            eval_kind,
            debugger: None,
            starlark_max_callstack_size: None,
            profile_listener: None,
        }
    }

    /// This constructs an appropriate StarlarkEvaluatorProvider to set up
    /// profiling/instrumentation/debugging in a starlark Evaluator for buck.
    /// The kind is used for the thread name when debugging and for enabling pattern-based profiling.
    pub async fn new(
        ctx: &mut DiceComputations<'_>,
        eval_kind: StarlarkEvalKind,
    ) -> buck2_error::Result<StarlarkEvaluatorProvider> {
        let profile_mode = ctx.get_starlark_profiler_mode(&eval_kind).await?;

        let root_buckconfig = ctx.get_legacy_root_config_on_dice().await?;
        let profile_listener = ctx.get_profile_event_listener().cloned();

        let starlark = ctx.dice_settings::<Starlark>().await?;
        let starlark_max_callstack_size =
            starlark.max_callstack_size(&mut root_buckconfig.view(ctx))?;

        let debugger_handle = ctx.get_starlark_debugger_handle();
        let debugger = match debugger_handle {
            Some(v) => Some(v.start_eval(&eval_kind.to_string()).await?),
            None => None,
        };

        Ok(Self {
            profiler_data: ProfilerData::new(profile_mode.profile_mode().copied()),
            eval_kind,
            debugger,
            starlark_max_callstack_size,
            profile_listener,
        })
    }

    /// Construct a "reentrant" evaluator, one where we can have multiple stages of interacting with
    /// the Evaluator. The underlying starlark Evaluator will persist across calls to
    /// ReentrantStarlarkEvaluator::with_evaluator.
    ///
    /// For example, this is used in analysis evaluations to:
    ///  (1) evaluate main analysis phase
    ///  (2) async wait for anon targets
    ///  (3) re-enter evaluation to resolve promises.
    pub fn make_reentrant_evaluator<'v, 'a, 'e>(
        mut self,
        module: &'a BuckStarlarkModule<'v>,
        cancellation: CancellationPoller,
    ) -> buck2_error::Result<ReentrantStarlarkEvaluator<'v, 'a, 'e>> {
        let (_, _v) = (buck2_error::Ok(()), 1);
        let mut eval = Evaluator::new(&module.0);
        if let Some(stack_size) = self.starlark_max_callstack_size {
            eval.set_max_callstack_size(stack_size)
                .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0))?;
        }

        eval.set_check_cancelled(Box::new(move || cancellation.is_cancelled()));

        let is_profiling_enabled = self.profiler_data.initialize(&mut eval)?;
        if let Some(v) = &mut self.debugger {
            v.initialize(&mut eval)?;
        }

        Ok(ReentrantStarlarkEvaluator {
            eval,
            provider: self,
            is_profiling_enabled,
        })
    }

    /// Applies a closure to a starlark Evaluator. Taking this via a closure ensures that the Evaluator
    /// isn't used in an async context and allows us to do things like the block_in_place required
    /// when debugging.
    pub fn with_evaluator<'v, 'a, 'e: 'a, R>(
        self,
        module: &'a BuckStarlarkModule<'v>,
        cancellation: CancellationPoller,
        closure: impl FnOnce(&mut Evaluator<'v, 'a, 'e>, bool) -> buck2_error::Result<R>,
    ) -> buck2_error::Result<(FinishedStarlarkEvaluation, R)> {
        let mut reentrant_eval: ReentrantStarlarkEvaluator<'v, '_, '_> =
            self.make_reentrant_evaluator(module, cancellation)?;
        let is_profiling_enabled = reentrant_eval.is_profiling_enabled;
        let res = reentrant_eval.with_evaluator(|eval| closure(eval, is_profiling_enabled))?;
        Ok((reentrant_eval.finish_evaluation(), res))
    }
}

pub struct ReentrantStarlarkEvaluator<'v, 'a, 'e: 'a> {
    eval: Evaluator<'v, 'a, 'e>,
    provider: StarlarkEvaluatorProvider,
    is_profiling_enabled: bool,
}

impl<'v, 'a, 'e: 'a> ReentrantStarlarkEvaluator<'v, 'a, 'e> {
    pub fn with_evaluator<R>(
        &mut self,
        closure: impl FnOnce(&mut Evaluator<'v, 'a, 'e>) -> buck2_error::Result<R>,
    ) -> buck2_error::Result<R> {
        // If we're debugging, we need to move this to a tokio blocking task.
        //
        // This is required because the debugger itself is running on the
        // tokio worker tasks, and if we have a starlark breakpoint in common
        // code we could get a lot of evaluators all blocked waiting on the debugger
        // and those could block all the tokio worker tasks and the debugger wouldn't
        // even get a chance to resume them.
        //
        // It's the debuggers responsibility to ensure that we don't run too many
        // evaluations concurrently (in the non-debugger case they are limited by the
        // tokio worker tasks, but once in a blocking task that limit is greatly
        // increased).

        // TODO(cjhopman): It would be nicer if we could have this functionality be
        // provided by the debugger handle, but I couldn't figure out a nice clean
        // way to do that. Potentially the thing would be to invert the dependencies
        // so we could operate against a concrete type rather than injecting a trait
        // implementation.

        if self.provider.debugger.is_some() {
            tokio::task::block_in_place(|| closure(self.eval()))
        } else {
            closure(self.eval())
        }
    }

    fn eval(&mut self) -> &mut Evaluator<'v, 'a, 'e> {
        &mut self.eval
    }

    pub fn finish_evaluation(self) -> FinishedStarlarkEvaluation {
        let ReentrantStarlarkEvaluator {
            mut eval,
            mut provider,
            ..
        } = self;
        provider.profiler_data.evaluation_complete(&mut eval);
        FinishedStarlarkEvaluation {
            profiler_data: provider.profiler_data,
            eval_kind: provider.eval_kind,
            listener: provider.profile_listener,
        }
    }
}

pub trait ProfileEventListener: Send + Sync {
    fn profile_collected(
        &self,
        eval_kind: StarlarkEvalKind,
        profile_data: &Arc<StarlarkProfileDataAndStats>,
    );
}

pub trait HasProfileEventListener {
    fn get_profile_event_listener(&self) -> Option<&Arc<dyn ProfileEventListener>>;
}

impl HasProfileEventListener for DiceComputations<'_> {
    fn get_profile_event_listener(&self) -> Option<&Arc<dyn ProfileEventListener>> {
        self.per_transaction_data()
            .data
            .get::<Arc<dyn ProfileEventListener>>()
            .ok()
    }
}

pub struct SetProfileEventListener;

impl SetProfileEventListener {
    pub fn set(data: &mut UserComputationData, listener: Arc<dyn ProfileEventListener>) {
        data.data.set(listener)
    }
}

/// A token that simply indicates that profiling has been reported. Used with
/// BuckStarlarkModule::with_profiling to ensure that profiling is reported.
pub struct ProfilingReportedToken(());

/// A simple wrapper around a starlark Module that allows us to ensure that profiling is reported.
pub struct BuckStarlarkModule<'v>(Module<'v>);

impl<'v> std::ops::Deref for BuckStarlarkModule<'v> {
    type Target = Module<'v>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl BuckStarlarkModule<'_> {
    /// This function allows us to ensure that profiling is reported (in the successful path) of any starlark evaluation.
    pub fn with_profiling<R, E>(
        func: impl for<'v> FnOnce(BuckStarlarkModule<'v>) -> Result<(ProfilingReportedToken, R), E>,
    ) -> Result<R, E> {
        // This is `BuckStarlarkModule`
        // ast-grep-ignore: rust/buck2-no-starlark-module
        match Module::with_temp_heap(|m| func(BuckStarlarkModule(m))) {
            Ok((ProfilingReportedToken(..), res)) => Ok(res),
            Err(e) => Err(e),
        }
    }

    /// This function allows us to ensure that profiling is reported (in the successful path) of any starlark evaluation.
    pub async fn with_profiling_async<F, R>(func: F) -> buck2_error::Result<R>
    where
        F: for<'v> AsyncFnOnce(
            BuckStarlarkModule<'v>,
        ) -> buck2_error::Result<(ProfilingReportedToken, R)>,
    {
        // This is `BuckStarlarkModule`
        // ast-grep-ignore: rust/buck2-no-starlark-module
        match Module::with_temp_heap_async(async |m| func(BuckStarlarkModule(m)).await).await {
            Ok((ProfilingReportedToken(..), res)) => Ok(res),
            Err(e) => Err(e),
        }
    }
}
