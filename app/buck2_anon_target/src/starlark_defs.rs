/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;

use allocative::Allocative;
use buck2_build_api::artifact_groups::promise::PromiseArtifact;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_promise_artifact::StarlarkPromiseArtifact;
use buck2_build_api::interpreter::rule_defs::context::ANALYSIS_ACTIONS_METHODS_ANON_TARGET;
use buck2_build_api::interpreter::rule_defs::context::AnalysisActions;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use buck2_hash::BuckMutInsertionMap;
use buck2_interpreter::downstream_crate_starlark_defs::REGISTER_BUCK2_ANON_TARGETS_GLOBALS;
use buck2_interpreter::execution_group::ExecutionGroupArg;
use buck2_interpreter::starlark_promise::StarlarkPromise;
use buck2_interpreter_for_build::rule::FrozenArtifactPromiseMappings;
use buck2_interpreter_for_build::rule::StarlarkRuleCallableView;
use dupe::Dupe;
use starlark::any::ProvidesStaticType;
use starlark::codemap::FileSpan;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::AllocValue;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkValue;
use starlark::values::StringValue;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::dict::DictType;
use starlark::values::list::AllocList;
use starlark::values::list_or_tuple::ListOrTupleType;
use starlark::values::none::NoneOr;
use starlark::values::starlark_value;

use crate::anon_targets::AnonTargetKey;
use crate::anon_targets::AnonTargetsRegistry;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
pub enum AnonTargetsError {
    #[error("artifact with name `{0}` was not found")]
    ArtifactNotFound(String),
}

#[derive(Debug, NoSerialize, ProvidesStaticType, Trace, Allocative, Clone)]
struct StarlarkAnonTarget<'v> {
    // Promise created by the anon rule
    promise: ValueTyped<'v, StarlarkPromise<'v>>,
    // Promised artifacts of the anon rule
    artifacts: BuckMutInsertionMap<StringValue<'v>, PromiseArtifact>,
    // Where the anon target was declared
    declaration_location: Option<FileSpan>,
}

impl<'v> StarlarkAnonTarget<'v> {
    fn new(
        declaration_location: Option<FileSpan>,
        anon_target_promise: ValueTyped<'v, StarlarkPromise<'v>>,
        frozen_artifact_mappings: &Option<FrozenArtifactPromiseMappings<'v>>,
        key: AnonTargetKey,
        registry: &mut AnonTargetsRegistry<'v>,
    ) -> buck2_error::Result<StarlarkAnonTarget<'v>> {
        let mut artifacts_map = BuckMutInsertionMap::new();
        if let Some(artifacts) = frozen_artifact_mappings {
            for (id, name) in artifacts.mappings.keys().enumerate() {
                let artifact =
                    registry.register_artifact(declaration_location.dupe(), key.clone(), id)?;
                artifacts_map.insert(*name, artifact);
            }
        }

        let anon_target = StarlarkAnonTarget {
            promise: anon_target_promise,
            artifacts: artifacts_map,
            declaration_location,
        };
        Ok(anon_target)
    }
}

impl Display for StarlarkAnonTarget<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<anon target")?;
        if let Some(location) = &self.declaration_location {
            write!(f, " declared at {location}")?;
        }
        write!(f, ">")?;
        Ok(())
    }
}

starlark::methods_static!(ANON_TARGET_METHODS = anon_target_methods);

#[starlark_value(type = "AnonTarget", StarlarkTypeRepr, UnpackValue)]
impl<'v> StarlarkValue<'v> for StarlarkAnonTarget<'v> {
    fn get_methods() -> Option<&'static Methods> {
        Some(ANON_TARGET_METHODS.methods())
    }
}

impl<'v> AllocValue<'v> for StarlarkAnonTarget<'v> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_complex_no_freeze(self)
    }
}

