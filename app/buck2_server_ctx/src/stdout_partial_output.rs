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
use std::io::BufWriter;
use std::io::Write;

use dice_futures::cancellation::CancellationPoller;
use dice_futures::cancellation::CancelledError;

use crate::partial_result_dispatcher::PartialResultDispatcher;
use crate::stderr_output_guard::raw_output_chunk_size_override;

// Buffer writes so that many individual small writes can't cause too many gRPC
// messages to be sent.
//
// Keep the buffer size reasonably small so that clients handling these
// messages have a chance to detect client interrupts like Ctrl-C, and
// drop/cancel the command mid-output.
//
// Note that `BufWriter` does not split large writes: anything at least as
// large as the buffer is passed straight through to the inner writer. The
// chunking of large writes into transport-sized messages is therefore done by
// `WriterWrapper`, not by this buffer.
const BUFFER_CAPACITY: usize = 16 * 1024;

// Maximum number of bytes carried by a single `StdoutBytes` message.
//
// Anything far below 2 GiB would do for correctness (see `chunk_size`).
// The default is chosen for the daemon's memory behaviour: with jemalloc
// (the fbcode default allocator), allocations of at least `oversize_threshold`
// (8 MiB by default) are served from a dedicated arena and returned to the OS
// as soon as they are freed, whereas smaller blocks linger in the regular
// arenas until decay-based purging runs. Chunks below that threshold measurably
// raised the daemon's peak RSS when streaming large outputs; see D123515148.
const DEFAULT_CHUNK_SIZE: usize = 16 * 1024 * 1024;

/// A wrapper that implements Write for a PartialResultDispatcher that emits StdoutBytes.
pub struct StdoutPartialOutput<'a> {
    inner: BufWriter<WriterWrapper<'a>>,
}

impl<'a> StdoutPartialOutput<'a> {
    pub fn new(
        dispatcher: &'a mut PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
        cancellation: CancellationPoller,
    ) -> Self {
        Self {
            inner: BufWriter::with_capacity(
                BUFFER_CAPACITY,
                WriterWrapper {
                    inner: dispatcher,
                    cancellation,
                },
            ),
        }
    }
}

impl Write for StdoutPartialOutput<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

struct WriterWrapper<'a> {
    inner: &'a mut PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
    cancellation: CancellationPoller,
}

impl WriterWrapper<'_> {
    fn cancelled() -> io::Error {
        io::Error::other(CancelledError)
    }
}

/// Each `write` call on `WriterWrapper` becomes exactly one `StdoutBytes`
/// partial result, which in turn is sent as a single gRPC message. HTTP/2 (and
/// the gRPC framing on top of it) cannot carry a message of 2 GiB or more, and
/// the daemon does not report such a failure as a command error: it just drops
/// the stream and the client sees a generic disconnect. Capping the size of a
/// single write keeps every message well below that limit regardless of how
/// much data callers hand us at once (e.g. `targets --streaming` writes the
/// output for an entire package in one `write_all`).
///
/// The `BUCK2_DEBUG_RAWOUTPUT_CHUNK_SIZE` override is shared with stderr
/// output and is used by tests to force small chunks.
fn chunk_size() -> io::Result<usize> {
    let chunk_size = raw_output_chunk_size_override()
        .map_err(|e| io::Error::other(format!("{e:#}")))?
        .unwrap_or(DEFAULT_CHUNK_SIZE);
    if chunk_size == 0 {
        return Err(io::Error::other(
            "Configured output chunk size must be greater than zero",
        ));
    }
    Ok(chunk_size)
}

