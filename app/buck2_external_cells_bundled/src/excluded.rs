/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

/// Prelude paths left out of the bundled prelude: the Android toolchain's own tests and their
/// test data, and the images of the Visual Studio integration's README. A project never loads
/// them. `build.rs` (cargo builds) and `bundled_cell.bzl` (Buck builds) both skip them.
pub const EXCLUDED_PREFIXES: &[&str] = &[
    "toolchains/android/test/",
    "ide_integrations/visual_studio/assets/",
];
