/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Metadata collection, for telemetry purposes.
use std::env;
use std::sync::OnceLock;

use buck2_core::ci::ci_identifiers;
use buck2_core::facebook_only;
use buck2_hash::IntentionallyStdHashMap;
use buck2_wrapper_common::BUCK2_WRAPPER_ENV_VAR;

use crate::daemon_id::DaemonId;

/// Collects metadata from the current binary and environment, merged with any extras, suitable for telemetry purposes.
///
/// Extras are user-supplied (e.g. from the `[buck2_metadata]` buckconfig section) and so are only
/// applied for keys not already populated by `collect`, ensuring user config cannot shadow trusted
/// telemetry fields such as `hostname`, `username` or `daemon_uuid`.
#[cfg(not(fbcode_build))]
pub fn collect_with_extras(
    daemon: &DaemonId,
    extras: &buck2_hash::BuckFixedSortedMap<String, String>,
) -> IntentionallyStdHashMap<String, String> {
    let mut map = collect(daemon);
    for (k, v) in extras.iter() {
        map.entry(k.clone()).or_insert_with(|| v.clone());
    }
    map
}

/// Collects metadata from the current binary and environment and writes it as map, suitable for telemetry purposes.
pub fn collect(daemon: &DaemonId) -> IntentionallyStdHashMap<String, String> {
    facebook_only();
    fn add_env_var(
        map: &mut IntentionallyStdHashMap<String, String>,
        key: &'static str,
        var: &'static str,
    ) {
        if let Ok(data) = env::var(var) {
            map.insert(key.to_owned(), data);
        }
    }

    let mut map = IntentionallyStdHashMap::new();

    let info = system_info();
    if let Some(hostname) = info.hostname {
        map.insert("hostname".to_owned(), hostname);
    }
    if let Some(username) = info.username {
        map.insert("username".to_owned(), username);
    }
    if let Some(system_fingerprint) = info.system_fingerprint {
        map.insert("system_fingerprint".to_owned(), system_fingerprint);
    }
    map.insert("arch".to_owned(), info.arch);

    if let Some(rev) = buck2_build_info::revision() {
        map.insert("buck2_revision".to_owned(), rev.to_owned());
    }

    if let Some(time) = buck2_build_info::time_iso8601() {
        map.insert("buck2_build_time".to_owned(), time.to_owned());
    }

    if let Some(ts) = buck2_build_info::release_timestamp() {
        map.insert("buck2_release_timestamp".to_owned(), ts.to_owned());
    }

    if is_proc_translated::is_proc_translated() {
        map.insert("rosetta".to_owned(), "1".to_owned());
    }

    if let Some(ces_id) = ces_id() {
        map.insert("ces_id".to_owned(), ces_id);
    }

    if let Some(devx_session_id) = devx_session_id() {
        map.insert("devx_session_id".to_owned(), devx_session_id);
    }

    // Global trace ID
    map.insert("daemon_uuid".to_owned(), daemon.to_string());

    map.insert("os".to_owned(), info.os);
    if let Some(version) = info.os_version {
        map.insert("os_version".to_owned(), version);
    }

    if let Some(environment) = environment() {
        map.insert("environment".to_owned(), environment);
    }

    add_env_var(&mut map, "launched_via_wrapper", BUCK2_WRAPPER_ENV_VAR);
    add_env_var(&mut map, "fbpackage_name", "FBPACKAGE_PACKAGE_NAME");
    add_env_var(&mut map, "fbpackage_version", "FBPACKAGE_PACKAGE_VERSION");
    add_env_var(&mut map, "fbpackage_release", "FBPACKAGE_PACKAGE_RELEASE");

    if let Ok(ci_identifiers) = ci_identifiers() {
        for (ci_name, ci_value) in ci_identifiers {
            if let Some(ci_value) = ci_value {
                map.insert(ci_name.to_owned(), ci_value.to_owned());
            }
        }
    }

    map
}

pub struct SystemInfo {
    pub username: Option<String>,
    pub hostname: Option<String>,
    pub os: String,
    pub os_version: Option<String>,
    pub system_fingerprint: Option<String>,
    pub arch: String,
}

pub fn system_info() -> SystemInfo {
    let hostname = hostname();
    let username = username().ok().flatten();

    SystemInfo {
        hostname,
        username,
        os: os_type(),
        os_version: os_version(),
        system_fingerprint: system_fingerprint(),
        arch: env::consts::ARCH.to_owned(),
    }
}

