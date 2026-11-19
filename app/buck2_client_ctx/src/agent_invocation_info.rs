/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use crate::agent_context::AgentContext;

/// Agent inputs and an independent observation for one client invocation.
/// The default has no metadata or inference and does not inspect processes.
#[derive(Clone, Debug, Default)]
pub struct AgentInvocationInfo {
    agent_context: AgentContext,
    inference: AgentCallInference,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct AgentCallInference {
    /// Best-effort client-ancestry inference, independent of explicit agent context.
    /// `Some(true)` is a heuristic match; `Some(false)` is a non-match.
    /// `None` means inference has not run or inspection was inconclusive.
    pub(crate) inferred_direct_call: Option<bool>,
    pub(crate) callers: Vec<buck2_data::CallerProcess>,
    /// Absent when inference has not run; otherwise explains where the walk stopped.
    pub(crate) stop_reason: Option<buck2_data::AgentDetectionStopReason>,
}

impl AgentInvocationInfo {
    /// Record inference in the client: a shared daemon's ancestry says nothing
    /// about the caller of an individual command.
    pub fn from_client(agent_context: AgentContext) -> Self {
        Self::with_inference(agent_context, || {
            #[cfg(all(fbcode_build, target_os = "linux"))]
            {
                crate::agent_detection::infer_direct_agent_call()
            }
            #[cfg(not(all(fbcode_build, target_os = "linux")))]
            {
                AgentCallInference::default()
            }
        })
    }

    fn with_inference(
        agent_context: AgentContext,
        detect: impl FnOnce() -> AgentCallInference,
    ) -> Self {
        Self {
            agent_context,
            inference: detect(),
        }
    }

    pub(crate) fn inferred_direct_call(&self) -> Option<bool> {
        self.inference.inferred_direct_call
    }

    pub(crate) fn inference(&self) -> &AgentCallInference {
        &self.inference
    }

    pub(crate) fn agent_context(&self) -> &AgentContext {
        &self.agent_context
    }
}

#[cfg(test)]
mod tests {
    use buck2_data::AgentContextEntry as AgentContextKeys;

    use super::*;
    use crate::agent_context::parse_agent_context;

    #[test]
    fn inference_is_independent_of_explicit_markers() {
        for inferred in [Some(true), Some(false), None] {
            for explicit in [None, Some("true"), Some("false"), Some("invalid"), Some("")] {
                let entries = explicit
                    .map(|value| parse_agent_context(&format!("direct_call={value}")).unwrap())
                    .into_iter()
                    .collect();
                let info = AgentInvocationInfo::with_inference(AgentContext::new(entries), || {
                    AgentCallInference {
                        inferred_direct_call: inferred,
                        ..Default::default()
                    }
                });
                assert_eq!(
                    info.agent_context().is_direct_call(),
                    explicit == Some("true")
                );
                let entries = info.agent_context().to_proto();
                assert_eq!(
                    entries
                        .iter()
                        .find(|e| e.key == AgentContextKeys::KEY_DIRECT_CALL)
                        .map(|e| e.value.as_str()),
                    explicit,
                );
                assert_eq!(info.inferred_direct_call(), inferred);
                assert_eq!(entries.len(), usize::from(explicit.is_some()));
            }
        }
    }

    #[test]
    fn inference_cannot_be_overridden_by_cli_entries() {
        for inferred in [Some(true), Some(false), None] {
            let entries = [
                "inferred_direct_call=true",
                "inferred_direct_call=false",
                "agent_callers=agent:fake",
                "agent_detection_stop_reason=agent",
            ]
            .map(|value| parse_agent_context(value).unwrap())
            .to_vec();
            let info = AgentInvocationInfo::with_inference(AgentContext::new(entries), || {
                AgentCallInference {
                    inferred_direct_call: inferred,
                    ..Default::default()
                }
            });
            assert!(!info.agent_context().is_direct_call());
            assert_eq!(info.inferred_direct_call(), inferred);
            assert_eq!(info.agent_context().to_proto().len(), 4);
            assert!(info.inference().callers.is_empty());
            assert_eq!(info.inference().stop_reason, None);
        }
    }
}
