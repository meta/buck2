/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::hash::Hash;

use allocative::Allocative;
use buck2_core::bxl::BxlFilePath;
use buck2_data::ToProtoMessage;
use derive_more::Display;
use pagable::Pagable;
use serde::Serialize;
use serde::Serializer;

/// The identifier used to find the implementation function for this bxl. Should point at the output of `bxl()`
#[derive(
    Debug,
    Clone,
    Display,
    Eq,
    PartialEq,
    Hash,
    Ord,
    PartialOrd,
    Allocative,
    strong_hash::StrongHash,
    Pagable
)]
#[display("{}:{}", bxl_path, name)]
pub struct BxlFunctionLabel {
    /// The cell, package, and file that contains the output of `bxl()`
    pub bxl_path: BxlFilePath,
    /// The name of the symbol that is bound to the output of `bxl()`, e.g. `custom_query`
    pub name: String,
}

impl Serialize for BxlFunctionLabel {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        s.serialize_str(&format!("{self}"))
    }
}

impl ToProtoMessage for BxlFunctionLabel {
    type Message = buck2_data::BxlFunctionLabel;

    fn as_proto(&self) -> Self::Message {
        buck2_data::BxlFunctionLabel {
            bxl_path: self.bxl_path.to_string(),
            name: self.name.clone(),
        }
    }
}
