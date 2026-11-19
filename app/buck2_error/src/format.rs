/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::error::Error as StdError;
use std::fmt;
use std::sync::Arc;

use dupe::Dupe;

use crate::__for_macro::ContextValue;
use crate::DynLateFormat;
use crate::context_value::StarlarkContext;
use crate::error::ErrorKind;

/// We currently implement formatting in the laziest way possible - we convert to an equivalent
/// `anyhow::Error` and format that.
///
/// In the long term, this is not what we want to do. Writing our own error formatter is not that
/// hard and will give us a huge amount of flexibility. However, the goal right now is to get large
/// amounts of `anyhow` compatibility with minimal work, and this achieves that.
///
/// If `should_late_format` is set, attempts to use the late formatter instead of the standard one.
/// That might not be present, so additionally returns a bool which indicates whether the late
/// formatter was used.
pub(crate) fn into_anyhow_for_format(
    mut error: &crate::Error,
    should_late_format: bool,
) -> (anyhow::Error, bool) {
    let mut context_stack = Vec::new();
    let mut was_late_formatted = false;

    let base = loop {
        match error.0.as_ref() {
            ErrorKind::Root(root) => {
                break AnyhowWrapperForFormat::Root(root.description().to_owned());
            }
            ErrorKind::WithContext(context, inner) => {
                context_stack.push(context);
                error = inner;
            }
            ErrorKind::Emitted(late_format, inner) => {
                if should_late_format {
                    was_late_formatted = true;
                    break AnyhowWrapperForFormat::LateFormat(late_format.dupe());
                }
                error = inner;
            }
        }
    };

    let mut starlark_error: Option<StarlarkContext> = None;
    let mut out: anyhow::Error = base.into();
    for context in context_stack.into_iter().rev() {
        if let ContextValue::StarlarkError(ctx) = context {
            // Because context_stack is reversed, the right ordering would be first error last to preserve stack ordering
            starlark_error = Some(ctx.concat(starlark_error));
            continue;
        }
        if let Some(ctx) = starlark_error {
            out = out.context(format!("{ctx}"));
            starlark_error = None;
        }
        if let Some(s) = context.display() {
            out = out.context(s);
        }
    }

    if let Some(ctx) = starlark_error {
        out = out.context(format!("{ctx}"));
    }
    (out, was_late_formatted)
}

// Keep 3 variables
// backtrace string which just continuously concatenates
// error message which is the first error message since stack is reversed so first error is the right one
// span which is the same as error message

impl fmt::Debug for crate::Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&into_anyhow_for_format(self, false).0, f)
    }
}

impl fmt::Display for crate::Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&into_anyhow_for_format(self, false).0, f)
    }
}

enum AnyhowWrapperForFormat {
    Root(String),
    LateFormat(Arc<DynLateFormat>),
}

impl fmt::Debug for AnyhowWrapperForFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Root(root) => fmt::Display::fmt(root, f),
            Self::LateFormat(late_format) => late_format(f),
        }
    }
}

impl fmt::Display for AnyhowWrapperForFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Root(root) => fmt::Display::fmt(root, f),
            Self::LateFormat(late_format) => late_format(f),
        }
    }
}

impl StdError for AnyhowWrapperForFormat {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        None
    }
}
