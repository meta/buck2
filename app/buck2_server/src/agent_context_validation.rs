/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Validation of `--agent-context` entries against a buckconfig-defined schema.
//!
//! Schema is defined in buckconfig as:
//! ```ini
//! [agent_context]
//! enforced_clients = devmate|claude_code|agentic_runtime
//!
//! [agent_context#intent]
//! required = false
//! values = build|test|query|fix|investigate
//! description = The purpose of this buck2 invocation
//! ```

use buck2_common::legacy_configs::agent_context::AgentContextSchema;
use buck2_data::AgentContextEntry;
use buck2_hash::BuckMutSet;

/// Pick the client id that agent-context schema enforcement applies to.
///
/// Tools wrap each other, so a command can assert several `id`s (with the
/// `-c client.id=` legacy value first). Enforcement must
/// not depend on which id happens to be last: if any asserted id is an
/// enforced client, that id governs. Otherwise the last id is returned; it
/// only feeds the enforced check inside `validate_agent_context`, which
/// passes for unenforced clients.
pub(crate) fn enforcement_client_id<'a>(
    schema: &AgentContextSchema,
    client_ids: &'a [String],
) -> Option<&'a str> {
    client_ids
        .iter()
        .rev()
        .find(|id| schema.is_enforced(id.as_str()))
        .or_else(|| client_ids.last())
        .map(String::as_str)
}

