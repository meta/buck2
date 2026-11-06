/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::RefCell;
use std::cell::RefMut;
use std::convert::Infallible;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;
use std::time::Instant;

use allocative::Allocative;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::provider::label::ProvidersName;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::conversion::from_any_with_tag;
use buck2_error::internal_error;
use buck2_execute::digest_config::DigestConfig;
use buck2_hash::BuckMutInsertionSet;
use buck2_interpreter::late_binding_ty::AnalysisContextReprLate;
use buck2_interpreter::types::configured_providers_label::StarlarkConfiguredProvidersLabel;
use buck2_settings::dice::HasBuckSettings;
use buck2_util::late_binding::LateBinding;
use derive_more::Display;
use dice::DiceComputations;
use futures::FutureExt;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::typing::Ty;
use starlark::values::AllocValue;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::none::NoneOr;
use starlark::values::starlark_value;
use starlark::values::structs::StructRef;
use starlark::values::type_repr::StarlarkTypeRepr;

use crate::analysis::AnalysisSplitInstants;
use crate::analysis::anon_promises_dyn::RunAnonPromisesAccessor;
use crate::analysis::registry::AnalysisRegistry;
use crate::deferred::calculation::GET_PROMISED_ARTIFACT;
use crate::interpreter::rule_defs::plugins::AnalysisPlugins;

/// Functions to allow users to interact with the Actions registry.
///
/// Accessed via `ctx.actions.<function>`
#[derive(ProvidesStaticType, Debug, Display, Trace, NoSerialize, Allocative)]
#[display("<ctx.actions>")]
pub struct AnalysisActions<'v> {
    /// Use a RefCell/Option so when we are done with it, without obtaining exclusive access,
    /// we can take the internal state without having to clone it.
    pub state: RefCell<Option<AnalysisRegistry<'v>>>,
    /// Copies from the ctx, so we can capture them for `dynamic`.
    pub attributes: Option<ValueOf<'v, StructRef<'v>>>,
    pub plugins: Option<ValueTyped<'v, AnalysisPlugins<'v>>>,
    /// Digest configuration to use when interpreting digests passed in analysis.
    pub digest_config: DigestConfig,
}

/// What one `AnalysisActions::run_promises` call did.
pub struct PromisesRun {
    /// Timestamps around promise resolution.
    pub split_instants: Option<AnalysisSplitInstants>,
    /// The anon targets those promises requested, deduplicated, in first-request
    /// order; empty when recording is off.
    pub requested_anon_targets: Vec<BaseDeferredKey>,
}

impl PromisesRun {
    /// Whether at least one promise batch was resolved.
    pub fn resolved_any(&self) -> bool {
        self.split_instants.is_some()
    }
}

impl<'v> AnalysisActions<'v> {
    pub fn state(&self) -> buck2_error::Result<RefMut<'_, AnalysisRegistry<'v>>> {
        let state = self
            .state
            .try_borrow_mut()
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0))
            .buck_error_context("AnalysisActions.state is already borrowed")?;
        RefMut::filter_map(state, |x| x.as_mut())
            .ok()
            .internal_error("state to be present during execution")
    }

    /// Resolves every pending promise (looping, since `promise.map` can enqueue more), then
    /// asserts short paths and resolves consumer promise artifacts.
    pub async fn run_promises<'a, 'e: 'a>(
        &self,
        accessor: &mut dyn RunAnonPromisesAccessor<'v, 'a, 'e>,
    ) -> buck2_error::Result<PromisesRun>
    where
        'v: 'a,
    {
        let pre_promises = Instant::now();
        let mut resolved_any = false;
        let mut record_requested = None;
        let mut requested_anon_targets: BuckMutInsertionSet<BaseDeferredKey> =
            BuckMutInsertionSet::new();
        loop {
            // Bind first so the state borrow ends before the promises run.
            let promises = self.state()?.take_promises();
            let Some(promises) = promises else { break };
            resolved_any = true;
            let record = match record_requested {
                Some(record) => record,
                None => {
                    let record = accessor
                        .with_dice(|dice| {
                            async move {
                                buck2_error::Ok(
                                    dice.global_data()
                                        .get_buck_settings()
                                        .analysis
                                        .record_requested_anon_targets(),
                                )
                            }
                            .boxed_local()
                        })
                        .await?;
                    record_requested = Some(record);
                    record
                }
            };
            requested_anon_targets.extend(promises.run_promises(accessor, record).await?);
        }

        accessor
            .with_dice(|dice| self.assert_short_paths_and_resolve(dice).boxed_local())
            .await?;
        let post_promises = Instant::now();

        Ok(PromisesRun {
            split_instants: resolved_any.then_some(AnalysisSplitInstants {
                pre_promises,
                post_promises,
            }),
            requested_anon_targets: requested_anon_targets.into_iter().collect(),
        })
    }

    // Called after `run_promises()` to assert short paths and resolve consumer's promise artifacts.
    pub async fn assert_short_paths_and_resolve(
        &self,
        dice: &mut DiceComputations<'_>,
    ) -> buck2_error::Result<()> {
        let (short_path_assertions, content_based_path_assertions, consumer_analysis_artifacts) = {
            let state = self.state()?;
            (
                state.short_path_assertions.clone(),
                state.content_based_path_assertions.clone(),
                state.consumer_analysis_artifacts(),
            )
        };

        for consumer_artifact in consumer_analysis_artifacts {
            let artifact = (GET_PROMISED_ARTIFACT.get()?)(&consumer_artifact, dice).await?;
            let id = consumer_artifact.id();
            let short_path = short_path_assertions.get(id).cloned();
            consumer_artifact.resolve(
                artifact.clone(),
                &short_path,
                content_based_path_assertions.contains(id),
            )?;
        }
        Ok(())
    }
}

