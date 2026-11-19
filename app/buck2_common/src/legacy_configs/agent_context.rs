/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The repository-defined schema for `--agent-context` entries.

use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

use crate::legacy_configs::configs::LegacyBuckConfig;
use crate::legacy_configs::key::BuckconfigKeyRef;

/// Schema for a single agent context field.
#[derive(Debug, Eq, PartialEq)]
pub struct AgentContextFieldSchema {
    required: bool,
    allowed_values: Vec<String>,
    description: String,
}

impl AgentContextFieldSchema {
    /// Whether enforced clients must provide a non-empty value for this field.
    pub fn is_required(&self) -> bool {
        self.required
    }

    /// The accepted values, or an empty slice when the field is freeform.
    pub fn allowed_values(&self) -> &[String] {
        &self.allowed_values
    }

    /// The repository-provided description of this field.
    pub fn description(&self) -> &str {
        &self.description
    }
}

/// Schema parsed from buckconfig `[agent_context]` and `[agent_context#*]` sections.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct AgentContextSchema {
    enforced_clients: Vec<String>,
    fields: BTreeMap<String, AgentContextFieldSchema>,
}

impl AgentContextSchema {
    /// Parse the schema from the repository's resolved buckconfig.
    pub fn from_config(config: &LegacyBuckConfig) -> Self {
        let enforced_clients = config
            .get(BuckconfigKeyRef {
                section: "agent_context",
                property: "enforced_clients",
            })
            .map(|v| v.split('|').map(|s| s.trim().to_owned()).collect())
            .unwrap_or_default();

        let fields = config
            .all_sections()
            .filter_map(|(section_name, section)| {
                let field_name = section_name.strip_prefix("agent_context#")?;
                let allowed_values = section
                    .get("values")
                    .map(|v| v.as_str().split('|').map(|s| s.trim().to_owned()).collect())
                    .unwrap_or_default();
                let required = section
                    .get("required")
                    .is_some_and(|v| v.as_str() == "true");
                let description = section
                    .get("description")
                    .map(|v| v.as_str().to_owned())
                    .unwrap_or_default();

                Some((
                    field_name.to_owned(),
                    AgentContextFieldSchema {
                        required,
                        allowed_values,
                        description,
                    },
                ))
            })
            .collect();

        Self {
            enforced_clients,
            fields,
        }
    }

    /// Whether the given client ID is subject to schema validation.
    pub fn is_enforced(&self, client_id: &str) -> bool {
        self.enforced_clients.iter().any(|c| c == client_id)
    }

    /// Client IDs that are subject to schema validation.
    pub fn enforced_clients(&self) -> &[String] {
        &self.enforced_clients
    }

    /// Field definitions, ordered by field name.
    pub fn fields(&self) -> &BTreeMap<String, AgentContextFieldSchema> {
        &self.fields
    }

    /// Whether the repository defines any agent-context fields.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

/// Formats the agent-supplied fields while keeping client enforcement internal to Buck2.
impl Display for AgentContextSchema {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return f.write_str("No fields are configured for this repository.");
        }

        f.write_str("Fields:")?;
        for (name, field) in &self.fields {
            write!(f, "\n- `{name}`\n  Required: {}", field.is_required())?;
            if !field.description().is_empty() {
                write!(f, "\n  Description: {}", field.description())?;
            }
            if field.allowed_values().is_empty() {
                f.write_str("\n  Allowed values: any")?;
            } else {
                write!(
                    f,
                    "\n  Allowed values: {}",
                    field.allowed_values().join(", ")
                )?;
            }
        }

        Ok(())
    }
}
