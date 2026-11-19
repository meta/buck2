/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The traits that defines the protocol between buck and a test executor.
//!
//! Test executors are expected to implement the trait `TestExecutor`. Buck will need to implement
//! the trait `Buck` for the test executor to interact against.

use std::time::Duration;

use buck2_error::ErrorTag;
use buck2_hash::BuckFixedSortedMap;
use dupe::Dupe;
use host_sharing::HostSharingRequirements;

use crate::data::ArgValue;
use crate::data::CasDigest;
use crate::data::ConfiguredTargetHandle;
use crate::data::DeclaredOutput;
use crate::data::ExecuteResponse;
use crate::data::ExecutorConfigOverride;
use crate::data::ExternalRunnerSpec;
use crate::data::PrepareForLocalExecutionResult;
use crate::data::RequiredLocalResources;
use crate::data::TestResult;
use crate::data::TestStage;

/// available to buck to interact with the test executor
#[async_trait::async_trait]
pub trait TestExecutor: Send + Sync {
    /// sends an external runner spec to the test executor
    async fn external_runner_spec(&self, s: ExternalRunnerSpec) -> buck2_error::Result<()>;

    // report that there are no more test specs to send
    async fn end_of_test_requests(&self) -> buck2_error::Result<()>;

    /// performs a heap dump and saves the dump to a file.
    async fn unstable_heap_dump(&self, _: &str) -> buck2_error::Result<()> {
        Err(buck2_error::buck2_error!(ErrorTag::Input, "Unimplemented!"))
    }
}

// available to the test executor to interact with the orchestrator
#[async_trait::async_trait]
pub trait TestOrchestrator: Send + Sync {
    /// executes the given command and returns the result of the execution
    async fn execute2(
        &self,
        // information about this execute request for Buck's UX
        stage: TestStage,
        // the label of the rule being tested
        target: ConfiguredTargetHandle,
        // the command to run
        cmd: Vec<ArgValue>,
        // environment variables to set at runtime
        env: BuckFixedSortedMap<String, ArgValue>,
        // timeout for command
        timeout: Duration,
        // parameters used to effectively share the executor host for this command.
        host_sharing_requirements: HostSharingRequirements,
        // outputs that need to be pre created as directories
        pre_create_dirs: Vec<DeclaredOutput>,
        // A specific executor to use for this. It must be declared on the underlying
        // ExternalRunnerTestInfo to work.
        executor_override: Option<ExecutorConfigOverride>,
        required_local_resources: RequiredLocalResources,
        // When true, disables test execution cache lookups even if the test rule
        // supports it.
        disable_test_execution_caching: bool,
    ) -> buck2_error::Result<ExecuteResponse>;

    /// reports a test is done
    async fn report_test_result(&self, r: TestResult) -> buck2_error::Result<()>;

    async fn report_tests_discovered(
        &self,
        target: ConfiguredTargetHandle,
        suite: String,
        name: Vec<String>,
    ) -> buck2_error::Result<()>;

    /// report a summary about the current test executor
    async fn report_test_session(
        &self,
        session_info: String,
        test_session_id: Option<String>,
    ) -> buck2_error::Result<()>;

    /// report that all tests are done and provide the exit code that this test executor wants to
    /// return for the test command, no more executions
    async fn end_of_test_results(&self, exit_code: i32) -> buck2_error::Result<()>;

    /// prepare the given test executable to be available for local execution.
    /// Return the actual command with all the args, env and cwd to be executed locally.
    async fn prepare_for_local_execution(
        &self,
        stage: TestStage,
        target: ConfiguredTargetHandle,
        cmd: Vec<ArgValue>,
        env: BuckFixedSortedMap<String, ArgValue>,
        pre_create_dirs: Vec<DeclaredOutput>,
        required_local_resources: RequiredLocalResources,
    ) -> buck2_error::Result<PrepareForLocalExecutionResult>;

    /// attach a message containing information that the executor wants to be surfaced
    /// to the user
    async fn attach_info_message(&self, message: String) -> buck2_error::Result<()>;

    /// Upload a local file to CAS and return its digest.
    ///
    /// This is called by tpx to upload local test artifacts to CAS instead of
    /// Everstore. Buck2 handles the actual CAS upload using its RE client.
    async fn upload_to_cas(
        &self,
        local_path: String,
        ttl_seconds: i64,
        use_case: String,
    ) -> buck2_error::Result<CasDigest>;
}

// TODO need to figure out what this is. we can go without it for now
#[derive(Debug, Default, Clone, Dupe)]
pub struct ExecPlatformRefinement;

/// the template name for external test executor to fill with the test execution location
pub const OUTPUT_DIR: &str = "output_dir";

/// the template name for buck to fill with the commands outputs
pub const OUTPUTS_TEMPLATE: &str = "outputs";

/// the template name for buck to fill with the run cmd based on the test's run information
/// this should be multi-arity
pub const TEST_RUN_CMD: &str = "test_run_cmd";

/// the template name for buck to fill with the run environment based on the test's run information
/// this should be multi-arity
pub const TEST_RUN_ENV_KEYS: &str = "test_run_env_keys";

/// the template name for buck to fill with the run environment based on the test's run information
/// this should be multi-arity
pub const TEST_RUN_ENV_VALUES: &str = "test_run_env_values";

/// the template name for the external test runner to fill with test filters
pub const TEST_FILTER_TEMPLATE: &str = "test_filter";