/// Accessors to the promise of the anon rule and the promised artifacts associated with the rule.
#[starlark_module]
fn anon_target_methods(builder: &mut MethodsBuilder) {
    /// Returns a dict where the key is the promise artifact's name, and the value is the `StarlarkPromiseArtifact`.
    ///
    /// To get a promise artifact where the short path is accessible, call `ctx.actions.assert_short_path(...)` and
    /// pass in the artifact retrieved from this dict.
    fn artifacts<'v>(
        this: &StarlarkAnonTarget<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<Value<'v>> {
        Ok(eval.heap().alloc_dict(
            this.artifacts
                .iter()
                .map(|(k, v)| {
                    Ok((
                        k.to_value().get_hashed()?,
                        eval.heap().alloc(StarlarkPromiseArtifact::new(
                            eval.call_stack_top_location(),
                            v.dupe(),
                            None,
                            false,
                        )),
                    ))
                })
                .collect::<starlark::Result<_>>()?,
        ))
    }

    /// Gets a specific `StarlarkPromiseArtifact` by name. Returns an error if the name was not found in the
    /// registered promise artifacts for the anon target.
    fn artifact<'v>(
        this: &StarlarkAnonTarget<'v>,
        name: &'v str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<Value<'v>> {
        match this.artifacts.get(name) {
            Some(v) => Ok(eval.heap().alloc(StarlarkPromiseArtifact::new(
                eval.call_stack_top_location(),
                v.dupe(),
                None,
                false,
            ))),
            None => {
                let buck2_error: buck2_error::Error =
                    AnonTargetsError::ArtifactNotFound(name.to_owned()).into();
                Err(buck2_error.into())
            }
        }
    }

    /// Returns the promise that maps to the result of the anon rule.
    #[starlark(attribute)]
    fn promise<'v>(
        this: &StarlarkAnonTarget<'v>,
    ) -> starlark::Result<ValueTyped<'v, StarlarkPromise<'v>>> {
        Ok(this.promise)
    }
}

#[derive(Debug, NoSerialize, ProvidesStaticType, Trace, Allocative, Clone)]
struct StarlarkAnonTargets<'v> {
    // Joined promise for all anon targets.
    promise: ValueTyped<'v, StarlarkPromise<'v>>,
    // All `StarlarkAnonTargets`
    anon_targets: Vec<StarlarkAnonTarget<'v>>,
    // Where the anon target were declared
    declaration_location: Option<FileSpan>,
}

impl Display for StarlarkAnonTargets<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<anon targets")?;
        if let Some(location) = &self.declaration_location {
            write!(f, " declared at {location}")?;
        }
        write!(f, ">")?;
        Ok(())
    }
}

starlark::methods_static!(ANON_TARGETS_METHODS = anon_targets_methods);

#[starlark_value(type = "AnonTargets", StarlarkTypeRepr, UnpackValue)]
impl<'v> StarlarkValue<'v> for StarlarkAnonTargets<'v> {
    fn get_methods() -> Option<&'static Methods> {
        Some(ANON_TARGETS_METHODS.methods())
    }
}

impl<'v> AllocValue<'v> for StarlarkAnonTargets<'v> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_complex_no_freeze(self)
    }
}

/// Accessors to the all the anon targets created, as well as the joined promise of these targets.
#[starlark_module]
fn anon_targets_methods(builder: &mut MethodsBuilder) {
    /// Returns a list of `StarlarkAnonTarget`s.
    #[starlark(attribute)]
    fn anon_targets<'v>(
        this: &StarlarkAnonTargets<'v>,
        heap: Heap<'v>,
    ) -> starlark::Result<Value<'v>> {
        Ok(heap.alloc(AllocList(
            this.anon_targets.iter().map(|a| heap.alloc(a.clone())),
        )))
    }

    /// Returns the promise that maps to the result of the joined anon rules.
    #[starlark(attribute)]
    fn promise<'v>(
        this: &StarlarkAnonTargets<'v>,
    ) -> starlark::Result<ValueTyped<'v, StarlarkPromise<'v>>> {
        Ok(this.promise)
    }
}

#[starlark_module]
#[starlark_types(StarlarkAnonTarget<'_> as AnonTarget, StarlarkAnonTargets<'_> as AnonTargets)]
pub(crate) fn register_anon_target_types(globals: &mut GlobalsBuilder) {}

pub(crate) fn init_register_anon_target_types() {
    REGISTER_BUCK2_ANON_TARGETS_GLOBALS.init(register_anon_target_types);
}

