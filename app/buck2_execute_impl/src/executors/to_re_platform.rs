/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::execution_types::executor_config::RePlatformFields;
use remote_execution as RE;

pub trait RePlatformFieldsToRePlatform {
    fn to_re_platform(&self) -> RE::Platform;
}

impl RePlatformFieldsToRePlatform for RePlatformFields {
    fn to_re_platform(&self) -> RE::Platform {
        RE::Platform {
            properties: self
                .properties
                .iter()
                .flat_map(|(name, values)| {
                    values.iter().map(|value| RE::Property {
                        name: name.clone(),
                        value: value.clone(),
                    })
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use buck2_core::execution_types::executor_config::RePlatformFields;
    use buck2_hash::BuckFixedSortedMap;

    use crate::executors::to_re_platform::RePlatformFieldsToRePlatform;

    #[test]
    fn test_multiple_values_become_repeated_properties() {
        let platform = RePlatformFields {
            properties: Arc::new(BuckFixedSortedMap::from_iter([(
                "arch".to_owned(),
                vec!["x86_64".to_owned(), "aarch64".to_owned()],
            )])),
        }
        .to_re_platform();

        assert_eq!(platform.properties.len(), 2);
        assert_eq!(platform.properties[0].name, "arch");
        assert_eq!(platform.properties[0].value, "x86_64");
        assert_eq!(platform.properties[1].name, "arch");
        assert_eq!(platform.properties[1].value, "aarch64");
    }
}
