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

use buck2_analysis::analysis::calculation::get_loaded_module;
use buck2_build_api::anon_target::AnonTargetDyn;
use buck2_build_api::anon_target::AnonTargetNodeInfo;
use buck2_build_api::anon_target::GET_ANON_TARGET_NODE_INFO;
use buck2_build_api::artifact_groups::promise::PromiseArtifactAttr;
use buck2_core::deferred::base_deferred_key::BaseDeferredKeyDyn;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_error::BuckErrorContext;
use buck2_error::conversion::from_any_with_tag;
use buck2_interpreter_for_build::rule::frozen_rule_attribute_spec;
use buck2_node::attrs::configured_traversal::ConfiguredAttrTraversal;
use buck2_node::attrs::fmt_context::AttrFmtContext;
use buck2_node::attrs::json::ToJsonWithContext;
use dice::DiceComputations;
use futures::FutureExt;

use crate::anon_target_attr_resolve::AnonTargetAttrTraversal;
use crate::anon_target_node::AnonTargetVariant;
use crate::anon_targets::AnonTargetKey;

async fn anon_target_node_info(
    dice: &mut DiceComputations<'_>,
    key: Arc<dyn BaseDeferredKeyDyn>,
) -> buck2_error::Result<AnonTargetNodeInfo> {
    let key = AnonTargetKey::downcast(key)?;
    let anon = &key.0;

    struct CollectDeps(Vec<String>);

    impl ConfiguredAttrTraversal for CollectDeps {
        fn dep(&mut self, dep: &ConfiguredProvidersLabel) -> buck2_error::Result<()> {
            self.0.push(dep.to_string());
            Ok(())
        }
    }

    struct CollectPromiseArtifactOwners(Vec<String>);

    impl AnonTargetAttrTraversal for CollectPromiseArtifactOwners {
        fn promise_artifact(
            &mut self,
            promise_artifact: &PromiseArtifactAttr,
        ) -> buck2_error::Result<()> {
            self.0.push(promise_artifact.id.owner().to_string());
            Ok(())
        }
    }

    // Attrs are stored by id; their names live on the rule's attribute spec.
    let module = get_loaded_module(dice, anon.rule_type()).await?;
    let (rule, _visibility) = module
        .env()
        .get_any_visibility(&anon.rule_type().name)
        .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0))
        .with_buck_error_context(|| format!("Couldn't find rule `{}`", anon.rule_type().name))?;
    let fmt_ctx = AttrFmtContext {
        package: Some(anon.name().pkg()),
        options: Default::default(),
    };
    let (attrs, deps, promise_artifact_owners) = rule.by_ref_with_reconstructor(|rule, _| {
        let attrs_spec = frozen_rule_attribute_spec(*rule)?;
        let mut attrs = serde_json::Map::new();
        let mut deps = CollectDeps(Vec::new());
        let mut promise_artifact_owners = CollectPromiseArtifactOwners(Vec::new());
        for (name, id, _) in attrs_spec.attr_specs() {
            let Some(value) = anon.attrs().get(id) else {
                continue;
            };
            attrs.insert(name.to_owned(), value.to_json(&fmt_ctx)?);
            value.traverse(anon.name().pkg(), &mut deps)?;
            value.traverse_anon_attr(&mut promise_artifact_owners)?;
        }
        buck2_error::Ok((attrs, deps, promise_artifact_owners))
    })?;

    Ok(AnonTargetNodeInfo {
        name: anon.name().to_string(),
        package: anon.name().pkg().to_string(),
        hash: format!("{:016x}", BaseDeferredKeyDyn::strong_hash(&**anon)),
        rule_type: anon.rule_type().to_string(),
        variant: match anon.anon_target_type() {
            AnonTargetVariant::Bzl => "bzl",
            AnonTargetVariant::Bxl(..) => "bxl",
        },
        execution_configuration: anon.exec_cfg().to_string(),
        attrs: serde_json::Value::Object(attrs),
        deps: deps.0,
        promise_artifact_deps: promise_artifact_owners.0,
    })
}

pub(crate) fn init_get_anon_target_node_info() {
    GET_ANON_TARGET_NODE_INFO.init(|dice, key| anon_target_node_info(dice, key).boxed());
}
