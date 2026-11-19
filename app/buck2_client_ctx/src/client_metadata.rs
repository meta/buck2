/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::LazyLock;

use buck2_core::buck2_env;
use regex::Regex;

const SESSION_ID_KEY: &str = "session_id";

/// A key / value metadata pair provided by the client. This will be injected into Buck2's logging.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientMetadata {
    pub key: String,
    pub value: String,
}

impl ClientMetadata {
    pub fn to_proto(&self) -> buck2_data::ClientMetadata {
        buck2_data::ClientMetadata {
            key: self.key.clone(),
            value: self.value.clone(),
        }
    }

    pub fn from_env() -> buck2_error::Result<Vec<Self>> {
        let client_metadata_str = buck2_env!("BUCK2_CLIENT_METADATA")?.unwrap_or_default();
        if client_metadata_str.is_empty() {
            return Ok(vec![]);
        }
        let mut client_metadatas = client_metadata_str
            .split(',')
            .map(parse_client_metadata)
            .collect::<buck2_error::Result<Vec<_>>>()?;

        // Codex freezes BUCK2_CLIENT_METADATA before its thread (session) id
        // exists, so unlike Claude Code it can only ever contribute an
        // `invocation_id`, which is unjoinable to a conversation. It does export
        // CODEX_THREAD_ID into every tool subprocess, so recover the session id
        // from there when the metadata itself doesn't already carry one.
        //
        // Read with `std::env::var`, not `buck2_env!`: this is an ambient
        // variable owned by codex, not one buck2 defines, so it does not belong
        // in the registry behind `buck2 help-env`.
        let codex_thread_id = std::env::var("CODEX_THREAD_ID").ok();
        add_session_id_if_missing(&mut client_metadatas, codex_thread_id.as_deref());

        Ok(client_metadatas)
    }
}

/// Add `session_id=<fallback>` when the client metadata does not already carry a
/// `session_id`. The fallback is trimmed, and an absent or blank one is a no-op:
/// the value is compared verbatim against the trajectory `conversation_id`, so a
/// blank or padded session id is just as unjoinable as none at all while looking
/// joinable to anything reading the logs.
fn add_session_id_if_missing(metadatas: &mut Vec<ClientMetadata>, fallback: Option<&str>) {
    let Some(session_id) = fallback.map(str::trim).filter(|id| !id.is_empty()) else {
        return;
    };
    if metadatas
        .iter()
        .any(|metadata| metadata.key == SESSION_ID_KEY)
    {
        return;
    }
    metadatas.push(ClientMetadata {
        key: SESSION_ID_KEY.to_owned(),
        value: session_id.to_owned(),
    });
}

/// Prepend the legacy `-c client.id=` buckconfig value to the front of the id
/// list when it isn't already there. `-c` predates `--client-metadata id=`
/// and never lands in `client_metadata` on its own; without forwarding,
/// dropping the flattened `metadata["client"]` copy would lose this id.
/// Insert first so `-c` loses to `--client-metadata id=` in the daemon, where
/// the last id wins.
pub fn add_legacy_client_id_if_missing(
    metadatas: &mut Vec<ClientMetadata>,
    legacy_id: Option<&str>,
) {
    let Some(id) = legacy_id.map(str::trim).filter(|id| !id.is_empty()) else {
        return;
    };
    if metadatas
        .iter()
        .any(|metadata| metadata.key == "id" && metadata.value == id)
    {
        return;
    }
    metadatas.insert(
        0,
        ClientMetadata {
            key: "id".to_owned(),
            value: id.to_owned(),
        },
    );
}

/// Extract the effective `-c client.id=` value from raw `-c`/`--config` flag
/// values, in order. Later flags win, matching config override semantics, so a
/// trailing blank value (`-c client.id=`) unsets an earlier one.
/// Cell-qualified values (`cell//client.id=...`) don't match: they don't land
/// in the root `client` section the daemon reads either.
pub fn legacy_client_id_from_config_flags<'a>(
    config_values: impl Iterator<Item = &'a str>,
) -> Option<String> {
    let id = config_values
        .filter_map(|value| value.strip_prefix("client.id="))
        .last()?
        .trim();
    if id.is_empty() {
        return None;
    }
    Some(id.to_owned())
}

