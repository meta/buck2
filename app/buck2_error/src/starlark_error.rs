/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Starlark::Error <-> buck2_error::Error conversion implementation.

use std::fmt;

use ref_cast::RefCast;

use crate::__for_macro::ContextValue;
use crate::any::from_any_with_tag_and_source_location;
use crate::context_value::StarlarkContext;
use crate::error::ErrorKind;
use crate::source_location::SourceLocation;

impl From<crate::Error> for starlark_syntax::Error {
    fn from(e: crate::Error) -> starlark_syntax::Error {
        let error = Into::into(StarlarkErrorWrapper(e));
        let error_kind = starlark_syntax::ErrorKind::Native(error);
        starlark_syntax::Error::new_kind(error_kind)
    }
}

/// Whether or not to mark a starlark error as an input/user error.
/// TODO(minglunli): This is used as an intermediate step so Native errors are categorized.
/// This is not 100% accurate and should be remove once Native errors are categorized properly
#[derive(PartialEq)]
pub enum NativeErrorHandling {
    InputError,
    Unknown,
}

impl From<starlark_syntax::Error> for crate::Error {
    #[cold]
    #[track_caller]
    fn from(e: starlark_syntax::Error) -> Self {
        from_starlark_impl(e, NativeErrorHandling::InputError, false)
    }
}

#[cold]
pub fn from_starlark_with_options(
    e: starlark_syntax::Error,
    error_handling: NativeErrorHandling,
    skip_stacktrace: bool,
) -> crate::Error {
    from_starlark_impl(e, error_handling, skip_stacktrace)
}

// If the top context is a string context, then it should be popped off as otherwise
// the error will be duplicated since the top level error is used to build the stacktrace.
fn error_with_starlark_context(
    e: &crate::Error,
    starlark_context: StarlarkContext,
) -> crate::Error {
    let mut context_stack: Vec<ContextValue> = Vec::new();
    let mut buck2_error = e.clone();

    loop {
        match buck2_error.0.as_ref() {
            ErrorKind::Root(root) => {
                let source_location = root.source_location().clone();
                let action_error = root.action_error().cloned();

                // We want to keep the metadata but want to change the error message
                let mut err = crate::Error::new(
                    format!("{starlark_context}"),
                    root.error_tag(),
                    source_location,
                    action_error,
                );

                for context in context_stack.into_iter().rev() {
                    err = err.context(context);
                }

                return err;
            }
            ErrorKind::WithContext(context_value, inner) => {
                match context_value {
                    ContextValue::Dyn(_) | ContextValue::Typed(_) => {
                        let mut inner_err = inner.clone();
                        for context in context_stack.into_iter().rev() {
                            inner_err = inner_err.context(context);
                        }

                        return inner_err
                            .clone()
                            .context_for_starlark_backtrace(starlark_context);
                    }
                    ContextValue::StarlarkError(_) => {
                        return buck2_error.context_for_starlark_backtrace(starlark_context);
                    }
                    ContextValue::Tags(_) | ContextValue::StringTag(_) => {
                        context_stack.push(context_value.clone())
                    }
                }

                buck2_error = inner.clone();
            }
            _ => return buck2_error,
        }
    }
}

