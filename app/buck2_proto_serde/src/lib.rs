/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

pub mod serialize_timestamp {
    use serde::Deserialize;
    use serde::Deserializer;
    use serde::Serialize;
    use serde::Serializer;

    pub fn serialize<S>(
        value: &Option<::prost_types::Timestamp>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let d = value.as_ref().map(|v| (v.seconds, v.nanos));
        d.serialize(serializer)
    }

    pub fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<Option<::prost_types::Timestamp>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let d = Option::<(i64, i32)>::deserialize(deserializer)?;
        let d = d.map(|(seconds, nanos)| ::prost_types::Timestamp { seconds, nanos });
        Ok(d)
    }
}

pub mod serialize_duration_as_micros {
    use serde::Deserialize;
    use serde::Deserializer;
    use serde::Serialize;
    use serde::Serializer;

    pub fn serialize<S>(
        value: &Option<::prost_types::Duration>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        value.as_ref().map(to_micros).serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<::prost_types::Duration>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Option::<i64>::deserialize(deserializer)?;
        Ok(value.map(from_micros))
    }

    const MICROS_IN_SECOND: i64 = 1000000;
    const NANOS_IN_MICRO: i32 = 1000;

    /// We saturate here since technically not all ::prost_types::Duration would fit (but all those
    /// we care about would).
    fn to_micros(duration: &::prost_types::Duration) -> i64 {
        let mut micros: i64 = 0;
        micros = micros.saturating_add(duration.seconds.saturating_mul(MICROS_IN_SECOND));
        micros = micros.saturating_add(duration.nanos.saturating_div(NANOS_IN_MICRO).into());
        micros
    }

    fn from_micros(duration: i64) -> ::prost_types::Duration {
        let seconds = duration / MICROS_IN_SECOND;
        let nanos = ((duration % MICROS_IN_SECOND) as i32) * NANOS_IN_MICRO;
        prost_types::Duration {
            seconds,
            nanos: nanos as _,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_value() {
            assert_eq!(
                to_micros(&::prost_types::Duration {
                    seconds: 1,
                    nanos: 1000
                }),
                1000001
            );
        }

        #[test]
        fn test_roundtrip() {
            for v in [-1, 10, i64::MAX, i64::MIN, 0] {
                assert_eq!(to_micros(&from_micros(v)), v);
            }
        }
    }
}
