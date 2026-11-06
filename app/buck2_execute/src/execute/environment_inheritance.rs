/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::ffi::OsString;
use std::sync::Arc;
use std::sync::OnceLock;

use dupe::Dupe;

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

/// N.B. this forces [`std::env::var_os()`] to be instantiated with concrete
/// type parameters.
fn real_getenv(key: &str) -> Option<OsString> {
    std::env::var_os(key)
}

/// The values captured from the built-in allowlist alone. We compute this *once* since getenv is
/// actually not cheap (being O(n) of the environment size).
fn builtin_test_allowlist_values() -> &'static Arc<[(String, OsString)]> {
    static TEST_CELL: OnceLock<Arc<[(String, OsString)]>> = OnceLock::new();

    TEST_CELL.get_or_init(|| {
        EnvironmentInheritance::test_allowlist_from(ENV_ALLOW_LIST, &[], &real_getenv).values
    })
}

fn no_values() -> Arc<[(String, OsString)]> {
    static EMPTY: OnceLock<Arc<[(String, OsString)]>> = OnceLock::new();
    EMPTY.get_or_init(|| Vec::new().into()).dupe()
}

#[derive(Clone, Dupe, Debug)]
pub struct EnvironmentInheritance {
    clear: bool,
    values: Arc<[(String, OsString)]>,
    exclusions: &'static [&'static str],
}

impl EnvironmentInheritance {
    pub fn test_allowlist() -> Self {
        Self::test_allowlist_with_extra(&[])
    }

    /// The built-in test allowlist, plus any extra variable names supplied by
    /// the caller (in practice, the `[test] env_allowlist` buckconfig).
    ///
    /// Note: The environment of tests derives from the *daemon's* environment,
    /// not the client's.
    pub fn test_allowlist_with_extra(extra: &[String]) -> Self {
        if extra.is_empty() {
            return Self {
                clear: true,
                values: builtin_test_allowlist_values().dupe(),
                exclusions: &[],
            };
        }
        Self::test_allowlist_from(ENV_ALLOW_LIST, extra, &real_getenv)
    }

    /// Capture the values of `builtin` and `extra` according to `getenv`.
    fn test_allowlist_from(
        builtin: &[&str],
        extra: &[String],
        getenv: &dyn Fn(&str) -> Option<OsString>,
    ) -> Self {
        let values = builtin
            .iter()
            .copied()
            .chain(extra.iter().map(String::as_str))
            .filter_map(|key| {
                // Trim and skip empties, as buck2's other comma-separated list configs do — see
                // `IgnoreSet::from_ignore_spec` and `CellPackageBoundaryExceptions::new`.
                // Otherwise `FOO, BAR` would silently do nothing for `BAR`.
                let key = key.trim();
                if key.is_empty() {
                    return None;
                }
                // A name appearing in both lists is harmless: both copies get the same value from
                // `getenv`, and every consumer of `values()` is last-wins.
                Some((key.to_owned(), getenv(key)?))
            })
            .collect();

        Self {
            clear: true,
            values,
            exclusions: &[],
        }
    }

    /// Exclude some vars that are known to cause issues. In an ideal world we should do a
    /// migration to lock this down everywhere.
    pub fn local_command_exclusions() -> Self {
        Self {
            clear: false,
            values: no_values(),
            exclusions: &[
                "PYTHONPATH",
                "PYTHONHOME",
                "PYTHONSTARTUP",
                "LD_LIBRARY_PATH",
                "LD_PRELOAD",
            ],
        }
    }

    pub fn empty() -> Self {
        Self {
            values: no_values(),
            exclusions: &[],
            clear: true,
        }
    }

    pub fn values(&self) -> impl Iterator<Item = (&str, &OsString)> + use<'_> {
        self.values.iter().map(|(k, v)| (k.as_str(), v))
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

    const BUILTIN: &[&str] = &["SET_BUILTIN", "UNSET_BUILTIN"];

