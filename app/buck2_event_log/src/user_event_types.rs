/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use buck2_common::convert::ProstDurationExt;
use buck2_data::ActionExecutionKind;
use buck2_data::ActionKind;
use buck2_data::ActionName;
use buck2_data::BuckEvent;
use buck2_data::StarlarkUserEvent;
use buck2_event_observer::display::TargetDisplayOptions;
use buck2_event_observer::display::display_action_owner;
use serde::Deserialize;
use serde::Serialize;

use crate::stream_value::StreamValue;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = InvalidEvent)]
pub(crate) enum SerializeUserEventError {
    #[error("Internal error: Missing `data` in `{0}`")]
    MissingData(String),
    #[error("Internal error: Missing `timestamp` in `BuckEvent`")]
    MissingTimestamp,
    #[error(
        "Internal error: Missing `input_materialization_duration` in `CommandExecutionMetadata`"
    )]
    MissingInputMaterializationDuration,
    #[error("Internal error: Missing `name` in `ActionExecutionEnd`")]
    MissingName,
    #[error("Internal error: `ActionKey` is malformed in `ActionExecutionEnd`")]
    MalformedActionKey,
}

/// Wrapper around StarlarkUserEvent so we discard the rest of BuckEvent fields.
#[derive(Serialize, Deserialize, Allocative, Clone)]
pub struct UserEvent {
    #[serde(flatten)]
    data: UserEventData,
    epoch_millis: u64,
}

/// BuckEvent types that are allowed in the user event log.
#[derive(Serialize, Deserialize, Allocative, Clone)]
pub enum UserEventData {
    StarlarkUserEvent(StarlarkUserEvent),
    ActionExecutionEvent(ActionExecutionEndSimple),
    BxlEnsureArtifactsEvent(BxlEnsureArtifactsEvent),
}

/// Simplified version of ActionExecutionEnd.
#[derive(Serialize, Deserialize, Allocative, Clone)]
pub struct ActionExecutionEndSimple {
    kind: ActionKind,
    name: ActionName,
    duration_millis: u64,
    output_size: u64,
    input_materialization_duration_millis: u64,
    execution_kind: ActionExecutionKind,
    owner: String,
}

#[derive(Serialize, Deserialize, Allocative, Clone)]
pub struct BxlEnsureArtifactsEvent {
    duration_millis: u64,
}

pub fn try_get_user_event_for_read(
    stream_value: &StreamValue,
) -> buck2_error::Result<Option<UserEvent>> {
    match stream_value {
        StreamValue::Event(buck_event) => try_get_user_event(buck_event.as_ref()),
        _ => Ok(None),
    }
}