pub fn parse_client_metadata(value: &str) -> buck2_error::Result<ClientMetadata> {
    const REGEX_TEXT: &str = "^[a-z][a-z0-9]*(_[a-z][a-z0-9]*)*$";
    static REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(REGEX_TEXT).unwrap());

    let (key, value) = value
        .split_once('=')
        .ok_or_else(|| ClientMetadataError::InvalidFormat(value.to_owned()))?;

    if !REGEX.is_match(key) {
        return Err(ClientMetadataError::InvalidKey(key.to_owned()).into());
    }

    Ok(ClientMetadata {
        key: key.to_owned(),
        value: value.to_owned(),
    })
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
pub enum ClientMetadataError {
    #[error(
        "Invalid client metadata format: `{0}`. Client metadata keys must be a `key=value` pair."
    )]
    InvalidFormat(String),

    #[error(
        "Invalid client metadata key: `{0}`. Client metadata keys must be snake_case identifiers."
    )]
    InvalidKey(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse() {
        assert_eq!(
            parse_client_metadata("foo=bar").unwrap(),
            ClientMetadata {
                key: "foo".to_owned(),
                value: "bar".to_owned()
            }
        );
        assert!(parse_client_metadata("foo").is_err());
        assert!(parse_client_metadata("=foo").is_err());
    }

    fn metadata(key: &str, value: &str) -> ClientMetadata {
        ClientMetadata {
            key: key.to_owned(),
            value: value.to_owned(),
        }
    }

    #[test]
    fn test_add_session_id_when_absent() {
        // Standard codex metadata (id + invocation_id) gains a session_id.
        let mut metadatas = vec![
            metadata("id", "codex"),
            metadata("invocation_id", "codex_invocation_abc"),
        ];
        add_session_id_if_missing(&mut metadatas, Some("thread-1"));
        assert_eq!(metadatas.len(), 3);
        assert_eq!(metadatas[2], metadata("session_id", "thread-1"));
    }

    #[test]
    fn test_add_session_id_skips_when_already_present() {
        // Claude Code already injects session_id in-band; don't overwrite it.
        let mut metadatas = vec![
            metadata("id", "claude_code"),
            metadata("session_id", "real"),
        ];
        add_session_id_if_missing(&mut metadatas, Some("thread-1"));
        assert_eq!(metadatas.len(), 2);
        assert_eq!(metadatas[1], metadata("session_id", "real"));
    }

    #[test]
    fn test_add_session_id_skips_without_fallback() {
        // Non-codex agents don't set CODEX_THREAD_ID at all.
        let mut metadatas = vec![metadata("id", "some_agent")];
        add_session_id_if_missing(&mut metadatas, None);
        add_session_id_if_missing(&mut metadatas, Some(""));
        assert_eq!(metadatas.len(), 1);
    }

    #[test]
    fn test_add_session_id_skips_blank_fallback() {
        // Whitespace-only is blank, not a session id.
        let mut metadatas = vec![metadata("id", "codex")];
        add_session_id_if_missing(&mut metadatas, Some(" "));
        add_session_id_if_missing(&mut metadatas, Some("\t\n"));
        assert_eq!(metadatas.len(), 1);
    }

    #[test]
    fn test_add_session_id_trims_fallback() {
        // A padded id joins to nothing; store the trimmed value.
        let mut metadatas = vec![metadata("id", "codex")];
        add_session_id_if_missing(&mut metadatas, Some("  thread-1\n"));
        assert_eq!(metadatas.len(), 2);
        assert_eq!(metadatas[1], metadata("session_id", "thread-1"));
    }

    #[test]
    fn test_add_session_id_ignores_suffix_keyed_match() {
        // `parent_session_id` is a different key and must NOT block the append.
        let mut metadatas = vec![metadata("parent_session_id", "outer")];
        add_session_id_if_missing(&mut metadatas, Some("thread-1"));
        assert_eq!(metadatas.len(), 2);
        assert_eq!(metadatas[1], metadata("session_id", "thread-1"));
    }

    #[test]
    fn test_add_legacy_client_id_when_absent() {
        let mut metadatas = vec![metadata("id", "cli")];
        add_legacy_client_id_if_missing(&mut metadatas, Some("legacy"));
        assert_eq!(metadatas.len(), 2);
        assert_eq!(metadatas[0], metadata("id", "legacy"));
    }

    #[test]
    fn test_add_legacy_client_id_skips_when_present() {
        let mut metadatas = vec![metadata("id", "legacy")];
        add_legacy_client_id_if_missing(&mut metadatas, Some("legacy"));
        assert_eq!(metadatas.len(), 1);
    }

    #[test]
    fn test_add_legacy_client_id_skips_blank_or_missing() {
        let mut metadatas = vec![metadata("id", "cli")];
        add_legacy_client_id_if_missing(&mut metadatas, None);
        add_legacy_client_id_if_missing(&mut metadatas, Some(""));
        add_legacy_client_id_if_missing(&mut metadatas, Some("  "));
        assert_eq!(metadatas.len(), 1);
    }

    #[test]
    fn test_add_legacy_client_id_trims() {
        let mut metadatas = vec![metadata("id", "cli")];
        add_legacy_client_id_if_missing(&mut metadatas, Some("  legacy\n"));
        assert_eq!(metadatas.len(), 2);
        assert_eq!(metadatas[0], metadata("id", "legacy"));
    }

    #[test]
    fn test_legacy_client_id_from_config_flags() {
        assert_eq!(legacy_client_id_from_config_flags(vec![].into_iter()), None);
        assert_eq!(
            legacy_client_id_from_config_flags(vec!["foo=1", "client.id=outer"].into_iter()),
            Some("outer".to_owned())
        );
        assert_eq!(
            legacy_client_id_from_config_flags(
                vec!["client.id=outer", "client.id=inner"].into_iter()
            ),
            Some("inner".to_owned())
        );
        assert_eq!(
            legacy_client_id_from_config_flags(vec!["client.id=", "client.id= "].into_iter()),
            None
        );
        // A trailing blank value unsets an earlier id, like buckconfig does.
        assert_eq!(
            legacy_client_id_from_config_flags(vec!["client.id=outer", "client.id="].into_iter()),
            None
        );
        assert_eq!(
            legacy_client_id_from_config_flags(vec!["client.id=outer", "client.id= "].into_iter()),
            None
        );
        // A blank value followed by a real id still yields that id.
        assert_eq!(
            legacy_client_id_from_config_flags(vec!["client.id=", "client.id=inner"].into_iter()),
            Some("inner".to_owned())
        );
        assert_eq!(
            legacy_client_id_from_config_flags(vec!["fbcode//client.id=x"].into_iter()),
            None
        );
        // Non-`client.id` keys are ignored.
        assert_eq!(
            legacy_client_id_from_config_flags(vec!["client.ids=x", "other=x"].into_iter()),
            None
        );
    }
}
