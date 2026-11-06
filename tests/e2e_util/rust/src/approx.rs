/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Comparing a number buck2 measures with an expected one, within a tolerance.

use std::fmt;

use serde_json::Value;

/// Compares equal to a number within an absolute tolerance ([`abs`](Self::abs)) or a relative one
/// ([`rel`](Self::rel)) of `expected`, so that a measured number can sit in an `assert_eq!`. With
/// neither, the tolerance is one in a million. A JSON value that is not a number compares unequal.
///
/// ```ignore
/// assert_eq!(report["artifact_count_sketch_cardinality"], approx(1.0).abs(0.5));
/// ```
#[derive(Clone, Copy)]
pub struct Approx {
    expected: f64,
    abs: Option<f64>,
    rel: Option<f64>,
}

/// A number that compares equal to `expected`, within a tolerance; see [`Approx`].
pub fn approx(expected: f64) -> Approx {
    Approx {
        expected,
        abs: None,
        rel: None,
    }
}

impl Approx {
    /// Matches a number at most `abs` away from the expected one.
    pub fn abs(mut self, abs: f64) -> Self {
        self.abs = Some(abs);
        self
    }

    /// Matches a number at most `rel` times the larger of the two away from the expected one.
    pub fn rel(mut self, rel: f64) -> Self {
        self.rel = Some(rel);
        self
    }

    /// Whether `actual` matches.
    pub fn matches(&self, actual: f64) -> bool {
        let (rel, abs) = match (self.rel, self.abs) {
            (None, None) => (1e-6, 1e-12),
            (rel, abs) => (rel.unwrap_or(0.0), abs.unwrap_or(0.0)),
        };
        if actual == self.expected {
            return true;
        }
        if actual.is_infinite() || self.expected.is_infinite() {
            return false;
        }
        let diff = (actual - self.expected).abs();
        diff <= rel * actual.abs().max(self.expected.abs()) || diff <= abs
    }
}

impl fmt::Debug for Approx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "approx({:?}", self.expected)?;
        if let Some(abs) = self.abs {
            write!(f, ", abs={abs:?}")?;
        }
        if let Some(rel) = self.rel {
            write!(f, ", rel={rel:?}")?;
        }
        write!(f, ")")
    }
}

impl PartialEq<Approx> for f64 {
    fn eq(&self, approx: &Approx) -> bool {
        approx.matches(*self)
    }
}

impl PartialEq<f64> for Approx {
    fn eq(&self, actual: &f64) -> bool {
        self.matches(*actual)
    }
}

impl PartialEq<Approx> for Value {
    fn eq(&self, approx: &Approx) -> bool {
        self.as_f64().is_some_and(|actual| approx.matches(actual))
    }
}

impl PartialEq<Value> for Approx {
    fn eq(&self, actual: &Value) -> bool {
        actual == self
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn test_the_default_tolerance_is_one_in_a_million() {
        assert_eq!(12.0, approx(12.0));
        assert_eq!(12.000001, approx(12.0), "1e-6 away, within 1e-6 * 12");
        assert_ne!(12.0001, approx(12.0));
    }

    #[test]
    fn test_an_absolute_tolerance() {
        assert_eq!(1.4, approx(1.0).abs(0.5));
        assert_eq!(0.5, approx(1.0).abs(0.5), "exactly `abs` away matches");
        assert_ne!(1.6, approx(1.0).abs(0.5));
    }

    #[test]
    fn test_a_relative_tolerance_scales_with_the_larger_number() {
        assert_eq!(110.0, approx(100.0).rel(0.1));
        assert_eq!(90.0, approx(100.0).rel(0.1), "10 is within 0.1 * 100");
        assert_ne!(80.0, approx(100.0).rel(0.1));
    }

    #[test]
    fn test_an_absolute_tolerance_alone_has_no_relative_one() {
        assert_ne!(1000.001, approx(1000.0).abs(0.0001));
    }

    #[test]
    fn test_infinities_match_only_themselves() {
        assert_eq!(f64::INFINITY, approx(f64::INFINITY));
        assert_ne!(f64::INFINITY, approx(1.0).abs(f64::INFINITY));
        assert_ne!(f64::NAN, approx(f64::NAN));
    }

    #[test]
    fn test_json_numbers_match_and_other_values_do_not() {
        assert_eq!(json!(12), approx(12.0));
        assert_eq!(json!(7.4), approx(7.5).abs(0.2));
        assert_ne!(json!(null), approx(0.0).abs(1.0));
        assert_ne!(json!("12"), approx(12.0));
        assert_eq!(approx(12.0), json!(12));
    }

    #[test]
    fn test_debug_shows_the_tolerances() {
        assert_eq!(format!("{:?}", approx(1.0)), "approx(1.0)");
        assert_eq!(
            format!("{:?}", approx(2.0).abs(0.5).rel(0.1)),
            "approx(2.0, abs=0.5, rel=0.1)"
        );
    }
}
