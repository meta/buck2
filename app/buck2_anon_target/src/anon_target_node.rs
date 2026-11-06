/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::any::Any;
use std::fmt;
use std::fmt::Write;
use std::hash::Hash;
use std::hash::Hasher;
use std::sync::Arc;

use allocative::Allocative;
use buck2_analysis::analysis::env::RuleAnalysisAttrResolutionContext;
use buck2_analysis::analysis::env::get_deps_from_analysis_results;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_build_api::anon_target::AnonTargetDependentAnalysisResults;
use buck2_build_api::anon_target::AnonTargetDyn;
use buck2_build_api::artifact_groups::promise::PromiseArtifactId;
use buck2_build_api::artifact_groups::promise::PromiseArtifactResolveError;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_artifact_like::StarlarkArtifactView;
use buck2_core::configuration::data::ConfigurationData;
use buck2_core::configuration::pair::ConfigurationNoExec;
use buck2_core::content_hash::ContentBasedPathHash;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_core::deferred::base_deferred_key::BaseDeferredKeyDyn;
use buck2_core::deferred::base_deferred_key::PathResolutionError;
use buck2_core::execution_types::execution::ExecutionPlatformResolution;
use buck2_core::fs::buck_out_path::BuckOutPathKind;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_core::global_cfg_options::GlobalCfgOptions;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_data::ToProtoMessage;
use buck2_data::action_key_owner::BaseDeferredKeyProto;
use buck2_error::internal_error;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_hash::BuckFixedSortedMap;
use buck2_hash::BuckHasher;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutMap;
use buck2_interpreter::dice::starlark_provider::DynEvalKindKey;
use buck2_interpreter::dice::starlark_provider::StarlarkEvalKind;
use buck2_node::attrs::spec::AttributeSpec;
use buck2_node::attrs::values::SortedAttrValues;
use buck2_node::rule_type::StarlarkRuleType;
use buck2_util::strong_hasher::Blake3StrongHasher;
use cmp_any::PartialEqAny;
use compact_str::CompactString;
use dupe::Dupe;
use pagable::Pagable;
use pagable::pagable_typetag;
use starlark::environment::Module;
use starlark::eval::Evaluator;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::structs::AllocStruct;
use starlark::values::structs::StructRef;
use strong_hash::StrongHash;

use crate::anon_target_attr::AnonTargetAttr;
use crate::anon_target_attr_resolve::AnonTargetAttrResolution;
use crate::anon_target_attr_resolve::AnonTargetAttrResolutionContext;

/// The attribute values of an anon target.
///
/// Unlike `TargetNode`, every non-internal attribute is present: defaults
/// are filled in eagerly so that key equality does not distinguish a default
/// from the same value passed explicitly. Attribute names are not stored;
/// ids resolve against the [`AttributeSpec`] of the rule identified by the
/// `rule_type` stored alongside this in [`AnonTarget`].
pub(crate) type AnonAttrValues = SortedAttrValues<AnonTargetAttr>;

#[derive(Eq, PartialEq, Clone, Debug, Allocative, Pagable)]
pub(crate) struct AnonTarget {
    /// Not necessarily a "real" target label that actually exists, but could be.
    name: TargetLabel,
    /// The type of the rule we are running.
    rule_type: Arc<StarlarkRuleType>,
    /// The attributes the target was defined with. Iteration order is
    /// deterministic (sorted by id), which the cached hashes below rely on.
    attrs: AnonAttrValues,
    /// The execution configuration - same as the parent.
    exec_cfg: ConfigurationNoExec,
    /// Variant of the anon target, either bxl or bzl.
    variant: AnonTargetVariant,
    /// The cached strong hash value - we do have to cache this, it's quite perf sensitive
    strong_hash: u64,
    /// Cached hash value
    hash: u64,
}

#[derive(Hash, Eq, PartialEq, Clone, Debug, Allocative, StrongHash, Pagable)]
pub(crate) enum AnonTargetVariant {
    Bzl,
    Bxl(GlobalCfgOptions),
}

impl fmt::Display for AnonTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} (anon: {:016x}) ({})",
            self.name(),
            self.strong_hash,
            self.exec_cfg()
        )
    }
}

impl Hash for AnonTarget {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.hash);
    }
}

impl StrongHash for AnonTarget {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.strong_hash);
    }
}

pagable::register_typetag!(AnonTarget as dyn DynEvalKindKey);

