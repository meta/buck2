/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![feature(error_generic_member_access)]

pub mod any;
pub mod classify;
mod context;
mod context_value;
pub mod conversion;
mod conversion_test;
mod derive_tests;
mod error;
mod exit_code;
mod format;
pub mod macros;
mod paging;
mod root;
pub mod source_location;
pub mod starlark_error;

/// A piece of metadata to indicate whether this error is an infra or user error.
///
/// You can attach this to an error by passing it to the [`Error::context`] method. Alternatively,
/// you can call [`.tag()`](`crate::BuckErrorContext::tag`) on a [`buck2_error::Result`][`Result`].
///
/// The category is fundamentally closed - the expectation is that it will not grow new variants in
/// the future.
#[doc(inline)]
pub use classify::Tier;
pub use context::BuckErrorContext;
pub use context::BuckErrorOptionContext;
pub use context_value::ContextValue;
pub use context_value::TypedContext;
pub use conversion::serde::BuckErrorSerde;
pub use error::DynLateFormat;
pub use error::Error;
pub use exit_code::ExitCode;
pub use root::UniqueRootId;

pub type Result<T> = std::result::Result<T, crate::Error>;

/// Allows simpler construction of the Ok case when the result type can't be inferred.
#[allow(non_snake_case)]
pub fn Ok<T>(t: T) -> Result<T> {
    Result::Ok(t)
}

/// See the documentation in the `error.proto` file for details.
pub use buck2_data::error::ErrorTag;
/// Generates an error impl for the type.
///
/// This macro is a drop-in replacement for [`thiserror::Error`]. In the near future, all uses of
/// `thiserror` in `buck2/app` will be replaced with this macro.
///
/// Currently, the only distinction from `thiserror::Error` is that an additional impl of
/// `AnyError` is generated for the type, which makes some of the interactions with `buck2_error` more
/// ergonomic. In the future, this macro will also be used to be able to annotate errors with
/// additional structured context information.
///
/// ## Example
///
/// ```rust
/// # #![feature(error_generic_member_access)]
/// #[derive(Debug, buck2_error::Error)]
/// #[error("My error type")]
/// #[buck2(tag = buck2_error::ErrorTag::Input)]
/// struct MyError;
///
/// let e = buck2_error::Error::from(MyError);
/// assert_eq!(&format!("{}", e), "My error type");
/// ```
#[doc(inline)]
pub use buck2_error_derive::Error;

#[doc(hidden)]
pub mod __for_macro {
    pub use anyhow;
    pub use thiserror;

    pub use crate::context_value::ContextValue;
}