impl Write for WriterWrapper<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.cancellation.is_cancelled() {
            return Err(Self::cancelled());
        }

        // Returning a short write is permitted by the `Write` contract; callers
        // using `write_all` (which includes `BufWriter`) loop until everything
        // has been written.
        let len = buf.len().min(chunk_size()?);
        if len > 0 {
            self.inner.emit(buck2_cli_proto::StdoutBytes {
                data: buf[..len].to_owned(),
            });
        }

        Ok(len)
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(Self::cancelled());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use buck2_cli_proto::partial_result;
    use buck2_events::Event;
    use buck2_events::create_source_sink_pair;
    use buck2_events::daemon_id::DaemonId;
    use buck2_events::dispatch::EventDispatcher;
    use buck2_wrapper_common::invocation_id::TraceId;
    use dice_futures::cancellation::CancellationContext;
    use dice_futures::cancellation::CancellationPoller;
    use dice_futures::cancellation::CancelledError;

    use crate::partial_result_dispatcher::PartialResultDispatcher;
    use crate::stdout_partial_output::StdoutPartialOutput;
    use crate::stdout_partial_output::WriterWrapper;

    #[test]
    fn test_cancelled_error_is_wrapped() {
        let err = WriterWrapper::cancelled();
        assert!(
            err.get_ref()
                .is_some_and(|e| e.downcast_ref::<CancelledError>().is_some())
        );
    }

    fn never_cancelled_poller() -> CancellationPoller {
        futures::executor::block_on(
            CancellationContext::testing()
                .with_structured_cancellation(|observer| async move { observer.into() }),
        )
    }

    /// Drain all emitted `StdoutBytes` messages from the source.
    fn drain_stdout(source: &mut buck2_events::source::ChannelEventSource) -> Vec<Vec<u8>> {
        let mut chunks = Vec::new();
        while let Some(event) = source.try_receive() {
            match event {
                Event::PartialResult(res) => match res.partial_result {
                    Some(partial_result::PartialResult::StdoutBytes(bytes)) => {
                        chunks.push(bytes.data)
                    }
                    other => panic!("unexpected partial result: {other:?}"),
                },
                other => panic!("unexpected event: {other:?}"),
            }
        }
        chunks
    }

    #[test]
    fn test_large_writes_are_chunked() {
        let (mut source, sink) = create_source_sink_pair();
        let dispatcher = EventDispatcher::new(TraceId::new(), DaemonId::new(), sink);
        let mut partial_result_dispatcher =
            PartialResultDispatcher::new(dispatcher, never_cancelled_poller());

        let chunk_size = super::DEFAULT_CHUNK_SIZE;
        // Well beyond both the `BufWriter` capacity and the chunk size, and not a
        // multiple of the chunk size so that the tail chunk is exercised.
        let payload: Vec<u8> = (0..(3 * chunk_size + 12345))
            .map(|i| (i % 251) as u8)
            .collect();

        {
            let mut out =
                StdoutPartialOutput::new(&mut partial_result_dispatcher, never_cancelled_poller());
            out.write_all(&payload).unwrap();
            out.flush().unwrap();
        }

        let chunks = drain_stdout(&mut source);
        assert!(
            chunks
                .iter()
                .all(|c| !c.is_empty() && c.len() <= chunk_size),
            "chunk sizes: {:?}",
            chunks.iter().map(|c| c.len()).collect::<Vec<_>>()
        );
        assert_eq!(chunks.len(), 4);
        assert_eq!(chunks.concat(), payload);
    }

    #[test]
    fn test_small_writes_are_buffered() {
        let (mut source, sink) = create_source_sink_pair();
        let dispatcher = EventDispatcher::new(TraceId::new(), DaemonId::new(), sink);
        let mut partial_result_dispatcher =
            PartialResultDispatcher::new(dispatcher, never_cancelled_poller());

        {
            let mut out =
                StdoutPartialOutput::new(&mut partial_result_dispatcher, never_cancelled_poller());
            for i in 0..100 {
                writeln!(out, "line {i}").unwrap();
            }
            out.flush().unwrap();
        }

        // Many small writes are coalesced into a single message.
        let chunks = drain_stdout(&mut source);
        assert_eq!(chunks.len(), 1);
        let expected: String = (0..100).map(|i| format!("line {i}\n")).collect();
        assert_eq!(chunks[0], expected.into_bytes());
    }
}
