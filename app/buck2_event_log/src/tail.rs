/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use std::task::ready;
use std::time::Duration;

use tokio::io::AsyncRead;
use tokio::io::ReadBuf;
use tokio::sync::watch;
use tokio::time::Instant;
use tokio::time::Sleep;

/// How long to keep polling for growth after the writer is reported finished, measured
/// from that report or from the last growth, whichever is later: enough for a final
/// flush already in flight to land, without waiting out the full `idle_timeout`.
const WRITER_FINISHED_GRACE: Duration = Duration::from_secs(1);

/// External hint about the state of the process writing the log, e.g. from a daemon's
/// view of its running commands.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WriterState {
    /// Nothing is known about the writer; `idle_timeout` alone decides when to give up.
    Unknown,
    /// The writer is known to still be running: keep polling through quiet stretches
    /// instead of reporting end-of-file at `idle_timeout`.
    Running,
    /// The writer is known to be finished: report end-of-file after a short grace
    /// period instead of waiting out `idle_timeout`.
    Finished,
}

/// Controls how a tailing reader waits for an event log that is still being written.
#[derive(Clone, Debug)]
pub struct TailOptions {
    /// How long to wait before checking for new data again after reaching the end of the file.
    pub poll_interval: Duration,
    /// Report end-of-file if the file has not grown for this long. `None` waits forever.
    pub idle_timeout: Option<Duration>,
    /// External hint about the writer; see [`WriterState`]. The state may change in either
    /// direction while tailing (e.g. a command missing from one daemon snapshot can appear
    /// again in the next).
    pub writer_state: Option<watch::Receiver<WriterState>>,
}

/// A reader that does not treat the end of the file as the end of the data: the file is
/// expected to still be growing, so wait and check for more instead.
///
/// This only ever uses the already-open handle; the file is never re-opened, locked, or
/// otherwise held beyond that handle. The writer owns the file and remains free to delete
/// it (opening with the Rust standard library shares delete access on Windows); a deleted
/// file simply stops producing data, which ends the tail via `idle_timeout` (or via the
/// writer-state hint reporting the writer finished).
pub(crate) struct TailReader<T> {
    inner: T,
    options: TailOptions,
    sleep: Option<Pin<Box<Sleep>>>,
    /// When the file last stopped growing; `None` while data is flowing.
    idle_since: Option<Instant>,
    /// When the writer was first observed to be finished; `None` while it isn't. The
    /// finished grace runs from this point, not from the last growth: the report often
    /// arrives long after a quiet file last grew, and the final flush comes after it.
    finished_since: Option<Instant>,
}

impl<T: AsyncRead + Unpin> TailReader<T> {
    pub(crate) fn new(inner: T, options: TailOptions) -> Self {
        Self {
            inner,
            options,
            sleep: None,
            idle_since: None,
            finished_since: None,
        }
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for TailReader<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            if let Some(sleep) = &mut this.sleep {
                ready!(sleep.as_mut().poll(cx));
                this.sleep = None;
            }

            let filled_before = buf.filled().len();
            ready!(Pin::new(&mut this.inner).poll_read(cx, buf))?;
            if buf.filled().len() > filled_before {
                this.idle_since = None;
                return Poll::Ready(Ok(()));
            }

            let now = Instant::now();
            let idle_since = *this.idle_since.get_or_insert(now);
            let state = this.options.writer_state.as_ref().map(|s| *s.borrow());
            if state != Some(WriterState::Finished) {
                this.finished_since = None;
            }
            let (give_up_after, quiet_since) = match state {
                Some(WriterState::Finished) => {
                    let finished_since = *this.finished_since.get_or_insert(now);
                    let grace = this
                        .options
                        .idle_timeout
                        .map_or(WRITER_FINISHED_GRACE, |t| t.min(WRITER_FINISHED_GRACE));
                    (Some(grace), idle_since.max(finished_since))
                }
                // A known-live writer that is merely quiet is not end-of-file.
                Some(WriterState::Running) => (None, idle_since),
                Some(WriterState::Unknown) | None => (this.options.idle_timeout, idle_since),
            };
            if let Some(give_up_after) = give_up_after
                && now.duration_since(quiet_since) >= give_up_after
            {
                return Poll::Ready(Ok(()));
            }
            this.sleep = Some(Box::pin(tokio::time::sleep(this.options.poll_interval)));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use tokio::io::AsyncReadExt;

    use super::*;

    /// A reader over a shared buffer that can grow while being read; reports (momentary)
    /// end-of-file when all currently available data has been consumed.
    #[derive(Clone, Default)]
    struct SharedBuf {
        state: Arc<Mutex<(Vec<u8>, usize)>>,
    }

    impl SharedBuf {
        fn append(&self, data: &[u8]) {
            self.state.lock().unwrap().0.extend_from_slice(data);
        }
    }

    impl AsyncRead for SharedBuf {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let mut state = self.state.lock().unwrap();
            let (data, pos) = (&state.0, state.1);
            let available = &data[pos..];
            let n = available.len().min(buf.remaining());
            buf.put_slice(&available[..n]);
            state.1 += n;
            Poll::Ready(Ok(()))
        }
    }

