/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::hash_map::DefaultHasher;
use std::env;
use std::fmt::Display;
use std::fmt::Formatter;
use std::hash::Hash;
use std::hash::Hasher;
use std::str::FromStr;

use allocative::Allocative;
use buck2_error::BuckErrorContext;
use dupe::Dupe;
use uuid::Uuid;

use crate::BUCK_WRAPPER_UUID_ENV_VAR;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Environment)]
enum TraceIdError {
    #[error("`{}` environment variable is not UTF-8", BUCK_WRAPPER_UUID_ENV_VAR)]
    EnvVarNotUtf8,
}

/// A TraceId is a unique identifier for a trace. Trace IDs are globally unique; their textual form is a v4 UUID.
///
/// TraceIds generally correspond to commands, but they do not have to, e.g. in the case of a Buck daemon producing
/// events even when a command is not running.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Allocative)]
pub struct TraceId(
    #[allocative(skip)] // `Uuid` is inline.
    Uuid,
);

// UUID is 16 bytes.
impl Dupe for TraceId {}

impl serde::ser::Serialize for TraceId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::de::Deserialize<'de> for TraceId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;

        <&str>::deserialize(deserializer)?
            .parse()
            .map_err(D::Error::custom)
    }
}

impl Display for TraceId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut buf = Uuid::encode_buffer();
        f.write_str(self.0.hyphenated().encode_lower(&mut buf))
    }
}

impl FromStr for TraceId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<TraceId, Self::Err> {
        Ok(TraceId(Uuid::parse_str(s)?))
    }
}

impl TraceId {
    /// Generates a new TraceId, suitable for identifying a particular trace.
    pub fn new() -> TraceId {
        TraceId(Uuid::new_v4())
    }

    pub fn null() -> TraceId {
        TraceId(Uuid::nil())
    }

    /// Returns the raw 16-byte UUID. Provided for callers that need byte-level
    /// access (e.g. sampling on a UUID nibble) without going through the
    /// textual form, which allocates.
    pub fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }

    /// Fetch `TraceId` from environment variable or generate a new one.
    pub fn from_env_or_new() -> buck2_error::Result<TraceId> {
        match env::var(BUCK_WRAPPER_UUID_ENV_VAR) {
            Ok(s) => Ok(TraceId::from_str(&s).with_buck_error_context(|| {
                format!("Parsing buck2 invocation id from env variable {BUCK_WRAPPER_UUID_ENV_VAR}")
            })?),
            Err(env::VarError::NotPresent) => Ok(TraceId::new()),
            Err(env::VarError::NotUnicode(_)) => Err(TraceIdError::EnvVarNotUtf8.into()),
        }
    }

    /// Generate short hash to be used as a message key for a Scribe client.
    pub fn hash(&self) -> i64 {
        let mut hasher = DefaultHasher::new();
        Hash::hash(self, &mut hasher);
        hasher.finish() as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let a = TraceId::new();
        let s = serde_json::to_string(&a).unwrap();
        let b = serde_json::from_str::<TraceId>(&s).unwrap();
        assert_eq!(a, b);
    }
}