impl AnonTarget {
    pub(crate) fn as_proto(&self) -> buck2_data::AnonTarget {
        buck2_data::AnonTarget {
            name: Some(self.name().as_proto()),
            execution_configuration: Some(self.exec_cfg().cfg().as_proto()),
            hash: format!("{:016x}", self.strong_hash),
        }
    }

    pub(crate) fn new(
        rule_type: Arc<StarlarkRuleType>,
        name: TargetLabel,
        attrs: AnonAttrValues,
        exec_cfg: ConfigurationNoExec,
        variant: AnonTargetVariant,
    ) -> Self {
        let mut full_hash = BuckHasher::default();
        rule_type.hash(&mut full_hash);
        name.hash(&mut full_hash);
        attrs.hash(&mut full_hash);
        exec_cfg.hash(&mut full_hash);
        variant.hash(&mut full_hash);
        let full_hash = full_hash.finish();

        let mut strong_hash = Blake3StrongHasher::new();
        rule_type.hash(&mut strong_hash);
        name.hash(&mut strong_hash);
        attrs.hash(&mut strong_hash);
        exec_cfg.hash(&mut strong_hash);
        variant.hash(&mut strong_hash);
        let strong_hash = strong_hash.finish();

        AnonTarget {
            name,
            rule_type,
            attrs,
            exec_cfg,
            variant,
            hash: full_hash,
            strong_hash,
        }
    }

    pub(crate) fn name(&self) -> &TargetLabel {
        &self.name
    }

    pub(crate) fn attrs(&self) -> &AnonAttrValues {
        &self.attrs
    }

    pub(crate) fn exec_cfg(&self) -> &ConfigurationNoExec {
        &self.exec_cfg
    }

    pub(crate) fn configured_label(&self) -> ConfiguredTargetLabel {
        // We need a configured label, but we don't have a real configuration (because it doesn't make sense),
        // so create a dummy version
        self.name().configure(ConfigurationData::unspecified())
    }

    pub(crate) fn anon_target_type(&self) -> &AnonTargetVariant {
        &self.variant
    }
}

impl AnonTargetDyn for AnonTarget {
    fn rule_type(&self) -> &Arc<StarlarkRuleType> {
        &self.rule_type
    }

    fn base_deferred_key(self: Arc<Self>) -> BaseDeferredKey {
        BaseDeferredKey::AnonTarget(self)
    }

    fn resolve_attrs<'v>(
        &self,
        env: &Module<'v>,
        attrs_spec: &AttributeSpec,
        dependents_analyses: AnonTargetDependentAnalysisResults<'_>,
        exec_resolution: ExecutionPlatformResolution,
    ) -> buck2_error::Result<ValueOf<'v, StructRef<'v>>> {
        let dep_analysis_results =
            get_deps_from_analysis_results(dependents_analyses.dep_analysis_results)?;

        // No attributes are allowed to contain macros or other stuff, so an empty resolution context works
        let rule_analysis_attr_resolution_ctx = RuleAnalysisAttrResolutionContext {
            module: env,
            dep_analysis_results,
            query_results: BuckMutMap::default(),
            execution_platform_resolution: exec_resolution,
        };

        let resolution_ctx = AnonTargetAttrResolutionContext {
            promised_artifacts_map: dependents_analyses.promised_artifacts,
            rule_analysis_attr_resolution_ctx,
        };

        // Both `self.attrs` and `attr_specs()` are sorted by id, so a single
        // merge walk recovers each attribute's name.
        let mut resolved_attrs = Vec::with_capacity(self.attrs().len());
        let mut attr_specs = attrs_spec.attr_specs();
        for (id, attr) in self.attrs().iter() {
            let name = loop {
                let (name, spec_id, _) = attr_specs.next().ok_or_else(|| {
                    internal_error!("anon target attr id not in the rule's attribute spec")
                })?;
                if spec_id == *id {
                    break name;
                }
            };
            resolved_attrs.push((
                name,
                attr.resolve_single(self.name().pkg(), &resolution_ctx)?,
            ));
        }
        // The field order of `ctx.attrs` is user-visible (e.g. `dir()`, repr).
        // Storage order (attribute ids) is an internal detail; present the
        // fields in name order.
        resolved_attrs.sort_unstable_by_key(|(name, _)| *name);
        Ok(ValueOf::alloc_struct(
            env.heap(),
            AllocStruct(resolved_attrs),
        ))
    }

    fn get_fulfilled_promise_artifacts<'v>(
        self: Arc<Self>,
        promise_artifact_mappings: BuckMutInsertionMap<String, Value<'v>>,
        anon_target_result: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> buck2_error::Result<BuckFixedSortedMap<PromiseArtifactId, Artifact>> {
        promise_artifact_mappings
            .values()
            .enumerate()
            .map(|(id, func)| {
                let artifact = eval.eval_function(*func, &[anon_target_result], &[])?;

                let promise_id =
                    PromiseArtifactId::new(BaseDeferredKey::AnonTarget(self.dupe()), id);

                match StarlarkArtifactView::unpack_value(artifact)? {
                    Some(artifact) => Ok((
                        promise_id,
                        artifact.as_input_artifact_like().get_bound_artifact()?,
                    )),
                    None => {
                        Err(PromiseArtifactResolveError::NotAnArtifact(artifact.to_repr()).into())
                    }
                }
            })
            .collect()
    }

    fn eval_kind(self: Arc<Self>) -> StarlarkEvalKind {
        StarlarkEvalKind::AnonTarget(self)
    }
}

