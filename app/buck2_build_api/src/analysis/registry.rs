/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::OnceCell;
use std::fmt::Debug;
use std::marker::PhantomData;
use std::sync::Arc;

use allocative::Allocative;
use buck2_artifact::actions::key::ActionIndex;
use buck2_artifact::actions::key::ActionKey;
use buck2_artifact::artifact::artifact_type::DeclaredArtifact;
use buck2_artifact::artifact::artifact_type::OutputArtifact;
use buck2_artifact::artifact::build_artifact::BuildArtifact;
use buck2_artifact::artifact::output_type::OutputType;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_core::deferred::key::DeferredHolderKey;
use buck2_core::execution_types::execution::ExecutionPlatformResolution;
use buck2_core::execution_types::execution_group::ExecutionGroup;
use buck2_core::fs::buck_out_path::BuckOutPathKind;
use buck2_error::BuckErrorOptionContext;
use buck2_error::internal_error;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutInsertionSet;
use buck2_hash::BuckMutMap;
use buck2_hash::BuckMutSet;
use buck2_interpreter::testing::Buck2TestHeapName;
use derivative::Derivative;
use dupe::Dupe;
use itertools::Itertools;
use mini_vec::MiniBoxSlice;
use starlark::StarlarkPagable;
use starlark::StarlarkPagablePanic;
use starlark::any::ProvidesStaticType;
use starlark::codemap::FileSpan;
use starlark::environment::FrozenModule;
use starlark::environment::Module;
use starlark::eval::Evaluator;
use starlark::pagable::StarlarkDeserializeContext;
use starlark::pagable::StarlarkSerialize;
use starlark::pagable::StarlarkSerializeContext;
use starlark::values::DeferredOwnedFrozen;
use starlark::values::DynStarlark;
use starlark::values::Freeze;
use starlark::values::FreezeResult;
use starlark::values::Freezer;
use starlark::values::FrozenValueTyped;
use starlark::values::Heap;
use starlark::values::OwnedFrozen;
use starlark::values::OwnedFrozenRef;
use starlark::values::Trace;
use starlark::values::Tracer;
use starlark::values::Value;
use starlark::values::ValueTyped;
use starlark::values::any_complex::StarlarkAnyComplex;
use starlark::values::typing::StarlarkCallable;

use crate::actions::ActionName;
use crate::actions::RegisteredAction;
use crate::actions::UnregisteredAction;
use crate::actions::registry::ActionsRegistry;
use crate::actions::registry::RecordedActions;
use crate::analysis::anon_promises_dyn::AnonPromisesDyn;
use crate::analysis::anon_targets_registry::ANON_TARGET_REGISTRY_NEW;
use crate::analysis::anon_targets_registry::AnonTargetsRegistryDyn;
use crate::analysis::extra_v::AnalysisExtraValue;
use crate::analysis::extra_v::FrozenAnalysisExtraValue;
use crate::analysis::extra_v::OwnedFrozenAnalysisValueStorage;
use crate::artifact_groups::deferred::TransitiveSetIndex;
use crate::artifact_groups::deferred::TransitiveSetKey;
use crate::artifact_groups::promise::PromiseArtifact;
use crate::artifact_groups::promise::PromiseArtifactId;
use crate::deferred::calculation::ActionLookup;
use crate::dynamic::storage::DYNAMIC_LAMBDA_PARAMS_STORAGES;
use crate::dynamic::storage::DynamicLambdaParamsStorage;
use crate::dynamic::storage::FrozenDynamicLambdaParamsStorageBox;
use crate::interpreter::rule_defs::artifact::associated::AssociatedArtifacts;
use crate::interpreter::rule_defs::artifact::output_artifact_like::OutputArtifactArg;
use crate::interpreter::rule_defs::artifact::starlark_declared_artifact::StarlarkDeclaredArtifact;
use crate::interpreter::rule_defs::provider::collection::FrozenProviderCollectionValueRef;
use crate::interpreter::rule_defs::provider::collection::ProviderCollection;
use crate::interpreter::rule_defs::transitive_set::FrozenTransitiveSetDefinition;
use crate::interpreter::rule_defs::transitive_set::TransitiveSet;

