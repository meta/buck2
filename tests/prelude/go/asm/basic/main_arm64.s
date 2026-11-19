// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is dual-licensed under either the MIT license found in the
// LICENSE-MIT file in the root directory of this source tree or the Apache
// License, Version 2.0 found in the LICENSE-APACHE file in the root directory
// of this source tree. You may select, at your option, one of the
// above-listed licenses.

//go:build arm64

TEXT ·add(SB),$0-24
  MOVD x+0(FP), R0
  MOVD y+8(FP), R1
  ADD R1, R0, R0
  MOVD R0, ret+16(FP)
  RET
