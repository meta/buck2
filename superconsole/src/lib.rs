/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The superconsole crate provides a handler and building blocks for powerful, yet minimally intrusive TUIs.
//! Built on-top of [`crossterm`], it cross-compiles on Windows 7+, Linux, and MacOS.
//!
//! Rendering is handled by [`SuperConsole`], which draws to [`stdout`](std::io::stdout).
//! The caller is responsible for re-rendering whenever necessary.
//! User input will cause aberrations in output; similarly, one should also not produce output from other sources while superconsole is active.
//!
//! The rendering can be divided into two principle components:
//!
//! * In the *scratch* area, the previous content is overwritten at each render.
//!
//! * In the *emitted* area, lines scroll away above the scratch with various diagnostic output.
//!
//! Components live in the scratch area.
//!
//! A set of pre-baked composition and testing oriented components are provided in the [`components`] module.

pub use components::Component;
pub use components::DrawMode;
pub use content::Hyperlink;
pub use content::Line;
pub use content::Lines;
pub use content::Span;
pub use content::SpanError;
pub use dimensions::Dimensions;
pub use dimensions::Direction;
pub use error::Error;

pub use crate::builder::Builder;
pub use crate::stdin::Stdin;
pub use crate::superconsole::SuperConsole;

#[allow(type_alias_bounds)]
pub type RenderResult<T, C: Component> = Result<T, Error<<C as Component>::Error>>;

pub(crate) mod ansi_support;
pub mod builder;
pub mod components;
pub mod content;
mod dimensions;
pub mod error;
pub mod output;
pub mod stdin;
pub mod style;
mod superconsole;
pub mod testing;
pub(crate) mod vec_as_fmt_write;
