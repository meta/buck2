/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Debug;

use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_core::target::label::label::TargetLabel;

pub trait CommandExecutionTarget: Send + Sync + Debug {
    fn re_action_key(&self) -> String;

    fn re_affinity_key(&self) -> String;

    fn as_proto_action_key(&self) -> buck2_data::ActionKey;

    fn as_proto_action_name(&self) -> buck2_data::ActionName;

    /// Optional mnemonic describing the action kind (e.g. `CxxCompile`).
    fn action_mnemonic(&self) -> Option<String> {
        None
    }

    /// Optional Bazel-style target label this action belongs to.
    ///
    /// This is sent as `RequestMetadata.target_id` to remote execution/cache
    /// servers. Keep the Buck2 configuration out of this field; the request
    /// metadata has a separate `configuration_id` field for that.
    fn target_label(&self) -> Option<String> {
        None
    }

    /// Optional hash identifying the build configuration of the target.
    fn configuration_hash(&self) -> Option<String> {
        None
    }
}

pub fn request_metadata_target_id(label: &ConfiguredTargetLabel) -> String {
    request_metadata_target_id_from_unconfigured(label.unconfigured())
}

pub fn request_metadata_target_id_from_unconfigured(label: &TargetLabel) -> String {
    let package = label.pkg();
    let cell_name = package.cell_name();
    let repository = if cell_name.as_str() == "root" {
        String::new()
    } else {
        format!("@{}", cell_name.as_str())
    };
    let package_path = package.cell_relative_path().as_str();

    if package_path.is_empty() {
        format!("{repository}//:{}", label.name())
    } else {
        format!("{repository}//{}:{}", package_path, label.name())
    }
}

#[cfg(test)]
mod tests {
    use buck2_core::configuration::data::ConfigurationData;
    use buck2_core::target::configured_target_label::ConfiguredTargetLabel;

    use super::*;

    #[test]
    fn request_metadata_target_id_uses_bazel_label_shape() {
        let root_target =
            ConfiguredTargetLabel::testing_parse("root//pkg:lib", ConfigurationData::testing_new());
        assert_eq!(request_metadata_target_id(&root_target), "//pkg:lib");

        let root_package_target =
            ConfiguredTargetLabel::testing_parse("root//:bin", ConfigurationData::testing_new());
        assert_eq!(request_metadata_target_id(&root_package_target), "//:bin");

        let external_cell_target = ConfiguredTargetLabel::testing_parse(
            "prelude//platforms:default",
            ConfigurationData::testing_new(),
        );
        assert_eq!(
            request_metadata_target_id(&external_cell_target),
            "@prelude//platforms:default"
        );
    }
}