#[derive(Derivative, Trace, Allocative)]
#[derivative(Debug)]
pub struct AnalysisRegistry<'v> {
    #[derivative(Debug = "ignore")]
    pub actions: ActionsRegistry<'v>,
    pub anon_targets: Box<DynStarlark<'v, dyn AnonTargetsRegistryDyn<'v>>>,
    pub analysis_value_storage: AnalysisValueStorage<'v>,
    pub short_path_assertions: BuckMutMap<PromiseArtifactId, ForwardRelativePathBuf>,
    pub content_based_path_assertions: BuckMutSet<PromiseArtifactId>,
}

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Input)]
enum DeclaredArtifactError {
    #[error("Can't declare an artifact with an empty filename component")]
    DeclaredEmptyFileName,
    #[error(
        "Artifact `{0}` was declared with `has_content_based_path = {1}`, but is now being used with `has_content_based_path = {2}`"
    )]
    AlreadyDeclaredWithDifferentContentBasedPathHashing(String, bool, bool),
}

impl<'v> AnalysisRegistry<'v> {
    pub fn new_from_owner(
        owner: BaseDeferredKey,
        execution_platform: ExecutionPlatformResolution,
    ) -> buck2_error::Result<AnalysisRegistry<'v>> {
        Self::new_from_owner_and_deferred(execution_platform, DeferredHolderKey::Base(owner))
    }

    pub fn new_from_owner_and_deferred(
        execution_platform: ExecutionPlatformResolution,
        self_key: DeferredHolderKey,
    ) -> buck2_error::Result<Self> {
        Ok(AnalysisRegistry {
            actions: ActionsRegistry::new(self_key.dupe(), execution_platform.dupe()),
            anon_targets: (ANON_TARGET_REGISTRY_NEW.get()?)(PhantomData, execution_platform),
            analysis_value_storage: AnalysisValueStorage::new(self_key),
            short_path_assertions: BuckMutMap::default(),
            content_based_path_assertions: BuckMutSet::default(),
        })
    }

    /// Reserves a path in an output directory. Doesn't declare artifact,
    /// but checks that there is no previously declared artifact with a path
    /// which is in conflict with claimed `path`.
    pub fn claim_output_path(
        &mut self,
        eval: &Evaluator<'_, '_, '_>,
        path: &ForwardRelativePath,
    ) -> buck2_error::Result<()> {
        let declaration_location = eval.call_stack_top_location();
        self.actions.claim_output_path(path, declaration_location)
    }

    pub fn declare_dynamic_output(
        &mut self,
        artifact: &BuildArtifact,
        heap: Heap<'v>,
    ) -> buck2_error::Result<DeclaredArtifact<'v>> {
        self.actions.declare_dynamic_output(artifact, heap)
    }

    pub fn declare_output(
        &mut self,
        prefix: Option<&str>,
        filename: &str,
        output_type: OutputType,
        declaration_location: Option<FileSpan>,
        path_resolution_method: BuckOutPathKind,
        heap: Heap<'v>,
    ) -> buck2_error::Result<DeclaredArtifact<'v>> {
        // We don't allow declaring `` as an output, although technically there's nothing preventing
        // that
        if filename.is_empty() {
            return Err(DeclaredArtifactError::DeclaredEmptyFileName.into());
        }

        let path = ForwardRelativePath::new(filename)?.to_owned();
        let prefix = match prefix {
            None => None,
            Some(x) => Some(ForwardRelativePath::new(x)?.to_owned()),
        };
        self.actions.declare_artifact(
            prefix,
            path,
            output_type,
            declaration_location,
            path_resolution_method,
            heap,
        )
    }

    /// Takes a string or artifact/output artifact and converts it into an output artifact
    ///
    /// This is handy for functions like `ctx.actions.write` where it's nice to just let
    /// the user give us a string if they want as the output name.
    ///
    /// This function can declare new artifacts depending on the input.
    /// If there is no error, it returns a wrapper around the artifact (ArtifactDeclaration) and the corresponding OutputArtifact
    ///
    /// The valid types for `value` and subsequent actions are as follows:
    ///  - `str`: A new file is declared with this name.
    ///  - `StarlarkOutputArtifact`: The original artifact is returned
    ///  - `StarlarkArtifact`/`StarlarkDeclaredArtifact`: If the artifact is already bound, an error is raised. Otherwise we proceed with the original artifact.
    pub fn get_or_declare_output(
        &mut self,
        eval: &Evaluator<'v, '_, '_>,
        value: OutputArtifactArg<'v>,
        output_type: OutputType,
        has_content_based_path: Option<bool>,
    ) -> buck2_error::Result<(ArtifactDeclaration<'v>, OutputArtifact<'v>)> {
        let declaration_location = eval.call_stack_top_location();
        let heap = eval.heap();
        let declared_artifact = match value {
            OutputArtifactArg::Str(path) => {
                let artifact = self.declare_output(
                    None,
                    path,
                    output_type,
                    declaration_location.dupe(),
                    match has_content_based_path {
                        Some(true) => BuckOutPathKind::ContentHash,
                        Some(false) => BuckOutPathKind::Configuration,
                        None => BuckOutPathKind::ContentHash,
                    },
                    heap,
                )?;
                heap.alloc_typed(StarlarkDeclaredArtifact::new(
                    declaration_location,
                    artifact,
                    AssociatedArtifacts::new(),
                ))
            }
            OutputArtifactArg::OutputArtifact(output) => output.inner(),
            OutputArtifactArg::DeclaredArtifact(artifact) => artifact,
            OutputArtifactArg::WrongArtifact(artifact) => {
                return Err(artifact.as_input_artifact_like().as_output_error());
            }
        };

        let output = declared_artifact.output_artifact();
        output.ensure_output_type(output_type)?;

        if let Some(has_content_based_path) = has_content_based_path {
            if has_content_based_path != output.has_content_based_path() {
                return Err(
                    DeclaredArtifactError::AlreadyDeclaredWithDifferentContentBasedPathHashing(
                        format!("{output}"),
                        output.has_content_based_path(),
                        has_content_based_path,
                    )
                    .into(),
                );
            }
        }

        Ok((
            ArtifactDeclaration {
                artifact: declared_artifact,
                heap,
            },
            output,
        ))
    }

    pub fn register_action<A: UnregisteredAction + 'static>(
        &mut self,
        outputs: BuckMutInsertionSet<OutputArtifact>,
        action: A,
        associated_value: Option<Value<'v>>,
        error_handler: Option<StarlarkCallable<'v>>,
    ) -> buck2_error::Result<()> {
        let id = self
            .actions
            .register(&self.analysis_value_storage.self_key, outputs, action)?;
        self.analysis_value_storage
            .set_action_data(id, (associated_value, error_handler))?;
        Ok(())
    }

    /// Like `register_action`, running the action on the execution platform of
    /// `execution_group`. `action_name` names the action in registration errors.
    pub fn register_action_in_group<A: UnregisteredAction + 'static>(
        &mut self,
        outputs: BuckMutInsertionSet<OutputArtifact>,
        action: A,
        associated_value: Option<Value<'v>>,
        error_handler: Option<StarlarkCallable<'v>>,
        execution_group: ExecutionGroup,
        action_name: impl FnOnce() -> ActionName,
    ) -> buck2_error::Result<()> {
        let id = self.actions.register_in_group(
            &self.analysis_value_storage.self_key,
            outputs,
            action,
            execution_group,
            action_name,
        )?;
        self.analysis_value_storage
            .set_action_data(id, (associated_value, error_handler))?;
        Ok(())
    }

    pub fn create_transitive_set(
        &mut self,
        definition: FrozenValueTyped<'v, FrozenTransitiveSetDefinition<'v>>,
        value: Option<Value<'v>>,
        children: Option<Value<'v>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<ValueTyped<'v, TransitiveSet<'v>>> {
        Ok(self
            .analysis_value_storage
            .register_transitive_set(move |key| {
                let set =
                    TransitiveSet::new_from_values(key.dupe(), definition, value, children, eval)?;
                Ok(eval.heap().alloc_typed(set))
            })?)
    }

    pub(crate) fn take_promises(&mut self) -> Option<Box<dyn AnonPromisesDyn<'v>>> {
        self.anon_targets.take_promises()
    }

    pub fn consumer_analysis_artifacts(&self) -> Vec<PromiseArtifact> {
        self.anon_targets.consumer_analysis_artifacts()
    }

    pub fn record_short_path_assertion(
        &mut self,
        short_path: ForwardRelativePathBuf,
        promise_artifact_id: PromiseArtifactId,
    ) {
        self.short_path_assertions
            .insert(promise_artifact_id, short_path);
    }

    pub fn record_has_content_based_path_assertion(
        &mut self,
        promise_artifact_id: PromiseArtifactId,
    ) {
        self.content_based_path_assertions
            .insert(promise_artifact_id);
    }

    pub fn assert_no_promises(&self) -> buck2_error::Result<()> {
        self.anon_targets.assert_no_promises()
    }

    pub fn num_declared_actions(&self) -> u64 {
        self.actions.actions_len() as u64
    }

    pub fn num_declared_artifacts(&self) -> u64 {
        self.actions.artifacts_len() as u64
    }

    /// You MUST pass the same module to both the first function and the second one.
    /// It requires both to get the lifetimes to line up.
    pub fn finalize(
        self,
        env: &Module<'v>,
    ) -> buck2_error::Result<
        impl FnOnce(&FrozenModule) -> buck2_error::Result<RecordedAnalysisValues> + use<>,
    > {
        let AnalysisRegistry {
            actions,
            anon_targets: _,
            analysis_value_storage,
            short_path_assertions: _,
            content_based_path_assertions: _,
        } = self;

        let finalize_actions = actions.finalize()?;

        let self_key = analysis_value_storage.self_key.dupe();
        analysis_value_storage.write_to_module(env)?;
        Ok(move |frozen_env: &FrozenModule| {
            let analysis_value_fetcher = AnalysisValueFetcher {
                self_key,
                frozen_module: Some(frozen_env.dupe()),
            };
            let actions = (finalize_actions)(&analysis_value_fetcher)?;
            let recorded_values = analysis_value_fetcher.get_recorded_values(actions)?;

            Ok(recorded_values)
        })
    }

    pub fn execution_platform(&self) -> &ExecutionPlatformResolution {
        self.actions.execution_platform()
    }
}

