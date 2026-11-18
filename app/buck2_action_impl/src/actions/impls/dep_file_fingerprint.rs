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

use allocative::Allocative;
use buck2_artifact::artifact::artifact_dyn::ArtifactDyn;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_build_api::actions::impls::json;
use buck2_build_api::actions::impls::json::JsonUnpack;
use buck2_build_api::artifact_groups::ArtifactGroup;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_artifact_like::StarlarkArtifactView;
use buck2_build_api::interpreter::rule_defs::cmd_args::ArtifactPathMapper;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineArtifactVisitor;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineSink;
use buck2_build_api::interpreter::rule_defs::cmd_args::InputProperties;
use buck2_build_api::interpreter::rule_defs::cmd_args::value_as::ValueAsCommandLineLike;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_execute::artifact::fs::ExecutorFs;
use derive_more::Display;
use dupe::Dupe;
use mini_vec::MiniBoxSlice;
use pagable::Pagable;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::starlark_complex_value;
use starlark::starlark_module;
use starlark::values::Freeze;
use starlark::values::NoSerialize;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::starlark_value;

use crate::actions::impls::run::DepFilesPlaceholderArtifactPathMapper;
use crate::actions::impls::write::WriteCommandLineOptions;

#[derive(Debug, Clone, Allocative, Pagable)]
pub(crate) enum DepFileFingerprintFormat {
    Json {
        pretty: bool,
    },
    Args {
        is_executable: bool,
        macro_files: Option<MiniBoxSlice<Artifact>>,
    },
}

/// Canonical file contents and their dependencies, explicitly supplied to `actions.run`.
#[derive(
    Debug,
    Clone,
    Display,
    Trace,
    Freeze,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable,
    NoSerialize
)]
#[display("<DepFileFingerprint>")]
pub(crate) struct StarlarkDepFileFingerprint<'v> {
    pub(crate) artifact: Value<'v>,
    pub(crate) content: Value<'v>,
    pub(crate) absolute: bool,
    #[trace(static)]
    #[freeze(identity)]
    #[starlark_pagable(pagable)]
    pub(crate) format: DepFileFingerprintFormat,
}

starlark_complex_value!(pub(crate) StarlarkDepFileFingerprint);

#[starlark_value(type = "DepFileFingerprint")]
impl<'v> StarlarkValue<'v> for StarlarkDepFileFingerprint<'v> {}

impl<'v> StarlarkDepFileFingerprint<'v> {
    pub(crate) fn visit_inputs(
        &self,
        visitor: &mut dyn CommandLineArtifactVisitor<'v>,
    ) -> buck2_error::Result<()> {
        if visitor.skip_hidden() {
            return Ok(());
        }
        match &self.format {
            DepFileFingerprintFormat::Json { .. } => {
                json::visit_json_artifacts(self.content, visitor)
            }
            DepFileFingerprintFormat::Args { macro_files, .. } => {
                ValueAsCommandLineLike::unpack_value_err(self.content)?
                    .0
                    .visit_artifacts(visitor)?;
                if let Some(files) = macro_files {
                    for artifact in files {
                        visitor.visit_input(
                            ArtifactGroup::Artifact(artifact.dupe()),
                            InputProperties::default(),
                            Vec::new(),
                        );
                    }
                }
                Ok(())
            }
        }
    }

    pub(crate) fn fingerprint(
        &self,
        fs: &ExecutorFs<'_>,
    ) -> buck2_error::Result<(ProjectRelativePathBuf, [u8; 32])> {
        let mapper = DepFilesPlaceholderArtifactPathMapper {};
        let artifact = StarlarkArtifactView::unpack_value_err(self.artifact)?
            .as_input_artifact_like()
            .get_bound_artifact()?;
        let path = artifact.resolve_path(fs.fs(), mapper.get(&artifact))?;
        let mut digest = blake3::Hasher::new();
        match &self.format {
            DepFileFingerprintFormat::Json { pretty } => {
                json::write_json(
                    JsonUnpack::unpack_value_err(self.content)?,
                    Some(fs),
                    &mut digest,
                    *pretty,
                    self.absolute,
                    &mapper,
                )?;
            }
            DepFileFingerprintFormat::Args {
                is_executable,
                macro_files,
            } => {
                digest.write_all(&[u8::from(*is_executable)])?;
                let mut sink = TextFingerprintSink {
                    writer: &mut digest,
                    first: true,
                    result: Ok(()),
                };
                WriteCommandLineOptions {
                    absolute: self.absolute,
                    macro_files: macro_files.as_deref(),
                }
                .render(self.content, fs, &mapper, &mut sink)?;
                sink.result?;
            }
        }
        Ok((path, *digest.finalize().as_bytes()))
    }
}

// Match the newline-joined bytes from `WriteAction::get_contents` exactly.
// Argument lists that write identical files should have identical fingerprints.
struct TextFingerprintSink<'a> {
    writer: &'a mut dyn Write,
    first: bool,
    result: std::io::Result<()>,
}

impl CommandLineSink for TextFingerprintSink<'_> {
    fn push_arg(&mut self, arg: Cow<'_, str>) {
        if self.result.is_err() {
            return;
        }
        self.result = (|| {
            if !self.first {
                self.writer.write_all(b"\n")?;
            }
            self.first = false;
            self.writer.write_all(arg.as_bytes())
        })();
    }
}

#[starlark_module]
#[starlark_types(StarlarkDepFileFingerprint<'_> as DepFileFingerprint)]
pub(crate) fn register_dep_file_fingerprint(globals: &mut GlobalsBuilder) {}
