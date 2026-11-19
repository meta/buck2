/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::process::ExitStatus;

use async_trait::async_trait;
use buck2_error::BuckErrorContext;
use buck2_error::conversion::from_any_with_tag;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_miniperf_proto::MiniperfOutput;

use crate::CollectedExecutionStats;

pub enum DecodedStatus {
    /// An actual status.
    Status {
        exit_code: i32,
        execution_stats: Option<CollectedExecutionStats>,
    },

    /// Spawn failed, provide the error.
    SpawnFailed(String),
}

#[async_trait]
pub trait StatusDecoder {
    /// Status decoders receive the exit status of the command we ran, but they might also obtain
    /// information out of band to obtain a different exit status.
    async fn decode_status(self, status: ExitStatus) -> buck2_error::Result<DecodedStatus>;

    /// Notify this decoder that it will not be used.
    async fn cancel(self) -> buck2_error::Result<()>;
}

pub struct DefaultStatusDecoder;

#[async_trait]
impl StatusDecoder for DefaultStatusDecoder {
    async fn decode_status(self, status: ExitStatus) -> buck2_error::Result<DecodedStatus> {
        Ok(DecodedStatus::Status {
            exit_code: default_decode_exit_code(status),
            execution_stats: None,
        })
    }

    async fn cancel(self) -> buck2_error::Result<()> {
        Ok(())
    }
}

fn default_decode_exit_code(status: ExitStatus) -> i32 {
    let exit_code;

    #[cfg(unix)]
    {
        // Shell convention on UNIX is to return 128 + signal number on a signal exit,
        // so we emulate this here.
        use std::os::unix::process::ExitStatusExt;
        exit_code = status.code().or_else(|| Some(128 + status.signal()?));
    }

    #[cfg(not(unix))]
    {
        exit_code = status.code();
    }

    exit_code.unwrap_or(-1)
}

pub struct MiniperfStatusDecoder {
    out_path: AbsNormPathBuf,
}

impl MiniperfStatusDecoder {
    #[allow(dead_code)]
    pub fn new(out_path: AbsNormPathBuf) -> Self {
        Self { out_path }
    }
}

#[async_trait]
impl StatusDecoder for MiniperfStatusDecoder {
    async fn decode_status(self, status: ExitStatus) -> buck2_error::Result<DecodedStatus> {
        if !status.success() {
            return Ok(DecodedStatus::Status {
                exit_code: default_decode_exit_code(status),
                execution_stats: None,
            });
        }

        let status = tokio::fs::read(&self.out_path)
            .await
            .with_buck_error_context(|| {
                format!(
                    "Error reading miniperf output at `{}`",
                    self.out_path.display()
                )
            })?;

        tokio::fs::remove_file(&self.out_path)
            .await
            .with_buck_error_context(|| {
                format!("Error removing miniperf output at `{}`", self.out_path)
            })?;

        let (status, _) = bincode::serde::decode_from_slice::<MiniperfOutput, _>(
            &status,
            bincode::config::legacy(),
        )
        .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0))
        .with_buck_error_context(|| {
            format!("Invalid miniperf output at `{}`", self.out_path.display())
        })?;

        match status.raw_exit_code {
            #[allow(unused_variables)]
            Ok(v) => {
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    let exit_code = default_decode_exit_code(ExitStatus::from_raw(v));
                    let execution_stats = status.counters.map(|counters| CollectedExecutionStats {
                        cpu_instructions_user: Some(counters.user_instructions.adjusted_count()),
                        cpu_instructions_kernel: Some(
                            counters.kernel_instructions.adjusted_count(),
                        ),
                        userspace_events: Some(counters.user_instructions.into()),
                        kernel_events: Some(counters.kernel_instructions.into()),
                    });

                    if let Err(e) = execution_stats.as_ref() {
                        // TODO @torozco: report this in the event log? Might be verbose for little
                        // value.
                        tracing::debug!("Miniperf stats not available: {}", e);
                    }

                    Ok(DecodedStatus::Status {
                        exit_code,
                        execution_stats: execution_stats.ok(),
                    })
                }

                #[cfg(not(unix))]
                {
                    Err(buck2_error::buck2_error!(
                        buck2_error::ErrorTag::Tier0,
                        "Attempted to use Miniperf output off-UNIX"
                    ))
                }
            }
            Err(e) => Ok(DecodedStatus::SpawnFailed(e)),
        }
    }

    async fn cancel(self) -> buck2_error::Result<()> {
        let res = tokio::fs::remove_file(&self.out_path).await;

        match res {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(buck2_error::Error::from(e).context(format!(
                "Error removing miniperf output at `{}`",
                self.out_path
            ))),
        }
    }
}
