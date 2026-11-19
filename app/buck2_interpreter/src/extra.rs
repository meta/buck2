/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

pub mod xcode;

use allocative::Allocative;
use dupe::Dupe;
use pagable::Pagable;

#[derive(Copy, Clone, Dupe, Debug, PartialEq, Allocative, Pagable)]
pub enum InterpreterHostPlatform {
    Linux,
    MacOS,
    Windows,
    FreeBsd,
    Unknown,
}

#[derive(Copy, Clone, Dupe, Debug, PartialEq, Allocative, Pagable)]
pub enum InterpreterHostArchitecture {
    AArch64,
    X86_64,
    Arm,
    Riscv64,
    X86,
    Mips,
    Mips64,
    PowerPc,
    PowerPc64,
    Unknown,
}
