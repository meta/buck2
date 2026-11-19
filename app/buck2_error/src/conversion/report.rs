/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_data::ActionError;
use buck2_data::ErrorReport;

use crate::ErrorTag;
use crate::context_value::ContextValue;
use crate::context_value::StringTag;
use crate::source_location::SourceLocation;

impl From<ErrorReport> for crate::Error {
    #[cold]
    #[track_caller]
    fn from(value: ErrorReport) -> Self {
        Self::from_error_report(value, None)
    }
}

impl crate::Error {
    #[cold]
    #[track_caller]
    pub(crate) fn from_error_report(value: ErrorReport, action: Option<ActionError>) -> Self {
        // TODO(ctolliday) convert remaining fields
        // (note: not using tags() as a workaround for likely false positive ASAN failure)
        let tags: Vec<ErrorTag> = value
            .tags
            .into_iter()
            .filter_map(|v| ErrorTag::try_from(v).ok())
            .collect();

        let mut error = crate::Error::new(
            value.message,
            *tags.first().unwrap_or(&ErrorTag::InvalidErrorReport),
            value
                .source_location
                .map_or(SourceLocation::new(file!(), line!()), |s| s.into()),
            action,
        )
        .tag(tags);

        for tag in value.string_tags {
            let tag = ContextValue::StringTag(StringTag { tag: tag.tag });
            error = error.context(tag);
        }
        error
    }
}

impl From<&crate::Error> for ErrorReport {
    fn from(err: &crate::Error) -> Self {
        let (message, telemetry_message) = if let Some(f) = err.is_emitted() {
            (format!("{f:?}"), Some(format!("{err:?}")))
        } else {
            (format!("{err:?}"), None)
        };

        let category_key = err.category_key();

        let sub_error_categories = if let Some(error_diagnostics) = err
            .action_error()
            .and_then(|e| e.error_diagnostics.as_ref())
        {
            if let Some(buck2_data::action_error_diagnostics::Data::SubErrors(sub_errors)) =
                &error_diagnostics.data
            {
                sub_errors
                    .sub_errors
                    .iter()
                    .map(|s| s.category.clone())
                    .collect::<Vec<_>>()
            } else {
                vec![]
            }
        } else {
            vec![]
        };
        let string_tags = err
            .iter_context()
            .filter_map(|kind| match kind {
                ContextValue::StringTag(val) => Some(buck2_data::error_report::StringTag {
                    tag: val.tag.clone(),
                }),
                _ => None,
            })
            .collect();

        buck2_data::ErrorReport {
            message,
            telemetry_message,
            source_location: Some(err.source_location().clone().into()),
            tags: err.tags().iter().map(|t| *t as i32).collect(),
            string_tags,
            sub_error_categories,
            category_key: Some(category_key),
        }
    }
}
