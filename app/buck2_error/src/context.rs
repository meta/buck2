/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use smallvec::smallvec;

use crate::context_value::ContextValue;
use crate::context_value::TypedContext;

/// Provides the `context` method for `Result`.
///
/// This trait is analogous to the `anyhow::Context` trait. It is mostly a drop-in replacement, and
/// in the near future, uses of `anyhow::Context` in `buck2/app` will be broadly replaced with use
/// of this trait. Subsequently, additional APIs will be provided for annotating errors with
/// structured context data.
pub trait BuckErrorContext<T>: Sealed {
    #[track_caller]
    fn buck_error_context<C: Into<ContextValue>>(self, context: C) -> crate::Result<T>;

    #[track_caller]
    fn with_buck_error_context<C, F>(self, f: F) -> crate::Result<T>
    where
        C: Into<ContextValue>,
        F: FnOnce() -> C;

    #[track_caller]
    fn tag(self, tag: crate::ErrorTag) -> crate::Result<T> {
        self.buck_error_context(ContextValue::Tags(smallvec![tag]))
    }

    #[track_caller]
    fn internal_error(self, message: &str) -> crate::Result<T> {
        self.with_internal_error(|| message.to_owned())
    }

    #[track_caller]
    fn with_internal_error<F>(self, f: F) -> crate::Result<T>
    where
        F: FnOnce() -> String,
    {
        self.with_buck_error_context(|| format!("{} (internal error)", f()))
            .tag(crate::ErrorTag::InternalError)
    }

    /// Supports adding context to an error by either augmenting the most recent context if its
    /// the requested type or by adding a new context.
    #[track_caller]
    fn compute_context<
        TC: TypedContext,
        C1: Into<ContextValue>,
        C2: Into<ContextValue>,
        F: FnOnce(Arc<TC>) -> C1,
        F2: FnOnce() -> C2,
    >(
        self,
        map_context: F,
        new_context: F2,
    ) -> crate::Result<T>;
}
pub trait Sealed: Sized {}

impl<T, E> Sealed for std::result::Result<T, E> where crate::Error: From<E> {}

impl<T, E> BuckErrorContext<T> for std::result::Result<T, E>
where
    crate::Error: From<E>,
{
    fn buck_error_context<C>(self, c: C) -> crate::Result<T>
    where
        C: Into<ContextValue>,
    {
        match self {
            Ok(x) => Ok(x),
            Err(e) => Err(crate::Error::from(e).context(c)),
        }
    }

    fn with_buck_error_context<C, F>(self, f: F) -> crate::Result<T>
    where
        C: Into<ContextValue>,
        F: FnOnce() -> C,
    {
        match self {
            Ok(x) => Ok(x),
            Err(e) => Err(crate::Error::from(e).context(f())),
        }
    }

    #[track_caller]
    fn compute_context<
        TC: TypedContext,
        C1: Into<ContextValue>,
        C2: Into<ContextValue>,
        F: FnOnce(Arc<TC>) -> C1,
        F2: FnOnce() -> C2,
    >(
        self,
        map_context: F,
        new_context: F2,
    ) -> crate::Result<T> {
        match self {
            Ok(x) => Ok(x),
            Err(e) => Err(crate::Error::from(e).compute_context(map_context, new_context)),
        }
    }
}

/// Provides `internal_error` for `Option`, turning an unexpected `None` into a tagged internal
/// error. `Option` deliberately gets only this subset of [`BuckErrorContext`]: there is no
/// underlying error to wrap, so the general context methods would have to synthesize a root
/// error out of an arbitrary `ContextValue`.
pub trait BuckErrorOptionContext<T>: Sealed {
    #[track_caller]
    fn internal_error(self, message: &str) -> crate::Result<T> {
        self.with_internal_error(|| message.to_owned())
    }

    #[track_caller]
    fn with_internal_error<F>(self, f: F) -> crate::Result<T>
    where
        F: FnOnce() -> String;
}

impl<T> Sealed for Option<T> {}

impl<T> BuckErrorOptionContext<T> for Option<T> {
    #[track_caller]
    fn with_internal_error<F>(self, f: F) -> crate::Result<T>
    where
        F: FnOnce() -> String,
    {
        match self {
            Some(x) => Ok(x),
            None => Err(crate::macros::internal_error_impl(format_args!("{}", f()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::error::Error as StdError;

    use allocative::Allocative;

    use super::*;
    use crate::source_location::SourceLocation;

    #[derive(Debug, derive_more::Display)]
    struct TestError;

    impl StdError for TestError {}

    impl From<TestError> for crate::Error {
        #[cold]
        fn from(_: TestError) -> Self {
            crate::Error::new(
                "".to_owned(),
                crate::ErrorTag::Input,
                SourceLocation::new(file!(), line!()),
                None,
            )
        }
    }

    #[derive(Debug, Allocative, Eq, PartialEq)]
    struct SomeContext(Vec<u32>);

    impl TypedContext for SomeContext {
        fn eq(&self, other: &dyn TypedContext) -> bool {
            match (other as &dyn Any).downcast_ref::<Self>() {
                Some(v) => self == v,
                None => false,
            }
        }

        fn display(&self) -> Option<String> {
            Some(format!("{self:?}"))
        }
    }

    #[test]
    fn test_option_internal_error() {
        assert_eq!(
            Some(42).internal_error("missing").unwrap(),
            42,
            "Some should pass through unchanged"
        );

        let err = None::<u32>.internal_error("missing").unwrap_err();
        assert!(
            format!("{err:#}").contains("missing (internal error)"),
            "message should match the Result path's shape, got: {err:#}"
        );
        assert!(err.has_tag(crate::ErrorTag::InternalError));

        let err = None::<u32>
            .with_internal_error(|| format!("key `{}`", 1))
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("key `1` (internal error)"),
            "lazy message should be formatted, got: {err:#}"
        );
    }

    #[test]
    fn test_compute_context() {
        crate::Error::check_equal(
            &crate::Error::from(TestError).context("string"),
            &crate::Error::from(TestError).compute_context(
                |_t: Arc<SomeContext>| -> SomeContext { panic!() },
                || "string",
            ),
        );

        crate::Error::check_equal(
            &crate::Error::from(TestError).context(SomeContext(vec![0, 1, 2])),
            &crate::Error::from(TestError)
                .context(SomeContext(vec![]))
                .compute_context(
                    |_t: Arc<SomeContext>| -> SomeContext { SomeContext(vec![0, 1, 2]) },
                    || "string",
                ),
        );
    }
}
