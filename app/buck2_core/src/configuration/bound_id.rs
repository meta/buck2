/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use crate::configuration::bound_label::BoundConfigurationLabel;
use crate::configuration::hash::ConfigurationHash;

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum BoundConfigurationIdError {
    #[error("Bound configuration id must contain a hash, got: `{0}`")]
    MissingHash(String),
}

#[derive(derive_more::Display, Eq, PartialEq, Clone, Debug)]
#[display("{}#{}", label, hash)]
pub struct BoundConfigurationId {
    pub label: BoundConfigurationLabel,
    pub hash: ConfigurationHash,
}

impl BoundConfigurationId {
    pub fn parse(id: &str) -> buck2_error::Result<BoundConfigurationId> {
        let (label, hash) = id
            .split_once('#')
            .ok_or_else(|| BoundConfigurationIdError::MissingHash(id.to_owned()))?;
        let label = BoundConfigurationLabel::new(label.to_owned())?;
        let hash = ConfigurationHash::from_str(hash)?;
        Ok(BoundConfigurationId { label, hash })
    }
}

#[cfg(test)]
mod tests {
    use crate::configuration::bound_id::BoundConfigurationId;

    #[test]
    fn test_parse() {
        assert_eq!(
            "foo//:bar#0123456789abcdef",
            BoundConfigurationId::parse("foo//:bar#0123456789abcdef")
                .unwrap()
                .to_string()
        );
    }
}
