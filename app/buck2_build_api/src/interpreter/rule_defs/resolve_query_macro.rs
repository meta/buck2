/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::fmt::Display;

use allocative::Allocative;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_util::size_assert;
use mini_vec::MiniBoxSlice;
use pagable::PagableDeserialize;
use pagable::PagableSerialize;
use starlark::pagable::StarlarkDeserialize;
use starlark::pagable::StarlarkDeserializeContext;
use starlark::pagable::StarlarkSerialize;
use starlark::pagable::StarlarkSerializeContext;
use starlark::values::StarlarkPagable;

use crate::interpreter::rule_defs::artifact::starlark_artifact::StarlarkArtifact;
use crate::interpreter::rule_defs::artifact::starlark_artifact_like::StarlarkInputArtifactLike;
use crate::interpreter::rule_defs::cmd_args::CommandLineArtifactVisitor;
use crate::interpreter::rule_defs::cmd_args::CommandLineBuilder;
use crate::interpreter::rule_defs::resolved_macro::add_output_to_arg;

#[derive(Debug, PartialEq, Allocative, StarlarkPagable)]
pub struct ResolvedQueryMacroTargetAndOutputs {
    #[starlark_pagable(pagable)]
    pub sep: Box<str>,
    // Mixed: `ConfiguredTargetLabel` is pagable-only (`buck2_core` cannot
    // depend on `starlark`), outputs are starlark-aware.
    #[starlark_pagable(
        serialize_with = "serialize_target_outputs",
        deserialize_with = "deserialize_target_outputs"
    )]
    pub list: MiniBoxSlice<(ConfiguredTargetLabel, MiniBoxSlice<StarlarkArtifact>)>,
}

fn serialize_target_outputs(
    field: &[(ConfiguredTargetLabel, MiniBoxSlice<StarlarkArtifact>)],
    ctx: &mut dyn StarlarkSerializeContext,
) -> starlark::Result<()> {
    PagableSerialize::pagable_serialize(&field.len(), ctx.pagable())?;
    for (target, outputs) in field.iter() {
        PagableSerialize::pagable_serialize(target, ctx.pagable())?;
        StarlarkSerialize::starlark_serialize(outputs, ctx)?;
    }
    Ok(())
}

fn deserialize_target_outputs(
    ctx: &mut dyn StarlarkDeserializeContext<'_, '_>,
) -> starlark::Result<MiniBoxSlice<(ConfiguredTargetLabel, MiniBoxSlice<StarlarkArtifact>)>> {
    let len = usize::pagable_deserialize(ctx.pagable())?;
    let mut v = Vec::with_capacity(len);
    for _ in 0..len {
        let target =
            <ConfiguredTargetLabel as PagableDeserialize>::pagable_deserialize(ctx.pagable())?;
        let outputs =
            <MiniBoxSlice<StarlarkArtifact> as StarlarkDeserialize>::starlark_deserialize(ctx)?;
        v.push((target, outputs));
    }
    Ok(v.into())
}

#[derive(Debug, PartialEq, Allocative, StarlarkPagable)]
pub enum ResolvedQueryMacro {
    Outputs(MiniBoxSlice<MiniBoxSlice<StarlarkArtifact>>),
    Targets(#[starlark_pagable(pagable)] MiniBoxSlice<ConfiguredTargetLabel>),
    TargetsAndOutputs(Box<ResolvedQueryMacroTargetAndOutputs>),
}

size_assert::words_of_type!(ResolvedQueryMacro, 2);

impl Display for ResolvedQueryMacro {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO(ndmitchell): Include the information in the format output
        match self {
            ResolvedQueryMacro::Outputs(_) => {
                write!(f, "$(query_outputs ...)")
            }
            ResolvedQueryMacro::Targets(_) => write!(f, "$(query_targets ...)"),

            ResolvedQueryMacro::TargetsAndOutputs(_) => {
                write!(f, "$(query_targets_and_outputs ...)")
            }
        }
    }
}

impl ResolvedQueryMacro {
    pub fn add_to_arg(&self, fmt: &mut CommandLineBuilder) -> buck2_error::Result<()> {
        match self {
            Self::Outputs(list) => {
                let mut first = true;
                for target_outputs in list.iter() {
                    for output in target_outputs.iter() {
                        if !first {
                            fmt.push_str(" ");
                        }
                        first = false;
                        add_output_to_arg(fmt, output)?;
                    }
                }
            }
            Self::TargetsAndOutputs(target_and_outputs) => {
                let ResolvedQueryMacroTargetAndOutputs { sep, list } = &**target_and_outputs;
                let mut first = true;
                for (target, target_outputs) in list.iter() {
                    for output in target_outputs.iter() {
                        if !first {
                            fmt.push_str(sep);
                        }
                        first = false;
                        fmt.push_string(target.unconfigured().to_string());
                        fmt.push_str(sep);
                        add_output_to_arg(fmt, output)?;
                    }
                }
            }
            Self::Targets(list) => {
                // This is defined to add the plain (unconfigured) labels.
                for (i, target) in list.iter().enumerate() {
                    if i != 0 {
                        fmt.push_str(" ");
                    }
                    fmt.push_string(target.unconfigured().to_string());
                }
            }
        }
        Ok(())
    }

    pub fn visit_artifacts(
        &self,
        visitor: &mut dyn CommandLineArtifactVisitor<'_>,
    ) -> buck2_error::Result<()> {
        match self {
            Self::Outputs(list) => {
                for target_outputs in list.iter() {
                    for artifact in target_outputs.iter() {
                        artifact.as_command_line_like().visit_artifacts(visitor)?;
                    }
                }
            }
            Self::TargetsAndOutputs(targets_and_outputs) => {
                for (_, target_outputs) in &*targets_and_outputs.list {
                    for artifact in target_outputs.iter() {
                        artifact.as_command_line_like().visit_artifacts(visitor)?;
                    }
                }
            }
            Self::Targets(..) => {
                // no inputs
            }
        }
        Ok(())
    }
}
