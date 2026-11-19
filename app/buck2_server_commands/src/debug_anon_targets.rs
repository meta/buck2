/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::fmt::Write;

use buck2_build_api::analysis::AnalysisResult;
use buck2_build_api::analysis::calculation::RuleAnalysisCalculation;
use buck2_build_api::anon_target::AnonTargetNodeInfo;
use buck2_build_api::anon_target::GET_ANON_TARGET_NODE_INFO;
use buck2_build_api::deferred::calculation::EVAL_ANON_TARGET;
use buck2_cli_proto::new_generic::AnonTargetsRequest;
use buck2_cli_proto::new_generic::AnonTargetsResponse;
use buck2_core::configuration::compatibility::MaybeCompatible;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_core::pattern::pattern_type::TargetPatternExtra;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_error::conversion::from_any_with_tag;
use buck2_error::internal_error;
use buck2_hash::BuckMutSet;
use buck2_node::nodes::attributes;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_node::nodes::configured_frontend::ConfiguredTargetNodeCalculation;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::ctx::ServerCommandDiceContext;
use buck2_server_ctx::pattern_parse_and_resolve::parse_and_resolve_patterns_to_targets_from_cli_args;
use buck2_server_ctx::target_resolution_config::TargetResolutionConfig;
use buck2_settings::dice::HasBuckSettings;
use dupe::Dupe;
use regex::RegexSet;

/// One anon target in the output: how it is defined, plus (optionally) which analyses
/// requested it.
struct AnonTargetEntry {
    info: AnonTargetNodeInfo,
    requested_by: Option<BTreeSet<String>>,
}

pub(crate) async fn debug_anon_targets_command(
    server_ctx: &dyn ServerCommandContextTrait,
    req: AnonTargetsRequest,
) -> buck2_error::Result<AnonTargetsResponse> {
    server_ctx
        .with_dice_ctx(|server_ctx, ctx| async move {
            let mut dice = ctx.ctx();
            if !dice
                .global_data()
                .get_buck_settings()
                .analysis
                .record_requested_anon_targets()
            {
                return Err(buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Input,
                    "Anon target recording is disabled; remove the `[analysis] record_requested_anon_targets = false` Buck setting to use this command"
                ));
            }
            let resolution_config =
                TargetResolutionConfig::from_args(&mut dice, &req.target_cfg, server_ctx, &[])
                    .await?;
            let target_labels = parse_and_resolve_patterns_to_targets_from_cli_args::<
                TargetPatternExtra,
            >(&mut dice, &req.patterns, server_ctx.working_dir())
            .await?;
            let mut roots: Vec<ConfiguredTargetLabel> = Vec::new();
            for label in &target_labels {
                roots.extend(
                    resolution_config
                        .get_configured_target(&mut dice, &label.target_label, None)
                        .await?,
                );
            }

            // The universe: the configured deps closure of the roots.
            let mut nodes: Vec<ConfiguredTargetNode> = Vec::new();
            let mut visited: BuckMutSet<ConfiguredTargetLabel> = BuckMutSet::default();
            let mut queue: VecDeque<ConfiguredTargetNode> = VecDeque::new();
            for label in roots {
                if let MaybeCompatible::Compatible(node) =
                    dice.get_configured_target_node(&label).await.ok()?
                    && visited.insert(node.label().dupe())
                {
                    queue.push_back(node.dupe());
                }
            }
            while let Some(node) = queue.pop_front() {
                for dep in node.deps() {
                    if visited.insert(dep.label().dupe()) {
                        queue.push_back(dep.dupe());
                    }
                }
                nodes.push(node);
            }

            // Analyze the universe; collect the anon targets each analysis requested.
            let analysis_results: Vec<(ConfiguredTargetLabel, Option<AnalysisResult>)> = dice
                .try_compute_join(nodes, async |ctx, node| {
                    let result = match ctx.get_analysis_result(node.label()).await.ok()? {
                        MaybeCompatible::Compatible(v) => Some(v.dupe()),
                        MaybeCompatible::Incompatible(..) => None,
                    };
                    buck2_error::Ok((node.label().dupe(), result))
                })
                .await?;

            let mut anon_queue: VecDeque<BaseDeferredKey> = VecDeque::new();
            let mut requesters: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            let mut seen: BuckMutSet<BaseDeferredKey> = BuckMutSet::default();
            for (label, result) in &analysis_results {
                let Some(result) = result else { continue };
                for key in result.requested_anon_targets() {
                    requesters
                        .entry(key.to_string())
                        .or_default()
                        .insert(label.to_string());
                    if seen.insert(key.dupe()) {
                        anon_queue.push_back(key.dupe());
                    }
                }
            }

            // Walk anon-in-anon requests transitively and render each unique key.
            let mut entries: BTreeMap<String, AnonTargetEntry> = BTreeMap::new();
            while let Some(key) = anon_queue.pop_front() {
                let BaseDeferredKey::AnonTarget(anon) = &key else {
                    return Err(internal_error!(
                        "`requested_anon_targets` recorded a non-anon key `{}`",
                        key
                    ));
                };
                let analysis = (EVAL_ANON_TARGET.get()?)(&mut dice, anon.dupe()).await?;
                for nested in analysis.requested_anon_targets() {
                    requesters
                        .entry(nested.to_string())
                        .or_default()
                        .insert(key.to_string());
                    if seen.insert(nested.dupe()) {
                        anon_queue.push_back(nested.dupe());
                    }
                }
                let info = (GET_ANON_TARGET_NODE_INFO.get()?)(&mut dice, anon.dupe()).await?;
                entries.insert(
                    key.to_string(),
                    AnonTargetEntry {
                        info,
                        requested_by: None,
                    },
                );
            }

            if req.with_requesters {
                for (key, entry) in &mut entries {
                    entry.requested_by = Some(requesters.remove(key).unwrap_or_default());
                }
            }

            let serialized = if req.json || !req.output_attributes.is_empty() {
                render_json(&entries, &req.output_attributes)?
            } else {
                render_text(&entries)?
            };

            Ok(AnonTargetsResponse { serialized })
        })
        .await
}