#[starlark_module]
fn analysis_actions_methods_anon_target(builder: &mut MethodsBuilder) {
    /// An anonymous target is defined by the hash of its attributes, rather than its name.
    /// During analysis, rules can define and access the providers of anonymous targets before producing their own providers.
    /// Two distinct rules might ask for the same anonymous target, sharing the work it performs.
    ///
    /// For more details see https://buck2.build/docs/rule_authors/anon_targets/
    fn anon_target<'v>(
        this: &AnalysisActions<'v>,
        // TODO(nga): this should be either positional or named, not both.
        rule: StarlarkRuleCallableView<'v>,
        attrs: ValueOf<'v, DictType<&'v str, Value<'v>>>,
        #[starlark(require = named, default = NoneOr::None)] exec_group: NoneOr<ExecutionGroupArg>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAnonTarget<'v>> {
        let exec_group = exec_group
            .into_option()
            .map(|ExecutionGroupArg { group }| group);
        let rule = rule.frozen()?;
        let anon_target_promise = eval.heap().alloc_typed(StarlarkPromise::new_unresolved());
        let mut this = this.state()?;
        let this = &mut *this;
        let registry = AnonTargetsRegistry::downcast_mut(&mut **this.anon_targets)?;
        let owner_key = this.analysis_value_storage.self_key.owner();
        let key = registry.anon_target_key(rule, attrs, owner_key, exec_group.as_ref())?;
        registry.register_one(anon_target_promise, key.dupe())?;

        Ok(StarlarkAnonTarget::new(
            eval.call_stack_top_location(),
            anon_target_promise,
            rule.artifact_promise_mappings(),
            key,
            registry,
        )?)
    }

    /// Generate a series of anonymous targets.
    fn anon_targets<'v>(
        this: &AnalysisActions<'v>,
        // TODO(nga): this should be either positional or named, not both.
        rules: ValueOf<
            'v,
            ListOrTupleType<(
                StarlarkRuleCallableView<'v>,
                ValueOf<'v, DictType<&'v str, Value<'v>>>,
            )>,
        >,
        #[starlark(require = named, default = NoneOr::None)] exec_group: NoneOr<ExecutionGroupArg>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkAnonTargets<'v>> {
        let exec_group = exec_group
            .into_option()
            .map(|ExecutionGroupArg { group }| group);
        let mut this = this.state()?;
        let this = &mut *this;
        let registry = AnonTargetsRegistry::downcast_mut(&mut **this.anon_targets)?;
        let declaration_location = eval.call_stack_top_location();

        let mut anon_targets = Vec::new();
        let mut promises_to_join = Vec::new();
        let owner_key = this.analysis_value_storage.self_key.owner();
        rules
            .try_items()
            .map(|rule| {
                let (rule, attributes) = rule?;
                let rule = rule.frozen()?;
                let key =
                    registry.anon_target_key(rule, attributes, owner_key, exec_group.as_ref())?;
                let anon_target_promise =
                    eval.heap().alloc_typed(StarlarkPromise::new_unresolved());

                promises_to_join.push(anon_target_promise);

                registry.register_one(anon_target_promise, key.dupe())?;
                let anon_target = StarlarkAnonTarget::new(
                    declaration_location.dupe(),
                    anon_target_promise,
                    rule.artifact_promise_mappings(),
                    key.dupe(),
                    registry,
                )?;

                anon_targets.push(anon_target);

                buck2_error::Ok(key)
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(StarlarkAnonTargets {
            promise: StarlarkPromise::join(promises_to_join, eval.heap()),
            anon_targets,
            declaration_location,
        })
    }

    /// Generate a promise artifact that has short path accessible on it. The short path's correctness will
    /// be asserted during analysis time.
    ///
    /// TODO - we would prefer the API to be `ctx.actions.anon_target(xxx).artifact("foo", short_path=yyy)`, but
    /// we cannot support this until we can get access to the `AnalysisContext` without passing it into this method.
    fn assert_short_path<'v>(
        this: &AnalysisActions<'v>,
        // TODO(nga): this should be either positional or named, not both.
        artifact: ValueTyped<'v, StarlarkPromiseArtifact>,
        short_path: &'v str,
    ) -> starlark::Result<StarlarkPromiseArtifact> {
        let mut this = this.state()?;
        let promise = artifact.artifact.dupe();

        let short_path = ForwardRelativePathBuf::new(short_path.to_owned())?;
        (*this).record_short_path_assertion(short_path.clone(), promise.id.clone());

        Ok(StarlarkPromiseArtifact::new(
            artifact.declaration_location.dupe(),
            promise,
            Some(short_path),
            artifact.has_content_based_path,
        ))
    }

    fn assert_has_content_based_path<'v>(
        this: &AnalysisActions<'v>,
        artifact: ValueTyped<'v, StarlarkPromiseArtifact>,
    ) -> starlark::Result<StarlarkPromiseArtifact> {
        let mut this = this.state()?;
        let promise = artifact.artifact.dupe();

        (*this).record_has_content_based_path_assertion(promise.id.clone());

        Ok(StarlarkPromiseArtifact::new(
            artifact.declaration_location.dupe(),
            promise,
            artifact.short_path.clone(),
            true,
        ))
    }
}

pub(crate) fn init_analysis_actions_methods_anon_target() {
    ANALYSIS_ACTIONS_METHODS_ANON_TARGET.init(analysis_actions_methods_anon_target);
}