    /// The only variables that exist as far as these tests are concerned.
    fn getenv(key: &str) -> Option<OsString> {
        match key {
            "SET_BUILTIN" => Some(OsString::from("builtin-value")),
            "SET_EXTRA" => Some(OsString::from("extra-value")),
            "SET_EXTRA_2" => Some(OsString::from("extra-value-2")),
            _ => None,
        }
    }

    fn captured(extra: &[&str]) -> Vec<(String, OsString)> {
        let extra: Vec<String> = extra.iter().map(|k| (*k).to_owned()).collect();
        EnvironmentInheritance::test_allowlist_from(BUILTIN, &extra, &getenv)
            .values()
            .map(|(k, v)| (k.to_owned(), v.clone()))
            .collect()
    }

    fn pairs(expected: &[(&str, &str)]) -> Vec<(String, OsString)> {
        expected
            .iter()
            .map(|(k, v)| ((*k).to_owned(), OsString::from(*v)))
            .collect()
    }

    #[test]
    fn test_builtin_only_drops_unset_names() {
        assert_eq!(captured(&[]), pairs(&[("SET_BUILTIN", "builtin-value")]));
    }

    #[test]
    fn test_extra_names_are_appended() {
        assert_eq!(
            captured(&["SET_EXTRA", "SET_EXTRA_2"]),
            pairs(&[
                ("SET_BUILTIN", "builtin-value"),
                ("SET_EXTRA", "extra-value"),
                ("SET_EXTRA_2", "extra-value-2"),
            ]),
        );
    }

    #[test]
    fn test_extra_names_are_trimmed() {
        // As the names arrive from `env_allowlist = SET_EXTRA, SET_EXTRA_2`.
        assert_eq!(
            captured(&["SET_EXTRA", " SET_EXTRA_2"]),
            captured(&["SET_EXTRA", "SET_EXTRA_2"]),
        );
    }

    #[test]
    fn test_extra_ignores_unset_names() {
        assert_eq!(captured(&["NEVER_SET"]), captured(&[]));
    }

    #[test]
    fn test_extra_ignores_empty_names() {
        // What `env_allowlist =` and `env_allowlist = SET_EXTRA,` parse to.
        assert_eq!(captured(&["", "   "]), captured(&[]));
        assert_eq!(captured(&["SET_EXTRA", ""]), captured(&["SET_EXTRA"]));
    }

    #[test]
    fn test_repeating_a_builtin_name_keeps_the_same_value() {
        // Duplicates are tolerated because every consumer of `values()` is last-wins, which is
        // only safe as long as both copies carry the same value.
        assert_eq!(
            captured(&["SET_BUILTIN"]),
            pairs(&[
                ("SET_BUILTIN", "builtin-value"),
                ("SET_BUILTIN", "builtin-value"),
            ]),
        );
    }

    #[test]
    fn test_allowlist_clears_the_environment_and_excludes_nothing() {
        let inheritance = EnvironmentInheritance::test_allowlist_from(
            BUILTIN,
            &["SET_EXTRA".to_owned()],
            &getenv,
        );
        assert!(inheritance.clear());
        assert_eq!(inheritance.exclusions().count(), 0);
    }

    #[test]
    fn test_empty_extra_takes_the_memoized_path() {
        // The `extra.is_empty()` fast path in `test_allowlist_with_extra` reads from a different
        // (memoized) source than `test_allowlist_from`, so check the two agree.
        let memoized: Vec<_> = EnvironmentInheritance::test_allowlist()
            .values()
            .map(|(k, v)| (k.to_owned(), v.clone()))
            .collect();
        let direct: Vec<_> =
            EnvironmentInheritance::test_allowlist_from(ENV_ALLOW_LIST, &[], &real_getenv)
                .values()
                .map(|(k, v)| (k.to_owned(), v.clone()))
                .collect();
        assert_eq!(memoized, direct);
    }
}
