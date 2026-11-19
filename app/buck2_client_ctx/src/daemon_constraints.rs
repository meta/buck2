/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_common::init::DaemonStartupConfig;
use buck2_core::buck2_env;
use buck2_core::ci::ci_identifiers;
use buck2_events::daemon_id::DaemonId;

use crate::version::BuckVersion;

/// Checks an environment variable to see if we were spawned by a buck daemon and if so, returns the
/// UUID of that daemon.
///
/// This is used to detect nested invocations, but returning `Some` does not guarantee that this is
/// a nested invocation.
pub fn get_possibly_nested_invocation_daemon_uuid() -> Option<String> {
    // Intentionally don't use `buck2_env!` because we don't want this showing up in help output
    std::env::var("BUCK2_DAEMON_UUID").ok()
}

/// Generates the daemon constraints *for the currently running daemon.*
///
/// Note that this function is called *from the daemon* and represents the daemon's constraints -
/// the constraints that the client would like the daemon to have are generated separately.
pub fn gen_daemon_constraints(
    daemon_startup_config: &DaemonStartupConfig,
    daemon_id: &DaemonId,
) -> buck2_error::Result<buck2_cli_proto::DaemonConstraints> {
    Ok(buck2_cli_proto::DaemonConstraints {
        version: version()?,
        user_version: user_version()?,
        daemon_id: daemon_id.to_string(),
        daemon_startup_config: Some(daemon_startup_config.serialize()?),
        extra: None,
    })
}

pub fn version() -> buck2_error::Result<String> {
    Ok(BuckVersion::get_unique_id()?.to_owned())
}

/// Used to make sure that daemons are restarted between CI jobs if they don't properly clean up
/// after themselves.
///
/// A client run by an action takes the value of the daemon running the action, present or
/// absent, which the daemon exports next to `BUCK2_DAEMON_UUID`, so that neither a job id the
/// action's environment dropped nor one it added restarts that daemon under the action.
pub fn user_version() -> buck2_error::Result<Option<String>> {
    // Intentionally don't use `buck2_env!` because we don't want this showing up in help output
    if get_possibly_nested_invocation_daemon_uuid().is_some() {
        return Ok(std::env::var("BUCK2_DAEMON_USER_VERSION").ok());
    }
    // This shouldn't really be necessary, but we used to check it so we'll keep it for now.
    if let Some(id) = buck2_env!("SANDCASTLE_ID", applicability = internal)? {
        return Ok(Some(id.to_owned()));
    }
    // The `ci_identifiers` function reports better identifiers earlier, so taking the first one is
    // enough
    Ok(ci_identifiers()?.find_map(|x| x.1).map(|s| s.to_owned()))
}