pub(crate) fn try_get_user_event(buck_event: &BuckEvent) -> buck2_error::Result<Option<UserEvent>> {
    let timestamp = buck_event
        .timestamp
        .as_ref()
        .ok_or(SerializeUserEventError::MissingTimestamp)?;
    let epoch_millis = timestamp.seconds as u64 * 1000 + timestamp.nanos as u64 / 1_000_000;

    match buck_event
        .data
        .as_ref()
        .ok_or_else(|| SerializeUserEventError::MissingData("BuckEvent".to_owned()))?
    {
        buck2_data::buck_event::Data::Instant(instant) => {
            match instant
                .data
                .as_ref()
                .ok_or_else(|| SerializeUserEventError::MissingData("InstantEvent".to_owned()))?
            {
                buck2_data::instant_event::Data::StarlarkUserEvent(event) => Ok(Some(UserEvent {
                    data: UserEventData::StarlarkUserEvent(event.clone()),
                    epoch_millis,
                })),
                _ => Ok(None),
            }
        }
        buck2_data::buck_event::Data::SpanEnd(span_end_event) => {
            let duration_millis = span_end_event
                .duration
                .as_ref()
                .ok_or(SerializeUserEventError::MissingTimestamp)?
                .try_into_duration()?
                .as_millis() as u64;

            match span_end_event
                .data
                .as_ref()
                .ok_or_else(|| SerializeUserEventError::MissingData("SpanEndEvent".to_owned()))?
            {
                buck2_data::span_end_event::Data::ActionExecution(action_execution) => {
                    let mut input_materialization_duration_millis = 0;

                    // Take the last command report's input materialization duration
                    if let Some(command) = action_execution.commands.last() {
                        if let Some(details) = &command.details {
                            input_materialization_duration_millis = details
                                .metadata
                                .as_ref()
                                .ok_or(
                                    SerializeUserEventError::MissingInputMaterializationDuration,
                                )?
                                .input_materialization_duration
                                .as_ref()
                                .ok_or(
                                    SerializeUserEventError::MissingInputMaterializationDuration,
                                )?
                                .try_into_duration()?
                                .as_millis()
                                as u64;
                        }
                    }

                    let owner = action_execution
                        .key
                        .as_ref()
                        .ok_or(SerializeUserEventError::MalformedActionKey)?
                        .owner
                        .as_ref()
                        .ok_or(SerializeUserEventError::MalformedActionKey)?;

                    // Let's just show the unconfigured label for simplicity
                    let owner = display_action_owner(owner, TargetDisplayOptions::for_log())?;

                    let action_event = ActionExecutionEndSimple {
                        kind: action_execution.kind(),
                        name: action_execution
                            .name
                            .as_ref()
                            .ok_or(SerializeUserEventError::MissingName)?
                            .clone(),
                        duration_millis,
                        output_size: action_execution.output_size,
                        input_materialization_duration_millis,
                        execution_kind: action_execution.execution_kind(),
                        owner,
                    };

                    Ok(Some(UserEvent {
                        data: UserEventData::ActionExecutionEvent(action_event),
                        epoch_millis,
                    }))
                }
                buck2_data::span_end_event::Data::BxlEnsureArtifacts(_) => {
                    let bxl_ensure_artifacts = BxlEnsureArtifactsEvent { duration_millis };

                    Ok(Some(UserEvent {
                        data: UserEventData::BxlEnsureArtifactsEvent(bxl_ensure_artifacts),
                        epoch_millis,
                    }))
                }
                _ => Ok(None),
            }
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use buck2_data::StarlarkUserEvent;
    use buck2_data::StarlarkUserMetadataDictValue;
    use buck2_data::StarlarkUserMetadataListValue;
    use buck2_data::StarlarkUserMetadataValue;
    use buck2_data::starlark_user_metadata_value::Value;
    use buck2_hash::IntentionallyStdHashMap;

    #[test]
    fn test_serialize_starlark_user_event() {
        let val1 = StarlarkUserMetadataValue {
            value: Some(Value::BoolValue(true)),
        };

        let mut metadata = IntentionallyStdHashMap::new();
        metadata.insert("bool_value".to_owned(), val1);

        let starlark_user_event = StarlarkUserEvent {
            id: "foo".to_owned(),
            metadata,
        };

        let serialized = serde_json::to_string_pretty(&starlark_user_event).unwrap();
        let expected = r#"{
  "id": "foo",
  "metadata": {
    "bool_value": true
  }
}"#;
        assert_eq!(expected, serialized);
    }

    #[test]
    fn test_serialize_starlark_user_event_list() {
        let list_val1 = StarlarkUserMetadataValue {
            value: Some(Value::StringValue("a".to_owned())),
        };
        let list_val2 = StarlarkUserMetadataValue {
            value: Some(Value::StringValue("b".to_owned())),
        };
        let list_val3 = StarlarkUserMetadataValue {
            value: Some(Value::StringValue("c".to_owned())),
        };
        let list = StarlarkUserMetadataListValue {
            value: vec![list_val1, list_val2, list_val3],
        };
        let val = StarlarkUserMetadataValue {
            value: Some(Value::ListValue(list)),
        };

        let mut metadata = IntentionallyStdHashMap::new();
        metadata.insert("list_value".to_owned(), val);

        let starlark_user_event = StarlarkUserEvent {
            id: "foo".to_owned(),
            metadata,
        };

        let serialized = serde_json::to_string_pretty(&starlark_user_event).unwrap();
        let expected = r#"{
  "id": "foo",
  "metadata": {
    "list_value": [
      "a",
      "b",
      "c"
    ]
  }
}"#;
        assert_eq!(expected, serialized);
    }

    #[test]
    fn test_serialize_starlark_user_event_dict() {
        let bar = StarlarkUserMetadataValue {
            value: Some(Value::StringValue("bar".to_owned())),
        };
        let dict = StarlarkUserMetadataDictValue {
            value: IntentionallyStdHashMap::from([("foo".to_owned(), bar)]),
        };
        let val = StarlarkUserMetadataValue {
            value: Some(Value::DictValue(dict)),
        };

        let mut metadata = IntentionallyStdHashMap::new();
        metadata.insert("dict_value".to_owned(), val);

        let starlark_user_event = StarlarkUserEvent {
            id: "foo".to_owned(),
            metadata,
        };

        let serialized = serde_json::to_string_pretty(&starlark_user_event).unwrap();
        let expected = r#"{
  "id": "foo",
  "metadata": {
    "dict_value": {
      "foo": "bar"
    }
  }
}"#;
        assert_eq!(expected, serialized);
    }
}
