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
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_build_api_derive::internal_provider;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_error::BuckErrorContext;
use buck2_hash::BuckMutInsertionMap;
use buck2_interpreter::types::configured_providers_label::StarlarkConfiguredProvidersLabel;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::values::Freeze;
use starlark::values::FreezeError;
use starlark::values::StarlarkPagable;
use starlark::values::StringValue;
use starlark::values::Trace;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::dict::DictType;

use crate as buck2_build_api;
use crate::interpreter::rule_defs::artifact::starlark_artifact_like::StarlarkArtifactView;

// Provider that signals a rule is installable (ex. android_binary)

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum InstallInfoProviderErrors {
    #[error("File with key `{key}`: `{artifact}` should not have any associated artifacts")]
    AssociatedArtifacts { key: String, artifact: String },
}

#[internal_provider(install_info_creator)]
#[derive(
    Clone,
    Debug,
    Freeze,
    Trace,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[repr(C)]
#[freeze(validator = validate_install_info)]
pub struct InstallInfo<'v> {
    // Label for the installer
    installer: ValueTyped<'v, StarlarkConfiguredProvidersLabel>,
    // list of files that need to be installed
    files: ValueOf<'v, DictType<StringValue<'v>, StarlarkArtifactView<'v>>>,
}

impl<'v> InstallInfo<'v> {
    pub fn get_installer(&self) -> ConfiguredProvidersLabel {
        self.installer.label().to_owned()
    }

    fn get_files_iter(
        &self,
    ) -> impl Iterator<Item = buck2_error::Result<(&'v str, StarlarkArtifactView<'v>)>> + 'v {
        self.files.iter().map(|entry| {
            let (k, v) = entry?;
            Ok((k.unpack().as_str(), v.unpack()))
        })
    }

    pub fn get_files(&self) -> buck2_error::Result<BuckMutInsertionMap<String, Artifact>> {
        self.get_files_iter()
            .map(|x| {
                let (k, v) = x?;
                Ok((
                    k.to_owned(),
                    v.as_input_artifact_like()
                        .get_bound_artifact()
                        .with_buck_error_context(|| format!("For key `{k}`"))?,
                ))
            })
            .collect()
    }
}

#[starlark_module]
fn install_info_creator(globals: &mut GlobalsBuilder) {
    fn InstallInfo<'v>(
        installer: ValueTyped<'v, StarlarkConfiguredProvidersLabel>,
        files: ValueOf<'v, DictType<StringValue<'v>, StarlarkArtifactView<'v>>>,
    ) -> starlark::Result<InstallInfo<'v>> {
        let info = InstallInfo { installer, files };
        validate_install_info(&info)?;
        Ok(info)
    }
}

fn validate_install_info<'v>(info: &InstallInfo<'v>) -> buck2_error::Result<()> {
    for x in info.get_files_iter() {
        let (k, v) = x?;
        if let Some(other_artifacts) = v.as_input_artifact_like().get_associated_artifacts() {
            if !other_artifacts.is_empty() {
                return Err(InstallInfoProviderErrors::AssociatedArtifacts {
                    key: k.to_owned(),
                    artifact: v.to_string(),
                }
                .into());
            }
        }
    }
    Ok(())
}
