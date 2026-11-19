/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use crate::execute::action_digest::ActionDigest;

/// Determines whether events for this action digest should be sampled.
///
/// This sampling is stable for a single action digest; return `Some(sampling_rate)` if the digest
/// should be taken and `None` otherwise
pub fn should_sample_action_digest(digest: &ActionDigest) -> Option<u64> {
    let lg_sample_rate = buck2_core::buck2_env!(
        "BUCK2_ACTION_DIGEST_TRACE_LG_SAMPLE_RATE",
        type=u32,
        applicability = testing
    )
    .unwrap()
    .unwrap_or(14);
    let bytes = digest.raw_digest().as_bytes();
    if count_trailing_zero_bits(bytes) >= lg_sample_rate {
        Some(1 << lg_sample_rate)
    } else {
        None
    }
}

/// Counts the number of trailing zero bits in a byte slice.
fn count_trailing_zero_bits(bytes: &[u8]) -> u32 {
    let mut count = 0;
    // Iterate from the end of the byte array
    for &byte in bytes.iter().rev() {
        if byte == 0 {
            count += 8;
        } else {
            // Count trailing zeros in this byte
            count += byte.trailing_zeros();
            break;
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_trailing_zero_bits() {
        // All zeros
        assert_eq!(count_trailing_zero_bits(&[0, 0, 0, 0]), 32);

        // Single byte with trailing zeros
        assert_eq!(count_trailing_zero_bits(&[0b1111_0000]), 4);
        assert_eq!(count_trailing_zero_bits(&[0b1000_0000]), 7);
        assert_eq!(count_trailing_zero_bits(&[0b0000_0001]), 0);

        // Multiple bytes
        assert_eq!(count_trailing_zero_bits(&[0xFF, 0x00]), 8);
        assert_eq!(count_trailing_zero_bits(&[0xFF, 0x00, 0x00]), 16);
        assert_eq!(count_trailing_zero_bits(&[0xFF, 0b1111_1100, 0x00]), 10);

        // No trailing zeros
        assert_eq!(count_trailing_zero_bits(&[0x01]), 0);
        assert_eq!(count_trailing_zero_bits(&[0xFF, 0x01]), 0);
    }
}