    fn tail(
        buf: &SharedBuf,
        idle_timeout: Option<Duration>,
    ) -> (TailReader<SharedBuf>, watch::Sender<WriterState>) {
        let (state_tx, state_rx) = watch::channel(WriterState::Unknown);
        let reader = TailReader::new(
            buf.clone(),
            TailOptions {
                poll_interval: Duration::from_millis(100),
                idle_timeout,
                writer_state: Some(state_rx),
            },
        );
        (reader, state_tx)
    }

    /// The finished grace must run from the finished report, not from the last growth:
    /// data flushed shortly after the report (e.g. a command result) must still be read
    /// even when the file had been quiet for longer than the grace before it.
    #[tokio::test(start_paused = true)]
    async fn test_finished_grace_runs_from_finished_report() {
        let buf = SharedBuf::default();
        buf.append(b"aa");
        let (mut reader, state_tx) = tail(&buf, Some(Duration::from_secs(15)));

        let mut out = [0u8; 16];
        assert_eq!(reader.read(&mut out).await.unwrap(), 2, "initial data");

        // A long quiet stretch, then the writer is reported finished while its final
        // flush is still in flight. The tail keeps polling throughout, so it has been
        // idle far longer than the grace by the time the report arrives.
        let writer = {
            let buf = buf.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(10)).await;
                state_tx.send(WriterState::Finished).unwrap();
                tokio::time::sleep(Duration::from_millis(500)).await;
                buf.append(b"bb");
                state_tx
            })
        };

        assert_eq!(
            reader.read(&mut out).await.unwrap(),
            2,
            "a flush shortly after the finished report should be read, not cut off"
        );
        let _state_tx = writer.await.unwrap();

        let start = Instant::now();
        assert_eq!(reader.read(&mut out).await.unwrap(), 0, "end of file");
        assert!(
            Instant::now() - start < Duration::from_secs(15),
            "the finished grace should cut the idle timeout short"
        );
    }

    /// A known-running writer that is merely quiet must not hit the idle timeout.
    #[tokio::test(start_paused = true)]
    async fn test_running_writer_suppresses_idle_timeout() {
        let buf = SharedBuf::default();
        let (mut reader, state_tx) = tail(&buf, Some(Duration::from_secs(1)));
        state_tx.send(WriterState::Running).unwrap();

        let flip = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            state_tx.send(WriterState::Unknown).unwrap();
            state_tx
        });

        let start = Instant::now();
        let mut out = [0u8; 16];
        assert_eq!(reader.read(&mut out).await.unwrap(), 0, "end of file");
        assert!(
            Instant::now() - start >= Duration::from_secs(30),
            "a quiet running writer should outlive the idle timeout"
        );
        flip.await.unwrap();
    }
}
