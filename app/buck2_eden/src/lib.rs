/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

// This can't be built in our OSS implementation.
#![cfg(fbcode_build)]

pub mod connection;
pub mod error;
pub mod io_provider;
pub mod tenting;

pub mod semaphore {
    use buck2_core::buck2_env;
    use tokio::sync::Semaphore;

    // This value was selected semi-randomly and should be revisited in the future. Anecdotally, we
    // have seen EdenFS struggle with <<< 2048 outstanding requests, but the exact number depends
    // on the size/complexity/cost of the outstanding requests.
    pub static DEFAULT_MAX_OUTSTANDING_REQUESTS: usize = 2048;

    /// A default semaphore that is used to limit the number of outstanding requests to EdenFS.
    pub fn default() -> Semaphore {
        Semaphore::new(DEFAULT_MAX_OUTSTANDING_REQUESTS)
    }

    /// A buck2-specific semaphore that is used to limit the number of outstanding requests to
    /// EdenFS. Reads buck2 specific environment variable "BUCK2_EDEN_SEMAPHORE" to determine the
    /// number of permits.
    pub fn buck2_default() -> buck2_error::Result<Semaphore> {
        let permits = buck2_env!("BUCK2_EDEN_SEMAPHORE", type=usize, default=DEFAULT_MAX_OUTSTANDING_REQUESTS, applicability=internal)?;
        with_permits(permits)
    }

    /// A semaphore with `permits` permits; zero is refused because such a semaphore never admits
    /// a request, so every Eden call would wait forever.
    pub(crate) fn with_permits(permits: usize) -> buck2_error::Result<Semaphore> {
        if permits == 0 {
            return Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::Input,
                "`BUCK2_EDEN_SEMAPHORE` must be at least 1: a semaphore without permits never admits a request"
            ));
        }
        Ok(Semaphore::new(permits))
    }
}

#[cfg(test)]
mod tests {
    use crate::semaphore::with_permits;

    #[test]
    fn zero_permits_is_an_error_naming_the_variable() {
        let err = with_permits(0).expect_err("zero permits must be an error");
        assert!(
            format!("{err:#}").contains("BUCK2_EDEN_SEMAPHORE"),
            "{err:#}"
        );
    }

    #[test]
    fn one_permit_is_accepted() {
        let semaphore = with_permits(1).expect("one permit is a valid semaphore");
        assert_eq!(1, semaphore.available_permits());
    }
}