pub struct ArtifactDeclaration<'v> {
    artifact: ValueTyped<'v, StarlarkDeclaredArtifact<'v>>,
    heap: Heap<'v>,
}

impl<'v> ArtifactDeclaration<'v> {
    pub fn into_declared_artifact(
        self,
        extra_associated_artifacts: AssociatedArtifacts,
    ) -> ValueTyped<'v, StarlarkDeclaredArtifact<'v>> {
        self.heap.alloc_typed(
            self.artifact
                .with_extended_associated_artifacts(extra_associated_artifacts),
        )
    }
}

/// Store `Value<'v>` values for actions registered in an implementation function
///
/// These values eventually are written into the mutable `Module`, and a wrapper is
/// made available to get them back out as `OwnedFrozen`s after that `Module` is frozen.
///
/// Note that this object has internal mutation and is only expected to live for the duration
/// of impl function execution.
///
/// At the end of impl function execution, `write_to_module` should be called
/// to write this object to `Module` extra value to get the values frozen.
#[derive(Debug, Allocative, ProvidesStaticType, StarlarkPagablePanic)]
pub struct AnalysisValueStorage<'v> {
    pub self_key: DeferredHolderKey,
    action_data:
        BuckMutInsertionMap<ActionIndex, (Option<Value<'v>>, Option<StarlarkCallable<'v>>)>,
    transitive_sets: Vec<ValueTyped<'v, TransitiveSet<'v>>>,
    pub lambda_params: Box<DynStarlark<'v, dyn DynamicLambdaParamsStorage<'v>>>,
    result_value: OnceCell<ValueTyped<'v, ProviderCollection<'v>>>,
}

