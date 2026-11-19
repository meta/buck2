/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

/// Diagnostics from rustc, see
/// <https://doc.rust-lang.org/beta/rustc/json.html#diagnostics>.
#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct Message {
    pub(crate) message: String,
    pub(crate) code: Option<Code>,
    pub(crate) level: String,
    pub(crate) spans: Vec<Span>,
    pub(crate) children: Vec<Message>,
    pub(crate) rendered: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct Span {
    pub(crate) file_name: PathBuf,
    pub(crate) byte_start: usize,
    pub(crate) byte_end: usize,
    pub(crate) line_start: usize,
    pub(crate) line_end: usize,
    pub(crate) column_start: usize,
    pub(crate) column_end: usize,
    pub(crate) is_primary: bool,
    pub(crate) text: Vec<Text>,
    pub(crate) label: Option<String>,
    pub(crate) suggested_replacement: Option<String>,
    pub(crate) suggestion_applicability: Option<Applicability>,
    pub(crate) expansion: Option<Expansion>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct Expansion {
    pub(crate) span: Box<Span>,
    pub(crate) macro_decl_name: String,
    pub(crate) def_site_span: Option<Box<Span>>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) enum Applicability {
    MachineApplicable,
    HasPlaceholders,
    MaybeIncorrect,
    Unspecified,
}

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct Code {
    pub(crate) code: String,
    pub(crate) explanation: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct Text {
    pub(crate) text: String,
    pub(crate) highlight_start: usize,
    pub(crate) highlight_end: usize,
}