starlark::methods_static!(
    ANALYSIS_ACTIONS_METHODS = |builder| {
        (ANALYSIS_ACTIONS_METHODS_ACTIONS.get().unwrap())(builder);
        (ANALYSIS_ACTIONS_METHODS_ANON_TARGET.get().unwrap())(builder);
    }
);

#[starlark_value(type = "AnalysisActions", StarlarkTypeRepr, UnpackValue)]
impl<'v> StarlarkValue<'v> for AnalysisActions<'v> {
    fn get_methods() -> Option<&'static Methods> {
        Some(ANALYSIS_ACTIONS_METHODS.methods())
    }
}

impl<'v> AllocValue<'v> for AnalysisActions<'v> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_complex_no_freeze(self)
    }
}

#[allow(dead_code)] // field `0` is never read
struct RefAnalysisAction<'v>(&'v AnalysisActions<'v>);

impl<'v> StarlarkTypeRepr for RefAnalysisAction<'v> {
    type Canonical = <AnalysisActions<'v> as StarlarkTypeRepr>::Canonical;

    fn starlark_type_repr() -> Ty {
        AnalysisActions::starlark_type_repr()
    }
}

impl<'v> UnpackValue<'v> for RefAnalysisAction<'v> {
    type Error = Infallible;

    fn unpack_value_impl(value: Value<'v>) -> Result<Option<Self>, Self::Error> {
        let Some(analysis_actions) = value.downcast_ref::<AnalysisActions>() else {
            return Ok(None);
        };
        Ok(Some(RefAnalysisAction(analysis_actions)))
    }
}

#[derive(ProvidesStaticType, Debug, Trace, NoSerialize, Allocative)]
pub struct AnalysisContext<'v> {
    attrs: Option<ValueOf<'v, StructRef<'v>>>,
    pub actions: ValueTyped<'v, AnalysisActions<'v>>,
    /// Only `None` when running a `dynamic_output` action from Bxl.
    label: Option<ValueTyped<'v, StarlarkConfiguredProvidersLabel>>,
    plugins: Option<ValueTyped<'v, AnalysisPlugins<'v>>>,
}

impl<'v> Display for AnalysisContext<'v> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "<ctx")?;
        if let Some(label) = &self.label {
            write!(f, " label=\"{label}\"")?;
        }
        write!(f, " attrs=...")?;
        write!(f, " actions=...")?;
        write!(f, ">")?;
        Ok(())
    }
}

impl<'v> AnalysisContext<'v> {
    /// The context that is provided to users' UDR implementation functions. Comprised of things like attribute values, actions, etc
    fn new(
        heap: Heap<'v>,
        attrs: Option<ValueOf<'v, StructRef<'v>>>,
        label: Option<ValueTyped<'v, StarlarkConfiguredProvidersLabel>>,
        plugins: Option<ValueTyped<'v, AnalysisPlugins<'v>>>,
        registry: AnalysisRegistry<'v>,
        digest_config: DigestConfig,
    ) -> Self {
        Self {
            attrs,
            actions: heap.alloc_typed(AnalysisActions {
                state: RefCell::new(Some(registry)),
                attributes: attrs,
                plugins,
                digest_config,
            }),
            label,
            plugins,
        }
    }

    pub fn prepare(
        heap: Heap<'v>,
        attrs: Option<ValueOf<'v, StructRef<'v>>>,
        label: Option<ConfiguredTargetLabel>,
        plugins: Option<ValueTyped<'v, AnalysisPlugins<'v>>>,
        registry: AnalysisRegistry<'v>,
        digest_config: DigestConfig,
    ) -> ValueTyped<'v, AnalysisContext<'v>> {
        let label = label.map(|label| {
            heap.alloc_typed(StarlarkConfiguredProvidersLabel::new(
                ConfiguredProvidersLabel::new(label, ProvidersName::Default),
            ))
        });

        let analysis_context = Self::new(heap, attrs, label, plugins, registry, digest_config);
        heap.alloc_typed(analysis_context)
    }