#[derive(Debug, Allocative, ProvidesStaticType, StarlarkPagable)]
pub struct FrozenAnalysisValueStorage<'fv> {
    #[starlark_pagable(pagable)]
    pub self_key: DeferredHolderKey,
    action_data:
        BuckMutInsertionMap<ActionIndex, (Option<Value<'fv>>, Option<StarlarkCallable<'fv>>)>,
    transitive_sets: MiniBoxSlice<ValueTyped<'fv, TransitiveSet<'fv>>>,
    #[starlark_pagable(
        serialize_with = "serialize_lambda_params",
        deserialize_with = "deserialize_lambda_params"
    )]
    pub lambda_params: FrozenDynamicLambdaParamsStorageBox<'fv>,
    result_value: Option<ValueTyped<'fv, ProviderCollection<'fv>>>,
}

fn serialize_lambda_params(
    field: &FrozenDynamicLambdaParamsStorageBox<'_>,
    ctx: &mut dyn StarlarkSerializeContext,
) -> starlark::Result<()> {
    StarlarkSerialize::starlark_serialize(&***field, ctx)
}

fn deserialize_lambda_params<'fv>(
    ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
) -> starlark::Result<FrozenDynamicLambdaParamsStorageBox<'fv>> {
    DYNAMIC_LAMBDA_PARAMS_STORAGES
        .get()?
        .deserialize_frozen_dynamic_lambda_params_storage(ctx)
}