/// The operating system - "linux" "darwin" "windows" etc.
fn os_type() -> String {
    if cfg!(target_os = "linux") {
        "linux".to_owned()
    } else if cfg!(target_os = "macos") {
        "darwin".to_owned()
    } else if cfg!(target_os = "windows") {
        "windows".to_owned()
    } else {
        "unknown".to_owned()
    }
}

#[cfg(target_os = "windows")]
fn os_version() -> Option<String> {
    winver::WindowsVersion::detect().map(|v| v.to_string())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn os_version() -> Option<String> {
    sys_info::os_release().ok()
}

pub fn hostname() -> Option<String> {
    static CELL: OnceLock<Option<String>> = OnceLock::new();

    CELL.get_or_init(|| {
        hostname::get()
            .ok()
            .map(|res| res.to_string_lossy().into_owned())
    })
    .clone()
}

pub fn ces_id() -> Option<String> {
    #[cfg(fbcode_build)]
    {
        use cross_env_session_id::CrossEnvironmentSessionId;

        CrossEnvironmentSessionId::get()
    }
    #[cfg(not(fbcode_build))]
    {
        None
    }
}

pub fn devx_session_id() -> Option<String> {
    #[cfg(fbcode_build)]
    {
        use devx_session_id::DevXSessionId;

        DevXSessionId::get()
    }
    #[cfg(not(fbcode_build))]
    {
        None
    }
}

pub fn environment() -> Option<String> {
    #[cfg(fbcode_build)]
    {
        use hostcaps::get_env;

        Some(get_env().to_string().to_lowercase())
    }
    #[cfg(not(fbcode_build))]
    {
        None
    }
}

pub fn username() -> buck2_error::Result<Option<String>> {
    #[cfg(fbcode_build)]
    {
        use buck2_error::conversion::from_any_with_tag;
        Ok(Some(user::current_username().map_err(|e| {
            from_any_with_tag(e, buck2_error::ErrorTag::InvalidUsername)
        })?))
    }
    #[cfg(not(fbcode_build))]
    {
        Ok::<Option<String>, buck2_error::Error>(None)
    }
}

pub fn is_devserver_placeholder_unixname(name: &str) -> bool {
    #[cfg(fbcode_build)]
    {
        user::is_devserver_placeholder_unixname(name)
    }
    #[cfg(not(fbcode_build))]
    {
        let _unused = name;
        false
    }
}

/// The AI agent driving this process, as the launcher that started it declared in
/// `BUCK2_CLIENT_METADATA`. `None` when no launcher set one.
///
/// Read straight from the environment rather than from the parsed `client_metadata`:
/// that vec merges in every `--client-metadata` argument, so its `id` is whatever
/// wrapper ran buck2 last, not the agent the wrapper is running under.
pub fn agent_identity_from_env() -> Option<String> {
    agent_id_in(&env::var("BUCK2_CLIENT_METADATA").ok()?)
}

/// The last non-empty `id` of a comma-separated `key=value` client-metadata string.
/// Keys and values are trimmed, and an entry whose value is empty does not shadow an
/// earlier one that names an agent.
fn agent_id_in(metadata: &str) -> Option<String> {
    metadata
        .split(',')
        .filter_map(|pair| pair.split_once('='))
        .filter(|(key, _)| key.trim() == "id")
        .map(|(_, value)| value.trim())
        .rfind(|value| !value.is_empty())
        .map(str::to_owned)
}

pub fn system_fingerprint() -> Option<String> {
    #[cfg(fbcode_build)]
    {
        use devserver_fingerprint::SystemFingerprintReader;
        let sfr = SystemFingerprintReader::get().ok()?;
        sfr.fingerprint().map(|s| s.to_owned())
    }
    #[cfg(not(fbcode_build))]
    {
        None
    }
}

/// The name of the VM template this devserver was provisioned from, e.g. `dev7_xlarge_2tb_rv`, as
/// the file every Meta devserver describes itself in records it. `None` on any other host, and on
/// a devserver whose file names no template. The file is read once per process: a devserver keeps
/// its template until it is provisioned again.
pub fn fog_template() -> Option<String> {
    #[cfg(fbcode_build)]
    {
        static FOG_TEMPLATE: OnceLock<Option<String>> = OnceLock::new();
        FOG_TEMPLATE
            .get_or_init(|| {
                fog_template_in(&std::fs::read_to_string("/etc/devserver.whoami").ok()?)
            })
            .clone()
    }
    #[cfg(not(fbcode_build))]
    {
        None
    }
}

/// The `fog_template` of a devserver's description of itself, which is a JSON object.
#[cfg(fbcode_build)]
fn fog_template_in(whoami: &str) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Whoami {
        fog_template: Option<String>,
    }

    let whoami: Whoami = serde_json::from_str(whoami).ok()?;
    whoami.fog_template
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn os_version_produces_reasonable_windows_version() {
        let data = collect(&DaemonId::new());
        // This logic used to use the `GetVersionExW` win32 API, which
        // always returns the value below on recent versions of windows. See
        // https://learn.microsoft.com/en-us/windows/win32/api/sysinfoapi/nf-sysinfoapi-getversionexw
        // for more details.
        assert_ne!(data["os_version"], "6.2.9200");
        // This is true for both Windows 10 and Windows 11: https://learn.microsoft.com/en-us/windows/win32/sysinfo/operating-system-version
        assert!(data["os_version"].starts_with("10.0."));
    }
}