    pub fn assert_no_promises(&self) -> buck2_error::Result<()> {
        self.actions.state()?.assert_no_promises()
    }

    /// Must take an `AnalysisContext` which has never had `take_state` called on it before.
    pub fn take_state(&self) -> AnalysisRegistry<'v> {
        self.actions
            .state
            .borrow_mut()
            .take()
            .expect("nothing to have stolen state yet")
    }
}

starlark::methods_static!(ANALYSIS_CONTEXT_METHODS = analysis_context_methods);

#[starlark_value(type = "AnalysisContext")]
impl<'v> StarlarkValue<'v> for AnalysisContext<'v> {
    fn get_methods() -> Option<&'static Methods> {
        Some(ANALYSIS_CONTEXT_METHODS.methods())
    }
}

impl<'v> AllocValue<'v> for AnalysisContext<'v> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_complex_no_freeze(self)
    }
}

struct RefAnalysisContext<'v>(&'v AnalysisContext<'v>);

impl<'v> StarlarkTypeRepr for RefAnalysisContext<'v> {
    type Canonical = <AnalysisContext<'v> as StarlarkTypeRepr>::Canonical;

    fn starlark_type_repr() -> Ty {
        AnalysisContext::starlark_type_repr()
    }
}

impl<'v> UnpackValue<'v> for RefAnalysisContext<'v> {
    type Error = Infallible;

    fn unpack_value_impl(value: Value<'v>) -> Result<Option<Self>, Self::Error> {
        let Some(analysis_context) = value.downcast_ref::<AnalysisContext>() else {
            return Ok(None);
        };
        Ok(Some(RefAnalysisContext(analysis_context)))
    }
}

/// The type used for defining rules, usually bound as `ctx`.
/// Usually the sole argument to the `impl` argument of the `rule` function.
///
/// ```python
/// def _impl_my_rule(ctx: AnalysisContext) -> ["provider"]:
///     return [DefaultInfo()]
/// my_rule = rule(impl = _impl_my_rule, attrs = {})
/// ```
#[starlark_module]
fn analysis_context_methods(builder: &mut MethodsBuilder) {
    /// Returns the attributes of the target as a Starlark struct with a field for each attribute, which varies per rule.
    /// As an example, given a rule with the `attrs` argument of `{"foo": attrs.string()}`, this field will be
    /// a `struct` containing a field `foo` of type string.
    #[starlark(attribute)]
    fn attrs<'v>(this: RefAnalysisContext<'v>) -> starlark::Result<ValueOf<'v, StructRef<'v>>> {
        Ok(this.0.attrs.ok_or_else(|| {
            internal_error!("`attrs` is not available for `dynamic_output` or BXL")
        })?)
    }

    /// Returns an `actions` value containing functions to define actual actions that are run.
    /// See the `actions` type for the operations that are available.
    #[starlark(attribute)]
    fn actions<'v>(
        this: RefAnalysisContext<'v>,
    ) -> starlark::Result<ValueTyped<'v, AnalysisActions<'v>>> {
        Ok(this.0.actions)
    }

    /// Returns a `label` representing the target, or `None` if being invoked from a
    /// `dynamic_output` in Bxl.
    #[starlark(attribute)]
    fn label<'v>(
        this: RefAnalysisContext<'v>,
    ) -> starlark::Result<NoneOr<ValueTyped<'v, StarlarkConfiguredProvidersLabel>>> {
        Ok(NoneOr::from_option(this.0.label))
    }

    /// An opaque value that can be indexed with a plugin kind to get a list of the available plugin
    /// deps of that kind. The rule must set an appropriate value on `uses_plugins` in its
    /// declaration.
    #[starlark(attribute)]
    fn plugins<'v>(
        this: RefAnalysisContext<'v>,
    ) -> starlark::Result<ValueTyped<'v, AnalysisPlugins<'v>>> {
        Ok(this.0.plugins.ok_or_else(|| {
            internal_error!("`plugins` is not available for `dynamic_output` or BXL")
        })?)
    }
}

#[starlark_module]
#[starlark_types(AnalysisContext<'_> as AnalysisContext, AnalysisActions<'_> as AnalysisActions)]
pub(crate) fn register_analysis_context(builder: &mut GlobalsBuilder) {}

pub static ANALYSIS_ACTIONS_METHODS_ACTIONS: LateBinding<fn(&mut MethodsBuilder)> =
    LateBinding::new("ANALYSIS_ACTIONS_METHODS_ACTIONS");
pub static ANALYSIS_ACTIONS_METHODS_ANON_TARGET: LateBinding<fn(&mut MethodsBuilder)> =
    LateBinding::new("ANALYSIS_ACTIONS_METHODS_ANON_TARGET");

pub(crate) fn init_analysis_context_ty() {
    AnalysisContextReprLate::init(AnalysisContext::starlark_type_repr());
}