unsafe impl<'v> Trace<'v> for AnalysisValueStorage<'v> {
    fn trace(&mut self, tracer: &Tracer<'v>) {
        let AnalysisValueStorage {
            action_data,
            transitive_sets,
            lambda_params,
            self_key,
            result_value,
        } = self;
        for (k, v) in action_data.iter_mut() {
            tracer.trace_static(k);
            v.trace(tracer);
        }
        for v in transitive_sets.iter_mut() {
            v.trace(tracer);
        }
        lambda_params.trace(tracer);
        tracer.trace_static(self_key);
        result_value.trace(tracer);
    }
}

impl<'v> Freeze<'v> for AnalysisValueStorage<'v> {
    type Frozen<'fv> = FrozenAnalysisValueStorage<'fv>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        let AnalysisValueStorage {
            self_key,
            action_data,
            transitive_sets,
            lambda_params,
            result_value,
        } = self;

        // N.B. collect::<Result<_>> sets the lower bound to zero,
        // which can cause over-allocations in frozen containers.
        let mut frozen_action_data = BuckMutInsertionMap::with_capacity(action_data.len());
        for (k, (data, error_handler)) in action_data {
            frozen_action_data.insert(
                k,
                (
                    Freeze::freeze(data, freezer)?,
                    Freeze::freeze(error_handler, freezer)?,
                ),
            );
        }
        let mut frozen_transitive_sets = Vec::with_capacity(transitive_sets.len());
        for v in transitive_sets {
            frozen_transitive_sets.push(v.freeze(freezer)?);
        }
        let result_value = result_value
            .into_inner()
            .map(|v| v.freeze(freezer))
            .transpose()?;
        Ok(FrozenAnalysisValueStorage {
            self_key,
            action_data: frozen_action_data,
            transitive_sets: frozen_transitive_sets
                .into_iter()
                .collect::<MiniBoxSlice<_>>(),
            lambda_params: lambda_params.freeze(freezer)?,
            result_value,
        })
    }
}

/// Simple fetcher that fetches the values written in `AnalysisValueStorage::write_to_module`
///
/// These values are pulled from the `FrozenModule` that results from `env.freeze()`.
/// This is used by the action registry to make an `OwnedFrozen` available to
/// Actions' register function.
pub struct AnalysisValueFetcher {
    self_key: DeferredHolderKey,
    frozen_module: Option<FrozenModule>,
}

impl AnalysisValueFetcher {
    pub fn testing_new(self_key: DeferredHolderKey) -> Self {
        AnalysisValueFetcher {
            self_key,
            frozen_module: None,
        }
    }
}

impl<'v> AnalysisValueStorage<'v> {
    fn new(self_key: DeferredHolderKey) -> Self {
        Self {
            self_key: self_key.dupe(),
            action_data: BuckMutInsertionMap::new(),
            transitive_sets: Vec::new(),
            lambda_params: DYNAMIC_LAMBDA_PARAMS_STORAGES
                .get()
                .unwrap()
                .new_dynamic_lambda_params_storage(self_key),
            result_value: OnceCell::new(),
        }
    }

    /// Write self to `module` extra value.
    fn write_to_module(self, module: &Module<'v>) -> buck2_error::Result<()> {
        let extra_v = AnalysisExtraValue::get_or_init(module)?;
        let res = extra_v.analysis_value_storage.set(
            module
                .heap()
                .alloc_typed(StarlarkAnyComplex { value: self }),
        );
        if res.is_err() {
            return Err(internal_error!("analysis_value_storage is already set"));
        }
        Ok(())
    }

    pub(crate) fn register_transitive_set<
        F: FnOnce(TransitiveSetKey) -> buck2_error::Result<ValueTyped<'v, TransitiveSet<'v>>>,
    >(
        &mut self,
        func: F,
    ) -> buck2_error::Result<ValueTyped<'v, TransitiveSet<'v>>> {
        let key = TransitiveSetKey::new(
            self.self_key.dupe(),
            TransitiveSetIndex(self.transitive_sets.len().try_into()?),
        );
        let set = func(key.dupe())?;
        self.transitive_sets.push(set.dupe());
        Ok(set)
    }

