/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Paging support for [`Error`](crate::Error).
//!
//! An error pages out as its [`ErrorReport`], rendered contexts, and optional action details.
//! Contexts and emitted summaries are restored as strings, preserving the root message
//! and error rendering.

use std::sync::Arc;

use buck2_data::ActionError;
use buck2_data::ErrorReport;
use pagable::PagableDeserialize;
use pagable::PagableDeserializer;
use pagable::PagableSerialize;
use pagable::PagableSerializer;
use prost::Message;

use crate::format::into_anyhow_for_format;

impl PagableSerialize for crate::Error {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> pagable::Result<()> {
        let mut report = ErrorReport::from(self);
        let emitted = report
            .telemetry_message
            .is_some()
            .then(|| report.message.clone());
        report.message = self.root_message().to_owned();
        let mut contexts: Vec<String> = into_anyhow_for_format(self, false)
            .0
            .chain()
            .map(ToString::to_string)
            .collect();
        contexts.pop();
        report.encode_to_vec().pagable_serialize(serializer)?;
        contexts.pagable_serialize(serializer)?;
        self.action_error()
            .map(Message::encode_to_vec)
            .pagable_serialize(serializer)?;
        emitted.pagable_serialize(serializer)
    }
}

impl<'de> PagableDeserialize<'de> for crate::Error {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        let bytes = Vec::<u8>::pagable_deserialize(deserializer)?;
        let report = ErrorReport::decode(bytes.as_slice())?;
        let contexts = Vec::<String>::pagable_deserialize(deserializer)?;
        let action = Option::<Vec<u8>>::pagable_deserialize(deserializer)?
            .map(|bytes| ActionError::decode(bytes.as_slice()))
            .transpose()?;
        let emitted = Option::<String>::pagable_deserialize(deserializer)?;
        let mut error = Self::from_error_report(report, action);
        for context in contexts.into_iter().rev() {
            error = error.context(context);
        }
        if let Some(message) = emitted {
            error = error.mark_emitted(Arc::new(move |f| f.write_str(&message)));
        }
        Ok(error)
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use pagable::PagableDeserialize;
    use pagable::PagableSerialize;

    pub(crate) fn round_trip(error: &crate::Error) -> crate::Error {
        let mut serializer = pagable::testing::TestingSerializer::new();
        error.pagable_serialize(&mut serializer).unwrap();
        let bytes = serializer.finish();
        let mut deserializer = pagable::testing::TestingDeserializer::new(&bytes);
        crate::Error::pagable_deserialize(&mut deserializer).unwrap()
    }

    /// The `Debug` rendering without the backtrace `anyhow` appends under
    /// `RUST_BACKTRACE`, which is captured where the error is formatted and so
    /// never compares equal between two renderings.
    pub(crate) fn rendering(error: &crate::Error) -> String {
        let rendered = format!("{error:?}");
        match rendered.split_once("\n\nStack backtrace:") {
            Some((message, _backtrace)) => message.to_owned(),
            None => rendered,
        }
    }

    /// Everything a paged-in error keeps; see the module doc.
    pub(crate) fn assert_same_error(restored: &crate::Error, original: &crate::Error) {
        assert_eq!(rendering(restored), rendering(original));
        assert_eq!(restored.tags(), original.tags());
        assert_eq!(restored.string_tags(), original.string_tags());
        assert_eq!(restored.source_location(), original.source_location());
        assert_eq!(restored.category_key(), original.category_key());
        assert_eq!(restored.to_string(), original.to_string());
        assert_eq!(restored.root_message(), original.root_message());
        assert_eq!(restored.action_error(), original.action_error());
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use buck2_data::ActionError;
    use buck2_data::CommandExecution;
    use buck2_data::CommandExecutionDetails;
    use buck2_data::ErrorReport;

    use super::testing::assert_same_error;
    use super::testing::round_trip;
    use crate::ErrorTag;
    use crate::context_value::StarlarkContext;
    use crate::source_location::SourceLocation;

    #[test]
    fn emitted_error_round_trips() {
        let original = crate::buck2_error!(ErrorTag::Input, "full action diagnostic")
            .context("inside the action")
            .mark_emitted(Arc::new(|f| f.write_str("Failed to build 'cell//pkg:t'")))
            .context("while evaluating BXL");
        let restored = round_trip(&original);
        assert_same_error(&restored, &original);
        assert!(restored.is_emitted().is_some());

        let expected = ErrorReport::from(&original);
        let actual = ErrorReport::from(&restored);
        assert_eq!(
            expected.message.split("\n\nStack backtrace:").next(),
            actual.message.split("\n\nStack backtrace:").next(),
        );
        assert!(!actual.message.contains("full action diagnostic"));
        assert!(
            actual
                .telemetry_message
                .expect("emitted error telemetry")
                .contains("full action diagnostic")
        );

        let propagated = ErrorReport::from(&restored.context("additional context"));
        assert!(propagated.message.contains("additional context"));
        assert!(propagated.message.contains("Failed to build 'cell//pkg:t'"));
        assert!(!propagated.message.contains("full action diagnostic"));
    }

    #[test]
    fn action_error_round_trips() {
        let original = crate::Error::new(
            "command failed".to_owned(),
            ErrorTag::Input,
            SourceLocation::new(file!(), line!()),
            Some(ActionError {
                last_command: Some(CommandExecution {
                    details: Some(CommandExecutionDetails {
                        cmd_stdout: "action stdout".to_owned(),
                        cmd_stderr: "action stderr".to_owned(),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
        )
        .context("while analyzing a dependency");
        assert_same_error(&round_trip(&original), &original);
    }

    #[test]
    fn context_chain_round_trips() {
        let original = crate::buck2_error!(ErrorTag::Analysis, "analysis of `cell//pkg:t` failed")
            .context("while resolving a lazy operation")
            .string_tag("inner_string_tag")
            .tag([ErrorTag::Bxl])
            .context("while evaluating `main.bxl`")
            .string_tag("outer_string_tag");

        assert_same_error(&round_trip(&original), &original);
    }

    #[test]
    fn starlark_backtrace_round_trips() {
        let original = crate::buck2_error!(ErrorTag::StarlarkFail, "fail() was called")
            .context_for_starlark_backtrace(StarlarkContext {
                call_stack: "Traceback (most recent call last):\n  * main.bxl:3, in _impl\n"
                    .to_owned(),
                error_msg: "fail() was called".to_owned(),
                span: None,
            })
            .context("while evaluating `main.bxl`");

        assert_same_error(&round_trip(&original), &original);
    }
}
