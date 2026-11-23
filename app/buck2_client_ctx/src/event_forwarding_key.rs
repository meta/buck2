/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::buck2_env;
use itertools::Itertools;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum EventForwardingKeyError {
    #[error("Event forwarding key `{0}` contains a comma; repeat the flag to give several keys")]
    Comma(String),
}

/// Parses one `--event-forwarding-key` flag, which gives one key. A key cannot contain a comma, as
/// the event forwarding helper gets a command's keys separated by commas.
pub fn parse_event_forwarding_key(key: &str) -> buck2_error::Result<String> {
    if key.contains(',') {
        return Err(EventForwardingKeyError::Comma(key.to_owned()).into());
    }
    Ok(key.to_owned())
}

/// The event forwarding keys of this command: those in the `BUCK2_EVENT_FORWARDING_KEY`
/// environment variable, separated by commas, then one for each `--event-forwarding-key` flag in
/// `flags`. Keys are trimmed, and blank keys and repeats are dropped.
///
/// The keys are opaque: the client passes them to the daemon in a field of their own rather than in
/// client metadata, and the daemon gives them to the event forwarding helper that Buck settings
/// name.
pub fn event_forwarding_keys(flags: &[String]) -> buck2_error::Result<Vec<String>> {
    let variable = buck2_env!("BUCK2_EVENT_FORWARDING_KEY")?.unwrap_or_default();
    Ok(keys(variable, flags))
}

fn keys(variable: &str, flags: &[String]) -> Vec<String> {
    variable
        .split(',')
        .chain(flags.iter().map(String::as_str))
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .unique()
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_come_from_the_variable_then_the_flags_trimmed_and_once_each() {
        let flags = [" ", "a", "c"].map(str::to_owned);
        assert_eq!(keys("b, a,", &flags), ["b", "a", "c"]);
    }
}
