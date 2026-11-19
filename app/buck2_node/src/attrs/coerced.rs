/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Coerced attribute values: the kinds a value can be, what each kind
//! means to traversal, configuration and rendering, and the select
//! vocabulary. Storage is [`crate::attrs::arena`].

pub mod attr_view;
pub mod configure;
pub mod kinds;
pub mod render;
pub mod select;
pub mod semantics;
pub mod traverse;

pub use attr_view::AttrView;
pub use kinds::CoercedAttrKind;
pub use kinds::CoercedAttrRefKind;
