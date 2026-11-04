/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

// prost represents proto `map` fields as `std::collections::HashMap`.
#![allow(clippy::disallowed_types)]

use std::borrow::Cow;
use std::fmt;

use buck2_miniperf_proto::MiniperfCounter;

pub mod action_key_owner;
pub mod agent_context_keys;

mod serialize_bytes {
    use serde::Deserialize;
    use serde::Deserializer;
    use serde::Serialize;
    use serde::Serializer;

    pub fn serialize<S>(value: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let d = hex::encode(value);
        d.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let d = String::deserialize(deserializer)?;
        let d = hex::decode(d).map_err(serde::de::Error::custom)?;
        Ok(d)
    }
}

mod serialize_action_kind {
    use serde::Deserialize;
    use serde::Deserializer;
    use serde::Serialize;
    use serde::Serializer;

    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub fn serialize<S>(value: &i32, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let k = crate::ActionKind::try_from(*value).map_err(|_| {
            serde::ser::Error::custom(format!("Invalid ActionKind enum value: {value}"))
        })?;
        k.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<i32, D::Error>
    where
        D: Deserializer<'de>,
    {
        let d = crate::ActionKind::deserialize(deserializer)?;
        Ok(d as i32)
    }
}

tonic::include_proto!("buck.data");

impl CallerKind {
    /// Stable lowercase label for caller telemetry.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Agent => "agent",
            Self::CommandShell => "command_shell",
            Self::Other => "other",
            Self::Wrapper => "wrapper",
            Self::CommandWrapper => "command_wrapper",
        }
    }
}

impl AgentDetectionStopReason {
    /// Stable lowercase label for caller telemetry.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unspecified => "unspecified",
            Self::Agent => "agent",
            Self::Other => "other",
            Self::Unknown => "unknown",
            Self::NoParent => "no_parent",
            Self::ClientUnavailable => "client_unavailable",
            Self::ParentUnavailable => "parent_unavailable",
            Self::NewerParent => "newer_parent",
            Self::Cycle => "cycle",
            Self::DepthLimit => "depth_limit",
        }
    }
}

// Millions of these can be queued. Keep rare large payloads boxed so they
// do not increase the allocation size of every common event.
mini_vec::size_assert::words_of_expr!(BuckEvent::default(), ~40);

pub mod error {
    tonic::include_proto!("buck.data.error");
}

// Preserve the event producers' `Into<Data>` conversions when a large variant is
// boxed. The generated derives also provide conversions from the boxed payloads.
macro_rules! impl_from_boxed_event {
    ($data:ident, $($variant:ident($event:ident)),+ $(,)?) => {
        $(
            impl From<$event> for $data::Data {
                fn from(event: $event) -> Self {
                    Self::$variant(Box::new(event))
                }
            }
        )+
    };
}

impl_from_boxed_event!(
    span_end_event,
    DepFileUpload(DepFileUploadEnd),
    TestDiscovery(TestDiscoveryEnd),
    TestRun(TestRunEnd),
);
impl_from_boxed_event!(
    instant_event,
    ActionError(ActionError),
    CleanStaleResult(CleanStaleResult),
    PageOutSummary(PageOutSummary),
    PagingSummary(PagingSummary),
    RageResult(RageResult),
    ResourceControlEvent(ResourceControlEvent),
);

/// Extract action digest from a list of command executions.
/// Returns the action digest from the last command execution if available.
pub fn get_action_digest(commands: &[CommandExecution]) -> Option<String> {
    if let Some(command_execution) = commands.last() {
        if let Some(details) = &command_execution.details {
            if let Some(command_kind) = &details.command_kind {
                if let Some(command) = &command_kind.command {
                    return match command {
                        command_execution_kind::Command::RemoteCommand(remote_command) => {
                            Some(remote_command.action_digest.to_owned())
                        }
                        command_execution_kind::Command::LocalCommand(local_command) => {
                            Some(local_command.action_digest.to_owned())
                        }
                        command_execution_kind::Command::WorkerCommand(worker_command) => {
                            Some(worker_command.action_digest.to_owned())
                        }
                        command_execution_kind::Command::OmittedLocalCommand(
                            omitted_local_command,
                        ) => Some(omitted_local_command.action_digest.to_owned()),
                        _ => None,
                    };
                }
            }
        }
    }
    None
}

/// Trait for things that can be converted into protobuf messages, for ease of emitting events. There are many core Buck
/// types that are represented in the Daemon API that use this trait to ease conversion.
pub trait ToProtoMessage {
    type Message: prost::Message;

    fn as_proto(&self) -> Self::Message;
}

// Lives here rather than on `MiniperfCounter` because the orphan rule allows
// only these two crates, and `buck2_miniperf_proto` deliberately has no
// `buck2_data` dep; see `buck2_miniperf_proto/BUCK`.
impl From<MiniperfCounter> for CpuCounter {
    fn from(counter: MiniperfCounter) -> Self {
        CpuCounter {
            count: counter.count,
            time_enabled: counter.time_enabled,
            time_running: counter.time_running,
        }
    }
}

impl fmt::Display for DaemonShutdown {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}, caller:", self.reason)?;

        for caller in self.callers.iter() {
            let max_len = 200;

            let short_caller = if caller.len() > max_len {
                Cow::Owned(
                    caller
                        .chars()
                        .take(max_len)
                        .chain(std::iter::repeat_n('.', 3))
                        .collect(),
                )
            } else {
                Cow::Borrowed(caller)
            };

            writeln!(f)?;
            write!(f, "  * {short_caller}")?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_counter_from_miniperf_counter() {
        let counter = CpuCounter::from(MiniperfCounter {
            count: 123,
            time_enabled: 100,
            time_running: 50,
        });

        assert_eq!(counter.count, 123);
        assert_eq!(counter.time_enabled, 100);
        assert_eq!(counter.time_running, 50);
    }
}
