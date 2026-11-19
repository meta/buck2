/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::time::SystemTime;

use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_common::chunk_reader::ChunkReader;
use buck2_common::manifold;
use buck2_common::manifold::ManifoldChunkedUploader;
use buck2_common::manifold::ManifoldClient;
use buck2_core::soft_error_or_panic;
use buck2_data::InstantEvent;
use buck2_data::PersistEventLogSubprocess;
use buck2_data::instant_event::Data;
use buck2_error::BuckErrorContext;
use buck2_event_log::ttl::manifold_event_log_ttl;
use buck2_events::BuckEvent;
use buck2_events::daemon_id::DaemonId;
use buck2_events::sink::remote::RemoteEventSink;
use buck2_events::sink::remote::ScribeConfig;
use buck2_events::sink::remote::new_remote_event_sink_if_enabled;
use buck2_fs::paths::abs_path::AbsPathBuf;
use buck2_wrapper_common::invocation_id::TraceId;
use tokio::fs::File;
use tokio::fs::OpenOptions;
use tokio::io;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncSeekExt;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use tokio::time::Duration;
use tokio::time::Instant;
use tokio::time::sleep;

const MAX_WAIT: Duration = Duration::from_mins(5);

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Tier0)]
pub(crate) enum PersistEventLogError {
    #[error("Read more bytes than are available")]
    ReadBytesOverflow,
}

/// Read binary event log from stdin and simultaneously write it to disk and optionally upload to Manifold.
///
/// This command is launched by the buck client to continue log streaming
/// after client command finishes. It is not intended to be used directly.
#[derive(Debug, clap::Parser)]
pub struct PersistEventLogsCommand {
    #[clap(long, help = "Name this log will take in Manifold")]
    manifold_name: String,
    #[clap(long, help = "Where to write this log to on disk")]
    local_path: String,
    #[clap(long, help = "If present, only write to disk and don't upload")]
    no_upload: bool,
    #[clap(
        long,
        help = "UUID of invocation that called this subcommand for logging purposes"
    )]
    trace_id: TraceId,
}

impl PersistEventLogsCommand {
    pub fn exec(
        self,
        _matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        buck2_core::facebook_only();
        events_ctx.log_invocation_record = false;
        let sink = create_scribe_sink(&ctx)?;
        let trace_id = self.trace_id.clone();
        ctx.with_runtime(|mut ctx| async move {
            let mut stdin = io::BufReader::new(ctx.stdin());
            let (local_result, remote_result) = self.write_and_upload(&mut stdin).await;

            let (local_error_messages, local_error_category, local_success) =
                status_from_result(local_result);
            let (remote_error_messages, remote_error_category, remote_success) =
                status_from_result(remote_result);

            let event_to_send = PersistEventLogSubprocess {
                local_error_messages,
                local_error_category,
                local_success,
                remote_error_messages,
                remote_error_category,
                remote_success,
                metadata: buck2_events::metadata::collect(&DaemonId::null()),
            };
            dispatch_event_to_scribe(sink.as_ref(), &trace_id, event_to_send).await;
        });
        ExitResult::success()
    }

    async fn write_and_upload(
        self,
        stdin: impl io::AsyncBufRead + Unpin,
    ) -> (buck2_error::Result<()>, buck2_error::Result<()>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let file = match create_log_file(self.local_path).await {
            Ok(f) => Mutex::new(f),
            Err(e) => {
                return (
                    Err(e),
                    Err(buck2_error::buck2_error!(
                        buck2_error::ErrorTag::Tier0,
                        "Not tried"
                    )),
                );
            }
        };
        let write = write_task(&file, tx, stdin);
        let upload = upload_task(&file, rx, self.manifold_name, self.no_upload);

        // Wait for both tasks to finish. If the upload fails we want to keep writing to disk
        let (write_result, upload_result) = tokio::join!(write, upload);
        (write_result, upload_result)
    }
}

