/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Display;
use std::path::Path;
use std::path::PathBuf;

use buck2_error::BuckErrorContext;
use buck2_error::internal_error;

pub const BUCK_AUTH_TOKEN_HEADER: &str = "x-buck-auth-token";

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Tier0)]
enum ConnectionTypeError {
    #[error("Failed to parse correct endpoint information {0}")]
    ParseError(String),
}

#[derive(Debug, Clone)]
pub enum ConnectionType {
    // TODO: remove Unix support a week after D40394826 lands.
    Uds { unix_socket: PathBuf },
    Tcp { port: u16 },
}

impl Display for ConnectionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // NOTE: `Display` must be compatible with `parse`.
        match self {
            ConnectionType::Uds { unix_socket } => write!(f, "uds:{}", unix_socket.display()),
            ConnectionType::Tcp { port } => write!(f, "tcp:{port}"),
        }
    }
}

impl ConnectionType {
    pub fn parse(endpoint: &str) -> buck2_error::Result<ConnectionType> {
        let (protocol, endpoint) = endpoint.split_once(":").ok_or_else(|| {
            internal_error!("endpoint `{endpoint}` is not in the format `protocol:endpoint`")
        })?;
        match protocol {
            "uds" => Ok(ConnectionType::Uds {
                unix_socket: Path::new(endpoint).to_path_buf(),
            }),
            "tcp" => Ok(ConnectionType::Tcp {
                port: endpoint.parse().with_buck_error_context(|| {
                    format!("port number is incorrect in `{endpoint}`")
                })?,
            }),
            _ => Err(ConnectionTypeError::ParseError(endpoint.to_owned()).into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::buckd_connection::ConnectionType;

    #[test]
    fn test_fmt_parse() {
        assert_eq!(
            "tcp:1719",
            &format!("{}", ConnectionType::Tcp { port: 1719 })
        );

        let path = if cfg!(windows) {
            PathBuf::from("c:\\path")
        } else {
            PathBuf::from("/path")
        };
        assert_eq!(
            &format!("uds:{}", path.display()),
            &format!("{}", ConnectionType::Uds { unix_socket: path })
        );
    }
}
