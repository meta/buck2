/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Implementation of the cli and query_* attr query language.

use buck2_core::fs::project::ProjectRoot;
use buck2_query_parser::spanned::Spanned;

/// While this is a std Error type, we generally don't use it directly. It's instead wrapped in a Spanned and can be converted to a normal error
/// with QueryError::convert_error (which will resolve the spans to context messages).
#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
pub enum QueryError {
    #[error("unknown function `{0}`")]
    UnknownFunction(String),
    #[error("binary op `{0}` unsupported in this context")]
    UnsupportedBinaryOp(String),
    #[error("expected a literal, got value of type `{actual}`")]
    ExpectedLiteral { actual: &'static str },
    #[error("expected value of type `{expected}`, got `{actual}`")]
    InvalidType {
        expected: &'static str,
        actual: &'static str,
    },
    #[error("too many args. function `{function}` accepts maximum {max} args, got {actual}")]
    TooManyArgs {
        function: String,
        max: usize,
        actual: usize,
    },
    #[error(
        "too few args. function `{function}` requires at least {min} args, got {actual}. Missing at least arg `{next_arg_name}`."
    )]
    TooFewArgs {
        function: String,
        next_arg_name: String,
        min: usize,
        actual: usize,
    },
    #[error(
        "None is not a valid value for function `{function}` argument [{arg_idx}] `{arg_name}: {arg_type}`"
    )]
    NoneNotAccepted {
        function: String,
        arg_name: String,
        arg_idx: String,
        arg_type: String,
    },
    #[error("function `{0}` is not implemented yet")]
    FunctionUnimplemented(&'static str),
    #[error("Argument `{1}` to `{0}` is not yet supported in buck2")]
    ArgNotYetSupported(String, String),
    #[error("Invalid traversal depth `{0}`")]
    InvalidDepth(u32),
    #[error("File literal `{1}` not within the project root `{}`", .0)]
    FileLiteralNotInProject(ProjectRoot, String),
    #[error("query function {0} not available in this context")]
    NotAvailableInContext(&'static str),
    #[error(
        "Set operation requires either two set types, or one set and one string, got `{0}` and `{1}`"
    )]
    SetIncompatibleTypes(&'static str, &'static str),
    /// Used to propagate up an inner error. The inner span will mark where the inner error was (which itself may be the
    /// propagation of another error). This error will end up in a Spanned that indicates where this error (the propagation) occurs.
    /// Since QueryError has an impl for `From<Spanned<QueryError>>`, just propagating inner eval errors via `?` will hit this case (and
    /// so will actually end up not just propagating the inner error but wrapping it in this and adding an additional Spanned to it).
    #[error("{0}")]
    Inner(Box<Spanned<QueryError>>),
    /// Used to propagate an inner error. Generally this would happen when we are calling something outside the evaluation
    /// (ex. target parsing). That inner error won't have span information and so the innermost span information will be the one
    /// attached to this error.
    #[error("{0}")]
    Error(buck2_error::Error),
}

impl From<buck2_error::Error> for QueryError {
    fn from(err: buck2_error::Error) -> Self {
        QueryError::Error(err)
    }
}

impl From<Spanned<QueryError>> for QueryError {
    fn from(value: Spanned<QueryError>) -> Self {
        QueryError::Inner(Box::new(value))
    }
}

impl QueryError {
    pub fn drop_spans(err: Spanned<Self>) -> buck2_error::Error {
        match err.value {
            Self::Error(inner) => inner,
            Self::Inner(inner) => Self::drop_spans(*inner),
            e => {
                // TODO(cjhopman): This is going to drop the backtrace attached to the error, we should figure
                // out how to keep that.
                e.into()
            }
        }
    }
    pub fn convert_error(err: Spanned<Self>, input: &str) -> buck2_error::Error {
        let context = err.get_err_context(input);

        match err.value {
            Self::Error(inner) => inner.context(format!("Error evaluating expression:{context}")),
            Self::Inner(inner) => Self::convert_error(*inner, input)
                .context(format!("Error evaluating expression:{context}")),
            e => {
                // TODO(cjhopman): This is going to drop the backtrace attached to the error, we should figure
                // out how to keep that.
                buck2_error::buck2_error!(buck2_error::ErrorTag::Input, "{}:{}", e, context)
            }
        }
    }
}
