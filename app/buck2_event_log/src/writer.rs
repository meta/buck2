/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::task::Context;
use std::task::Poll;

use async_compression::tokio::write::GzipEncoder;
use async_compression::tokio::write::ZstdEncoder;
use buck2_core::soft_error_or_panic;
use counting_reader::CountingReader;
use pin_project::pin_project;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;

use crate::read::EventLogPathBuf;
use crate::utils::Compression;
use crate::utils::LogMode;

type EventLogWriter = Box<dyn AsyncWrite + Send + Sync + Unpin + 'static>;

mod counting_reader {
    use super::*;

    #[pin_project]
    pub struct CountingReader<T> {
        #[pin]
        pub(super) inner: T,
        pub(super) stats: Option<Arc<AtomicU64>>,
    }
}

impl<T> CountingReader<T> {
    fn new(inner: T, stats: Option<Arc<AtomicU64>>) -> Self {
        Self { inner, stats }
    }
}

impl<T> AsyncWrite for CountingReader<T>
where
    T: AsyncWrite,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.project();
        let bytes = futures::ready!(this.inner.poll_write(cx, buf))?;
        if let Some(stats) = this.stats {
            stats.fetch_add(bytes as u64, Ordering::Relaxed);
        }

        Poll::Ready(Ok(bytes))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::result::Result<(), std::io::Error>> {
        self.project().inner.poll_flush(cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::result::Result<(), std::io::Error>> {
        self.project().inner.poll_shutdown(cx)
    }
}

#[derive(Eq, PartialEq, Copy, Clone)]
pub(crate) enum EventLogType {
    System,
    User,
}
pub(crate) struct NamedEventLogWriter {
    path: EventLogPathBuf,
    file: EventLogWriter,
    event_log_type: EventLogType,
}

impl NamedEventLogWriter {
    pub(crate) fn new(
        path: EventLogPathBuf,
        file: impl AsyncWrite + std::marker::Send + std::marker::Unpin + std::marker::Sync + 'static,
        bytes_written: Option<Arc<AtomicU64>>,
        event_log_type: EventLogType,
    ) -> Self {
        let file = match path.encoding.compression {
            Compression::None => {
                Box::new(CountingReader::new(file, bytes_written)) as EventLogWriter
            }
            Compression::Gzip => Box::new(GzipEncoder::with_quality(
                CountingReader::new(file, bytes_written),
                async_compression::Level::Fastest,
            )) as EventLogWriter,
            Compression::Zstd => Box::new(ZstdEncoder::with_quality(
                CountingReader::new(file, bytes_written),
                async_compression::Level::Precise(9),
            )) as EventLogWriter,
        };
        Self {
            path,
            file,
            event_log_type,
        }
    }

    pub(crate) async fn flush(&mut self) -> buck2_error::Result<()> {
        match self.file.flush().await {
            Ok(_) => Ok(()),
            // What read the pipe that the user had the log copied to is gone.
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
            Err(e) => Err(buck2_error::Error::from(e).context(format!(
                "Error flushing log file at {}",
                self.path.path.display()
            ))),
        }
    }

    pub(crate) async fn shutdown(&mut self) {
        match self.file.shutdown().await {
            Ok(()) => {}
            // What read the pipe that the user had the log copied to is gone.
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
            Err(e) => {
                soft_error_or_panic!(
                    "event_log_flush_failed",
                    buck2_error::Error::from(e)
                        .context(format!("Failed to flush log file at `{}`", self.path.path))
                );
            }
        }
    }

    fn serialize_event<'b, T>(&self, buf: &mut Vec<u8>, event: &T) -> buck2_error::Result<()>
    where
        T: SerializeForLog + 'b,
    {
        match self.event_log_type {
            EventLogType::System => {
                match self.path.encoding.mode {
                    LogMode::Json => {
                        event.serialize_to_json(buf)?;
                        buf.push(b'\n');
                    }
                    LogMode::Protobuf => event.serialize_to_protobuf_length_delimited(buf)?,
                };
            }
            EventLogType::User => {
                if event.maybe_serialize_user_event(buf)? {
                    buf.push(b'\n');
                }
            }
        }
        Ok(())
    }

    async fn write_all(&mut self, buf: &[u8]) -> buck2_error::Result<()> {
        match self.file.write_all(buf).await {
            Ok(_) => Ok(()),
            // What read the pipe that the user had the log copied to is gone. The command
            // goes on without the rest of the copy.
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
            Err(e) => Err(buck2_error::Error::from(e).context("Failed to write event")),
        }
    }

    pub(crate) async fn write_events<'b, T, I>(
        &mut self,
        buf: &mut Vec<u8>,
        events: &I,
    ) -> Result<(), buck2_error::Error>
    where
        T: SerializeForLog + 'b,
        I: IntoIterator<Item = &'b T> + Clone + 'b,
    {
        for event in events.clone() {
            self.serialize_event(buf, event)?;
        }
        self.write_all(buf).await
    }
}

pub(crate) trait SerializeForLog {
    fn serialize_to_json(&self, buf: &mut Vec<u8>) -> buck2_error::Result<()>;
    fn serialize_to_protobuf_length_delimited(&self, buf: &mut Vec<u8>) -> buck2_error::Result<()>;
    fn maybe_serialize_user_event(&self, buf: &mut Vec<u8>) -> buck2_error::Result<bool>;
}
