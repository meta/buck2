/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::ffi::OsString;
use std::str::FromStr;

use buck2_error::buck2_error;
use dupe::Dupe;
use os_str_bytes::OsStrBytes;

/// Returns true or false for percentage-based feature rollouts based on a configuration string.
/// Configurations supported today are random and hostname.
///
/// - Daemon: Enabled by setting `daemon:<value>`, ex. `daemon:0.5`.
///   Checks whether to enable feature based on a random roll on daemon startup.
///
/// - Hostname: Set by `hostname:<value>`, ex. `hostname:0.5`, or by setting value directly,
///   ex. `0.5`. Checks whether to roll out feature based on hash of hostname. Useful when you
///   want the same host to consistently get the same feature enabled/disabled.
///
/// It's possible to extend this system to support per-username rollout as well in addition to
/// per-host rollout.
#[derive(Copy, Clone, Dupe, Debug)]
pub struct RolloutPercentage {
    inner: Inner,
}

impl RolloutPercentage {
    pub fn roll(&self) -> bool {
        self.roll_inner(|| hostname::get().ok())
    }

    fn roll_inner<F>(&self, get_hostname: F) -> bool
    where
        F: FnOnce() -> Option<OsString>,
    {
        match self.inner {
            Inner::Hostname(pct) => {
                match get_hostname() {
                    Some(h) => {
                        let hash = blake3::hash(&h.to_raw_bytes());
                        // For simplicity, we just divide the value of the first byte by 256 to get a
                        // decimal value to compare against our percentage. Note that the first byte
                        // can never exceed 255, so if we set the percentage to 100% we return true.
                        // TODO(scottcao): Use get_shard internally
                        (hash.as_bytes()[0] as f64 / 256_f64) < pct
                    }
                    None => {
                        crate::soft_error_or_panic!(
                            "hostname_unavailable",
                            buck2_error::internal_error!("Unable to obtain hostname")
                        );
                        pct == 1f64 // 1.0 is an exact value in floats, we *can* compare that one.
                    }
                }
            }
            Inner::Rate(pct) => rand::random::<f64>() < pct,
            Inner::Bool(b) => b,
        }
    }

    pub fn from_bool(val: bool) -> Self {
        Self {
            inner: Inner::Bool(val),
        }
    }

    /// Whether every roll on one host answers the same. False for the `daemon:<value>` form,
    /// which rolls afresh each time, so it cannot configure anything that several processes on
    /// a host must agree on.
    pub fn is_stable_per_host(&self) -> bool {
        !matches!(self.inner, Inner::Rate(_))
    }

    pub fn never() -> Self {
        Self::from_bool(false)
    }

    pub fn always() -> Self {
        Self::from_bool(true)
    }
}

impl FromStr for RolloutPercentage {
    type Err = buck2_error::Error;

    fn from_str(val: &str) -> Result<Self, Self::Err> {
        Ok(Self {
            inner: val.parse()?,
        })
    }
}

#[derive(Copy, Clone, Dupe, Debug)]
enum Inner {
    Rate(f64),
    Hostname(f64),
    Bool(bool),
}

impl FromStr for Inner {
    type Err = buck2_error::Error;

    fn from_str(val: &str) -> Result<Self, Self::Err> {
        if let Some(val) = val.strip_prefix("hostname:") {
            return Ok(Inner::Hostname(rate(val.parse()?)?));
        }

        if let Some(val) = val.strip_prefix("daemon:") {
            return Ok(Inner::Rate(rate(val.parse()?)?));
        }

        if let Ok(val) = val.parse() {
            return Ok(Inner::Hostname(rate(val)?));
        }

        if let Ok(val) = val.parse() {
            return Ok(Inner::Bool(val));
        }

        Err(buck2_error!(
            buck2_error::ErrorTag::Input,
            "RolloutPercentage must be either a float, a bool, a `daemon:<float>` or a `hostname:<float>` (got: {})",
            &val,
        ))
    }
}

/// A share of a keyed population, for rolling a feature out key by key rather than host by host.
/// `includes(key)` answers the same for a key wherever it is asked, so a rollout at a fraction
/// covers the same keys on every host, and raising the fraction only adds keys. Parsed from a
/// float in [0, 1].
#[derive(Copy, Clone, Dupe, Debug, Default, PartialEq)]
pub struct RolloutFraction(f64);

impl RolloutFraction {
    pub fn none() -> Self {
        Self(0.0)
    }

    pub fn all() -> Self {
        Self(1.0)
    }

    /// Whether no key is included.
    pub fn is_none(&self) -> bool {
        self.0 <= 0.0
    }

    pub fn includes(&self, key: &[u8]) -> bool {
        if self.0 >= 1.0 {
            return true;
        }
        if self.0 <= 0.0 {
            return false;
        }
        let hash = blake3::hash(key);
        let point = u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap());
        // `point` is uniform over the u64s; as a fraction of 2^64 it is uniform over [0, 1).
        (point as f64 / 2f64.powi(64)) < self.0
    }
}

