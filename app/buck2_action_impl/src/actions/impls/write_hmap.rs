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
use std::collections::BTreeMap;
use std::slice;
use std::time::Instant;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_artifact::artifact::artifact_dyn::ArtifactDyn;
use buck2_artifact::artifact::build_artifact::BuildArtifact;
use buck2_build_api::actions::Action;
use buck2_build_api::actions::ActionExecutionCtx;
use buck2_build_api::actions::UnregisteredAction;
use buck2_build_api::actions::errors::execute_error::ExecuteError;
use buck2_build_api::actions::execute::action_executor::ActionExecutionKind;
use buck2_build_api::actions::execute::action_executor::ActionExecutionMetadata;
use buck2_build_api::actions::execute::action_executor::ActionOutputs;
use buck2_build_api::artifact_groups::ArtifactGroup;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_artifact_like::StarlarkArtifactView;
use buck2_build_api::interpreter::rule_defs::cmd_args::ArtifactPathMapper;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineArtifactVisitor;
use buck2_build_api::interpreter::rule_defs::cmd_args::path_format;
use buck2_build_signals::env::WaitingData;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_core::category::CategoryRef;
use buck2_core::content_hash::ContentBasedPathHash;
use buck2_core::execution_types::executor_config::PathSeparatorKind;
use buck2_error::conversion::from_any_with_tag;
use buck2_execute::artifact::fs::ExecutorFs;
use buck2_execute::execute::command_executor::ActionExecutionTimingData;
use buck2_execute::materialize::materializer::WriteRequest;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutInsertionSet;
use buck2_hash::buck_mut_insertion_map;
use dupe::Dupe;
use hmap::ByteOrder;
use hmap::HeaderMap;
use hmap::PathStyle;
use hmap::WriteFormat;
use pagable::Pagable;
use pagable::pagable_typetag;
use starlark::values::OwnedFrozen;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::dict::DictRef;
use starlark::values::dict::DictType;

use crate::actions::impls::produce;
use crate::actions::impls::write::CommandLineContentBasedInputVisitor;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Tier0)]
enum WriteHmapActionValidationError {
    #[error("WriteHmapAction received no outputs")]
    NoOutputs,
    #[error("WriteHmapAction received more than one output")]
    TooManyOutputs,
}

/// Header map entries: include name to `(artifact, path below it)`.
/// An empty path means the artifact itself.
pub(crate) type HmapMappings<'v> = DictType<&'v str, (StarlarkArtifactView<'v>, &'v str)>;

fn for_each_mapping<'v>(
    mappings: Value<'v>,
    mut f: impl FnMut(&'v str, StarlarkArtifactView<'v>, &'v str) -> buck2_error::Result<()>,
) -> buck2_error::Result<()> {
    for (name, entry) in DictRef::unpack_value_err(mappings)?.iter() {
        let name = <&str>::unpack_value_err(name)?;
        let (artifact, subpath) = <(StarlarkArtifactView, &str)>::unpack_value_err(entry)?;
        f(name, artifact, subpath)?;
    }
    Ok(())
}

pub(crate) fn visit_hmap_artifacts<'v>(
    mappings: Value<'v>,
    visitor: &mut dyn CommandLineArtifactVisitor<'v>,
) -> buck2_error::Result<()> {
    for_each_mapping(mappings, |_, artifact, _| {
        artifact
            .as_input_artifact_like()
            .as_command_line_like()
            .visit_artifacts(visitor)
    })
}

#[derive(Allocative, Debug, Pagable)]
pub(crate) struct UnregisteredWriteHmapAction {}

impl UnregisteredAction for UnregisteredWriteHmapAction {
    fn register(
        self: Box<Self>,
        outputs: BuckMutInsertionSet<BuildArtifact>,
        starlark_data: Option<OwnedFrozen<Value<'static>>>,
        _error_handler: Option<OwnedFrozen<Value<'static>>>,
    ) -> buck2_error::Result<Box<dyn Action>> {
        let contents = starlark_data.expect("module data to be present");
        let action = WriteHmapAction::new(contents, outputs)?;
        Ok(Box::new(action))
    }
}

#[derive(Debug, Allocative, Pagable)]
struct WriteHmapAction {
    contents: OwnedFrozen<Value<'static>>,
    output: BuildArtifact,
}

