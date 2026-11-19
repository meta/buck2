/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::time::Duration;

use futures::future::Future;
use http::StatusCode;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Http)]
pub enum HttpError {
    #[error(transparent)]
    Client(crate::HttpError),

    #[error("HTTP Transfer Error when querying URL: {}. Failed after {} bytes", .url, .received)]
    Transfer {
        received: u64,
        url: String,
        #[source]
        source: hyper::Error,
    },
}

impl From<crate::HttpError> for HttpError {
    fn from(value: crate::HttpError) -> Self {
        Self::Client(value)
    }
}

impl HttpErrorForRetry for HttpError {
    fn is_retryable(&self) -> bool {
        match self {
            Self::Client(client_error) => match client_error {
                crate::HttpError::Status { status, .. } => {
                    status.is_server_error() || *status == StatusCode::TOO_MANY_REQUESTS
                }
                crate::HttpError::Timeout { .. } => true,
                crate::HttpError::SendRequest { .. } => true,
                _ => false,
            },
            Self::Transfer { .. } => true,
        }
    }
}

pub trait HttpErrorForRetry {
    fn is_retryable(&self) -> bool;
}

pub trait IntoBuck2Error {
    fn into_buck2_error(self) -> buck2_error::Error;
}

pub async fn http_retry<Exec, F, T, E>(exec: Exec, mut intervals: Vec<Duration>) -> Result<T, E>
where
    Exec: Fn() -> F,
    E: IntoBuck2Error + HttpErrorForRetry + std::fmt::Display + Send + Sync + 'static,
    F: Future<Output = Result<T, E>>,
{
    intervals.insert(0, Duration::from_secs(0));
    let mut backoff = intervals.into_iter().peekable();

    while let Some(duration) = backoff.next() {
        tokio::time::sleep(duration).await;

        let err = match exec().await {
            Ok(val) => return Ok(val),
            Err(err) => err,
        };

        if err.is_retryable() {
            if let Some(b) = backoff.peek() {
                tracing::debug!(
                    "Retrying a HTTP error after {} seconds: {:#}",
                    b.as_secs(),
                    // Print as a buck2_error to make sure we get the source
                    err.into_buck2_error()
                );
                continue;
            }
        }

        return Err(err);
    }

    unreachable!("The loop above will exit before we get to the end")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Mock {
        responses: std::cell::RefCell<Vec<Result<String, HttpTestError>>>,
    }

    impl Mock {
        fn new(responses: Vec<Result<String, HttpTestError>>) -> Self {
            Self {
                responses: std::cell::RefCell::new(responses),
            }
        }

        async fn exec(&self) -> Result<String, HttpTestError> {
            // This function is expected to be of type Fn and not FnMut
            // We use internal mutability (RefCell) here to ensure that
            let mut responses = self.responses.borrow_mut();
            responses.remove(0)
        }
    }

    #[derive(Debug, buck2_error::Error)]
    #[buck2(tag = Http)]
    enum HttpTestError {
        #[error("Error in test")]
        Client(#[source] HttpError),
    }

    impl From<HttpError> for HttpTestError {
        fn from(value: HttpError) -> Self {
            Self::Client(value)
        }
    }

    impl HttpErrorForRetry for HttpTestError {
        fn is_retryable(&self) -> bool {
            match self {
                Self::Client(e) => e.is_retryable(),
            }
        }
    }

    impl IntoBuck2Error for HttpTestError {
        fn into_buck2_error(self) -> buck2_error::Error {
            buck2_error::Error::from(self)
        }
    }

    fn ok_response() -> Result<String, HttpTestError> {
        Ok("Success".to_owned())
            .map_err(|_: buck2_error::Error| test_error(StatusCode::IM_A_TEAPOT))
    }

    fn retryable() -> Result<String, HttpTestError> {
        Err(test_error(StatusCode::INTERNAL_SERVER_ERROR))
    }

    fn non_retryable() -> Result<String, HttpTestError> {
        Err(test_error(StatusCode::NOT_FOUND))
    }

    fn test_error(status: StatusCode) -> HttpTestError {
        HttpTestError::Client(HttpError::Client(crate::HttpError::Status {
            status,
            uri: "something".to_owned(),
            text: "something else".to_owned(),
        }))
    }

    fn retries(n: usize) -> Vec<Duration> {
        vec![Duration::from_secs(0); n]
    }

    #[tokio::test]
    async fn test_http_retry_success() {
        let mock = Mock::new(vec![ok_response()]);
        let result = http_retry(|| mock.exec(), retries(0)).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_http_retry_retryable() {
        let mock = Mock::new(vec![retryable(), ok_response()]);
        let result = http_retry(|| mock.exec(), retries(1)).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_http_retry_exhaust_retries() {
        let mock = Mock::new(vec![retryable(), ok_response()]);
        let result = http_retry(|| mock.exec(), retries(0)).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_http_retry_non_retryable() {
        let mock = Mock::new(vec![non_retryable(), ok_response()]);
        let result = http_retry(|| mock.exec(), retries(1)).await;
        assert!(result.is_err());
    }
}
