/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::ffi::OsStr;
use std::ffi::OsString;
use std::sync::Arc;
use std::sync::OnceLock;

use dupe::Dupe;

/// The variable in which AI agent launchers describe the agent session a process runs for.
pub const CODING_AGENT_METADATA: &str = "CODING_AGENT_METADATA";

#[cfg(fbcode_build)]
const ENV_ALLOW_LIST: &[&str] = test_env_allowlist::LEGACY_TESTPILOT_ALLOW_LIST;

#[cfg(all(unix, not(fbcode_build)))]
const ENV_ALLOW_LIST: &[&str] = &[
    "PATH",
    "USER",
    "LOGNAME",
    "HOME",
    "TMPDIR",
    // Generally needed to keep systemd working
    "XDG_RUNTIME_DIR",
];

// The standard (built-in) variables.
// https://ss64.com/nt/syntax-variables.html
#[cfg(all(windows, not(fbcode_build)))]
const ENV_ALLOW_LIST: &[&str] = &[
    "ALLUSERSPROFILE",
    "APPDATA",
    "COMPUTERNAME",
    "COMSPEC",
    "CommonProgramFiles",
    "CommonProgramFiles(x86)",
    "HOMEDRIVE",
    "HOMEPATH",
    "LOCALAPPDATA",
    "NUMBER_OF_PROCESSORS",
    "OS",
    "PATH",
    "PATHEXT",
    "PROCESSOR_ARCHITECTURE",
    "PROCESSOR_ARCHITEW6432",
    "PROCESSOR_IDENTIFIER",
    "PROCESSOR_LEVEL",
    "PROCESSOR_REVISION",
    "PSModulePath",
    "ProgramData",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramW6432",
    "Public",
    "SYSTEMDRIVE",
    "SYSTEMROOT",
    "TEMP",
    "TMP",
    "USERDOMAIN",
    "USERNAME",
    "USERPROFILE",
    "UserDnsDomain",
    "WINDIR",
];

/// The invoking client's `CODING_AGENT_METADATA` for its local tests, if printable ASCII.
#[derive(Clone, Dupe, Debug, Default)]
pub struct ClientCodingAgentMetadata(Option<Arc<str>>);

impl ClientCodingAgentMetadata {
    pub fn new(value: Option<&str>) -> Self {
        Self(
            value
                .filter(|v| !v.is_empty() && v.bytes().all(|b| (b' '..=b'~').contains(&b)))
                .map(Arc::from),
        )
    }
}

#[derive(Clone, Dupe, Debug)]
pub struct EnvironmentInheritance {
    clear: bool,
    values: &'static [(&'static str, OsString)],
    exclusions: &'static [&'static str],
    coding_agent_metadata: ClientCodingAgentMetadata,
}

impl EnvironmentInheritance {
    pub fn test_allowlist() -> Self {
        // This is made to be a list of lists in case we want to include lists from different
        // provenances, like the test_env_allowlist::ENV_LIST_HACKY.
        let allowlists = &[ENV_ALLOW_LIST];

        // We create this *once* since getenv is actually not cheap (being O(n) of the environment
        // size).
        static TEST_CELL: OnceLock<Vec<(&'static str, OsString)>> = OnceLock::new();

        let values = TEST_CELL.get_or_init(|| {
            let mut ret = Vec::new();
            for list in allowlists.iter() {
                for key in list.iter() {
                    if let Some(value) = std::env::var_os(key) {
                        ret.push((*key, value));
                    }
                }
            }
            ret
        });

        Self {
            clear: true,
            values,
            exclusions: &[],
            coding_agent_metadata: ClientCodingAgentMetadata::default(),
        }
    }

    /// Also passes on the `CODING_AGENT_METADATA` of the client that invoked the command.
    pub fn with_client_coding_agent_metadata(
        mut self,
        coding_agent_metadata: ClientCodingAgentMetadata,
    ) -> Self {
        self.coding_agent_metadata = coding_agent_metadata;
        self
    }

    /// Exclude some vars that are known to cause issues. In an ideal world we should do a
    /// migration to lock this down everywhere.
    pub fn local_command_exclusions() -> Self {
        Self {
            clear: false,
            values: &[],
            exclusions: &[
                "PYTHONPATH",
                "PYTHONHOME",
                "PYTHONSTARTUP",
                "LD_LIBRARY_PATH",
                "LD_PRELOAD",
            ],
            coding_agent_metadata: ClientCodingAgentMetadata::default(),
        }
    }

    pub fn empty() -> Self {
        Self {
            values: &[],
            exclusions: &[],
            clear: true,
            coding_agent_metadata: ClientCodingAgentMetadata::default(),
        }
    }

    pub fn values(&self) -> impl Iterator<Item = (&'static str, &OsStr)> + '_ {
        self.values.iter().map(|(k, v)| (*k, v.as_os_str())).chain(
            self.coding_agent_metadata
                .0
                .as_deref()
                .map(|v| (CODING_AGENT_METADATA, OsStr::new(v))),
        )
    }

    pub fn exclusions(&self) -> impl Iterator<Item = &'static str> + use<> {
        self.exclusions.iter().copied()
    }

    pub fn clear(&self) -> bool {
        self.clear
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inherits_client_coding_agent_metadata() {
        let inheritance = EnvironmentInheritance::empty().with_client_coding_agent_metadata(
            ClientCodingAgentMetadata::new(Some("id=agent,session_id=s1")),
        );
        assert_eq!(
            inheritance.values().collect::<Vec<_>>(),
            [(CODING_AGENT_METADATA, OsStr::new("id=agent,session_id=s1"))]
        );
    }

    #[test]
    fn test_drops_client_coding_agent_metadata_that_is_not_a_header_value() {
        for value in [
            None,
            Some(""),
            Some("id=agent\r\nX: y"),
            Some("id=\t"),
            Some("id=agént"),
        ] {
            let inheritance = EnvironmentInheritance::empty()
                .with_client_coding_agent_metadata(ClientCodingAgentMetadata::new(value));
            assert_eq!(
                inheritance.values().count(),
                0,
                "`{value:?}` should not be inherited"
            );
        }
    }
}