impl WriteHmapAction {
    fn new(
        contents: OwnedFrozen<Value<'static>>,
        outputs: BuckMutInsertionSet<BuildArtifact>,
    ) -> buck2_error::Result<Self> {
        let mut outputs = outputs.into_iter();

        let output = match (outputs.next(), outputs.next()) {
            (Some(o), None) => o,
            (None, ..) => return Err(WriteHmapActionValidationError::NoOutputs.into()),
            (Some(..), Some(..)) => {
                return Err(WriteHmapActionValidationError::TooManyOutputs.into());
            }
        };

        Ok(WriteHmapAction { contents, output })
    }

    fn get_contents(
        &self,
        fs: &ExecutorFs,
        artifact_path_mapping: &dyn ArtifactPathMapper,
    ) -> buck2_error::Result<Vec<u8>> {
        self.contents.by_ref(|v| {
            let mappings = DictRef::unpack_value_err(*v)?;
            encode_hmap(
                fs.path_separator(),
                mappings.iter().map(|(name, entry)| {
                    let name = <&str>::unpack_value_err(name)?;
                    let (artifact, subpath) =
                        <(StarlarkArtifactView, &str)>::unpack_value_err(entry)?;
                    let artifact = artifact.as_input_artifact_like().get_bound_artifact()?;
                    let artifact_path =
                        artifact.resolve_path(fs.fs(), artifact_path_mapping.get(&artifact))?;
                    let mut path =
                        path_format(artifact_path.as_ref(), fs.path_separator()).into_owned();
                    if !subpath.is_empty() {
                        // Match the non-native strings to preserve hmap bytes and cache keys.
                        // cmd_args expands `{}` in subpaths and keeps their other spelling.
                        let subpath = if subpath.contains("{}") {
                            Cow::Owned(subpath.replace("{}", &path))
                        } else {
                            Cow::Borrowed(subpath)
                        };
                        path.push('/');
                        path.push_str(&subpath);
                    }
                    Ok((
                        hmap_wrapper_arg(Cow::Borrowed(name)),
                        hmap_wrapper_arg(Cow::Owned(path)),
                    ))
                }),
            )
        })
    }
}

#[pagable_typetag]
#[async_trait]
impl Action for WriteHmapAction {
    fn kind(&self) -> buck2_data::ActionKind {
        buck2_data::ActionKind::Write
    }

    fn inputs(&self) -> buck2_error::Result<Cow<'_, [ArtifactGroup]>> {
        let mut visitor = CommandLineContentBasedInputVisitor::new();
        self.contents
            .by_ref(|v| visit_hmap_artifacts(*v, &mut visitor))?;
        Ok(Cow::Owned(
            visitor.content_based_inputs.into_iter().collect(),
        ))
    }

    fn outputs(&self) -> Cow<'_, [BuildArtifact]> {
        Cow::Borrowed(slice::from_ref(&self.output))
    }

    fn first_output(&self) -> &BuildArtifact {
        &self.output
    }

    fn category(&self) -> CategoryRef<'_> {
        CategoryRef::unchecked_new("write_hmap")
    }

    fn identifier(&self) -> Option<&str> {
        Some(self.output.get_path().path().as_str())
    }

    fn aquery_attributes(
        &self,
        fs: &ExecutorFs,
        artifact_path_mapping: &dyn ArtifactPathMapper,
    ) -> buck2_error::Result<BuckMutInsertionMap<String, String>> {
        Ok(buck_mut_insertion_map! {
            "contents_hex".to_owned() => hex::encode(self.get_contents(fs, artifact_path_mapping)?),
        })
    }

    async fn execute(
        &self,
        ctx: &mut dyn ActionExecutionCtx,
        waiting_data: WaitingData,
    ) -> Result<(ActionOutputs, ActionExecutionMetadata), ExecuteError> {
        let fs = ctx.fs();

        let execution_start = Instant::now();
        let content = self.get_contents(&ctx.executor_fs(), &ctx.artifact_path_mapping(None))?;
        let path = fs.resolve_build(
            self.output.get_path(),
            if self.output.get_path().is_content_based_path() {
                let digest = TrackedFileDigest::from_content(
                    &content,
                    ctx.digest_config().cas_digest_config(),
                );
                Some(ContentBasedPathHash::new(digest.raw_digest().as_bytes())?)
            } else {
                None
            }
            .as_ref(),
        )?;
        let (value, _) = produce::write_here(
            ctx,
            WriteRequest {
                path,
                content,
                is_executable: false,
                path_kind: self.output.get_path().path_resolution_method(),
            },
        )
        .await?;

        let wall_time = Instant::now() - execution_start;

        Ok((
            ActionOutputs::new(buck_mut_insertion_map![self.output.get_path().dupe() => value]),
            ActionExecutionMetadata {
                dep_file_db_writes_queued: 0,
                execution_kind: ActionExecutionKind::Simple,
                timing: ActionExecutionTimingData { wall_time },
                input_files_bytes: None,
                waiting_data,
            },
        ))
    }
}

