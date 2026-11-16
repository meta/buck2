/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use crate::test_response::TestStatuses;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = TestExecutor)]
enum ExecutorError {
    #[buck2(tag = TestRunnerInternal)]
    #[error("Internal error in test runner")]
    InternalError,
    #[buck2(tag = Input)]
    #[error("Tests completed with cancellations")]
    CompletedWithCancellations,
    #[buck2(tag = Input)]
    #[error("Tests passed with warnings")]
    PassWithWarnings,
    #[error(transparent)]
    Fail(TestStatusError),
    #[error(transparent)]
    NeedsBaseRevisionRetry(TestStatusError),
    #[error(transparent)]
    NeedsAdditionalVerification(TestStatusError),
    #[buck2(tag = TestRunnerUnknownExitCode)]
    #[error("Test Executor Failed with exit code {0}")]
    UnexpectedExitCode(i32),
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = TestExecutor)]
enum TestStatusError {
    #[error("Test execution completed but the tests failed")]
    #[buck2(tag = TestFailed)]
    TestFailed,
    #[error("Test listing failed")]
    #[buck2(tag = TestListingFailed)]
    ListingFailed,
    #[error("Fatal error encountered during test execution")]
    #[buck2(tag = TestFatal)]
    Fatal,
    #[error("Infra Failure error encountered during test execution")]
    #[buck2(tag = TestInfraFailure)]
    InfraFailure,
    #[error("Test execution completed but some tests timed out")]
    #[buck2(tag = TestTimeout)]
    TestTimeout,
    #[error("Unexpected failure during test execution")]
    #[buck2(tag = TestStatusUnknown)]
    Unknown,
}

impl ExecutorError {
    fn new(exit_code: i32, test_statuses: &TestStatuses) -> Option<Self> {
        let status_error = TestStatusError::new(test_statuses);
        // exit codes from tpx::outcome::RunVerdict
        match exit_code {
            0 => None,
            1 => Some(Self::InternalError),
            2 => Some(Self::CompletedWithCancellations),
            32 => Some(Self::Fail(status_error)),
            42 => Some(Self::NeedsBaseRevisionRetry(status_error)),
            43 => Some(Self::NeedsAdditionalVerification(status_error)),
            64 => Some(Self::PassWithWarnings),
            _ => Some(Self::UnexpectedExitCode(exit_code)),
        }
    }
}

impl TestStatusError {
    fn new(test_statuses: &TestStatuses) -> Self {
        if let Some(fatal) = &test_statuses.fatals
            && fatal.count > 0
        {
            Self::Fatal
        } else if let Some(infra_failure) = &test_statuses.infra_failure
            && infra_failure.count > 0
        {
            Self::InfraFailure
        } else if let Some(listing_failed) = &test_statuses.listing_failed
            && listing_failed.count > 0
        {
            Self::ListingFailed
        } else if let Some(failed) = &test_statuses.failed
            && failed.count > 0
        {
            Self::TestFailed
        } else if let Some(timed_out) = &test_statuses.timed_out
            && timed_out.count > 0
        {
            Self::TestTimeout
        } else {
            Self::Unknown
        }
    }
}

/// The error that a `buck2 test` whose test executor exited with `executor_exit_code` fails
/// with, for tagging and categorization only, or `None` when the executor succeeded.
pub fn test_executor_error(
    executor_exit_code: i32,
    test_statuses: &TestStatuses,
) -> Option<buck2_error::Error> {
    if let Some(error) = ExecutorError::new(executor_exit_code, test_statuses) {
        let exit_code_tag = if let ExecutorError::UnexpectedExitCode(exit_code) = error {
            Some(exit_code.to_string())
        } else {
            None
        };

        let mut error = buck2_error::Error::from(error);
        if let Some(tag) = exit_code_tag {
            error = error.string_tag(&tag);
        }
        Some(error)
    } else {
        None
    }
}
