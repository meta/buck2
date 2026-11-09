/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use starlark::values::ValueOf;

use crate::interpreter::rule_defs::cmd_args::value_as::ValueAsCommandLineLike;

/// A value checked to be command-line-like: what `cmd_args` holds.
pub type CommandLineArg<'v> = ValueOf<'v, ValueAsCommandLineLike<'v>>;
