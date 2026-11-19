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
use std::str::FromStr;
use std::sync::LazyLock;

use anyhow::Context;
use regex::Regex;

#[derive(Clone, Default, PartialEq, Eq, Hash, Debug)]
pub struct TDigest {
    pub hash: String,
    pub size_in_bytes: i64,
    pub _dot_dot: (),
}

impl Display for TDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.hash, self.size_in_bytes)
    }
}

impl FromStr for TDigest {
    type Err = anyhow::Error;

    fn from_str(digest: &str) -> anyhow::Result<TDigest> {
        static DIGEST_RE: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new("^([0-9a-f]+):([0-9]+)$").expect("Failed to compile digest regex")
        });

        let matches = DIGEST_RE
            .captures(digest)
            .with_context(|| format!("Digest format not valid: {digest}"))?;
        Ok(TDigest {
            hash: matches[1].to_string(),
            size_in_bytes: matches[2].parse::<i64>().with_context(|| {
                format!("Digest size {} could not be parsed as a i64", &matches[2])
            })?,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::TDigest;

    /// Only `<lowercase hex>:<decimal>` parses.
    #[test]
    fn test_from_str_rejects_junk_around_the_digest() {
        assert!("XYZabc:12:junk".parse::<TDigest>().is_err());
        assert!("ABCDEF0123:5".parse::<TDigest>().is_err());
        let digest: TDigest = "abc:12".parse().unwrap();
        assert_eq!(digest.hash, "abc");
        assert_eq!(digest.size_in_bytes, 12);
    }
}
