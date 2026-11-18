/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_agent_interactive::Advice;
use buck2_agent_interactive::Decision;
use buck2_agent_interactive::Invocation;
use buck2_cli_proto::ClientContext;
use buck2_core::pattern::pattern::ParsedPattern;
use buck2_core::pattern::pattern::ParsedPatternWithModifiers;
use buck2_core::pattern::pattern_type::PatternType;
use buck2_data::AgentContextEntry;
use buck2_data::TagEvent;
use buck2_error::ErrorTag;
use buck2_events::dispatch::instant_event;
use buck2_settings::InvocationBuckSettings;

/// Check parsed patterns before recursive package discovery, outside DICE's cached computations.
/// `run` shares the build RPC and must not inherit build/test advice.
pub fn check_recursive_patterns<T: PatternType>(
    settings: &InvocationBuckSettings,
    context: Option<&ClientContext>,
    patterns: &[ParsedPatternWithModifiers<T>],
) -> buck2_error::Result<()> {
    let Some(context) = context else {
        return Ok(());
    };
    if !matches!(context.command_name.as_str(), "build" | "test") {
        return Ok(());
    }
    let last = |key: &str| {
        context
            .agent_context
            .iter()
            .rev()
            .find(|entry| entry.key == key)
            .map(|entry| entry.value.as_str())
    };
    if last(AgentContextEntry::KEY_DIRECT_CALL) != Some("true") {
        return Ok(());
    }
    let advice = Advice::RecursivePatterns;
    let enabled = settings.agent_recursive_patterns.enabled();
    // Include narrowed retries in the experiment denominator, even without a recursive pattern.
    let mut tags = vec![advice.state_tag(enabled)];
    let recursive_pattern = patterns
        .iter()
        .find(|p| matches!(p.parsed_pattern, ParsedPattern::Recursive(_)));
    let Some(pattern) = recursive_pattern else {
        instant_event(TagEvent { tags });
        return Ok(());
    };
    tags.push("agent_interactive.recursive_patterns.detected".to_owned());
    let decision = advice.evaluate(
        &Invocation {
            enabled,
            direct_call: true,
            reason: last("reason"),
        },
        context.agent_context.iter().any(|entry| {
            entry.key == AgentContextEntry::KEY_ADVICE_ACK && entry.value == advice.id()
        }),
    );
    if let Some(tag) = decision.tag() {
        tags.push(tag.to_owned());
    }
    instant_event(TagEvent { tags });
    match decision {
        Decision::Inactive | Decision::Acknowledged(_) => Ok(()),
        Decision::Block(advice) => Err(buck2_error::buck2_error!(
            ErrorTag::AgentAdviceBlocked,
            "Buck advice [{}]\nRecursive target pattern `{}`.\n\n{}",
            advice.id(),
            pattern.parsed_pattern,
            advice.message(),
        )),
    }
}
