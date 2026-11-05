/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;
use std::io::Write;
use std::io::sink;
use std::sync::Arc;

use buck2_artifact::artifact::artifact_dyn::ArtifactDyn;
use buck2_core::content_hash::ContentBasedPathHash;
use buck2_error::BuckErrorContext;
use buck2_execute::artifact::fs::ExecutorFs;
use buck2_hash::BuckMutMap;
use buck2_interpreter::types::cell_path::StarlarkCellPath;
use buck2_interpreter::types::configured_providers_label::StarlarkConfiguredProvidersLabel;
use buck2_interpreter::types::target_label::StarlarkTargetLabel;
use buck2_util::threads::check_stack_overflow;
use serde::Serialize;
use serde::Serializer;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::dict::DictRef;
use starlark::values::enumeration::EnumValue;
use starlark::values::list::ListRef;
use starlark::values::none::NoneType;
use starlark::values::record::Record;
use starlark::values::structs::StructRef;
use starlark::values::tuple::TupleRef;
use starlark::values::type_repr::StarlarkTypeRepr;

use crate::artifact_groups::ArtifactGroup;
use crate::bxl::select::StarlarkSelectConcat;
use crate::bxl::select::StarlarkSelectDict;
use crate::interpreter::rule_defs::artifact::starlark_artifact_like::StarlarkInputArtifactLike;
use crate::interpreter::rule_defs::artifact::starlark_artifact_like::ValueAsInputArtifactLike;
use crate::interpreter::rule_defs::artifact::starlark_output_artifact::StarlarkOutputArtifactUnpack;
use crate::interpreter::rule_defs::artifact_tagging::StarlarkTaggedValue;
use crate::interpreter::rule_defs::cmd_args::ArtifactPathMapper;
use crate::interpreter::rule_defs::cmd_args::CommandLineArtifactVisitor;
use crate::interpreter::rule_defs::cmd_args::CommandLineBuilder;
use crate::interpreter::rule_defs::cmd_args::FrozenStarlarkCmdArgs;
use crate::interpreter::rule_defs::cmd_args::InputProperties;
use crate::interpreter::rule_defs::cmd_args::StarlarkCmdArgs;
use crate::interpreter::rule_defs::cmd_args::path_format;
use crate::interpreter::rule_defs::cmd_args::path_format_absolute;
use crate::interpreter::rule_defs::cmd_args::value::CommandLineArg;
use crate::interpreter::rule_defs::cmd_args::value_as::ValueAsCommandLineLike;
use crate::interpreter::rule_defs::provider::ValueAsProviderLike;
use crate::interpreter::rule_defs::transitive_set::TransitiveSetJsonProjection;

/// A wrapper with a Serialize instance so we can pass down the necessary context.
pub struct SerializeValue<'a, 'v> {
    pub value: JsonUnpack<'v>,
    pub fs: Option<&'a ExecutorFs<'a>>,
    pub absolute: bool,
    pub artifact_path_mapping: &'a dyn ArtifactPathMapper,
}

struct Buck2ErrorResultOfSerializedValue<'a, 'v> {
    result: buck2_error::Result<SerializeValue<'a, 'v>>,
}

impl<'a, 'v> Serialize for Buck2ErrorResultOfSerializedValue<'a, 'v> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match &self.result {
            Ok(v) => v.serialize(serializer),
            Err(e) => Err(serde::ser::Error::custom(format!("{e:#}"))),
        }
    }
}

impl<'a, 'v> SerializeValue<'a, 'v> {
    fn with_value(&self, x: Value<'v>) -> Buck2ErrorResultOfSerializedValue<'a, 'v> {
        Buck2ErrorResultOfSerializedValue {
            result: JsonUnpack::unpack_value_err(x)
                .map_err(buck2_error::Error::from)
                .map(|value| SerializeValue {
                    value,
                    fs: self.fs,
                    absolute: self.absolute,
                    artifact_path_mapping: self.artifact_path_mapping,
                }),
        }
    }
}