/// Validate agent context entries against the schema.
///
/// Returns `Ok(())` if validation passes or the client is not enforced.
/// Returns `Err` with a descriptive error if validation fails for an enforced client.
pub(crate) fn validate_agent_context(
    schema: &AgentContextSchema,
    client_id: Option<&str>,
    entries: &[buck2_data::AgentContextEntry],
) -> buck2_error::Result<()> {
    // Built-in CLI metadata must not opt an otherwise context-free call into schema enforcement.
    if schema.is_empty()
        || entries
            .iter()
            .all(|entry| AgentContextEntry::BUILTIN_CLI_KEYS.contains(&entry.key.as_str()))
    {
        return Ok(());
    }

    let is_enforced = client_id.is_some_and(|id| schema.is_enforced(id));

    // Non-enforced clients: store as-is, no validation.
    if !is_enforced {
        return Ok(());
    }

    // Collect provided keys with non-empty values for required-field checking.
    let provided_keys: BuckMutSet<&str> = entries
        .iter()
        .filter(|e| !e.value.is_empty())
        .map(|e| e.key.as_str())
        .collect();

    // Check all required fields are present.
    // BTreeMap iterates in sorted order, so error messages are deterministic.
    let missing: Vec<_> = schema
        .fields()
        .iter()
        .filter(|(name, f)| f.is_required() && !provided_keys.contains(name.as_str()))
        .map(|(name, f)| {
            if f.description().is_empty() {
                format!("  - {}", name)
            } else {
                format!("  - {}: {}", name, f.description())
            }
        })
        .collect();

    if !missing.is_empty() {
        return Err(buck2_error::buck2_error!(
            buck2_error::ErrorTag::Input,
            "Missing required agent-context field(s):\n{}",
            missing.join("\n")
        ));
    }

    // Enforced clients: validate each entry.
    for entry in entries {
        let key = &entry.key;
        let value = &entry.value;

        // Built-in keys do not need repository schema entries.
        if AgentContextEntry::ENV_INJECTED_KEYS.contains(&key.as_str())
            || AgentContextEntry::BUILTIN_CLI_KEYS.contains(&key.as_str())
        {
            continue;
        }

        match schema.fields().get(key.as_str()) {
            None => {
                let valid_keys: Vec<&str> = schema.fields().keys().map(|k| k.as_str()).collect();
                return Err(buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Input,
                    "Unknown agent-context key `{}`.\n  Valid keys: {}",
                    key,
                    valid_keys.join(", ")
                ));
            }
            Some(field_schema) => {
                if !field_schema.allowed_values().is_empty()
                    && !field_schema.allowed_values().iter().any(|v| v == value)
                {
                    let desc = if field_schema.description().is_empty() {
                        String::new()
                    } else {
                        format!("\n  {}: {}", key, field_schema.description())
                    };
                    return Err(buck2_error::buck2_error!(
                        buck2_error::ErrorTag::Input,
                        "Invalid agent-context value `{}` for key `{}`.{}\n  Valid values: {}",
                        value,
                        key,
                        desc,
                        field_schema.allowed_values().join(", ")
                    ));
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use buck2_common::legacy_configs::configs::testing::parse;

    use super::*;

    fn schema_with_intent() -> AgentContextSchema {
        let config = parse(
            &[(
                "test",
                "[agent_context]\n\
                 enforced_clients = claude_code\n\
                 [agent_context#intent]\n\
                 values = build|test\n",
            )],
            "test",
        )
        .expect("test agent context schema should parse");
        AgentContextSchema::from_config(&config)
    }

    fn entry(key: &str, value: &str) -> buck2_data::AgentContextEntry {
        buck2_data::AgentContextEntry {
            key: key.to_owned(),
            value: value.to_owned(),
        }
    }

    fn schema_with_required_fields() -> AgentContextSchema {
        let config = parse(
            &[(
                "test",
                "[agent_context]\n\
                 enforced_clients = claude_code\n\
                 [agent_context#intent]\n\
                 required = true\n\
                 values = build|test\n",
            )],
            "test",
        )
        .expect("test agent context schema should parse");
        AgentContextSchema::from_config(&config)
    }

    #[test]
    fn test_builtin_cli_keys_alone_do_not_enable_schema_enforcement() {
        let schema = schema_with_required_fields();
        for builtin in [
            entry("direct_call", "true"),
            entry("advice_ack", "build_intent"),
        ] {
            assert!(validate_agent_context(&schema, Some("claude_code"), &[builtin]).is_ok());
        }
    }

    #[test]
    fn test_custom_context_without_direct_call() {
        let schema = schema_with_required_fields();
        let entries = [entry("intent", "build")];
        assert!(validate_agent_context(&schema, Some("claude_code"), &entries).is_ok());
    }

    #[test]
    fn test_builtin_cli_keys_bypass_schema_validation() {
        let schema = schema_with_required_fields();
        for builtin in [
            entry("direct_call", "true"),
            entry("direct_call", "false"),
            entry("direct_call", "invalid"),
            entry("advice_ack", "build_intent"),
            entry("advice_ack", "other_advice"),
        ] {
            let entries = [builtin, entry("intent", "build")];
            assert!(validate_agent_context(&schema, Some("claude_code"), &entries).is_ok());
        }
    }

    #[test]
    fn test_builtin_cli_keys_do_not_bypass_required_custom_fields() {
        let schema = schema_with_required_fields();
        for builtin in [
            entry("direct_call", "true"),
            entry("advice_ack", "build_intent"),
        ] {
            let entries = [builtin, entry("intent", "")];
            let error = validate_agent_context(&schema, Some("claude_code"), &entries)
                .expect_err("empty required intent should be rejected");
            assert!(
                error
                    .to_string()
                    .contains("Missing required agent-context field")
            );
        }
    }

    #[test]
    fn test_builtin_cli_keys_do_not_bypass_custom_field_values() {
        let schema = schema_with_required_fields();
        for builtin in [
            entry("direct_call", "true"),
            entry("advice_ack", "build_intent"),
        ] {
            let entries = [builtin, entry("intent", "invalid")];
            let error = validate_agent_context(&schema, Some("claude_code"), &entries)
                .expect_err("invalid intent should be rejected");
            assert!(error.to_string().contains("Invalid agent-context value"));
        }
    }

    #[test]
    fn test_env_injected_keys_bypass_validation() {
        let schema = schema_with_intent();
        let entries = vec![
            entry("id", "claude_code"),
            entry("invocation_id", "claude_code_invocation_abc123"),
            entry("intent", "test"),
        ];
        assert!(validate_agent_context(&schema, Some("claude_code"), &entries).is_ok());
    }

    #[test]
    fn test_env_injected_keys_alone_pass_validation() {
        let schema = schema_with_intent();
        let entries = vec![
            entry("id", "claude_code"),
            entry("invocation_id", "claude_code_invocation_abc123"),
        ];
        assert!(validate_agent_context(&schema, Some("claude_code"), &entries).is_ok());
    }

    #[test]
    fn test_unknown_key_still_rejected() {
        let schema = schema_with_intent();
        let entries = vec![
            entry("id", "claude_code"),
            entry("direct_call", "true"),
            entry("bogus_key", "value"),
        ];
        assert!(validate_agent_context(&schema, Some("claude_code"), &entries).is_err());
    }

    #[test]
    fn test_enforcement_client_id_prefers_enforced_id() {
        let schema = schema_with_intent();
        assert_eq!(
            enforcement_client_id(&schema, &["claude_code".to_owned(), "wrapper".to_owned()]),
            Some("claude_code")
        );
        assert_eq!(
            enforcement_client_id(&schema, &["wrapper".to_owned(), "claude_code".to_owned()]),
            Some("claude_code")
        );
        assert_eq!(
            enforcement_client_id(&schema, &["outer".to_owned(), "wrapper".to_owned()]),
            Some("wrapper")
        );
        assert_eq!(enforcement_client_id(&schema, &[]), None);
    }

    #[test]
    fn test_legacy_unenforced_id_does_not_dodge_enforcement() {
        let schema = schema_with_required_fields();
        // Missing required field must fail even though an unenforced legacy
        // `-c client.id=` value is also asserted.
        let entries = [entry("intent", "")];
        let ids = vec!["legacy-wrapper".to_owned(), "claude_code".to_owned()];
        let error = validate_agent_context(&schema, enforcement_client_id(&schema, &ids), &entries)
            .expect_err("enforced client must not dodge validation via legacy id");
        assert!(
            error
                .to_string()
                .contains("Missing required agent-context field")
        );
    }
}