fn from_starlark_impl(
    e: starlark_syntax::Error,
    error_handling: NativeErrorHandling,
    skip_stacktrace: bool,
) -> crate::Error {
    if let starlark_syntax::ErrorKind::Native(err) = e.kind() {
        if let Some(wrapper) = err.downcast_ref::<StarlarkErrorWrapper>() {
            if !e.has_diagnostic() {
                return wrapper.0.clone();
            }

            let starlark_context = StarlarkContext {
                call_stack: format!("{}", e.call_stack()),
                error_msg: format!("{}", e.without_diagnostic()),
                span: e.span().cloned(),
            };

            return error_with_starlark_context(&wrapper.0, starlark_context);
        }
    };

    let mut runtime_type_kind = None;
    let tag = match e.kind() {
        starlark_syntax::ErrorKind::Fail(_) => crate::ErrorTag::StarlarkFail,
        starlark_syntax::ErrorKind::StackOverflow(_) => crate::ErrorTag::StarlarkStackOverflow,
        starlark_syntax::ErrorKind::Value(_) => crate::ErrorTag::StarlarkValue,
        starlark_syntax::ErrorKind::Function(_) => crate::ErrorTag::StarlarkFunction,
        starlark_syntax::ErrorKind::RuntimeType(kind, _) => {
            runtime_type_kind = Some(*kind);
            match kind {
                starlark_syntax::RuntimeTypeErrorKind::FunctionCall => {
                    crate::ErrorTag::StarlarkFunction
                }
                starlark_syntax::RuntimeTypeErrorKind::Parameter
                | starlark_syntax::RuntimeTypeErrorKind::ValueUnpack
                | starlark_syntax::RuntimeTypeErrorKind::Operation
                | starlark_syntax::RuntimeTypeErrorKind::Attribute => {
                    crate::ErrorTag::StarlarkValue
                }
                starlark_syntax::RuntimeTypeErrorKind::Annotation => crate::ErrorTag::StarlarkError,
            }
        }
        starlark_syntax::ErrorKind::Scope(_) => crate::ErrorTag::StarlarkScope,
        starlark_syntax::ErrorKind::Parser(_) => crate::ErrorTag::StarlarkParser,
        starlark_syntax::ErrorKind::Internal(_) => crate::ErrorTag::StarlarkInternal,
        starlark_syntax::ErrorKind::DeferredRead(_) => crate::ErrorTag::DeferredFieldRead,
        starlark_syntax::ErrorKind::Native(_) | starlark_syntax::ErrorKind::Other(_)
            if error_handling == NativeErrorHandling::InputError =>
        {
            crate::ErrorTag::StarlarkNativeInput
        }
        _ => crate::ErrorTag::StarlarkError,
    };

    let variant_name = match e.kind() {
        starlark_syntax::ErrorKind::Fail(_) => "StarlarkError::Fail",
        starlark_syntax::ErrorKind::StackOverflow(_) => "StarlarkError::StackOverflow",
        starlark_syntax::ErrorKind::Internal(_) => "StarlarkError::Internal",
        starlark_syntax::ErrorKind::DeferredRead(_) => "StarlarkError::DeferredRead",
        starlark_syntax::ErrorKind::Value(_) => "StarlarkError::Value",
        starlark_syntax::ErrorKind::Function(_) => "StarlarkError::Function",
        starlark_syntax::ErrorKind::RuntimeType(_, _) => "StarlarkError::RuntimeType",
        starlark_syntax::ErrorKind::Scope(_) => "StarlarkError::Scope",
        starlark_syntax::ErrorKind::Parser(_) => "StarlarkError::Parser",
        starlark_syntax::ErrorKind::Native(_) => "StarlarkError::Native",
        _ => "StarlarkError",
    };
    let source_location =
        SourceLocation::new(std::file!(), std::line!()).with_type_name(variant_name);
    let description = if skip_stacktrace {
        format!("{}", e.without_diagnostic())
    } else {
        format!("{e}")
    };

    match e.into_kind() {
        starlark_syntax::ErrorKind::Fail(e)
        | starlark_syntax::ErrorKind::StackOverflow(e)
        | starlark_syntax::ErrorKind::Internal(e)
        | starlark_syntax::ErrorKind::Value(e)
        | starlark_syntax::ErrorKind::Function(e)
        | starlark_syntax::ErrorKind::RuntimeType(_, e)
        | starlark_syntax::ErrorKind::Scope(e)
        | starlark_syntax::ErrorKind::Parser(e)
        | starlark_syntax::ErrorKind::Other(e)
        | starlark_syntax::ErrorKind::Native(e) => {
            let error = BuckStarlarkError(e, description);

            let error = from_any_with_tag_and_source_location(&error, source_location, tag);
            match runtime_type_kind {
                Some(kind) => error
                    .tag([crate::ErrorTag::StarlarkRuntimeTypeError])
                    .string_tag(&format!("starlark_runtime_type_error={}", kind.as_ref())),
                None => error,
            }
        }
        _ => crate::Error::new(description, tag, source_location, None),
    }
}

pub(crate) struct BuckStarlarkError(pub(crate) anyhow::Error, String);

impl fmt::Debug for BuckStarlarkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.1)
    }
}

impl fmt::Display for BuckStarlarkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.1)
    }
}

impl std::error::Error for BuckStarlarkError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }

    fn provide<'a>(&'a self, request: &mut std::error::Request<'a>) {
        self.0.provide(request);
    }
}

