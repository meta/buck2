/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_wrapper_common::DOT_BUCKCONFIG_D;
use buck2_wrapper_common::EXPERIMENTS_FILENAME;
use const_format::concatcp;

const EXPERIMENT_PATH_SUFFIX: &str = concatcp!("/", DOT_BUCKCONFIG_D, "/", EXPERIMENTS_FILENAME);

const EXPERIMENTS: &str = "experiments";

pub fn get_experiment_tags(components: &[buck2_data::BuckconfigComponent]) -> Vec<String> {
    let mut init = Vec::new();
    for component in components {
        use buck2_data::buckconfig_component::Data;
        if let Some(Data::GlobalExternalConfigFile(external_config_file)) = &component.data
            && external_config_file
                .origin_path
                .ends_with(EXPERIMENT_PATH_SUFFIX)
        {
            external_config_file.values.iter().for_each(|config_value| {
                if config_value.section == EXPERIMENTS {
                    // all enabled GK experiments have their value set to true by definition
                    init.push(format!("{}.{}", EXPERIMENTS, config_value.key.clone()));
                }
            });
        }
    }
    init
}
