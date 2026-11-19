/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::str::FromStr;

use async_trait::async_trait;
use buck2_client_ctx::common::CommonCommandOptions;
use buck2_client_ctx::common::target_cfg::TargetCfgUnusedOptions;
use dupe::Dupe;

use crate::AuditSubcommand;

#[derive(
    Debug,
    Dupe,
    Clone,
    Copy,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    clap::ValueEnum
)]
#[clap(rename_all = "snake_case")]
pub enum OutputFormat {
    Simple,
    InlineSection,
    Json,
}

#[derive(
    Debug,
    Clone,
    Copy,
    Dupe,
    serde::Serialize,
    serde::Deserialize,
    clap::ValueEnum
)]
pub enum LocationStyle {
    None,
    Direct,
    Extended,
}

impl FromStr for LocationStyle {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "none" => Ok(Self::None),
            "direct" => Ok(Self::Direct),
            "extended" => Ok(Self::Extended),
            _ => Err("bad".to_owned()),
        }
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    Dupe,
    serde::Serialize,
    serde::Deserialize,
    clap::ValueEnum
)]
pub enum ValueStyle {
    Resolved,
    Raw,
    Both,
}

impl FromStr for ValueStyle {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "resolved" => Ok(Self::Resolved),
            "raw" => Ok(Self::Raw),
            "both" => Ok(Self::Both),
            _ => Err("bad".to_owned()),
        }
    }
}

#[derive(Debug, clap::Parser, serde::Serialize, serde::Deserialize)]
#[clap(name = "audit-config", about = "Read and display buckconfig values.")]
pub struct AuditConfigCommand {
    /// Query config for a specific cell (default: the cell of the current working directory).
    #[clap(long = "cell")]
    pub cell: Option<String>,

    /// Produce information for all cells.
    #[clap(long, conflicts_with = "cell")]
    pub all_cells: bool,

    /// Output format.
    #[clap(
        long,
        alias = "style",
        ignore_case = true,
        value_enum,
        conflicts_with = "json"
    )]
    pub output_format: Option<OutputFormat>,

    /// Output in JSON format (shorthand for `--output-format=json`).
    #[clap(long, conflicts_with = "output_format")]
    pub json: bool,

    /// Show where config values are defined.
    #[clap(
        long = "location",
        default_value = "none",
        ignore_case = true,
        value_enum
    )]
    pub location_style: LocationStyle,

    /// Show resolved values, raw (pre-substitution) values, or both.
    #[clap(
        long = "value",
        default_value = "resolved",
        ignore_case = true,
        value_enum
    )]
    pub value_style: ValueStyle,

    /// config section/key specs of the form `section` or `section.key`.
    /// If any specs are provided, only values matching a spec will be printed
    /// (section headers will be printed only for sections with a key matching the spec).
    #[clap(name = "SPECS")]
    pub specs: Vec<String>,

    /// Command doesn't need these flags, but they are used in mode files, so we need to keep them.
    #[clap(flatten)]
    _target_cfg: TargetCfgUnusedOptions,

    #[clap(flatten)]
    common_opts: CommonCommandOptions,
}

impl AuditConfigCommand {
    pub fn output_format(&self) -> OutputFormat {
        if let Some(format) = &self.output_format {
            *format
        } else if self.json {
            OutputFormat::Json
        } else {
            OutputFormat::Simple
        }
    }
}

#[async_trait]
impl AuditSubcommand for AuditConfigCommand {
    fn common_opts(&self) -> &CommonCommandOptions {
        &self.common_opts
    }
}
