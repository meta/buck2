/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Parse some inputs to a `[`StarlarkUserEvent`].

use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_core::fs::project::ProjectRoot;
use buck2_data::StarlarkUserEvent;
use buck2_data::StarlarkUserMetadataDictValue;
use buck2_data::StarlarkUserMetadataListValue;
use buck2_data::StarlarkUserMetadataValue;
use buck2_data::starlark_user_metadata_value::Value::BoolValue;
use buck2_data::starlark_user_metadata_value::Value::DictValue;
use buck2_data::starlark_user_metadata_value::Value::IntValue;
use buck2_data::starlark_user_metadata_value::Value::ListValue;
use buck2_data::starlark_user_metadata_value::Value::StringValue;
use buck2_hash::IntentionallyStdHashMap;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::dict::DictRef;
use starlark::values::dict::DictType;
use starlark::values::float::UnpackFloat;
use starlark::values::list::ListRef;
use starlark::values::string::StarlarkStr;

use super::artifacts::EnsuredArtifact;
use super::context::output::get_artifact_path_display;

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Input)]
enum StarlarkUserEventUnpack {
    #[error("Metadata keys should be strings. Got type: `{0}`")]
    InvalidKey(String),
    #[error(
        "Metadata values should be strings, ints, bools, or dicts/lists of the mentioned types. Key `{0}` had value type `{1}`"
    )]
    InvalidValue(String, String),
}

pub(crate) struct StarlarkUserEventParser<'v> {
    pub(crate) artifact_fs: &'v ArtifactFs,
    pub(crate) project_fs: &'v ProjectRoot,
}

impl<'v> StarlarkUserEventParser<'v> {
    pub(crate) fn parse(
        &self,
        id: &str,
        metadata: ValueOf<'v, DictType<StarlarkStr, Value<'v>>>,
    ) -> buck2_error::Result<StarlarkUserEvent> {
        Ok(StarlarkUserEvent {
            id: id.to_owned(),
            metadata: self.unpack_metadata_map(metadata.dict())?,
        })
    }

    fn unpack_metadata_map(
        &self,
        metadata: DictRef<'v>,
    ) -> buck2_error::Result<IntentionallyStdHashMap<String, StarlarkUserMetadataValue>> {
        metadata
            .iter()
            .map(|(k, v)| {
                let k = match k.unpack_str() {
                    Some(k) => k.to_owned(),
                    None => {
                        return Err(
                            StarlarkUserEventUnpack::InvalidKey(k.get_type().to_owned()).into()
                        );
                    }
                };

                let v = self.get_metadata_value(&k, v)?;
                Ok((k, v))
            })
            .collect::<buck2_error::Result<IntentionallyStdHashMap<_, _>>>()
    }

    fn get_metadata_value(
        &self,
        k: &str,
        v: Value<'v>,
    ) -> buck2_error::Result<StarlarkUserMetadataValue> {
        if let Some(v) = v.unpack_str() {
            Ok(StarlarkUserMetadataValue {
                value: Some(StringValue(v.into())),
            })
        } else if let Some(v) = v.unpack_bool() {
            Ok(StarlarkUserMetadataValue {
                value: Some(BoolValue(v)),
            })
        } else if let Some(v) = v.unpack_i32() {
            Ok(StarlarkUserMetadataValue {
                value: Some(IntValue(v)),
            })
        // Let's also accept floats since `instant()` methods return floats, but cast them to ints
        } else if let Some(v) = UnpackFloat::unpack_value(v)? {
            Ok(StarlarkUserMetadataValue {
                value: Some(IntValue(v.0 as i32)),
            })
        } else if let Some(v) = <&EnsuredArtifact>::unpack_value(v)? {
            let path = get_artifact_path_display(
                v.get_artifact_path(),
                v.abs(),
                self.project_fs,
                self.artifact_fs,
            )?;
            Ok(StarlarkUserMetadataValue {
                value: Some(StringValue(path)),
            })
        } else if let Some(dict) = DictRef::from_value(v) {
            let dict = self.unpack_metadata_map(dict)?;
            let dict = StarlarkUserMetadataDictValue { value: dict };
            Ok(StarlarkUserMetadataValue {
                value: Some(DictValue(dict)),
            })
        } else if let Some(v) = ListRef::from_value(v) {
            let list = v
                .iter()
                .map(|e| self.get_metadata_value(k, e))
                .collect::<buck2_error::Result<Vec<_>>>()?;
            let list = StarlarkUserMetadataListValue { value: list };
            Ok(StarlarkUserMetadataValue {
                value: Some(ListValue(list)),
            })
        } else {
            Err(StarlarkUserEventUnpack::InvalidValue(k.to_owned(), v.get_type().to_owned()).into())
        }
    }
}