#[pagable_typetag]
impl BaseDeferredKeyDyn for AnonTarget {
    fn eq_token(&self) -> PartialEqAny<'_> {
        PartialEqAny::new(self)
    }

    fn hash(&self) -> u64 {
        self.hash
    }

    fn strong_hash(&self) -> u64 {
        self.strong_hash
    }

    fn make_hashed_path(
        &self,
        base: &ProjectRelativePath,
        prefix: &ForwardRelativePath,
        action_key: Option<&str>,
        path: &ForwardRelativePath,
        path_resolution_method: BuckOutPathKind,
        content_hash: Option<&ContentBasedPathHash>,
    ) -> buck2_error::Result<ProjectRelativePathBuf> {
        let cell_relative_path = self.name().pkg().cell_relative_path().as_str();
        let mut configuration_path_hash = CompactString::with_capacity(16);
        let path_hash = if path_resolution_method == BuckOutPathKind::Configuration {
            // Unpadded on purpose, unlike the key's `Display` and every other
            // key hash: this hex is baked into existing output paths, and
            // padding it would change ~1/16 of anon paths and invalidate
            // their action cache entries.
            write!(&mut configuration_path_hash, "{:x}", self.strong_hash)
                .expect("u64 hex formatting fits in 16 bytes");
            configuration_path_hash.as_str()
        } else if let Some(content_hash) = content_hash {
            content_hash.as_str()
        } else {
            return Err(PathResolutionError::ContentBasedPathWithNoContentHash(
                path.to_buf(),
            ))?;
        };

        // It is performance critical that we use slices and allocate via `join` instead of
        // repeated calls to `join` on the path object because `join` allocates on each call,
        // which has a significant impact.
        let parts = [
            base.as_str(),
            "/",
            prefix.as_str(),
            "-anon/",
            self.name().pkg().cell_name().as_str(),
            if path_resolution_method == BuckOutPathKind::Configuration {
                "/"
            } else {
                ""
            },
            if path_resolution_method == BuckOutPathKind::Configuration {
                self.exec_cfg().cfg().output_hash().as_str()
            } else {
                ""
            },
            cell_relative_path,
            if cell_relative_path.is_empty() {
                ""
            } else {
                "/"
            },
            path_hash,
            "/__",
            self.name().name().as_str(),
            "__",
            action_key.unwrap_or_default(),
            if action_key.is_none() { "" } else { "__" },
            "/",
            path.as_str(),
        ];

        Ok(ProjectRelativePathBuf::unchecked_new(parts.concat()))
    }

    fn configured_label(&self) -> Option<ConfiguredTargetLabel> {
        Some(self.configured_label())
    }

    fn rule_type_name(&self) -> Option<&str> {
        Some(self.rule_type.name.as_str())
    }

    fn to_proto(&self) -> BaseDeferredKeyProto {
        BaseDeferredKeyProto::AnonTarget(self.as_proto())
    }

    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }

    fn global_cfg_options(&self) -> Option<GlobalCfgOptions> {
        match &self.variant {
            AnonTargetVariant::Bzl => None,
            AnonTargetVariant::Bxl(global_cfg_options) => Some(global_cfg_options.dupe()),
        }
    }
}