async fn write_task(
    file_mutex: &Mutex<File>,
    tx: tokio::sync::mpsc::UnboundedSender<u64>,
    mut stdin: impl io::AsyncBufRead + Unpin,
) -> buck2_error::Result<()> {
    let mut write_position = 0;
    let mut buf = vec![0; 64 * 1024]; // maximum pipe size in linux
    loop {
        let bytes_read = stdin.read(&mut buf).await?;
        if bytes_read == 0 {
            break; // closed stdin
        }
        let mut file = file_mutex.lock().await;
        write_to_file(&mut file, write_position, &buf[..bytes_read]).await?;
        drop(file);
        write_position += bytes_read as u64;
        let _ignored = tx.send(bytes_read as u64); // If this errors, that means the upload task died.
    }

    Ok(())
}

async fn create_log_file(local_path: String) -> Result<tokio::fs::File, buck2_error::Error> {
    let local_path = AbsPathBuf::new(local_path)?;

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .write(true)
        .read(true)
        .open(&local_path)
        .await
        .with_buck_error_context(|| {
            format!(
                "Failed to open event log for writing at `{}`",
                local_path.display()
            )
        })?;
    Ok(file)
}

async fn upload_task(
    file_mutex: &Mutex<File>,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<u64>,
    manifold_name: String,
    no_upload: bool,
) -> buck2_error::Result<()> {
    if no_upload {
        return Ok(());
    }

    let manifold_client = ManifoldClient::new().await?;
    let manifold_path = format!("flat/{manifold_name}");
    let mut uploader = Uploader::new(file_mutex, &manifold_path, &manifold_client)?;

    loop {
        tokio::select! {
            res = rx.recv() => {
                match res {
                    Some(n) => {
                        uploader.bump_total_bytes(n);
                        while uploader.can_fill_chunk()? {
                            uploader.last_upload_attempt = Instant::now();
                            uploader.upload_chunk().await?;
                        }
                    },
                    // This indicates that we have finished writing to the log file
                    None => break
                }
            }
            _ = sleep(uploader.wait()) => {
                // We have waited enough since the last upload
                uploader.last_upload_attempt = Instant::now();
                if uploader.something_to_upload() {
                    uploader.upload_chunk().await?;
                }
            }
        }
    }

    // When tx gets dropped, rx will return None
    while uploader.can_fill_chunk()? {
        uploader.upload_chunk().await?;
    }

    // Last chunk to upload is smaller than &reader
    uploader.upload_chunk().await?;

    Ok(())
}

/// Provides methods to:
/// - decide when to upload a chunk of the log file
/// - do the actual upload, which is mostly managed by `ManifoldChunkedUploader`
struct Uploader<'a> {
    file_mutex: &'a Mutex<File>,
    manifold: ManifoldChunkedUploader<'a>,
    reader: ChunkReader,
    total_bytes: u64,
    last_upload_attempt: Instant,
}

impl<'a> Uploader<'a> {
    fn new(
        file_mutex: &'a Mutex<File>,
        manifold_path: &'a str,
        manifold_client: &'a ManifoldClient,
    ) -> buck2_error::Result<Self> {
        let manifold = manifold_client.start_chunked_upload(
            manifold::Bucket::EVENT_LOGS,
            manifold_path,
            manifold_event_log_ttl()?,
        );

        Ok(Self {
            file_mutex,
            manifold,
            reader: ChunkReader::new()?,
            total_bytes: 0,
            last_upload_attempt: Instant::now(),
        })
    }

    /// Uploads at most 'chunk size' bytes to Manifold
    async fn upload_chunk(&mut self) -> buck2_error::Result<()> {
        let mut file = self.file_mutex.lock().await;
        file.seek(io::SeekFrom::Start(self.manifold.position()))
            .await
            .buck_error_context("Failed to seek log file")?;
        let buf = self.reader.read(&mut *file).await?;
        drop(file);

        self.manifold.write(buf.into()).await?;
        Ok(())
    }

