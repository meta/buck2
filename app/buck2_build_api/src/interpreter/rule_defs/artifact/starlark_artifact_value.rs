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
use std::io::BufReader;

use allocative::Allocative;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_error::BuckErrorContext;
use buck2_error::ErrorTag;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_hash::BuckMutInsertionMap;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::Value;
use starlark::values::dict::AllocDict;
use starlark::values::list::AllocList;
use starlark::values::starlark_value;

#[derive(Debug, ProvidesStaticType, NoSerialize, Allocative, StarlarkPagable)]
pub struct StarlarkArtifactValue {
    // We only keep the artifact for Display, since we don't want to leak the underlying path by default
    #[starlark_pagable(pagable)]
    artifact: Artifact,
    #[starlark_pagable(pagable)]
    path: ProjectRelativePathBuf,
    #[starlark_pagable(pagable)]
    fs: ProjectRoot,
}

impl Display for StarlarkArtifactValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.artifact, f)
    }
}

starlark_simple_value!(StarlarkArtifactValue);

impl StarlarkArtifactValue {
    /// Create a new artifact value. Must be materialised to disk before calling this function.
    pub fn new(artifact: Artifact, path: ProjectRelativePathBuf, fs: ProjectRoot) -> Self {
        Self { artifact, path, fs }
    }
}

starlark::methods_static!(ARTIFACT_VALUE_METHODS = artifact_value_methods);

#[starlark_value(type = "ArtifactValue")]
impl<'v> StarlarkValue<'v> for StarlarkArtifactValue {
    fn get_methods() -> Option<&'static Methods> {
        Some(ARTIFACT_VALUE_METHODS.methods())
    }
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum JsonError {
    #[error("JSON number is outside the bounds that Starlark supports, `{0}`")]
    NumberOutOfBounds(String),
}

fn json_convert<'v>(v: serde_json::Value, heap: Heap<'v>) -> starlark::Result<Value<'v>> {
    match v {
        serde_json::Value::Null => Ok(Value::new_none()),
        serde_json::Value::Bool(x) => Ok(Value::new_bool(x)),
        serde_json::Value::Number(x) => {
            if let Some(x) = x.as_i64().and_then(|x| i32::try_from(x).ok()) {
                Ok(heap.alloc(x).to_value())
            } else if let Some(x) = x.as_f64() {
                Ok(heap.alloc(x).to_value())
            } else {
                Err(buck2_error::Error::from(JsonError::NumberOutOfBounds(x.to_string())).into())
            }
        }
        serde_json::Value::String(x) => Ok(heap.alloc(x).to_value()),
        serde_json::Value::Array(xs) => Ok(heap
            .alloc(AllocList(
                xs.into_iter()
                    .map(|v| json_convert(v, heap))
                    .collect::<Result<Vec<_>, _>>()?,
            ))
            .to_value()),
        serde_json::Value::Object(mp) => {
            let mut res = BuckMutInsertionMap::with_capacity(mp.len());
            for (k, v) in mp.into_iter() {
                res.insert_hashed(
                    heap.alloc(k).to_value().get_hashed()?,
                    json_convert(v, heap)?,
                );
            }
            Ok(heap.alloc(AllocDict(res)).to_value())
        }
    }
}

/// A reference to an artifact whose contents can be accessed in starlark.
///
/// `Artifact`s normally only provide access to path information and do not allow starlark code to read the contents; this type represents access to an artifact in those cases where reading the contents is supported.
///
///
/// # Where ArtifactValue is Used
///
/// ## 1. Dynamic Dependencies
///
/// Dynamic attributes declared with `dynattrs.artifact_value()` in `dynamic_actions()`
/// provide access to artifact contents in the implementation function.
///
/// ## 2. Action Error Handlers
///
/// The `output_artifacts` field in `ActionErrorContext` provides access to output
/// artifacts that can be read to extract structured error information.
///
/// # Examples
///
/// ## Example 1: Dynamic Dependencies
///
/// ```python
/// def _dynamic_impl(actions: AnalysisActions, config: ArtifactValue, metadata: ArtifactValue, out: OutputArtifact):
///     # Read configuration as string
///     config_content = config.read_string()
///
///     # Parse JSON metadata
///     data = metadata.read_json()
///     version = data["version"]
///
///     # Make build decisions based on content
///     if "feature_enabled" in config_content:
///         actions.write(out, "Feature enabled for version {}".format(version))
///     else:
///         actions.write(out, "Feature disabled")
///
///     return [DefaultInfo()]
/// ```
///
/// ## Example 2: Error Handler with ArtifactValue
///
/// ```python
/// def _error_handler(ctx: ActionErrorContext) -> list[ActionSubError]:
///     # Access output artifacts from failed action
///     errors = []
///     for artifact_value in ctx.output_artifacts:
///         # Read error logs to extract structured information
///         error_json = artifact_value.read_json()
///         errors.append(
///             ctx.new_sub_error(
///                 category="category",
///                 message=error_json["message"],
///                 file=error_json["path"],
///                 lnum=error_json["line"],
///                 col=error_json["col"],
///             ),
///         )
///
///     return errors
/// ```
#[starlark_module]
fn artifact_value_methods(builder: &mut MethodsBuilder) {
    /// Reads the entire contents of the artifact as a string.
    fn read_string(this: &StarlarkArtifactValue) -> starlark::Result<String> {
        let path = this.fs.resolve(&this.path);
        let contents = fs_util::read_to_string(path)
            // input path from starlark
            .categorize_input()
            .tag(ErrorTag::StarlarkValue)?;
        Ok(contents)
    }

    /// Reads and parses the artifact as JSON
    fn read_json<'v>(this: &StarlarkArtifactValue, heap: Heap<'v>) -> starlark::Result<Value<'v>> {
        let path = this.fs.resolve(&this.path);
        let file = fs_util::open_file(&path)
            // input path from starlark
            .categorize_input()
            .tag(ErrorTag::StarlarkValue)?;
        let reader = BufReader::new(file);
        let value: serde_json::Value = serde_json::from_reader(reader)
            .with_buck_error_context(|| format!("Error parsing JSON file `{path}`"))?;
        json_convert(value, heap)
    }
}

#[starlark_module]
#[starlark_types(StarlarkArtifactValue as ArtifactValue)]
pub(crate) fn register_artifact_value(globals: &mut GlobalsBuilder) {}

#[cfg(test)]
mod tests {

    use starlark::values::Heap;

    use super::*;

    #[test]
    fn test_json_convert() {
        Heap::temp(|heap| {
            let testcase = "{\"test\": [1, true, \"pi\", 7.5, {}]}";
            let value: serde_json::Value = serde_json::from_str(testcase).unwrap();
            let res = json_convert(value, heap).unwrap().to_repr();
            assert_eq!(res, testcase.replace("true", "True"))
        });
    }
}