    fn set_action_data(
        &mut self,
        id: ActionKey,
        action_data: (Option<Value<'v>>, Option<StarlarkCallable<'v>>),
    ) -> buck2_error::Result<()> {
        if &self.self_key != id.holder_key() {
            return Err(internal_error!(
                "Wrong action owner: expecting `{}`, got `{}`",
                self.self_key,
                id
            ));
        }
        self.action_data.insert(id.action_index(), action_data);
        Ok(())
    }

    pub fn set_result_value(
        &self,
        providers: ValueTyped<'v, ProviderCollection<'v>>,
    ) -> buck2_error::Result<()> {
        if self.result_value.set(providers).is_err() {
            return Err(internal_error!("result_value is already set"));
        }
        Ok(())
    }
}

impl AnalysisValueFetcher {
    /// Get the action's starlark data and error handler for an `ActionKey`, if present
    pub fn get_action_data(
        &self,
        id: &ActionKey,
    ) -> buck2_error::Result<(
        Option<OwnedFrozen<Value<'static>>>,
        Option<OwnedFrozen<Value<'static>>>,
    )> {
        let Some(module) = &self.frozen_module else {
            return Ok((None, None));
        };

        let storage = FrozenAnalysisExtraValue::analysis_value_storage(module)?;
        let storage = storage.as_ref();

        let self_key = &storage.value().as_ref().value.self_key;
        if id.holder_key() != self_key {
            return Err(internal_error!(
                "Wrong action owner: expecting `{self_key}`, got `{id}`"
            ));
        }

        // Looking the entries up inside brand-generic closures is what witnesses that they are
        // kept alive by `storage`'s heap.
        let index = id.action_index();
        let starlark_data =
            storage.maybe_map::<Value<'static>, _>(|v| v.as_ref().value.action_data.get(&index)?.0);
        let error_handler = storage.maybe_map::<Value<'static>, _>(|v| {
            Some(v.as_ref().value.action_data.get(&index)?.1?.0)
        });
        Ok((
            starlark_data.map(|v| v.to_owned()),
            error_handler.map(|v| v.to_owned()),
        ))
    }

    pub(crate) fn get_recorded_values(
        &self,
        actions: RecordedActions,
    ) -> buck2_error::Result<RecordedAnalysisValues> {
        let analysis_storage = match &self.frozen_module {
            None => None,
            Some(module) => Some(FrozenAnalysisExtraValue::analysis_value_storage(module)?),
        };

        Ok(RecordedAnalysisValues {
            self_key: self.self_key.dupe(),
            analysis_storage: analysis_storage.map(DeferredOwnedFrozen::new),
            actions,
        })
    }
}

/// The analysis values stored in DeferredHolder.
#[derive(Debug, Allocative, pagable::Pagable)]
pub struct RecordedAnalysisValues {
    self_key: DeferredHolderKey,
    analysis_storage: Option<
        DeferredOwnedFrozen<
            ValueTyped<'static, StarlarkAnyComplex<FrozenAnalysisValueStorage<'static>>>,
        >,
    >,
    actions: RecordedActions,
}

starlark::register_starlark_any_complex!(AnalysisValueStorage<'_>, frozen FrozenAnalysisValueStorage<'_>);

impl RecordedAnalysisValues {
    /// Creates a minimal RecordedAnalysisValues for testing action lookups only.
    /// This version doesn't require DYNAMIC_LAMBDA_PARAMS_STORAGES to be initialized.
    pub fn testing_new_actions_only(self_key: DeferredHolderKey, actions: RecordedActions) -> Self {
        Self {
            self_key,
            analysis_storage: None,
            actions,
        }
    }

    pub fn testing_new(
        self_key: DeferredHolderKey,
        transitive_sets: Vec<(
            TransitiveSetKey,
            OwnedFrozen<ValueTyped<'static, TransitiveSet<'static>>>,
        )>,
        actions: RecordedActions,
    ) -> Self {
        let analysis_storage: OwnedFrozenAnalysisValueStorage =
            OwnedFrozen::build(Buck2TestHeapName::frozen_heap_name(), |heap| {
                let alloced_tsets: Vec<_> = transitive_sets
                    .iter()
                    .sorted_by_key(|(key, _)| key.index().0)
                    .map(|(_key, tset)| tset.as_ref().add_to_frozen_heap(heap))
                    .collect();

                heap.alloc_simple_typed(StarlarkAnyComplex {
                    value: FrozenAnalysisValueStorage {
                        self_key: self_key.dupe(),
                        action_data: BuckMutInsertionMap::new(),
                        transitive_sets: alloced_tsets.into_iter().collect(),
                        lambda_params: DYNAMIC_LAMBDA_PARAMS_STORAGES
                            .get()
                            .unwrap()
                            .new_frozen_dynamic_lambda_params_storage(),
                        result_value: Some(ProviderCollection::testing_new_default(heap)),
                    },
                })
            });
        Self {
            self_key,
            analysis_storage: Some(DeferredOwnedFrozen::new(analysis_storage)),
            actions,
        }
    }

