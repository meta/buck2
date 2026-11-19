/*
 * Copyright 2019 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use std::fmt;
use std::mem;

use crate::call_stack::CallStack;
use crate::codemap::CodeMap;
use crate::codemap::FileSpan;
use crate::codemap::Span;
use crate::diagnostic::WithDiagnostic;
use crate::diagnostic::diagnostic_display;

/// An error produced by starlark.
///
/// This error is composed of an error kind, together with some diagnostic information indicating
/// where it occurred.
///
/// In order to prevent accidental conversions to `anyhow::Error`, this type intentionally does not
/// implement `std::error::Error`. That should probably change in the future.
pub struct Error(pub(crate) WithDiagnostic<ErrorKind>);

const _: () = assert!(mem::size_of::<Error>() == mem::size_of::<usize>());

impl Error {
    /// Create a new error
    #[cold]
    pub fn new_kind(kind: ErrorKind) -> Self {
        Self(WithDiagnostic::new_empty(kind))
    }

    /// Create a new error with a span
    #[cold]
    pub fn new_spanned(kind: ErrorKind, span: Span, codemap: &CodeMap) -> Self {
        Self(WithDiagnostic::new_spanned(kind, span, codemap))
    }

    /// Create a new error with no diagnostic and of kind [`ErrorKind::Other`]
    #[cold]
    pub fn new_other(e: impl Into<anyhow::Error>) -> Self {
        Self(WithDiagnostic::new_empty(ErrorKind::Other(e.into())))
    }

    /// Create a new error with no diagnostic and of kind [`ErrorKind::Native`]
    #[cold]
    pub fn new_native(e: impl Into<anyhow::Error>) -> Self {
        Self(WithDiagnostic::new_empty(ErrorKind::Native(e.into())))
    }

    /// Create a new error with no diagnostic and of kind [`ErrorKind::Value`]
    #[cold]
    pub fn new_value(e: impl Into<anyhow::Error>) -> Self {
        Self(WithDiagnostic::new_empty(ErrorKind::Value(e.into())))
    }

    /// Create a new runtime type error with no diagnostic.
    #[cold]
    pub fn new_runtime_type(kind: RuntimeTypeErrorKind, e: impl Into<anyhow::Error>) -> Self {
        Self(WithDiagnostic::new_empty(ErrorKind::RuntimeType(
            kind,
            e.into(),
        )))
    }

    /// The kind of this error
    pub fn kind(&self) -> &ErrorKind {
        self.0.inner()
    }

    /// The underlying error, without the kind or the diagnostic.
    pub fn inner(&self) -> &anyhow::Error {
        self.kind().inner()
    }

    /// Convert the error into the underlying kind
    pub fn into_kind(self) -> ErrorKind {
        self.0.into_inner()
    }

    pub fn has_diagnostic(&self) -> bool {
        self.0.span().is_some() || !self.0.call_stack().is_empty()
    }

    /// Convert this error into an `anyhow::Error`
    #[cold]
    pub fn into_anyhow(self) -> anyhow::Error {
        struct Wrapped(Error);

        impl fmt::Display for Wrapped {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl fmt::Debug for Wrapped {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Debug::fmt(&self.0, f)
            }
        }

        impl std::error::Error for Wrapped {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                self.0.kind().source()
            }
        }

        anyhow::Error::new(Wrapped(self))
    }

    /// Returns a value that can be used to format this error without including the diagnostic
    /// information
    ///
    /// This is the same as [`kind`](crate::Error::kind), just a bit more explicit.
    pub fn without_diagnostic<'a>(&'a self) -> impl fmt::Debug + fmt::Display + 'a {
        self.0.inner()
    }

    pub fn span(&self) -> Option<&FileSpan> {
        self.0.span()
    }

    pub fn call_stack(&self) -> &CallStack {
        self.0.call_stack()
    }

    /// Set the span, unless it's already been set.
    pub fn set_span(&mut self, span: Span, codemap: &CodeMap) {
        self.0.set_span(span, codemap);
    }

    /// Set the `call_stack` field, unless it's already been set.
    pub fn set_call_stack(&mut self, call_stack: impl FnOnce() -> CallStack) {
        self.0.set_call_stack(call_stack);
    }

    /// Wrap this error with additional context (anyhow-style).
    /// Preserves the existing `Diagnostic` (span and call stack).
    pub fn with_context(self, context: String) -> Self {
        let (kind, diagnostic) = self.0.into_parts();
        Self(WithDiagnostic::new_from_parts(
            kind.with_context(context),
            diagnostic,
        ))
    }

    /// Print an error to the stderr stream. If the error has diagnostic information it will use
    /// color-codes when printing.
    ///
    /// Note that this function doesn't print any context information if the error is a diagnostic,
    /// so you might prefer to use `eprintln!("{:#}"), err)` if you suspect there is useful context
    /// (although you won't get pretty colors).
    pub fn eprint(&self) {
        if self.has_diagnostic() {
            let mut stderr = String::new();
            diagnostic_display(&self.0, true, &mut stderr, true).unwrap();
            eprint!("{stderr}");
        } else {
            eprintln!("{self:#}")
        }
    }

    /// Change error kind to internal error.
    pub fn into_internal_error(self) -> Error {
        if let ErrorKind::Internal(_) = self.kind() {
            self
        } else {
            Error(self.0.map(ErrorKind::into_internal_error))
        }
    }

    /// Change error kind to deferred read error.
    pub fn into_deferred_read_error(self) -> Error {
        if let ErrorKind::DeferredRead(_) = self.kind() {
            self
        } else {
            Error(self.0.map(ErrorKind::into_deferred_read_error))
        }
    }
}

fn fmt_impl(this: &Error, is_debug: bool, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    if this.has_diagnostic() {
        // Not showing the context trace without `{:#}` or `{:?}` is the same thing that anyhow does
        let with_context = (f.alternate() || is_debug) && this.kind().source().is_some();
        diagnostic_display(&this.0, false, f, with_context)
    } else {
        fmt::Display::fmt(&this.without_diagnostic(), f)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_impl(self, false, f)
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_impl(self, true, f)
    }
}

/// Stable classification of runtime type errors.
///
/// These categories are used by Buck2 telemetry. Variants should only be added or renamed with
/// consideration for existing telemetry queries.
#[derive(Debug, Clone, Copy, Eq, PartialEq, strum::AsRefStr)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
pub enum RuntimeTypeErrorKind {
    /// The arguments supplied to a function do not match its signature.
    FunctionCall,
    /// A native function parameter could not be unpacked to its declared type.
    Parameter,
    /// A Starlark value could not be unpacked to the requested type.
    ValueUnpack,
    /// An operation is not supported for the supplied operand types.
    Operation,
    /// A value does not expose the requested attribute.
    Attribute,
    /// A value does not match a function argument or return type annotation.
    Annotation,
}

/// The different kinds of errors that can be produced by starlark
#[non_exhaustive]
pub enum ErrorKind {
    /// An explicit `fail` invocation
    Fail(anyhow::Error),
    /// Starlark call stack overflow.
    StackOverflow(anyhow::Error),
    /// An error approximately associated with a value.
    ///
    /// Includes unsupported operations, missing attributes, things of that sort.
    Value(anyhow::Error),
    /// Errors relating to the way a function is called (wrong number of args, etc.)
    Function(anyhow::Error),
    /// A runtime type error, with a stable category for telemetry.
    RuntimeType(RuntimeTypeErrorKind, anyhow::Error),
    /// Out of scope variables and similar
    Scope(anyhow::Error),
    /// Syntax error.
    Parser(anyhow::Error),
    /// Freeze errors. Should have no metadata attached
    Freeze(anyhow::Error),
    /// Indicates a logic bug in starlark
    Internal(anyhow::Error),
    /// A field of a value restored from storage could not be read back.
    /// Not caused by the Starlark code being evaluated.
    DeferredRead(anyhow::Error),
    /// Error from user provided native function
    /// (but not from native functions provided by starlark crate).
    /// When a native function declares `anyhow::Result<_>`
    /// return type, it is automatically converted to this variant.
    Native(anyhow::Error),
    /// Fallback option
    ///
    /// For errors produced by starlark which have not yet been assigned their own kind
    Other(anyhow::Error),
}

impl ErrorKind {
    /// The underlying error, whatever the kind.
    pub fn inner(&self) -> &anyhow::Error {
        match self {
            Self::Fail(e)
            | Self::StackOverflow(e)
            | Self::Value(e)
            | Self::Function(e)
            | Self::Scope(e)
            | Self::Parser(e)
            | Self::Freeze(e)
            | Self::Internal(e)
            | Self::RuntimeType(_, e)
            | Self::DeferredRead(e)
            | Self::Native(e)
            | Self::Other(e) => e,
        }
    }

    /// The source of the error, akin to `[std::error::Error::source]`
    pub fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Fail(_) => None,
            Self::StackOverflow(_) => None,
            Self::Value(_) => None,
            Self::Function(_) => None,
            Self::RuntimeType(_, _) => None,
            Self::Scope(_) => None,
            Self::Freeze(_) => None,
            Self::Parser(_) => None,
            Self::Internal(_) => None,
            Self::DeferredRead(e) => e.source(),
            Self::Native(e) => e.source(),
            Self::Other(e) => e.source(),
        }
    }

    /// Add an `anyhow`-style context layer to the underlying error, preserving
    /// the variant.
    pub fn with_context(self, context: String) -> Self {
        match self {
            Self::Fail(e) => Self::Fail(e.context(context)),
            Self::StackOverflow(e) => Self::StackOverflow(e.context(context)),
            Self::Value(e) => Self::Value(e.context(context)),
            Self::Function(e) => Self::Function(e.context(context)),
            Self::RuntimeType(kind, e) => Self::RuntimeType(kind, e.context(context)),
            Self::Scope(e) => Self::Scope(e.context(context)),
            Self::Freeze(e) => Self::Freeze(e.context(context)),
            Self::Parser(e) => Self::Parser(e.context(context)),
            Self::Internal(e) => Self::Internal(e.context(context)),
            Self::DeferredRead(e) => Self::DeferredRead(e.context(context)),
            Self::Native(e) => Self::Native(e.context(context)),
            Self::Other(e) => Self::Other(e.context(context)),
        }
    }

    /// Change type to `Internal`.
    pub(crate) fn into_internal_error(self) -> ErrorKind {
        match self {
            ErrorKind::Internal(e)
            | ErrorKind::Fail(e)
            | ErrorKind::Value(e)
            | ErrorKind::Function(e)
            | ErrorKind::RuntimeType(_, e)
            | ErrorKind::Scope(e)
            | ErrorKind::Freeze(e)
            | ErrorKind::Parser(e)
            | ErrorKind::StackOverflow(e)
            | ErrorKind::DeferredRead(e)
            | ErrorKind::Native(e)
            | ErrorKind::Other(e) => ErrorKind::Internal(e),
        }
    }

    /// Change type to `DeferredRead`.
    pub(crate) fn into_deferred_read_error(self) -> ErrorKind {
        match self {
            ErrorKind::Internal(e)
            | ErrorKind::Fail(e)
            | ErrorKind::Value(e)
            | ErrorKind::Function(e)
            | ErrorKind::Scope(e)
            | ErrorKind::Freeze(e)
            | ErrorKind::Parser(e)
            | ErrorKind::StackOverflow(e)
            | ErrorKind::RuntimeType(_, e)
            | ErrorKind::DeferredRead(e)
            | ErrorKind::Native(e)
            | ErrorKind::Other(e) => ErrorKind::DeferredRead(e),
        }
    }
}

impl fmt::Debug for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fail(s) => write!(f, "fail:{s}"),
            Self::Value(e) => fmt::Debug::fmt(e, f),
            Self::StackOverflow(e) => fmt::Debug::fmt(e, f),
            Self::Function(e) => fmt::Debug::fmt(e, f),
            Self::RuntimeType(_, e) => fmt::Debug::fmt(e, f),
            Self::Scope(e) => fmt::Debug::fmt(e, f),
            Self::Freeze(e) => fmt::Debug::fmt(e, f),
            Self::Parser(e) => fmt::Debug::fmt(e, f),
            Self::Internal(e) => write!(f, "Internal error: {e}"),
            Self::DeferredRead(e) => fmt::Debug::fmt(e, f),
            Self::Native(e) => fmt::Debug::fmt(e, f),
            Self::Other(e) => fmt::Debug::fmt(e, f),
        }
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fail(s) => write!(f, "fail:{s}"),
            Self::StackOverflow(e) => fmt::Display::fmt(e, f),
            Self::Value(e) => fmt::Display::fmt(e, f),
            Self::Function(e) => fmt::Display::fmt(e, f),
            Self::RuntimeType(_, e) => fmt::Display::fmt(e, f),
            Self::Scope(e) => fmt::Display::fmt(e, f),
            Self::Freeze(e) => fmt::Display::fmt(e, f),
            Self::Parser(e) => fmt::Display::fmt(e, f),
            Self::Internal(e) => write!(f, "Internal error: {e}"),
            Self::DeferredRead(e) => fmt::Display::fmt(e, f),
            Self::Native(e) => fmt::Display::fmt(e, f),
            Self::Other(e) => fmt::Display::fmt(e, f),
        }
    }
}

impl From<anyhow::Error> for Error {
    #[cold]
    fn from(e: anyhow::Error) -> Self {
        Self(WithDiagnostic::new_empty(ErrorKind::Other(e)))
    }
}

pub trait StarlarkResultExt<T> {
    fn into_anyhow_result(self) -> anyhow::Result<T>;
}

impl<T> StarlarkResultExt<T> for crate::Result<T> {
    #[inline]
    fn into_anyhow_result(self) -> anyhow::Result<T> {
        self.map_err(Error::into_anyhow)
    }
}

#[doc(hidden)]
#[cold]
pub fn internal_error_impl(args: fmt::Arguments<'_>) -> Error {
    Error::new_kind(ErrorKind::Internal(anyhow::anyhow!("{}", args)))
}

#[doc(hidden)]
#[cold]
pub fn other_error_impl(args: fmt::Arguments<'_>) -> Error {
    Error::new_kind(ErrorKind::Other(anyhow::anyhow!("{}", args)))
}

#[doc(hidden)]
#[cold]
pub fn value_error_impl(args: fmt::Arguments<'_>) -> Error {
    Error::new_kind(ErrorKind::Value(anyhow::anyhow!("{}", args)))
}

#[doc(hidden)]
#[cold]
pub fn function_error_impl(args: fmt::Arguments<'_>) -> Error {
    Error::new_kind(ErrorKind::Function(anyhow::anyhow!("{}", args)))
}

/// Internal error of starlark.
#[macro_export]
macro_rules! internal_error {
    ($format:literal) => {
        internal_error!($format,)
    };
    ($format:literal, $($args:tt)*) => {
        $crate::error::internal_error_impl(format_args!($format, $($args)*))
    };
}

#[macro_export]
macro_rules! other_error {
    ($format:literal) => {
        other_error!($format,)
    };
    ($format:literal, $($args:tt)*) => {
        $crate::error::other_error_impl(format_args!($format, $($args)*))
    };
}

#[macro_export]
macro_rules! value_error {
    ($format:literal) => {
        value_error!($format,)
    };
    ($format:literal, $($args:tt)*) => {
        $crate::error::value_error_impl(format_args!($format, $($args)*))
    };
}

#[macro_export]
macro_rules! function_error {
    ($format:literal) => {
        function_error!($format,)
    };
    ($format:literal, $($args:tt)*) => {
        $crate::error::function_error_impl(format_args!($format, $($args)*))
    };
}
