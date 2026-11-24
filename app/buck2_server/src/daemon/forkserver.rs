/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_common::invocation_paths::InvocationPaths;
use buck2_execute_impl::executors::local::ForkserverAccess;
use buck2_fs::paths::file_name::FileNameBuf;
use buck2_resource_control::buck_cgroup_tree::BuckCgroupTree;

/// Launches the forkserver on the platforms that have one.
#[cfg(unix)]
pub async fn launch_forkserver(
    paths: &InvocationPaths,
    cgroup_tree: Option<&BuckCgroupTree>,
    isolation_dir: &FileNameBuf,
) -> buck2_error::Result<ForkserverAccess> {
    use buck2_error::BuckErrorContext;

    let state_dir = paths.forkserver_state_dir()?;
    let exe = std::env::current_exe().buck_error_context("Cannot access current_exe")?;
    Ok(ForkserverAccess::Client(
        buck2_forkserver::launch::launch_forkserver(
            exe,
            // `--isolation-dir` is not read by the forkserver itself; it is carried on the
            // command line so that `buck2 killall --isolation-dir` can attribute the process.
            &["forkserver", "--isolation-dir", isolation_dir.as_str()],
            &state_dir,
            cgroup_tree,
        )
        .await?,
    ))
}

/// Launches the forkserver on the platforms that have one.
#[cfg(not(unix))]
pub async fn launch_forkserver(
    _paths: &InvocationPaths,
    _cgroup_tree: Option<&BuckCgroupTree>,
    _isolation_dir: &FileNameBuf,
) -> buck2_error::Result<ForkserverAccess> {
    Ok(ForkserverAccess::None)
}
