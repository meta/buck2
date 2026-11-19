/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//!
//! A 'target' is an instance of a rule declared in the build file. Each
//! 'target' is a node on the 'static graph'. Targets are determined by parsing;
//! no extra analysis or building is necessary to determine the 'target's
//! available.
//!
//! For example, the below is a target, defined in some 'Package' with the given
//! name "foo".
//!
//!```ignored
//! java_library(
//!    name = "foo",
//!    srcs = [ ... ],
//!    ...
//! )
//! ```
//!
//! Target names are limited to non-empty alpha numeric characters `,`, `=`,
//! `-`, `/`, and `_`. No other special characters, e.g. spaces, are allowed.
//! Currently, `+` is allow for backwards compatibility but may be removed.
//!
//! 'TargetLabel's are labels/keys that uniquely map to a 'target' in the static
//! graph. These are of the form `<cell>//<path to build file>:<target name>`.
//! e.g. `mycell//my/package/path:my_target`, where `mycell` is the cell,
//! `my/package/path` is the package, and `my_target` is the target name
//! belonging to the package.

pub mod configured_or_unconfigured;
pub mod configured_target_label;
pub mod label;
pub mod name;
pub mod name_glob;
pub mod target_configured_target_label;
