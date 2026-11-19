/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Computing expiration timestamps from TTLs reported by RE.

use std::fmt::Display;

use buck2_core::soft_error_or_panic;
use jiff::SignedDuration;
use jiff::Timestamp;

/// Whether an expiration had to be clamped to the representable datetime range.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Clamped {
    No,
    Min,
    Max,
}

/// Never panics: results outside the representable timestamp range are clamped to it, in the
/// direction of the TTL's sign.
fn expiration_from_ttl(now: Timestamp, ttl_seconds: i64) -> (Timestamp, Clamped) {
    match now.checked_add(SignedDuration::from_secs(ttl_seconds)) {
        Ok(expiration) => (expiration, Clamped::No),
        Err(_) if ttl_seconds >= 0 => (Timestamp::MAX, Clamped::Max),
        Err(_) => (Timestamp::MIN, Clamped::Min),
    }
}

/// Computes the expiration timestamp for something that RE reports as living for another
/// `ttl_seconds` seconds.
///
/// TTLs reported by RE are not trusted: values for which the expiration is not representable
/// produce a clamped expiration and a quiet soft error naming `what`, instead of a panic.
pub fn re_expiration_from_ttl(now: Timestamp, ttl_seconds: i64, what: &dyn Display) -> Timestamp {
    let (expiration, clamped) = expiration_from_ttl(now, ttl_seconds);
    if clamped != Clamped::No {
        soft_error_or_panic!(
            "re_digest_ttl_out_of_bounds",
            buck2_error::buck2_error!(
                buck2_error::ErrorTag::ReInvalidGetCasResponse,
                "RE returned a TTL outside the supported range; clamping the expiration of `{}`. TTL seconds: `{}`",
                what,
                ttl_seconds
            )
        );
    }
    expiration
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The S671995 pattern: a TTL in nanoseconds mistakenly reported in a seconds field.
    const NS_AS_SECONDS_TTL: i64 = 2_600_000_000_000_000;

    #[test]
    fn test_in_range() {
        let now = Timestamp::now();
        assert_eq!(
            expiration_from_ttl(now, 600),
            (now + SignedDuration::from_secs(600), Clamped::No)
        );
        assert_eq!(
            expiration_from_ttl(now, -3600),
            (now - SignedDuration::from_secs(3600), Clamped::No)
        );
        assert_eq!(expiration_from_ttl(now, 0), (now, Clamped::No));
    }

    #[test]
    fn test_expiration_exceeds_timestamp_range() {
        let now = Timestamp::now();
        assert_eq!(
            expiration_from_ttl(now, NS_AS_SECONDS_TTL),
            (Timestamp::MAX, Clamped::Max)
        );
        assert_eq!(
            expiration_from_ttl(now, -NS_AS_SECONDS_TTL),
            (Timestamp::MIN, Clamped::Min)
        );
        assert_eq!(
            expiration_from_ttl(now, i64::MAX),
            (Timestamp::MAX, Clamped::Max)
        );
        assert_eq!(
            expiration_from_ttl(now, i64::MIN),
            (Timestamp::MIN, Clamped::Min)
        );
    }
}