fn hmap_wrapper_arg(value: Cow<'_, str>) -> Cow<'_, str> {
    // cmd_args shell quoting escapes these characters inside double quotes.
    // Python's shlex.split keeps those backslashes, and its text-mode read
    // converts CR/CRLF to LF. Match those strings to preserve the hmap bytes.
    if value.contains(['$', '`', '\r']) {
        Cow::Owned(
            value
                .replace("\r\n", "\n")
                .replace('\r', "\n")
                .replace('$', "\\$")
                .replace('`', "\\`"),
        )
    } else {
        value
    }
}

fn encode_hmap<'v>(
    path_separator: PathSeparatorKind,
    mappings: impl IntoIterator<Item = buck2_error::Result<(Cow<'v, str>, Cow<'v, str>)>>,
) -> buck2_error::Result<Vec<u8>> {
    let mut entries = BTreeMap::new();
    for mapping in mappings {
        let (key, value) = mapping?;
        entries.insert(key, value.clone());
        // Clang uses this entry to stop searching later header maps.
        entries.insert(value.clone(), value);
    }
    // Match the action's Python writer, which can run on a different OS from buckd.
    let path_style = match path_separator {
        PathSeparatorKind::Unix => PathStyle::Unix,
        PathSeparatorKind::Windows => PathStyle::Windows,
    };
    let hmap =
        HeaderMap::from_mappings(entries.into_iter(), WriteFormat::PythonHmaptool(path_style))
            .map_err(|error| from_any_with_tag(error, buck2_error::ErrorTag::Input))?;
    Ok(hmap.to_bytes(ByteOrder::Little))
}

#[cfg(test)]
mod tests {
    use sha1::Digest;
    use sha1::Sha1;

    use super::*;

    #[test]
    fn encodes_fixed_mappings_for_action_path_style() {
        for (path_separator, expected) in [
            (
                PathSeparatorKind::Unix,
                "31ae93cbbda69db3eedfa7eb9bb32444baced2bb",
            ),
            (
                PathSeparatorKind::Windows,
                "0d904775ed13764063c8b18394677645ed59bfa5",
            ),
        ] {
            let mappings = [
                ("foo.h", r".\foo.h"),
                ("alias.h", r"C:\headers with spaces\actual.h"),
            ]
            .into_iter()
            .map(|(key, value)| Ok((Cow::Borrowed(key), Cow::Borrowed(value))));
            let bytes = encode_hmap(path_separator, mappings).unwrap();
            assert_eq!(hex::encode(Sha1::digest(bytes)), expected);
        }
    }

    #[test]
    fn encodes_fixed_native_mappings() {
        // Each snapshot starts from fixed native string mappings.
        let cases: &[(&str, &[(&str, &str)], &str)] = &[
            ("empty", &[], "e38d90597ba1a80432956ac87368cfea0b7cde70"),
            (
                "root_header",
                &[("foo.h", "./foo.h")],
                "30ba58dda8bd8e23aea9fce27ceb4b5594548de0",
            ),
            (
                "plain",
                &[("header.h", "header.h")],
                "fe233b42695ff8f2b43975af848019823fbe98da",
            ),
            (
                "mixed",
                &[
                    ("ba.h", "path/header.h"),
                    ("ab.h", "path/header.h"),
                    ("A.h", "a//b.h"),
                    ("dir.h", "path/"),
                    ("empty.h", ""),
                    ("space.h", "some dir/a header.h"),
                ],
                "c5c83aedadcb7fb17f55e606babe024d32549385",
            ),
            (
                "overlap",
                &[("z.h", "a.h"), ("a.h", "b.h")],
                "a1680c5af81b52e482d7f22b9225c7ab32f38490",
            ),
        ];

        for &(name, mappings, expected) in cases {
            let mappings = mappings
                .iter()
                .map(|&(key, value)| Ok((Cow::Borrowed(key), Cow::Owned(value.to_owned()))));
            let bytes =
                encode_hmap(PathSeparatorKind::Unix, mappings).expect("header map should encode");
            assert_eq!(hex::encode(Sha1::digest(bytes)), expected, "{name}");
        }
    }
}