#[derive(derive_more::Display, RefCast)]
#[repr(transparent)]
struct StarlarkErrorWrapper(crate::Error);

impl fmt::Debug for StarlarkErrorWrapper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl std::error::Error for StarlarkErrorWrapper {}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use allocative::Allocative;

    use crate as buck2_error;
    use crate::buck2_error;
    use crate::context_value::StarlarkContext;
    use crate::starlark_error::error_with_starlark_context;

    #[derive(buck2_error::Error, Debug, Allocative)]
    #[error("FullMetadataError")]
    #[buck2(tag = crate::ErrorTag::Tier0)]
    struct FullMetadataError;

    impl crate::TypedContext for FullMetadataError {
        fn eq(&self, _other: &dyn crate::TypedContext) -> bool {
            true
        }

        fn display(&self) -> Option<String> {
            Some("FullMetadataError".to_owned())
        }
    }

    #[test]
    fn test_roundtrip_starlark() {
        // Tests buck2_error->starlark->buck2_error
        let e = crate::Error::from(FullMetadataError).context("context 1");
        let e2: crate::Error = starlark_syntax::Error::from(e.clone()).into();
        crate::Error::check_equal(&e, &e2);
    }

    #[test]
    fn test_runtime_type_error_metadata() {
        let e: crate::Error = starlark_syntax::Error::new_runtime_type(
            starlark_syntax::RuntimeTypeErrorKind::Annotation,
            anyhow::anyhow!("runtime type mismatch"),
        )
        .into();

        assert!(e.tags().contains(&crate::ErrorTag::StarlarkError));
        assert!(
            e.tags()
                .contains(&crate::ErrorTag::StarlarkRuntimeTypeError)
        );
        assert!(
            e.category_key()
                .ends_with("starlark_runtime_type_error=ANNOTATION")
        );
    }

    #[test]
    fn test_pop_dyn_error_from_context() {
        let context_error = "Some error message that's getting popped";
        let context_key = "Some context key";
        let error_tag = crate::ErrorTag::IoWindowsSharingViolation;
        let starlark_context = StarlarkContext {
            call_stack: "Some call stack".to_owned(),
            error_msg: "Some error message".to_owned(),
            span: None,
        };

        let e = crate::Error::from(FullMetadataError);
        let e = e.context(context_error);
        let e = e.tag([error_tag]);
        let e = e.string_tag(context_key);

        let context_popped = error_with_starlark_context(&e, starlark_context);

        assert!(!context_popped.to_string().contains(context_error));
        assert!(context_popped.tags().contains(&error_tag));
        assert!(context_popped.category_key().ends_with(context_key));
    }

    #[test]
    fn test_pop_base_error_from_context() {
        let base_error = "Some base error";
        let starlark_error = "Starlark error message";
        let starlark_call_stack = "Some call stack";
        let starlark_context = StarlarkContext {
            call_stack: starlark_call_stack.to_owned(),
            error_msg: starlark_error.to_owned(),
            span: None,
        };

        let e = buck2_error!(crate::ErrorTag::StarlarkError, "{}", base_error);

        let base_replaced = error_with_starlark_context(&e, starlark_context);

        assert!(!base_replaced.to_string().contains(base_error));
        assert!(base_replaced.to_string().contains(starlark_call_stack));
        assert!(base_replaced.to_string().contains(starlark_error));
    }

    #[test]
    fn test_conversion_with_typed_context() {
        let base_error = "Some base error";
        let e = buck2_error!(crate::ErrorTag::StarlarkError, "{}", base_error);
        let e = e.compute_context(
            |_: Arc<FullMetadataError>| FullMetadataError,
            || FullMetadataError,
        );

        let starlark_call_stack = "Some call stack";
        let starlark_error_msg = "Starlark error message";
        let starlark_context = StarlarkContext {
            call_stack: starlark_call_stack.to_owned(),
            error_msg: starlark_error_msg.to_owned(),
            span: None,
        };

        let starlark_err = error_with_starlark_context(&e, starlark_context);
        let starlark_err_string = format!("{starlark_err:#}");

        assert!(starlark_err_string.contains(starlark_call_stack));
        assert!(starlark_err_string.contains(starlark_error_msg));
        // Root error shouldn't be lost
        assert!(starlark_err_string.contains(base_error));
    }
}