#[cfg(test)]
mod agent_id_tests {
    use super::*;

    fn agent_id(metadata: &str) -> Option<String> {
        agent_id_in(metadata)
    }

    #[test]
    fn reads_the_id_a_launcher_writes() {
        assert_eq!(agent_id("id=claude_code"), Some("claude_code".to_owned()));
        assert_eq!(
            agent_id("id=claude_code,invocation_id=inv,session_id=sess"),
            Some("claude_code".to_owned()),
        );
    }

    #[test]
    fn keeps_the_warmup_variant_distinct_from_the_agent() {
        // The launcher suffixes only this variable, so warmup must not collapse
        // into `claude_code` -- it is the larger share of buck2 traffic.
        assert_eq!(
            agent_id("id=claude_code_warmup,invocation_id=inv"),
            Some("claude_code_warmup".to_owned()),
        );
    }

    #[test]
    fn finds_the_id_in_any_position() {
        assert_eq!(
            agent_id("invocation_id=inv,id=codex"),
            Some("codex".to_owned()),
        );
    }

    #[test]
    fn a_duplicate_id_resolves_to_the_last() {
        assert_eq!(agent_id("id=first,id=last"), Some("last".to_owned()));
    }

    #[test]
    fn an_empty_id_does_not_shadow_an_earlier_one() {
        assert_eq!(
            agent_id("id=claude_code,id="),
            Some("claude_code".to_owned())
        );
        assert_eq!(
            agent_id("id=claude_code,id=   "),
            Some("claude_code".to_owned()),
        );
    }

    #[test]
    fn keys_and_values_are_trimmed() {
        assert_eq!(agent_id("id= claude_code "), Some("claude_code".to_owned()));
        assert_eq!(
            agent_id("invocation_id=inv, id =claude_code"),
            Some("claude_code".to_owned()),
        );
    }

    #[test]
    fn a_key_that_merely_ends_in_id_is_not_the_id() {
        assert_eq!(agent_id("invocation_id=inv"), None);
        assert_eq!(agent_id("session_id=sess"), None);
        assert_eq!(agent_id("parent_id=outer"), None);
    }

    #[test]
    fn names_no_agent_without_a_usable_id() {
        assert_eq!(agent_id(""), None);
        assert_eq!(agent_id("id="), None);
        assert_eq!(agent_id("no_equals_sign"), None);
    }

    #[test]
    fn a_value_may_contain_the_separator() {
        assert_eq!(agent_id("id=a=b"), Some("a=b".to_owned()));
    }

    #[test]
    fn a_padded_key_still_names_the_id() {
        assert_eq!(
            agent_id("invocation_id=inv, id=claude_code"),
            Some("claude_code".to_owned()),
        );
    }
}

#[cfg(all(test, fbcode_build))]
mod fog_template_tests {
    use super::*;

    #[test]
    fn reads_the_template_a_devserver_names() {
        assert_eq!(
            fog_template_in(
                r#"{"hostname":"devvm1.abc0.facebook.com","fog_reservation_pool":null,"fog_template":"dev7_xlarge_2tb_rv"}"#
            ),
            Some("dev7_xlarge_2tb_rv".to_owned()),
        );
    }

    #[test]
    fn a_devserver_that_names_no_template_has_none() {
        assert_eq!(
            fog_template_in(r#"{"hostname":"devvm1.abc0.facebook.com","fog_template":null}"#),
            None,
        );
        assert_eq!(
            fog_template_in(r#"{"hostname":"devvm1.abc0.facebook.com"}"#),
            None,
        );
    }

    #[test]
    fn a_file_that_is_not_a_json_object_has_none() {
        assert_eq!(fog_template_in(""), None);
        assert_eq!(fog_template_in("not json"), None);
    }
}