/// Flattens an entry into a cquery-style attribute map: special attributes are
/// `buck.`-prefixed, and the anon target's own attrs sit at the top level (their names
/// cannot contain a `.`, so cquery's `-B` regex selects exactly them plus `buck.package`
/// and `buck.type`).
fn flatten_entry(
    key: &str,
    entry: &AnonTargetEntry,
) -> buck2_error::Result<serde_json::Map<String, serde_json::Value>> {
    let info = &entry.info;
    let mut map = serde_json::Map::new();
    map.insert("buck.key".to_owned(), serde_json::to_value(key)?);
    map.insert("buck.name".to_owned(), serde_json::to_value(&info.name)?);
    map.insert(
        attributes::PACKAGE.to_owned(),
        serde_json::to_value(&info.package)?,
    );
    map.insert(
        attributes::TYPE.to_owned(),
        serde_json::to_value(&info.rule_type)?,
    );
    map.insert(
        "buck.variant".to_owned(),
        serde_json::to_value(info.variant)?,
    );
    map.insert(
        "buck.execution_configuration".to_owned(),
        serde_json::to_value(&info.execution_configuration)?,
    );
    map.insert("buck.hash".to_owned(), serde_json::to_value(&info.hash)?);
    map.insert(
        attributes::DEPS.to_owned(),
        serde_json::to_value(&info.deps)?,
    );
    map.insert(
        "buck.promise_artifact_deps".to_owned(),
        serde_json::to_value(&info.promise_artifact_deps)?,
    );
    if let Some(requested_by) = &entry.requested_by {
        map.insert(
            "buck.requested_by".to_owned(),
            serde_json::to_value(requested_by)?,
        );
    }
    if let serde_json::Value::Object(attrs) = &info.attrs {
        for (name, value) in attrs {
            map.insert(name.clone(), value.clone());
        }
    }
    Ok(map)
}

fn render_json(
    entries: &BTreeMap<String, AnonTargetEntry>,
    output_attributes: &[String],
) -> buck2_error::Result<String> {
    let filter = if output_attributes.is_empty() {
        None
    } else {
        Some(
            RegexSet::new(output_attributes)
                .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Input))?,
        )
    };
    let mut rendered = serde_json::Map::new();
    for (key, entry) in entries {
        let mut map = flatten_entry(key, entry)?;
        if let Some(filter) = &filter {
            map.retain(|name, _| filter.is_match(name));
        }
        rendered.insert(key.clone(), serde_json::Value::Object(map));
    }
    Ok(serde_json::to_string_pretty(&serde_json::Value::Object(
        rendered,
    ))?)
}

fn render_text(entries: &BTreeMap<String, AnonTargetEntry>) -> buck2_error::Result<String> {
    let mut out = String::new();
    for (key, entry) in entries {
        let attrs = serde_json::to_string(&entry.info.attrs)?;
        write_text_entry(&mut out, key, entry, &attrs)
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0))?;
    }
    Ok(out)
}

fn write_text_entry(
    out: &mut String,
    key: &str,
    entry: &AnonTargetEntry,
    attrs: &str,
) -> Result<(), std::fmt::Error> {
    let info = &entry.info;
    writeln!(out, "{key}")?;
    writeln!(out, "  rule_type: {}", info.rule_type)?;
    writeln!(out, "  variant: {}", info.variant)?;
    writeln!(
        out,
        "  execution_configuration: {}",
        info.execution_configuration
    )?;
    writeln!(out, "  attrs: {attrs}")?;
    if !info.deps.is_empty() {
        writeln!(out, "  deps: [{}]", info.deps.join(", "))?;
    }
    if !info.promise_artifact_deps.is_empty() {
        writeln!(
            out,
            "  promise_artifact_deps: [{}]",
            info.promise_artifact_deps.join(", ")
        )?;
    }
    if let Some(requested_by) = &entry.requested_by {
        writeln!(out, "  requested_by:")?;
        for requester in requested_by {
            writeln!(out, "    {requester}")?;
        }
    }
    Ok(())
}