    fn bump_total_bytes(&mut self, n: u64) {
        self.total_bytes += n
    }

    fn can_fill_chunk(&mut self) -> buck2_error::Result<bool> {
        Ok(self
            .total_bytes
            .checked_sub(self.manifold.position())
            .ok_or(PersistEventLogError::ReadBytesOverflow)?
            > self.reader.chunk_size())
    }

    fn something_to_upload(&self) -> bool {
        self.total_bytes > self.manifold.position()
    }

    fn wait(&self) -> Duration {
        MAX_WAIT.saturating_sub(Instant::now() - self.last_upload_attempt)
    }
}

async fn write_to_file(
    file: &mut File,
    write_position: u64,
    buf: &[u8],
) -> Result<(), buck2_error::Error> {
    file.seek(io::SeekFrom::Start(write_position))
        .await
        .buck_error_context("Failed to seek log file")?;
    file.write_all(buf).await?;
    file.flush().await?;
    Ok(())
}

fn status_from_result(res: buck2_error::Result<()>) -> (Vec<String>, Option<String>, bool) {
    // Returns a tuple of error messages, error category, and success/failure
    if let Err(e) = res {
        let status = (
            vec![e.to_string()],
            Some(categorize_error(&e).to_owned()),
            false,
        );
        soft_error_or_panic!(categorize_error(&e), e);
        status
    } else {
        (vec![], None, true)
    }
}

fn categorize_error(err: &buck2_error::Error) -> &'static str {
    // This is for internal error tracking in `logview buck2`
    // Each category should point to 1 root cause
    // In case any of this is to be changed, just give a heads up
    // to anybody who may be actively tracking these errors
    let err_msg = format!("{err:#}");
    if err_msg.contains("CertificateRequired") {
        "persist_log_certificate_required"
    } else if err_msg.contains("No space left on device") {
        "persist_log_no_space_on_device"
    } else if err_msg.contains("Timed out while making request") {
        "persist_log_http_write_timeout"
    } else if err_msg.contains("Input/output error") {
        "persist_log_input_output_error"
    } else if err_msg.contains("failed to lookup address information") {
        "persist_log_dns_failed_lookup"
    } else if err_msg.contains("BadCertificate") {
        "persist_log_bad_certificate"
    } else if err_msg.contains("connection reset") {
        "persist_log_connection_reset"
    } else if err_msg.contains("Broken pipe") {
        "persist_log_broken_pipe"
    } else if err_msg.contains("Error loading system root certificates") {
        "persist_log_error_loading_system_root_certs"
    } else {
        "persist_log_other"
    }
}

async fn dispatch_event_to_scribe(
    sink: Option<&RemoteEventSink>,
    invocation_id: &TraceId,
    result: PersistEventLogSubprocess,
) {
    let data = Some(Data::PersistEventLogSubprocess(result));
    let event = InstantEvent { data };
    if let Some(sink) = sink {
        let _res = sink
            .send_now(BuckEvent::new(
                SystemTime::now(),
                invocation_id.to_owned(),
                None,
                None,
                event.into(),
            ))
            .await;
    }
}

fn create_scribe_sink(ctx: &ClientCommandContext) -> buck2_error::Result<Option<RemoteEventSink>> {
    new_remote_event_sink_if_enabled(ctx.fbinit(), ScribeConfig::default())
}

#[cfg(test)]
mod tests {
    use buck2_error::buck2_error;

    use super::*;

    #[test]
    fn test_categorize_error() {
        let err = buck2_error!(buck2_error::ErrorTag::Environment, "CertificateRequired");
        assert_eq!(categorize_error(&err), "persist_log_certificate_required");

        let err = buck2_error!(buck2_error::ErrorTag::Tier0, "Some other error");
        assert_eq!(categorize_error(&err), "persist_log_other");
    }
}
