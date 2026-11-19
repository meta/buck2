/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Shared transfer-cost assumptions for DAG replay models.

use crate::analyses::Stages;

/// Byte-size inputs to a hypothetical transfer model, not measured traffic.
#[derive(Clone, Copy, Debug, Default)]
pub struct TransferBytes {
    /// Declared logical input bytes (`args.bytes_in`), including files
    /// that may already be cache-resident. A proxy for preparation and
    /// the client input upload when the
    /// capture did not record the upload batch size. A proxy, not a
    /// bound: directory and inline blobs and retries are not in it, and
    /// storage already held most of the declared files
    pub bytes_in: i64,
    /// Declared logical output bytes (`args.bytes_out`), not measured traffic.
    pub bytes_out: i64,
    /// the instrumented CAS upload call inside the input-upload band:
    /// `None` when the capture has no sub-stages, `Some(0)` when the
    /// producer measured no transfer at all
    pub iu_cas_call_us: Option<i64>,
    /// the upload batch size the producer recorded (nominal, before
    /// retries); `None` when the capture did not record it
    pub iu_bytes: Option<i64>,
    /// A rejected call duration or batch size blocks only this upload leg;
    /// absence from an older capture may still use the documented proxy.
    pub iu_invalid: bool,
}

/// Modelled change to one action's duration when its recorded transfer
/// stages are replaced by byte-size / `bps`. `None` when no leg is
/// priceable or the rate is invalid. Logical sizes are not actual
/// network traffic; this model can overstate even a local preparation.
///
/// - input fetch: `bytes_in` against `materialize_us`;
/// - client input upload: the recorded batch size (or `bytes_in` as a
///   proxy when the capture has none) against the CAS call when the
///   capture has the sub-stage, because the permit wait, presence check and
///   local materialization inside that band are not transfer and do
///   not shrink with the link, else against the whole band. A measured
///   zero CAS call moved nothing and is left alone; a zero-byte batch
///   with a positive call has no recorded bytes to price: control work,
///   coordination or another cause may explain the wait, but the cause
///   is not established by the byte count;
/// - legacy `upload_us` (worker output upload, plus the client input
///   upload on pre-split captures): `bytes_out`, the only byte count
///   whose direction matches any of it.
///
/// The DAG what-if scenarios share this transfer pricing helper.
pub fn transfer_delta(st: &Stages, bytes: TransferBytes, bps: f64) -> Option<i64> {
    if !bps.is_finite() || bps <= 0.0 {
        return None;
    }
    let modelled = |b: i64| (b as f64 / bps * 1e6) as i64;
    let mut delta = 0i64;
    let mut touched = false;
    if bytes.bytes_in > 0 && st.materialize_us > 0 {
        delta = delta.saturating_add(modelled(bytes.bytes_in).saturating_sub(st.materialize_us));
        touched = true;
    }
    if st.input_upload_us > 0 && !bytes.iu_invalid {
        let transfer = bytes.iu_cas_call_us.unwrap_or(st.input_upload_us);
        let moved = bytes.iu_bytes.unwrap_or(bytes.bytes_in);
        if transfer > 0 && moved > 0 {
            delta = delta.saturating_add(modelled(moved).saturating_sub(transfer));
            touched = true;
        }
    }
    if bytes.bytes_out > 0 && st.upload_us > 0 {
        delta = delta.saturating_add(modelled(bytes.bytes_out).saturating_sub(st.upload_us));
        touched = true;
    }
    touched.then_some(delta)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One action: fetch 1s and client input upload 10s for 100 MiB of
    /// input, 1 MiB of output, 1s execute. These are the transfer deltas
    /// applied to the DAG weights for the same row.
    #[test]
    fn transfer_delta_prices_each_leg_with_its_own_bytes() {
        let mib = 1i64 << 20;
        let st = Stages {
            materialize_us: 1_000_000,
            input_upload_us: 10_000_000,
            execute_us: 1_000_000,
            ..Default::default()
        };
        let bytes = TransferBytes {
            bytes_in: 100 * mib,
            bytes_out: mib,
            iu_cas_call_us: None,
            iu_bytes: None,
            iu_invalid: false,
        };
        // 100 MiB/s: the fetch is already at rate (1s - 1s), the upload
        // leg shrinks from 10s to 1s; -9s in total. The old pricing
        // charged the upload with the 1 MiB of output bytes.
        assert_eq!(
            transfer_delta(&st, bytes, (100 * mib) as f64),
            Some(-9_000_000)
        );
        // infinite: both legs go to zero, only the execute stays
        assert_eq!(
            transfer_delta(&st, bytes, (1i64 << 60) as f64),
            Some(-11_000_000)
        );
        // instrumented: only the 2s CAS call inside the 10s band is
        // transfer; the permit wait and presence check do not shrink
        let instrumented = TransferBytes {
            iu_cas_call_us: Some(2_000_000),
            ..bytes
        };
        assert_eq!(
            transfer_delta(&st, instrumented, (100 * mib) as f64),
            Some(-1_000_000)
        );
        // nothing priceable: no bytes
        assert_eq!(
            transfer_delta(&st, TransferBytes::default(), (100 * mib) as f64),
            None
        );
    }

    /// The producer's fresh-inputs shape: 9 s parked on the upload
    /// permit, 1 s on the presence check, nothing missing, so the CAS call
    /// is a measured zero. There is no transfer for a faster link to
    /// shorten, and an uninstrumented band is still replaced whole.
    #[test]
    fn measured_zero_cas_call_is_not_a_transfer_to_speed_up() {
        let mib = 1i64 << 20;
        let st = Stages {
            input_upload_us: 10_000_000,
            execute_us: 1_000_000,
            ..Default::default()
        };
        let measured_zero = TransferBytes {
            bytes_in: 100 * mib,
            bytes_out: 0,
            iu_cas_call_us: Some(0),
            iu_bytes: None,
            iu_invalid: false,
        };
        assert_eq!(transfer_delta(&st, measured_zero, (100 * mib) as f64), None);
        assert_eq!(
            transfer_delta(&st, measured_zero, (1i64 << 60) as f64),
            None
        );
        // absent instrumentation: the whole band is the transfer, as before
        let uninstrumented = TransferBytes {
            iu_cas_call_us: None,
            ..measured_zero
        };
        assert_eq!(
            transfer_delta(&st, uninstrumented, (100 * mib) as f64),
            Some(-9_000_000)
        );
        assert_eq!(
            transfer_delta(&st, uninstrumented, (1i64 << 60) as f64),
            Some(-10_000_000)
        );
        // with a fetch leg beside it, only the fetch re-prices
        let with_fetch = Stages {
            materialize_us: 1_000_000,
            ..st
        };
        assert_eq!(
            transfer_delta(&with_fetch, measured_zero, (100 * mib) as f64),
            Some(0)
        );
        assert_eq!(
            transfer_delta(&with_fetch, measured_zero, (1i64 << 60) as f64),
            Some(-1_000_000)
        );
    }

    /// When the producer recorded the upload batch size, the upload leg
    /// is priced with it rather than with the declared input size; a
    /// zero-byte batch behind a positive call has no transfer to price;
    /// the byte count alone does not explain what the call waited for.
    #[test]
    fn recorded_upload_bytes_price_the_upload_leg_over_the_declared_size() {
        let mib = 1i64 << 20;
        let st = Stages {
            input_upload_us: 10_000_000,
            ..Default::default()
        };
        let bytes = TransferBytes {
            bytes_in: 100 * mib,
            bytes_out: 0,
            iu_cas_call_us: Some(2_000_000),
            iu_bytes: Some(10 * mib),
            iu_invalid: false,
        };
        // 10 MiB at 100 MiB/s = 100 ms replaces the 2 s call; the
        // declared 100 MiB would have priced it at 1 s
        assert_eq!(
            transfer_delta(&st, bytes, (100 * mib) as f64),
            Some(100_000 - 2_000_000)
        );
        let deduped = TransferBytes {
            iu_bytes: Some(0),
            ..bytes
        };
        assert_eq!(transfer_delta(&st, deduped, (100 * mib) as f64), None);
    }

    #[test]
    fn rejected_upload_measurements_do_not_fall_back_or_hide_other_legs() {
        let bytes = TransferBytes {
            bytes_in: 100,
            bytes_out: 50,
            iu_invalid: true,
            ..Default::default()
        };
        let stages = Stages {
            input_upload_us: 900,
            ..Default::default()
        };
        assert_eq!(transfer_delta(&stages, bytes, 1e12), None);
        let stages = Stages {
            materialize_us: 200,
            upload_us: 100,
            ..stages
        };
        assert_eq!(transfer_delta(&stages, bytes, 1e12), Some(-300));
    }
}
