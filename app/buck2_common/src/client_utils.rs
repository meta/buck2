/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cmp;
use std::net::Ipv4Addr;
use std::path::Path;
use std::time::Duration;

use buck2_error::BuckErrorContext;
use buck2_error::ErrorTag;
use buck2_grpc::configure_endpoint;
use futures::Future;
use tokio::time::Instant;
use tonic::transport::Channel;
use tonic::transport::Endpoint;

pub static UDS_DAEMON_FILENAME: &str = "buckd.uds";

#[cfg(unix)]
pub async fn get_channel_uds(
    unix_socket: &Path,
    change_to_parent_dir: bool,
) -> buck2_error::Result<Channel> {
    use buck2_fs::error::IoResultExt;
    use buck2_fs::fs_util;

    use crate::home_buck_tmp::home_buck_tmp_dir;
    use crate::temp_path::TempPath;

    // Symlink to temp file to connect to unix domain socket
    // since the unix domain socket path is limited to 108 characters.
    // https://man7.org/linux/man-pages/man7/unix.7.html
    if change_to_parent_dir {
        let symlink = TempPath::new_in(home_buck_tmp_dir()?)?;

        fs_util::symlink(unix_socket, symlink.path()).categorize_internal()?;

        let r = get_channel_uds_no_symlink(symlink.path())
            .await
            .with_buck_error_context(|| {
                format!(
                    "Failed to connect to unix domain socket `{}` using symlink `{}`",
                    unix_socket.display(),
                    symlink.path().display(),
                )
            })?;

        symlink.close()?;

        Ok(r)
    } else {
        get_channel_uds_no_symlink(unix_socket)
            .await
            .with_buck_error_context(|| {
                format!(
                    "Failed to connect to unix domain socket `{}`",
                    unix_socket.display()
                )
            })
    }
}

#[cfg(unix)]
async fn get_channel_uds_no_symlink(connect_to: &Path) -> buck2_error::Result<Channel> {
    use tonic::codegen::http::Uri;
    use tower::service_fn;

    let connect_to = connect_to.to_owned();
    // This URL string is not relevant to the connection. Some URL is required for the function to work but the closure running inside connect_with_connector()
    // deals with connecting to the unix domain socket.
    configure_endpoint(Endpoint::try_from("http://[::]:50051")?)
        .connect_with_connector(service_fn(move |_: Uri| {
            let path = connect_to.clone();
            async move {
                let io = tokio::net::UnixStream::connect(&path).await?;
                Ok::<_, std::io::Error>(hyper_util::rt::tokio::TokioIo::new(io))
            }
        }))
        .await
        .tag(ErrorTag::ServerTransportError)
}

#[cfg(windows)]
pub async fn get_channel_uds(
    _unix_filename: &Path,
    _chg_dir: bool,
) -> buck2_error::Result<Channel> {
    Err(buck2_error::buck2_error!(
        buck2_error::ErrorTag::WindowsUnsupported,
        "Unix domain sockets are not supported on Windows",
    ))
}

pub async fn get_channel_tcp(socket_addr: Ipv4Addr, port: u16) -> buck2_error::Result<Channel> {
    configure_endpoint(Endpoint::try_from(format!("http://{socket_addr}:{port}"))?)
        .connect()
        .await
        .tag(ErrorTag::ServerTransportError)
        .with_buck_error_context(|| format!("failed to connect to port {port}"))
}

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Environment)]
pub enum RetryError<E> {
    #[error("Timed out after {0:.2}s")]
    Timeout(f64),
    #[error("Failed after {tries} attempts over {timeout_secs:.2}s, with error: {last_error}")]
    Failed {
        timeout_secs: f64,
        tries: i32,
        last_error: E,
    },
}

pub async fn retrying<L, E, Fut: Future<Output = Result<L, E>>, F: FnMut() -> Fut>(
    initial_delay: Duration,
    max_delay: Duration,
    timeout: Duration,
    mut func: F,
) -> Result<L, RetryError<E>> {
    let deadline = Instant::now() + timeout;
    let mut wait = initial_delay;
    let mut tries = 0;
    let mut last_error = None;

    loop {
        tries += 1;
        match tokio::time::timeout_at(deadline, func()).await {
            Ok(Ok(val)) => return Ok(val),
            Ok(Err(e)) => {
                last_error = Some(e);
                let now = Instant::now();
                if now >= deadline {
                    return Err(make_error(tries, last_error, timeout));
                }
                tokio::time::sleep_until(cmp::min(deadline, now + wait)).await;
                wait = cmp::min(max_delay, wait * 2);
            }
            Err(_) => return Err(make_error(tries, last_error, timeout)),
        }
    }

    fn make_error<E>(tries: i32, last_error: Option<E>, timeout: Duration) -> RetryError<E> {
        let timeout_secs = timeout.as_secs_f64();
        match last_error {
            Some(e) => RetryError::Failed {
                timeout_secs,
                tries,
                last_error: e,
            },
            None => RetryError::Timeout(timeout_secs),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::client_utils::RetryError;
    use crate::client_utils::retrying;

    #[tokio::test]
    async fn test_retrying_error_forever() {
        // Testing against real timer is not good,
        // but setting up proper mock timer is too much boilerplate.
        let future = retrying(
            Duration::from_millis(1),
            Duration::from_millis(1),
            Duration::from_millis(1),
            || async {
                Err(buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Input,
                    "test"
                ))
            },
        );
        let result: Result<(), RetryError<buck2_error::Error>> = future.await;
        assert!(result.is_err());
    }
}
