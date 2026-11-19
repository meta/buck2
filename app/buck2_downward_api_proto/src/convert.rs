/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_error::BuckErrorContext;
use buck2_hash::BuckMutMap;
use tracing::Level;

use crate::proto;

impl TryInto<BuckMutMap<String, String>> for proto::Event {
    type Error = buck2_error::Error;

    fn try_into(self) -> Result<BuckMutMap<String, String>, Self::Error> {
        use std::collections::hash_map::Entry;

        use proto::event::Item;

        let proto::Event { items } = self;

        let mut ret = BuckMutMap::default();
        for Item { key, value } in items {
            match ret.entry(key) {
                Entry::Vacant(e) => {
                    e.insert(value);
                }
                Entry::Occupied(e) => {
                    return Err(buck2_error::buck2_error!(
                        buck2_error::ErrorTag::Tier0,
                        "Duplicate key: {}",
                        e.key()
                    ));
                }
            }
        }

        Ok(ret)
    }
}

impl<T> From<T> for proto::Event
where
    T: IntoIterator<Item = (String, String)>,
{
    fn from(iter: T) -> proto::Event {
        use proto::event::Item;

        let items = iter
            .into_iter()
            .map(|(key, value)| Item { key, value })
            .collect();

        proto::Event { items }
    }
}

impl TryInto<Level> for proto::LogLevel {
    type Error = buck2_error::Error;

    fn try_into(self) -> Result<Level, Self::Error> {
        use proto::log_level::Value;

        let proto::LogLevel { value } = self;
        let value = Value::try_from(value).buck_error_context("Invalid `value`")?;

        Ok(match value {
            Value::NotSet => {
                return Err(buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Input,
                    "Missing `value`"
                ));
            }
            Value::Trace => Level::TRACE,
            Value::Debug => Level::DEBUG,
            Value::Info => Level::INFO,
            Value::Warn => Level::WARN,
            Value::Error => Level::ERROR,
        })
    }
}

impl TryFrom<Level> for proto::LogLevel {
    type Error = buck2_error::Error;

    fn try_from(level: Level) -> Result<proto::LogLevel, Self::Error> {
        use proto::log_level::Value;

        let value = match level {
            v if v == Level::TRACE => Value::Trace,
            v if v == Level::DEBUG => Value::Debug,
            v if v == Level::INFO => Value::Info,
            v if v == Level::WARN => Value::Warn,
            v if v == Level::ERROR => Value::Error,
            v => {
                return Err(buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Input,
                    "Unsupported Level: {:?}",
                    v
                ));
            }
        };

        Ok(proto::LogLevel {
            value: value as i32,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_level_roundtrip() {
        for v in &[
            Level::TRACE,
            Level::DEBUG,
            Level::INFO,
            Level::WARN,
            Level::ERROR,
        ] {
            let p = proto::LogLevel::try_from(*v).unwrap();
            let v2: Level = p.try_into().unwrap();
            assert_eq!(*v, v2);
        }
    }
}
