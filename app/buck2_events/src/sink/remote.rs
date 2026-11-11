/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! A Sink for forwarding events directly to Remote service.
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use fbinit::FacebookInit;

#[cfg(not(fbcode_build))]
pub use crate::sink::bes_client::BesChannelSettings;
#[cfg(not(fbcode_build))]
pub use crate::sink::bes_client::BesTls;
#[cfg(not(fbcode_build))]
pub use crate::sink::bes_client::DEFAULT_REPLAY_SPILL_MAX_BYTES;
#[cfg(not(fbcode_build))]
pub use crate::sink::bes_client::DEFAULT_RETRY_WINDOW_SECS;
#[cfg(not(fbcode_build))]
pub use crate::sink::scribe::BesEventFormat;
pub use crate::sink::scribe::RemoteEventConfig;
pub use crate::sink::scribe::RemoteEventSink;
pub(crate) use crate::sink::scribe::scribe_category;

fn new_remote_event_sink_if_fbcode(
    fb: FacebookInit,
    config: RemoteEventConfig,
) -> buck2_error::Result<Option<RemoteEventSink>> {
    #[cfg(not(fbcode_build))]
    if !config.bes_enabled() {
        return Ok(None);
    }
    Ok(Some(RemoteEventSink::new(fb, scribe_category()?, config)?))
}

pub fn new_remote_event_sink_if_enabled(
    fb: FacebookInit,
    config: RemoteEventConfig,
) -> buck2_error::Result<Option<RemoteEventSink>> {
    if is_enabled() {
        new_remote_event_sink_if_fbcode(fb, config)
    } else {
        Ok(None)
    }
}

/// Whether or not remote event logging is enabled for this process. It must be explicitly disabled via `disable()`.
static REMOTE_EVENT_SINK_ENABLED: AtomicBool = AtomicBool::new(true);

/// Returns whether this process should actually write to remote sink, even if it is fully supported by the platform and
/// binary.
pub fn is_enabled() -> bool {
    REMOTE_EVENT_SINK_ENABLED.load(Ordering::Relaxed)
}

/// Disables remote event logging for this process. Remote event logging must be disabled explicitly on startup, otherwise it is
/// on by default.
pub fn disable() {
    REMOTE_EVENT_SINK_ENABLED.store(false, Ordering::Relaxed);
}

#[cfg(not(fbcode_build))]
pub fn expand_bes_config_env_vars(raw: &str) -> String {
    expand_bes_config_env_vars_with(raw, |name| std::env::var(name).ok())
}

/// `[bes] header` entries as name and value, and the variables they named that were unset or
/// empty, which turn the BES sink off.
#[cfg(not(fbcode_build))]
pub type BesHeadersAndMissingVars = (Vec<(String, String)>, Vec<String>);

#[cfg(not(fbcode_build))]
pub fn expand_bes_config_env_vars_with<F>(raw: &str, env: F) -> String
where
    F: FnMut(&str) -> Option<String>,
{
    expand_bes_config_env_vars_reporting(raw, env).0
}

/// `expand_bes_config_env_vars_with`, which also names the variables that were unset or empty
/// and so expanded to nothing.
#[cfg(not(fbcode_build))]
pub fn expand_bes_config_env_vars_reporting<F>(raw: &str, mut env: F) -> (String, Vec<String>)
where
    F: FnMut(&str) -> Option<String>,
{
    let mut missing = Vec::new();
    let mut lookup = |name: &str| match env(name).filter(|value| !value.is_empty()) {
        Some(value) => value,
        None => {
            missing.push(name.to_owned());
            String::new()
        }
    };
    let mut expanded = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch != '$' {
            expanded.push(ch);
            continue;
        }

        match chars.peek().copied() {
            Some('{') => {
                chars.next();
                let mut name = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '}' {
                        closed = true;
                        break;
                    }
                    name.push(c);
                }

                if closed && is_env_name(&name) {
                    expanded.push_str(&lookup(&name));
                } else {
                    expanded.push('$');
                    expanded.push('{');
                    expanded.push_str(&name);
                    if closed {
                        expanded.push('}');
                    }
                }
            }
            Some(c) if is_env_name_start(c) => {
                let mut name = String::new();
                while let Some(c) = chars.peek().copied() {
                    if !is_env_name_char(c) {
                        break;
                    }
                    name.push(c);
                    chars.next();
                }
                expanded.push_str(&lookup(&name));
            }
            _ => expanded.push('$'),
        }
    }

    (expanded, missing)
}

/// The warning for a `bes.header` entry that names an unset variable, which turns BES off
/// rather than send the header empty: a server takes an empty API key for no key at all. It
/// names the variables and the key, never a value.
#[cfg(not(fbcode_build))]
pub fn missing_bes_header_env_warning(missing: &[String], whose: &str) -> String {
    let names = missing
        .iter()
        .map(|name| format!("${name}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "BES disabled: `bes.header` names {names}, which is not set in {whose} environment, so its value would be sent empty"
    )
}

#[cfg(not(fbcode_build))]
fn is_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(is_env_name_start) && chars.all(is_env_name_char)
}

#[cfg(not(fbcode_build))]
fn is_env_name_start(c: char) -> bool {
    c == '_' || c.is_ascii_alphabetic()
}

#[cfg(not(fbcode_build))]
fn is_env_name_char(c: char) -> bool {
    c == '_' || c.is_ascii_alphanumeric()
}

#[cfg(all(test, not(fbcode_build)))]
mod tests {
    use super::*;

    #[test]
    fn expands_bes_config_env_vars() {
        assert_eq!(
            expand_bes_config_env_vars_with(
                "key=$BUILDBUDDY_API_KEY,run=${BUILDBUDDY_RUN_ID},missing=$MISSING,literal=$9",
                test_env,
            ),
            "key=secret,run=run-id,missing=,literal=$9",
        );
    }

    #[test]
    fn reports_the_variables_that_expand_to_nothing() {
        let (expanded, missing) = expand_bes_config_env_vars_reporting(
            "key=$BUILDBUDDY_API_KEY,missing=$MISSING,empty=${EMPTY},literal=$9",
            |name| match name {
                "EMPTY" => Some(String::new()),
                other => test_env(other),
            },
        );
        assert_eq!(expanded, "key=secret,missing=,empty=,literal=$9");
        assert_eq!(missing, vec!["MISSING".to_owned(), "EMPTY".to_owned()]);

        let (literal, none) = expand_bes_config_env_vars_reporting("x-literal=value", test_env);
        assert_eq!((literal.as_str(), none.len()), ("x-literal=value", 0));
    }

    #[test]
    fn the_missing_header_warning_names_variables_never_values() {
        let warning =
            missing_bes_header_env_warning(&["BUILDBUDDY_API_KEY".to_owned()], "the daemon's");
        assert!(warning.contains("$BUILDBUDDY_API_KEY"), "{warning}");
        assert!(warning.contains("`bes.header`"), "{warning}");
        assert!(!warning.contains("secret"), "{warning}");
    }

    fn test_env(name: &str) -> Option<String> {
        match name {
            "BUILDBUDDY_API_KEY" => Some("secret".to_owned()),
            "BUILDBUDDY_RUN_ID" => Some("run-id".to_owned()),
            _ => None,
        }
    }
}
