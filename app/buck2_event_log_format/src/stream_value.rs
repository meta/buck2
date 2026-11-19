/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use buck2_cli_proto::CommandResult;
use buck2_cli_proto::PartialResult;
use serde::Deserialize;
use serde::Serialize;

#[derive(Allocative, Deserialize, Serialize)]
#[allow(clippy::large_enum_variant)]
pub enum StreamValue {
    Result(Box<CommandResult>),
    PartialResult(Box<PartialResult>),
    Event(Box<buck2_data::BuckEvent>),
}