impl FromStr for RolloutFraction {
    type Err = buck2_error::Error;

    fn from_str(val: &str) -> Result<Self, Self::Err> {
        let val: f64 = val.parse().map_err(|_| {
            buck2_error!(
                buck2_error::ErrorTag::Input,
                "RolloutFraction must be a float within [0,1] (got: {})",
                val
            )
        })?;
        Ok(Self(rate(val)?))
    }
}

fn rate(val: f64) -> buck2_error::Result<f64> {
    if (0.0..=1.0).contains(&val) {
        Ok(val)
    } else {
        Err(buck2_error!(
            buck2_error::ErrorTag::Input,
            "RolloutPercentage floats must be within [0,1] (got: {})",
            val
        ))
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use rand::distr::Alphanumeric;
    use rand::distr::SampleString;

    use super::*;

    #[test]
    fn test_parse_ok() {
        assert_matches!("true".parse(), Ok(Inner::Bool(true)));

        // NOTE: 0.5 and 1 are exact values for floats
        assert_matches!("0.5".parse(), Ok(Inner::Hostname(v)) if v == 0.5);
        assert_matches!("1".parse(), Ok(Inner::Hostname(v)) if v == 1.0);

        assert_matches!("daemon:0.5".parse(), Ok(Inner::Rate(v)) if v == 0.5);
        assert_matches!("hostname:0.5".parse(), Ok(Inner::Hostname(v)) if v == 0.5);
    }

    #[test]
    fn test_parse_err() {
        assert_matches!(Inner::from_str("foo"), Err(..));
        assert_matches!(Inner::from_str("-1"), Err(..));
        assert_matches!(Inner::from_str("1.1"), Err(..));
    }

    #[test]
    fn test_roll() {
        let hostname = || {
            Some(OsString::from(
                Alphanumeric.sample_string(&mut rand::rng(), 16),
            ))
        };

        assert!(
            RolloutPercentage {
                inner: Inner::Bool(true)
            }
            .roll_inner(hostname)
        );

        for _ in 0..1000 {
            assert!(
                RolloutPercentage {
                    inner: Inner::Rate(1.0)
                }
                .roll_inner(hostname)
            );
            assert!(
                !RolloutPercentage {
                    inner: Inner::Rate(0.0)
                }
                .roll_inner(hostname)
            );
        }

        for _ in 0..1000 {
            assert!(
                RolloutPercentage {
                    inner: Inner::Hostname(1.0)
                }
                .roll_inner(hostname)
            );
            assert!(
                !RolloutPercentage {
                    inner: Inner::Hostname(0.0)
                }
                .roll_inner(hostname)
            );
        }
    }

    #[test]
    fn test_fraction_parse() {
        assert_eq!(
            "0".parse::<RolloutFraction>().unwrap(),
            RolloutFraction::none()
        );
        assert_eq!(
            "1".parse::<RolloutFraction>().unwrap(),
            RolloutFraction::all()
        );
        assert_eq!(
            "0.25".parse::<RolloutFraction>().unwrap(),
            RolloutFraction(0.25)
        );
        assert_matches!(RolloutFraction::from_str("1.5"), Err(..));
        assert_matches!(RolloutFraction::from_str("true"), Err(..));
        assert_matches!(RolloutFraction::from_str(""), Err(..));
    }

    #[test]
    fn test_fraction_includes() {
        // Miri interprets each hash, so it checks a smaller sample.
        let count = if cfg!(miri) { 1_000u32 } else { 4_000 };
        let keys: Vec<Vec<u8>> = (0..count).map(|i| i.to_le_bytes().to_vec()).collect();
        for key in &keys {
            assert!(!RolloutFraction::none().includes(key));
            assert!(RolloutFraction::all().includes(key));
        }
        let mut previous = 0;
        for fraction in [0.1, 0.3, 0.5, 0.9] {
            let fraction = RolloutFraction(fraction);
            let included = keys.iter().filter(|key| fraction.includes(key)).count();
            // Within a few percent of the fraction, and a larger fraction keeps every key a
            // smaller one had.
            let expected = (fraction.0 * keys.len() as f64) as usize;
            assert!(
                included.abs_diff(expected) < keys.len() / 25,
                "{fraction:?}: {included}"
            );
            assert!(included > previous);
            previous = included;
        }
        let small = RolloutFraction(0.3);
        let large = RolloutFraction(0.6);
        for key in &keys {
            if small.includes(key) {
                assert!(large.includes(key));
            }
        }
    }

    #[test]
    fn test_roll_no_hostname() {
        let hostname = || None;

        assert!(
            !RolloutPercentage {
                inner: Inner::Hostname(0.99999)
            }
            .roll_inner(hostname)
        );

        assert!(
            RolloutPercentage {
                inner: Inner::Hostname(1.0)
            }
            .roll_inner(hostname)
        );
    }
}
