/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_common::manifold::Ttl;
use buck2_core::buck2_env;
use buck2_events::metadata::is_devserver_placeholder_unixname;
use buck2_events::metadata::username;
use buck2_events::schedule_type::SandcastleScheduleType;

// Copied from "Is User Command" from scuba buck2_builds
const ROBOTS: &[&str] = &[
    "twsvcscm",
    "svcscm",
    "facebook",
    "root",
    "svc-si_admin",
    "svc-fbsi_datamgr",
];

const USER_TTL_DAYS: u64 = 365;
const DEFAULT_TTL_DAYS: u64 = 60;
// diff signal retention is 4 weeks
const CI_EXCEPT_CONTINUOUS_TTL_DAYS: u64 = 28;

pub fn manifold_event_log_ttl() -> buck2_error::Result<Ttl> {
    manifold_event_log_ttl_impl(
        ROBOTS,
        username().ok().flatten(),
        SandcastleScheduleType::new()?,
    )
}

fn manifold_event_log_ttl_impl(
    robots: &[&str],
    username: Option<String>,
    schedule_type: SandcastleScheduleType,
) -> buck2_error::Result<Ttl> {
    // 1. return if this is a test
    let env = buck2_env!("BUCK2_TEST_MANIFOLD_TTL_S", type=u64, applicability=testing)?;
    if let Some(env) = env {
        return Ok::<Ttl, buck2_error::Error>(Ttl::from_secs(env));
    }

    // 2. return if this is a user
    if let Some(username) = username {
        if !robots.contains(&(username.as_str())) && !is_devserver_placeholder_unixname(&username) {
            return Ok::<Ttl, buck2_error::Error>(Ttl::from_days(USER_TTL_DAYS));
        }
    }

    // 3. return if it's not continuous
    if schedule_type.is_some() && !schedule_type.is_continuous() {
        return Ok(Ttl::from_days(CI_EXCEPT_CONTINUOUS_TTL_DAYS));
    }

    // 4. use default
    Ok::<Ttl, buck2_error::Error>(Ttl::from_days(DEFAULT_TTL_DAYS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_a_user() -> buck2_error::Result<()> {
        assert_eq!(
            manifold_event_log_ttl_impl(
                &["twsvcscm"],
                Some("random_person".to_owned()),
                SandcastleScheduleType::testing_new("continuous")
            )?
            .as_secs(),
            365 * 24 * 60 * 60,
        );
        Ok(())
    }

    #[test]
    fn test_not_a_user() -> buck2_error::Result<()> {
        assert_eq!(
            manifold_event_log_ttl_impl(
                &["twsvcscm"],
                Some("twsvcscm".to_owned()),
                SandcastleScheduleType::testing_empty()
            )?
            .as_secs(),
            60 * 24 * 60 * 60,
        );
        Ok(())
    }

    #[cfg(fbcode_build)]
    #[test]
    fn test_devserver_placeholder_is_not_a_user() -> buck2_error::Result<()> {
        assert_eq!(
            manifold_event_log_ttl_impl(
                &["twsvcscm"],
                Some("devenvuser2556".to_owned()),
                SandcastleScheduleType::testing_empty()
            )?
            .as_secs(),
            60 * 24 * 60 * 60,
        );
        Ok(())
    }

    #[test]
    fn test_not_a_user_and_not_continuous() -> buck2_error::Result<()> {
        assert_eq!(
            manifold_event_log_ttl_impl(
                &["twsvcscm"],
                Some("twsvcscm".to_owned()),
                SandcastleScheduleType::testing_new("foo")
            )?
            .as_secs(),
            28 * 24 * 60 * 60,
        );
        Ok(())
    }

    #[test]
    fn test_not_a_user_and_continuous() -> buck2_error::Result<()> {
        assert_eq!(
            manifold_event_log_ttl_impl(
                &["twsvcscm"],
                Some("twsvcscm".to_owned()),
                SandcastleScheduleType::testing_new("continuous")
            )?
            .as_secs(),
            60 * 24 * 60 * 60,
        );
        Ok(())
    }
}