    pub(crate) fn lookup_transitive_set(
        &self,
        key: &TransitiveSetKey,
    ) -> buck2_error::Result<OwnedFrozen<ValueTyped<'static, TransitiveSet<'static>>>> {
        if key.holder_key() != &self.self_key {
            return Err(internal_error!(
                "Wrong owner for transitive set: expecting `{}`, got `{}`",
                self.self_key,
                key
            ));
        }
        Ok(self
            .analysis_storage
            .as_ref()
            .with_internal_error(|| format!("Missing analysis storage for `{key}`"))?
            .read()?
            .maybe_map::<ValueTyped<'static, TransitiveSet<'static>>, _>(|v| {
                v.as_ref()
                    .value
                    .transitive_sets
                    .get(key.index().0 as usize)
                    .copied()
            })
            .with_internal_error(|| format!("Missing transitive set `{key}`"))?
            .to_owned())
    }

    pub fn lookup_action(&self, key: &ActionKey) -> buck2_error::Result<ActionLookup> {
        if key.holder_key() != &self.self_key {
            return Err(internal_error!(
                "Wrong owner for action: expecting `{}`, got `{}`",
                self.self_key,
                key
            ));
        }
        self.actions.lookup(key)
    }

    pub fn num_actions(&self) -> usize {
        self.actions.num_actions()
    }

    /// Reads each action created in this analysis on demand.
    pub fn iter_actions(
        &self,
    ) -> impl Iterator<Item = buck2_error::Result<&Arc<RegisteredAction>>> + '_ {
        self.actions.iter_actions()
    }

    pub fn analysis_storage(
        &self,
    ) -> buck2_error::Result<OwnedFrozenRef<'_, &'static FrozenAnalysisValueStorage<'static>>> {
        Ok(self
            .analysis_storage
            .as_ref()
            .internal_error("missing analysis storage")?
            .read()?
            .map::<&'static FrozenAnalysisValueStorage<'static>, _>(|v| &v.as_ref().value))
    }

    /// Iterates over the declared dynamic_output/actions.
    pub fn iter_dynamic_lambda_outputs(
        &self,
    ) -> buck2_error::Result<impl Iterator<Item = BuildArtifact> + '_> {
        let storage = match &self.analysis_storage {
            Some(storage) => Some(storage.read()?),
            None => None,
        };
        Ok(storage.into_iter().flat_map(|v| {
            v.map::<&'static FrozenAnalysisValueStorage<'static>, _>(|v| &v.as_ref().value)
                .value()
                .lambda_params
                .dynamic_lambda_outputs()
        }))
    }

    pub fn provider_collection(&self) -> buck2_error::Result<FrozenProviderCollectionValueRef<'_>> {
        let inner = self
            .analysis_storage
            .as_ref()
            .internal_error("missing analysis storage")?
            .read()?
            .try_map::<FrozenValueTyped<'static, ProviderCollection<'static>>, buck2_error::Error, _>(
                |v| {
                    let collection = v
                        .as_ref()
                        .value
                        .result_value
                        .internal_error("missing provider collection")?;
                    FrozenValueTyped::new(collection.to_value())
                        .internal_error("provider collection is not a frozen `ProviderCollection`")
                },
            )?;
        Ok(FrozenProviderCollectionValueRef::from_inner(inner))
    }

    pub(crate) fn provider_collection_if_present(
        &self,
    ) -> buck2_error::Result<Option<FrozenProviderCollectionValueRef<'_>>> {
        if self.analysis_storage.is_none() {
            return Ok(None);
        }
        self.provider_collection().map(Some)
    }

    pub(crate) fn retained_memory(&self) -> buck2_error::Result<usize> {
        Ok(self
            .analysis_storage
            .as_ref()
            .internal_error("missing analysis storage")?
            .owner()
            .allocated_bytes())
    }
}