/// Serializes a value, or fails the serialization with the error that stood in for it.
pub(crate) struct SerializeOrFail<T>(pub(crate) buck2_error::Result<T>);

impl<T: Serialize> Serialize for SerializeOrFail<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match &self.0 {
            Ok(value) => value.serialize(serializer),
            Err(e) => Err(serde::ser::Error::custom(format!("{e:#}"))),
        }
    }
}

fn err<R, E: serde::ser::Error>(res: buck2_error::Result<R>) -> Result<R, E> {
    match res {
        Ok(v) => Ok(v),
        Err(e) => Err(serde::ser::Error::custom(format!("{e:#}"))),
    }
}

/// Grab the value as an artifact, if you can.
/// We want to deal with both normal artifacts, and .as_output() artifacts,
/// since otherwise the .as_output ones will fall through as a cmd_args
/// and end up getting wrapped in a list below.
#[derive(UnpackValue, StarlarkTypeRepr)]
pub enum JsonArtifact<'v> {
    ValueAsInputArtifactLike(ValueAsInputArtifactLike<'v>),
    StarlarkOutputArtifact(StarlarkOutputArtifactUnpack<'v>),
}

/// Partially unpack the value into JSON writable with `ctx.actions.write_json`.
/// This does not help typechecker much (because it only validates top-level types),
/// but it provides better documentation.
#[derive(UnpackValue, StarlarkTypeRepr)]
pub enum JsonUnpack<'v> {
    None(NoneType),
    String(&'v str),
    Number(i64),
    Bool(bool),
    List(&'v ListRef<'v>),
    Tuple(&'v TupleRef<'v>),
    Dict(DictRef<'v>),
    Struct(StructRef<'v>),
    Record(&'v Record<'v>),
    Enum(&'v EnumValue<'v>),
    TransitiveSetJsonProjection(&'v TransitiveSetJsonProjection<'v>),
    TargetLabel(&'v StarlarkTargetLabel),
    ConfiguredProvidersLabel(&'v StarlarkConfiguredProvidersLabel),
    CellPath(&'v StarlarkCellPath),
    Artifact(JsonArtifact<'v>),
    CommandLine(CommandLineArg<'v>),
    Provider(ValueAsProviderLike<'v>),
    TaggedValue(&'v StarlarkTaggedValue<'v>),
    BxlSelectConcat(&'v StarlarkSelectConcat),
    BxlSelectDict(&'v StarlarkSelectDict),
}

impl<'a, 'v> Serialize for SerializeValue<'a, 'v> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // The recursion depth of this function is determined by the nesting
        // depth of the value, which is unbounded. Detect such cases and return
        // an error nicely rather than crashing.
        err(check_stack_overflow())?;
        match &self.value {
            JsonUnpack::None(_) => serializer.serialize_none(),
            JsonUnpack::String(x) => serializer.serialize_str(x),
            JsonUnpack::Number(x) => serializer.serialize_i64(*x),
            JsonUnpack::Bool(x) => serializer.serialize_bool(*x),
            JsonUnpack::List(x) => serializer.collect_seq(x.iter().map(|v| self.with_value(v))),
            JsonUnpack::Tuple(x) => serializer.collect_seq(x.iter().map(|v| self.with_value(v))),
            JsonUnpack::Dict(x) => serializer.collect_map(
                x.iter()
                    .map(|(k, v)| (self.with_value(k), self.with_value(v))),
            ),
            JsonUnpack::Struct(x) => {
                serializer.collect_map(x.iter().map(|(k, v)| (k, self.with_value(v))))
            }
            JsonUnpack::Record(x) => {
                serializer.collect_map(x.iter().map(|(k, v)| (k, self.with_value(v))))
            }
            JsonUnpack::Enum(x) => x.serialize(serializer),
            JsonUnpack::TransitiveSetJsonProjection(x) => {
                match self.fs {
                    // Skip validation when fs == None because it can be expensive.
                    // TransitiveSet::new already did validate_json for projected values.
                    None => serializer.collect_seq(std::iter::empty::<u8>()),
                    Some(_) => serializer.collect_seq(
                        err(x.collect_values())?
                            .into_iter()
                            .map(|v| self.with_value(v)),
                    ),
                }
            }
            JsonUnpack::TargetLabel(x) => {
                // Users could do this with `str(ctx.label.raw_target())`, but in some benchmarks that causes
                // a lot of additional memory to be retained for all those strings
                x.serialize(serializer)
            }
            JsonUnpack::ConfiguredProvidersLabel(x) => {
                // Users could do this with `str(ctx.label)`, but a bit wasteful
                x.serialize(serializer)
            }
            JsonUnpack::CellPath(x) => x.serialize(serializer),
            JsonUnpack::Artifact(x) => {
                match self.fs {
                    None => {
                        // Invariant: If fs == None, then the writer = sink().
                        // Therefore, in the None branch, we only care about getting Serde errors,
                        // so pass something of the right type, but don't worry about the value.
                        serializer.serialize_str("")
                    }
                    Some(fs) => {
                        let p = match x {
                            JsonArtifact::ValueAsInputArtifactLike(x) => {
                                let art = err(x.0.get_bound_artifact())?;
                                err(art.resolve_path(fs.fs(), self.artifact_path_mapping.get(&art)))?
                            }
                            JsonArtifact::StarlarkOutputArtifact(x) => {
                                let art = match x {
                                    StarlarkOutputArtifactUnpack::Unfrozen(x) => {
                                        err((*x.inner()).get_bound_artifact())?
                                    }
                                    StarlarkOutputArtifactUnpack::Frozen(x) => x.inner().artifact(),
                                };

                                err(art.resolve_path(
                                    fs.fs(),
                                    Some(&ContentBasedPathHash::for_output_artifact()),
                                ))?
                            }
                        };
                        let p = if self.absolute {
                            Cow::Owned(err(path_format_absolute(
                                &p,
                                fs.path_separator(),
                                fs.fs().fs(),
                            ))?)
                        } else {
                            path_format(p.as_ref(), fs.path_separator())
                        };
                        serializer.serialize_str(p.as_ref())
                    }
                }
            }
            JsonUnpack::CommandLine(x) => {
                let singleton = is_singleton_cmdargs(*x);
                match self.fs {
                    None => {
                        // See a few lines up for fs == None details.
                        if singleton {
                            serializer.serialize_str("")
                        } else {
                            serializer.collect_seq(&[""])
                        }
                    }
                    Some(fs) => {
                        let mut items = Vec::<String>::new();
                        let mut fmt = CommandLineBuilder::new_with_options(
                            &mut items,
                            self.artifact_path_mapping,
                            fs,
                            self.absolute,
                            // WriteJsonCommandLineArgGen assumes that any args/write-to-file macros are
                            // rejected here and needs to be updated if that changes.
                            None,
                        );
                        err(x.as_command_line_arg().add_to_command_line(&mut fmt))?;

                        // We change the type, based on the value - singleton = String, otherwise list.
                        // That's a little annoying (type based on value), but otherwise there would be
                        // no way to produce a cmd_args as a single string.
                        if singleton {
                            serializer.serialize_str(&items.concat())
                        } else {
                            serializer.collect_seq(items)
                        }
                    }
                }
            }
            JsonUnpack::Provider(x) => {
                serializer.collect_map(x.0.items().iter().map(|(k, v)| (k, self.with_value(*v))))
            }
            JsonUnpack::TaggedValue(x) => self.with_value(x.value()).serialize(serializer),
            JsonUnpack::BxlSelectConcat(x) => x.serialize(serializer),
            JsonUnpack::BxlSelectDict(x) => x.serialize(serializer),
        }
    }
}

fn is_singleton_cmdargs(x: CommandLineArg) -> bool {
    if let Some(x) = x.to_value().downcast_ref::<StarlarkCmdArgs>() {
        x.is_concat()
    } else if let Some(x) = x.to_value().downcast_ref::<FrozenStarlarkCmdArgs>() {
        x.is_concat()
    } else {
        false
    }
}

pub fn validate_json(x: JsonUnpack) -> buck2_error::Result<()> {
    write_json(x, None, &mut sink(), false, false, &BuckMutMap::default())
}

pub fn write_json(
    value: JsonUnpack,
    fs: Option<&ExecutorFs>,
    mut writer: &mut dyn Write,
    pretty: bool,
    absolute: bool,
    artifact_path_mapping: &dyn ArtifactPathMapper,
) -> buck2_error::Result<()> {
    let value = SerializeValue {
        value,
        fs,
        absolute,
        artifact_path_mapping,
    };
    (|| {
        if pretty {
            serde_json::to_writer_pretty(&mut writer, &value)?;
            // serde_json does not add a trailing line, but we add it because
            // "pretty" implies that this JSON is for people to read.
            writer.write_all(b"\n")?;
        } else {
            serde_json::to_writer(&mut writer, &value)?;
        }
        buck2_error::Ok(())
    })()
    .buck_error_context("Error converting to JSON for `write_json`")
}

pub fn visit_json_artifacts<'v>(
    v: Value<'v>,
    visitor: &mut dyn CommandLineArtifactVisitor<'v>,
) -> buck2_error::Result<()> {
    // The recursion depth of this function is determined by the nesting depth
    // of the value, which is unbounded. Detect such cases and return an error
    // nicely rather than crashing.
    check_stack_overflow()?;
    match JsonUnpack::unpack_value_err(v)? {
        JsonUnpack::None(_)
        | JsonUnpack::String(_)
        | JsonUnpack::Number(_)
        | JsonUnpack::Bool(_)
        | JsonUnpack::TargetLabel(_)
        | JsonUnpack::Enum(_)
        | JsonUnpack::ConfiguredProvidersLabel(_)
        | JsonUnpack::BxlSelectConcat(_)
        | JsonUnpack::BxlSelectDict(_)
        | JsonUnpack::CellPath(_) => {}

        JsonUnpack::List(x) => {
            for x in x.iter() {
                visit_json_artifacts(x, visitor)?;
            }
        }
        JsonUnpack::Tuple(x) => {
            for x in x.iter() {
                visit_json_artifacts(x, visitor)?;
            }
        }
        JsonUnpack::Dict(x) => {
            for (k, v) in x.iter() {
                visit_json_artifacts(k, visitor)?;
                visit_json_artifacts(v, visitor)?;
            }
        }
        JsonUnpack::Struct(x) => {
            for (_k, v) in x.iter() {
                visit_json_artifacts(v, visitor)?;
            }
        }
        JsonUnpack::Record(x) => {
            for (_k, v) in x.iter() {
                visit_json_artifacts(v, visitor)?;
            }
        }
        JsonUnpack::TransitiveSetJsonProjection(x) => visitor.visit_input(
            ArtifactGroup::TransitiveSetProjection(Arc::new(x.to_projection_key_wrapper()?)),
            InputProperties::default(),
            vec![],
        ),
        JsonUnpack::Artifact(_x) => {
            // The _x function requires that the artifact is already bound, but we may need to visit artifacts
            // before that happens. Treating it like an opaque command_line works as we want for any artifact
            // type.
            ValueAsCommandLineLike::unpack_value_err(v)?
                .0
                .visit_artifacts(visitor)?;
        }
        JsonUnpack::CommandLine(x) => x.as_command_line_arg().visit_artifacts(visitor)?,
        JsonUnpack::Provider(x) => {
            for (_, v) in x.0.items() {
                visit_json_artifacts(v, visitor)?;
            }
        }
        JsonUnpack::TaggedValue(v) => {
            let mut visitor = v.wrap_visitor(visitor);
            visit_json_artifacts(v.value(), &mut visitor)?;
        }
    }
    Ok(())
}
