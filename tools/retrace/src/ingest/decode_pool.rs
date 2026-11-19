/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Ordered, bounded-batch parallel protobuf decoding.
//!
//! The existing event-log reader validates the invocation and supplies framed
//! bytes. One framer thread copies those views into independently owned slabs;
//! it assigns slabs round-robin to persistent decoder threads. Rendezvous raw
//! channels retain no queued slabs. Each worker has one queued decoded chunk
//! and one active/building/blocked chunk; the consumer retains one more chunk.
//! Results are consumed in slab order, never worker-completion order. There is
//! no shared post-decode byte semaphore on which an early slab could deadlock
//! behind later completed slabs.
//!
//! Byte settings are flush targets, NOT compatibility limits. A final record
//! can cross a target, and arbitrarily large records are never rejected here.
//! Record ceilings are exact. Decoded charges include vector capacity and
//! uniquely owned allocations (reachable allocation accounting for rare Buck
//! errors). Raw charges include owned slab/entry capacities and error payloads.
//! The modeled bound in test receipts uses observed largest slab/chunk sizes,
//! not wire-size expansion guesses. It excludes the reader/decompressor, one
//! source frame while copied, temporary protobuf allocations, allocator/RSS
//! overhead, thread stacks and anything retained by the reducer. It is not a
//! process-memory limit. Oversized-record allocation happens before accounting.
//!
//! Test-only statistics are aggregated locally and published per chunk/slab, not through
//! shared atomics per record. A live snapshot can therefore lag current work.
//! Consumer record counts describe delivery, not successful reduction. In
//! batch mode an early reducer refusal can leave delivered records unvisited.
//! A panic can discard unpublished partial counters; modeled bounds from a
//! failed run must not be read as a complete allocation receipt.
//! Times are wall durations, not CPU time. Shutdown closes result receivers
//! before joining, waking blocked workers/framer; an input-read cancellation
//! signal also wakes a pending asynchronous read. A currently running decode
//! must finish before its thread can observe cancellation.
//! Unwinding worker panics are join failures. A `panic=abort` binary instead
//! terminates the process; it cannot recover such a panic through `join`.

use std::any::Any;
use std::mem::size_of;
use std::ops::Range;
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::thread::JoinHandle;
#[cfg(test)]
use std::time::Instant;

use buck2_error::BuckErrorContext as _;
use buck2_event_log::stream_value::StreamValue;
use futures::FutureExt as _;
use futures::StreamExt as _;
use futures::channel::oneshot;
use futures::future::Either;
use futures::stream::BoxStream;
use prost::Message as _;
use prost::bytes::Bytes;
use prost::bytes::BytesMut;
#[cfg(test)]
use serde::Serialize;

// Queue accounting itself is production work. Aggregate receipts and clocks
// exist only in tests: they must not add locks or timing calls to ingestion.
macro_rules! record_stats {
    ($($body:tt)*) => {{
        #[cfg(test)]
        { $($body)* }
    }};
}

pub type LogItem<T = StreamValue> = buck2_error::Result<T>;
pub type FrameStream = BoxStream<'static, buck2_error::Result<BytesMut>>;

/// Static dispatch keeps projection free of per-record indirect calls.
/// Production transports normalized records; tests also retain the original
/// raw decoder as an independent reference. Normalization follows validation.
pub(super) trait DecodeRecord: Sized + Send + 'static {
    fn project(value: StreamValue) -> Self;
    fn heap_bytes(item: &LogItem<Self>, mode: super::HeapMode) -> usize;
    #[cfg(test)]
    fn action_kind(&self) -> u8 {
        0
    }
}

#[cfg(test)]
impl DecodeRecord for StreamValue {
    fn project(value: StreamValue) -> Self {
        value
    }

    fn heap_bytes(item: &LogItem<Self>, mode: super::HeapMode) -> usize {
        mode.bytes(item)
    }
}

impl DecodeRecord for super::normalized::Record {
    fn project(value: StreamValue) -> Self {
        super::normalized::project(value)
    }

    fn heap_bytes(item: &LogItem<Self>, mode: super::HeapMode) -> usize {
        match item {
            Ok(value) => value.heap_bytes(mode),
            Err(error) => error_heap_bytes(error),
        }
    }

    #[cfg(test)]
    fn action_kind(&self) -> u8 {
        match self {
            Self::Raw(_) => 0,
            Self::Start(_) => 1,
            Self::End(_) => 2,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub expected_run: Option<[u8; 36]>,
    pub workers: usize,
    /// Wire-byte flush target; slab descriptors/error storage are additional.
    pub frame_bytes: usize,
    pub frame_records: usize,
    /// Retained decoded storage flush target, including reserved vector slots.
    pub decoded_bytes: usize,
    pub decoded_records: usize,
    pub heap_mode: super::HeapMode,
    #[cfg(test)]
    pub compact: bool,
    pub phases: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            expected_run: None,
            workers: 2,
            frame_bytes: 1024 * 1024,
            frame_records: 2048,
            decoded_bytes: 4 * 1024 * 1024,
            decoded_records: 2048,
            heap_mode: super::HeapMode::Fast,
            #[cfg(test)]
            compact: true,
            phases: true,
        }
    }
}

impl Config {
    fn validate(self) -> anyhow::Result<()> {
        anyhow::ensure!(
            (1..=8).contains(&self.workers),
            "decode pool workers must be 1..=8"
        );
        for (name, value) in [
            ("frame_bytes", self.frame_bytes),
            ("decoded_bytes", self.decoded_bytes),
        ] {
            anyhow::ensure!(
                (1..=256 * 1024 * 1024).contains(&value),
                "decode pool {name} must be 1..=256 MiB"
            );
        }
        for (name, value) in [
            ("frame_records", self.frame_records),
            ("decoded_records", self.decoded_records),
        ] {
            anyhow::ensure!(
                (1..=65_536).contains(&value),
                "decode pool {name} must be 1..=65536"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
#[derive(Clone, Debug, Default, Serialize)]
pub struct FramerStats {
    pub records: u64,
    pub error_records: u64,
    pub slabs: u64,
    pub wire_bytes: u64,
    pub largest_frame_wire_bytes: usize,
    pub largest_slab_owned_bytes: usize,
    pub oversized_frames: u64,
    pub over_target_slabs: u64,
    pub fill_ns: u64,
    pub send_ns: u64,
    pub input_completed: bool,
}

#[cfg(test)]
#[derive(Clone, Debug, Default, Serialize)]
pub struct WorkerStats {
    pub slabs: u64,
    pub chunks: u64,
    pub records: u64,
    pub error_records: u64,
    pub normalized_action_starts: u64,
    pub normalized_action_ends: u64,
    pub largest_record_owned_bytes: usize,
    pub largest_chunk_owned_bytes: usize,
    pub largest_chunk_records: usize,
    pub oversized_records: u64,
    pub over_target_chunks: u64,
    /// Decode, unique-allocation accounting and packing; excludes send waits.
    pub decode_and_account_ns: u64,
    pub receive_ns: u64,
    pub send_ns: u64,
}

#[cfg(test)]
#[derive(Clone, Debug, Default, Serialize)]
pub struct ConsumerStats {
    pub chunks: u64,
    /// Decoded/frame records delivered by either API, not records reduced.
    /// Synthetic pool failures are not decoded/frame records.
    pub records: u64,
    pub error_records: u64,
    /// Nonempty iterators returned by `next_batch`, including pool failures.
    pub batches: u64,
    /// Subset of `records` delivered in batches; may exceed the reducer prefix.
    pub batch_records: u64,
    pub batch_error_records: u64,
    pub receive_ns: u64,
    pub thread_failures: u64,
}

#[cfg(test)]
#[derive(Clone, Debug, Serialize)]
pub struct Stats {
    pub framer: FramerStats,
    pub workers: Vec<WorkerStats>,
    pub consumer: ConsumerStats,
    /// Observed largest slab times (workers + producer); not an actual HWM.
    pub modeled_raw_owned_bytes_bound: usize,
    /// Observed largest chunk times (2 * workers + consumer); not an actual HWM.
    pub modeled_decoded_owned_bytes_bound: usize,
}

#[cfg(not(test))]
type FramerStats = ();
#[cfg(not(test))]
type WorkerStats = ();

struct Shared {
    #[cfg(test)]
    framer: Mutex<FramerStats>,
    #[cfg(test)]
    workers: Vec<Mutex<WorkerStats>>,
    #[cfg(test)]
    consumer: Mutex<ConsumerStats>,
    cancelled: AtomicBool,
}

#[derive(Clone)]
pub struct Metrics(Arc<Shared>);

#[cfg(test)]
fn publish<T: Clone>(destination: &Mutex<T>, value: &T) {
    *destination.lock().unwrap_or_else(|e| e.into_inner()) = value.clone();
}

#[cfg(test)]
fn snapshot<T: Clone>(source: &Mutex<T>) -> T {
    source.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(test)]
impl Metrics {
    pub fn snapshot(&self) -> Stats {
        let framer = snapshot(&self.0.framer);
        let workers: Vec<_> = self.0.workers.iter().map(snapshot).collect();
        let largest_chunk = workers
            .iter()
            .map(|s| s.largest_chunk_owned_bytes)
            .max()
            .unwrap_or(0);
        Stats {
            modeled_raw_owned_bytes_bound: framer
                .largest_slab_owned_bytes
                .saturating_mul(workers.len() + 1),
            modeled_decoded_owned_bytes_bound: largest_chunk.saturating_mul(2 * workers.len() + 1),
            framer,
            workers,
            consumer: snapshot(&self.0.consumer),
        }
    }
}

#[cfg(test)]
fn elapsed_ns(start: Instant) -> u64 {
    start.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64
}

fn error_heap_bytes(error: &buck2_error::Error) -> usize {
    allocative::size_of_reachable(error).saturating_sub(size_of::<buck2_error::Error>())
}

enum Frame {
    Data(Range<usize>),
    Error(buck2_error::Error),
}

struct RawSlab {
    ordinal: u64,
    bytes: Bytes,
    frames: Vec<Frame>,
}

struct SlabBuilder {
    bytes: Vec<u8>,
    frames: Vec<Frame>,
    #[cfg(test)]
    error_heap_bytes: usize,
}

impl SlabBuilder {
    fn new(config: Config) -> Self {
        Self {
            bytes: Vec::with_capacity(config.frame_bytes),
            frames: Vec::with_capacity(config.frame_records),
            #[cfg(test)]
            error_heap_bytes: 0,
        }
    }

    #[cfg(test)]
    fn owned_bytes(&self) -> usize {
        self.bytes
            .capacity()
            .saturating_add(self.frames.capacity().saturating_mul(size_of::<Frame>()))
            .saturating_add(self.error_heap_bytes)
    }

    fn push(
        &mut self,
        frame: buck2_error::Result<BytesMut>,
        _config: Config,
        _stats: &mut FramerStats,
    ) {
        record_stats! {
            _stats.records += 1;
        }
        match frame {
            Ok(frame) => {
                record_stats! {
                    _stats.wire_bytes = _stats.wire_bytes.saturating_add(frame.len() as u64);
                }
                record_stats! {
                    _stats.largest_frame_wire_bytes = _stats.largest_frame_wire_bytes.max(frame.len());
                }
                record_stats! {
                    _stats.oversized_frames += u64::from(frame.len() > _config.frame_bytes);
                }
                let start = self.bytes.len();
                // Avoid geometric doubling when the final frame crosses the
                // target. The old shared BytesMut backing is dropped here.
                self.bytes.reserve_exact(frame.len());
                self.bytes.extend_from_slice(&frame);
                self.frames.push(Frame::Data(start..self.bytes.len()));
            }
            Err(error) => {
                record_stats! {
                    _stats.error_records += 1;
                }
                record_stats! {
                    self.error_heap_bytes = self
                        .error_heap_bytes
                        .saturating_add(error_heap_bytes(&error));
                }
                self.frames.push(Frame::Error(error));
            }
        }
        record_stats! {
            _stats.largest_slab_owned_bytes = _stats.largest_slab_owned_bytes.max(self.owned_bytes());
        }
    }

    fn finish(self, ordinal: u64, _config: Config, _stats: &mut FramerStats) -> RawSlab {
        record_stats! {
            _stats.slabs += 1;
        }
        record_stats! {
            _stats.over_target_slabs += u64::from(self.bytes.len() > _config.frame_bytes);
        }
        record_stats! {
            _stats.largest_slab_owned_bytes = _stats.largest_slab_owned_bytes.max(self.owned_bytes());
        }
        RawSlab {
            ordinal,
            bytes: Bytes::from(self.bytes),
            frames: self.frames,
        }
    }
}

async fn frame_input(
    mut input: FrameStream,
    senders: Vec<mpsc::SyncSender<RawSlab>>,
    cancel: oneshot::Receiver<()>,
    config: Config,
    shared: Arc<Shared>,
) -> Result<(), String> {
    let mut cancelled = cancel.fuse();
    let mut stats = FramerStats::default();
    let mut worker = 0;
    let mut ordinal = 0;
    loop {
        #[cfg(test)]
        let started = Instant::now();
        let mut slab = SlabBuilder::new(config);
        record_stats! {
            stats.largest_slab_owned_bytes = stats.largest_slab_owned_bytes.max(slab.owned_bytes());
        }
        let mut eof = false;
        loop {
            let item = match futures::future::select(&mut cancelled, input.next()).await {
                Either::Left(_) => {
                    record_stats! {
                        stats.fill_ns = stats.fill_ns.saturating_add(elapsed_ns(started));
                    }
                    record_stats! {
                        publish(&shared.framer, &stats);
                    }
                    return Ok(());
                }
                Either::Right((item, _)) => item,
            };
            let Some(item) = item else {
                eof = true;
                break;
            };
            let error = item.is_err();
            slab.push(item, config, &mut stats);
            if error
                || slab.frames.len() == config.frame_records
                || slab.bytes.len() >= config.frame_bytes
            {
                break;
            }
        }
        record_stats! {
            stats.fill_ns = stats.fill_ns.saturating_add(elapsed_ns(started));
        }
        if !slab.frames.is_empty() {
            let slab = slab.finish(ordinal, config, &mut stats);
            record_stats! {
                publish(&shared.framer, &stats);
            }
            #[cfg(test)]
            let started = Instant::now();
            let sent = senders[worker].send(slab);
            record_stats! {
                stats.send_ns = stats.send_ns.saturating_add(elapsed_ns(started));
            }
            record_stats! {
                publish(&shared.framer, &stats);
            }
            if sent.is_err() {
                return if shared.cancelled.load(Ordering::Relaxed) {
                    Ok(())
                } else {
                    Err(format!("decoder {worker} stopped accepting slab {ordinal}"))
                };
            }
            worker = (worker + 1) % config.workers;
            ordinal += 1;
        }
        if eof {
            record_stats! {
                stats.input_completed = true;
            }
            record_stats! {
                publish(&shared.framer, &stats);
            }
            return Ok(());
        }
    }
}

fn decode_frame(frame: Bytes) -> LogItem {
    let value = buck2_cli_proto::CommandProgress::decode_length_delimited(frame)
        .buck_error_context("Invalid CommandProgress")?;
    match value.progress {
        Some(buck2_cli_proto::command_progress::Progress::Event(event)) => {
            Ok(StreamValue::Event(event))
        }
        Some(buck2_cli_proto::command_progress::Progress::Result(result)) => {
            Ok(StreamValue::Result(result))
        }
        Some(buck2_cli_proto::command_progress::Progress::PartialResult(result)) => {
            Ok(StreamValue::PartialResult(result))
        }
        None => Err(buck2_error::buck2_error!(
            buck2_error::ErrorTag::InvalidEvent,
            "Event type not recognized"
        )),
    }
}

struct Chunk<T> {
    ordinal: u64,
    final_in_slab: bool,
    items: std::vec::IntoIter<LogItem<T>>,
}

struct ChunkBuilder<T> {
    items: Vec<LogItem<T>>,
    heap_bytes: usize,
}

impl<T: DecodeRecord> ChunkBuilder<T> {
    fn new(config: Config) -> Self {
        // Do not reserve more slots than the byte target by itself permits.
        // Together with post-push flushing this prevents vector growth beyond
        // that reservation; a target smaller than one slot is an explicit
        // one-record oversize case.
        let slots = config
            .decoded_records
            .min((config.decoded_bytes / size_of::<LogItem<T>>()).max(1));
        Self {
            items: Vec::with_capacity(slots),
            heap_bytes: 0,
        }
    }

    fn owned_bytes(&self) -> usize {
        self.items
            .capacity()
            .saturating_mul(size_of::<LogItem<T>>())
            .saturating_add(self.heap_bytes)
    }

    fn push(&mut self, item: LogItem<T>, config: Config, _stats: &mut WorkerStats) {
        let heap = T::heap_bytes(&item, config.heap_mode);
        #[cfg(test)]
        let owned = heap.saturating_add(size_of::<LogItem<T>>());
        record_stats! {
            _stats.records += 1;
        }
        record_stats! {
            _stats.error_records += u64::from(item.is_err());
        }
        record_stats! {
            if let Ok(value) = &item {
                match value.action_kind() {
                    1 => _stats.normalized_action_starts += 1,
                    2 => _stats.normalized_action_ends += 1,
                    _ => {}
                }
            }
        }
        record_stats! {
            _stats.largest_record_owned_bytes = _stats.largest_record_owned_bytes.max(owned);
        }
        record_stats! {
            _stats.oversized_records += u64::from(owned > config.decoded_bytes);
        }
        self.heap_bytes = self.heap_bytes.saturating_add(heap);
        self.items.push(item);
    }

    fn finish(
        self,
        ordinal: u64,
        final_in_slab: bool,
        _config: Config,
        _stats: &mut WorkerStats,
    ) -> Chunk<T> {
        record_stats! {
            _stats.chunks += 1;
        }
        record_stats! {
            _stats.largest_chunk_owned_bytes = _stats.largest_chunk_owned_bytes.max(self.owned_bytes());
        }
        record_stats! {
            _stats.largest_chunk_records = _stats.largest_chunk_records.max(self.items.len());
        }
        record_stats! {
            _stats.over_target_chunks += u64::from(self.owned_bytes() > _config.decoded_bytes);
        }
        Chunk {
            ordinal,
            final_in_slab,
            items: self.items.into_iter(),
        }
    }
}

// A batch-boundary-only test hook can force out-of-order completion or panic;
// there are no hooks or extra per-record branches on the production path.
type BeforeDecode = Arc<dyn Fn(usize, u64) + Send + Sync>;

fn decode_slabs<T: DecodeRecord>(
    worker: usize,
    receiver: mpsc::Receiver<RawSlab>,
    sender: mpsc::SyncSender<Chunk<T>>,
    config: Config,
    shared: Arc<Shared>,
    before_decode: Option<BeforeDecode>,
) -> Result<(), String> {
    let mut stats = WorkerStats::default();
    loop {
        #[cfg(test)]
        let started = Instant::now();
        let slab = receiver.recv();
        record_stats! {
            stats.receive_ns = stats.receive_ns.saturating_add(elapsed_ns(started));
        }
        let Ok(slab) = slab else {
            record_stats! {
                publish(&shared.workers[worker], &stats);
            }
            return Ok(());
        };
        if shared.cancelled.load(Ordering::Relaxed) {
            record_stats! {
                publish(&shared.workers[worker], &stats);
            }
            return Ok(());
        }
        if let Some(before_decode) = &before_decode {
            before_decode(worker, slab.ordinal);
        }
        #[cfg(test)]
        let started = Instant::now();
        #[cfg(test)]
        let send_before = stats.send_ns;
        let count = slab.frames.len();
        let mut chunk = ChunkBuilder::new(config);
        record_stats! {
            stats.largest_chunk_owned_bytes = stats.largest_chunk_owned_bytes.max(chunk.owned_bytes());
        }
        for (index, frame) in slab.frames.into_iter().enumerate() {
            let item = match frame {
                Frame::Data(range) => decode_frame(slab.bytes.slice(range)),
                Frame::Error(error) => Err(error),
            };
            let item = item.map(|value| {
                #[cfg(test)]
                if !config.compact {
                    return value;
                }
                super::compact::clean_for_run(value, config.phases, config.expected_run.as_ref())
            });
            // Raw and normalized payloads may coexist transiently here. The
            // byte target below bounds retained results, not this allocation.
            let item = item.map(T::project);
            let error = item.is_err();
            chunk.push(item, config, &mut stats);
            let final_in_slab = index + 1 == count;
            if error
                || final_in_slab
                || chunk.items.len() == config.decoded_records
                || chunk.owned_bytes() >= config.decoded_bytes
            {
                let finished = chunk.finish(slab.ordinal, final_in_slab, config, &mut stats);
                record_stats! {
                    publish(&shared.workers[worker], &stats);
                }
                #[cfg(test)]
                let sent_started = Instant::now();
                let sent = sender.send(finished);
                record_stats! {
                    stats.send_ns = stats.send_ns.saturating_add(elapsed_ns(sent_started));
                }
                if sent.is_err() || shared.cancelled.load(Ordering::Relaxed) {
                    record_stats! {
                        stats.decode_and_account_ns = stats.decode_and_account_ns.saturating_add(
                            elapsed_ns(started).saturating_sub(stats.send_ns - send_before),
                        );
                    }
                    record_stats! {
                        publish(&shared.workers[worker], &stats);
                    }
                    return Ok(());
                }
                chunk = ChunkBuilder::new(config);
            }
        }
        record_stats! {
            stats.slabs += 1;
        }
        record_stats! {
            stats.decode_and_account_ns = stats
                .decode_and_account_ns
                .saturating_add(elapsed_ns(started).saturating_sub(stats.send_ns - send_before));
        }
        record_stats! {
            publish(&shared.workers[worker], &stats);
        }
    }
}

struct Thread {
    name: String,
    handle: JoinHandle<Result<(), String>>,
}

/// Blocking iterator. Wrap in `futures::stream::iter(pool).boxed()` to use the
/// existing stream-shaped ordered scan loop. Decoder/framer work has its own
/// threads/runtime; the caller's runtime is not required to make progress.
/// `next_batch` avoids the per-record pool adapter on the ordered reducer path.
pub struct Pool<T: DecodeRecord = super::normalized::Record> {
    receivers: Vec<mpsc::Receiver<Chunk<T>>>,
    current: Option<Chunk<T>>,
    expected_slab: u64,
    worker: usize,
    threads: Vec<Thread>,
    cancel: Option<oneshot::Sender<()>>,
    metrics: Metrics,
    #[cfg(test)]
    consumed: ConsumerStats,
    finished: bool,
}

#[cfg(test)]
impl Pool<StreamValue> {
    pub fn new(input: FrameStream, config: Config) -> anyhow::Result<Self> {
        Self::new_inner(input, config, None)
    }

    fn new_inner(
        input: FrameStream,
        config: Config,
        before_decode: Option<BeforeDecode>,
    ) -> anyhow::Result<Self> {
        Self::new_projected_inner(input, config, before_decode)
    }
}

impl Pool<super::normalized::Record> {
    pub(super) fn new_normalized(input: FrameStream, config: Config) -> anyhow::Result<Self> {
        Self::new_projected_inner(input, config, None)
    }
}

impl<T: DecodeRecord> Pool<T> {
    fn new_projected_inner(
        input: FrameStream,
        config: Config,
        before_decode: Option<BeforeDecode>,
    ) -> anyhow::Result<Self> {
        config.validate()?;
        let shared = Arc::new(Shared {
            #[cfg(test)]
            framer: Mutex::new(FramerStats::default()),
            #[cfg(test)]
            workers: (0..config.workers)
                .map(|_| Mutex::new(WorkerStats::default()))
                .collect(),
            #[cfg(test)]
            consumer: Mutex::new(ConsumerStats::default()),
            cancelled: AtomicBool::new(false),
        });
        let (cancel, cancelled) = oneshot::channel();
        let mut pool = Self {
            receivers: Vec::new(),
            current: None,
            expected_slab: 0,
            worker: 0,
            threads: Vec::new(),
            cancel: Some(cancel),
            metrics: Metrics(shared.clone()),
            #[cfg(test)]
            consumed: ConsumerStats::default(),
            finished: false,
        };
        // These senders must drop BEFORE `pool` on a thread-spawn failure so
        // already started workers cannot remain blocked on their input recv.
        let mut inputs = Vec::new();
        for worker in 0..config.workers {
            let (input_sender, input_receiver) = mpsc::sync_channel(0);
            let (output_sender, output_receiver) = mpsc::sync_channel(1);
            inputs.push(input_sender);
            pool.receivers.push(output_receiver);
            let name = format!("retrace-decode-{worker}");
            let state = shared.clone();
            let hook = before_decode.clone();
            let handle = match std::thread::Builder::new()
                .name(name.clone())
                .spawn(move || {
                    decode_slabs::<T>(worker, input_receiver, output_sender, config, state, hook)
                }) {
                Ok(handle) => handle,
                Err(error) => {
                    drop(inputs);
                    return Err(error.into());
                }
            };
            pool.threads.push(Thread { name, handle });
        }
        let name = "retrace-protobuf-framer".to_owned();
        let handle = std::thread::Builder::new()
            .name(name.clone())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| format!("runtime initialization: {e}"))?;
                runtime.block_on(frame_input(input, inputs, cancelled, config, shared))
            })?;
        pool.threads.push(Thread { name, handle });
        Ok(pool)
    }

    #[cfg(test)]
    pub fn stats_handle(&self) -> Metrics {
        self.metrics.clone()
    }

    /// Deliver one ordered chunk, or the remainder of one partially consumed
    /// through `Iterator::next`. Calls to the two APIs may be mixed, but the
    /// caller must consume/drop a returned batch before requesting more input
    /// to preserve the modeled one-consumer-chunk memory bound. Retaining old
    /// batches is caller-owned storage, like retaining individual log records.
    ///
    /// Workers flush on every error, so only the last item can be an error.
    /// Errors are delivered in place, and a caller may continue after them
    /// just as with `next`.
    ///
    /// Metrics count the whole delivered batch, even if the caller visits only
    /// a prefix. Use the scanner's record count for actual reduction progress.
    pub fn next_batch(&mut self) -> Option<std::vec::IntoIter<LogItem<T>>> {
        loop {
            if self.finished {
                return None;
            }
            let chunk = match self.current.take() {
                Some(chunk) => chunk,
                None => match self.receive_chunk() {
                    Ok(Some(chunk)) => chunk,
                    Ok(None) => return None,
                    Err(error) => {
                        record_stats! {
                            self.consumed.batches += 1;
                        }
                        record_stats! {
                            publish(&self.metrics.0.consumer, &self.consumed);
                        }
                        return Some(vec![Err(error)].into_iter());
                    }
                },
            };
            self.retire_chunk(chunk.final_in_slab);
            if chunk.items.len() == 0 {
                // A scalar caller may have exhausted the current chunk.
                continue;
            }
            #[cfg(test)]
            let records = chunk.items.len() as u64;
            #[cfg(test)]
            let errors = u64::from(chunk.items.as_slice().last().is_some_and(Result::is_err));
            record_stats! {
                self.consumed.records += records;
            }
            record_stats! {
                self.consumed.error_records += errors;
            }
            record_stats! {
                self.consumed.batches += 1;
            }
            record_stats! {
                self.consumed.batch_records += records;
            }
            record_stats! {
                self.consumed.batch_error_records += errors;
            }
            record_stats! {
                publish(&self.metrics.0.consumer, &self.consumed);
            }
            return Some(chunk.items);
        }
    }

    fn retire_chunk(&mut self, final_in_slab: bool) {
        if final_in_slab {
            self.expected_slab += 1;
            self.worker = (self.worker + 1) % self.receivers.len();
        }
    }

    fn receive_chunk(&mut self) -> buck2_error::Result<Option<Chunk<T>>> {
        record_stats! {
            publish(&self.metrics.0.consumer, &self.consumed);
        }
        #[cfg(test)]
        let started = Instant::now();
        let received = self.receivers[self.worker].recv();
        record_stats! {
            self.consumed.receive_ns = self.consumed.receive_ns.saturating_add(elapsed_ns(started));
        }
        match received {
            Ok(chunk) => {
                if chunk.ordinal != self.expected_slab {
                    let ordinal = chunk.ordinal;
                    let expected = self.expected_slab;
                    drop(chunk);
                    drop(self.shutdown());
                    return Err(buck2_error::buck2_error!(
                        buck2_error::ErrorTag::Tier0,
                        "decode pool expected slab {expected}, received {ordinal}"
                    ));
                }
                record_stats! {
                    self.consumed.chunks += 1;
                }
                Ok(Some(chunk))
            }
            Err(_) => match self.shutdown() {
                Some(error) => Err(error),
                None => Ok(None),
            },
        }
    }

    /// Drop all output receivers before joining any thread: later workers can
    /// be blocked on output while the framer is blocked handing them input.
    fn shutdown(&mut self) -> Option<buck2_error::Error> {
        self.finished = true;
        self.metrics.0.cancelled.store(true, Ordering::Relaxed);
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        drop(self.current.take());
        self.receivers.clear();
        let mut failures = Vec::new();
        for Thread { name, handle } in self.threads.drain(..) {
            match handle.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => failures.push(format!("{name}: {error}")),
                Err(payload) => {
                    failures.push(format!("{name} panicked: {}", panic_message(&*payload)))
                }
            }
        }
        record_stats! {
            self.consumed.thread_failures += failures.len() as u64;
        }
        record_stats! {
            publish(&self.metrics.0.consumer, &self.consumed);
        }
        // A worker failure may race the framer's next send. The derivative
        // disconnect adds no diagnosis, and its slab ordinal depends on that
        // race; keep the originating worker failure instead.
        if failures
            .iter()
            .any(|failure| failure.starts_with("retrace-decode-"))
        {
            failures.retain(|failure| {
                !(failure.starts_with("retrace-protobuf-framer:")
                    && failure.contains("stopped accepting slab"))
            });
        }
        if failures.is_empty() {
            None
        } else {
            Some(buck2_error::buck2_error!(
                buck2_error::ErrorTag::Tier0,
                "retrace decode pool: {}",
                failures.join("; ")
            ))
        }
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> &str {
    if let Some(message) = payload.downcast_ref::<String>() {
        message
    } else if let Some(message) = payload.downcast_ref::<&'static str>() {
        message
    } else {
        "non-string panic payload"
    }
}

impl<T: DecodeRecord> Iterator for Pool<T> {
    type Item = LogItem<T>;

    fn next(&mut self) -> Option<LogItem<T>> {
        loop {
            if self.finished {
                return None;
            }
            if let Some(chunk) = &mut self.current {
                if let Some(item) = chunk.items.next() {
                    record_stats! {
                        self.consumed.records += 1;
                    }
                    record_stats! {
                        self.consumed.error_records += u64::from(item.is_err());
                    }
                    return Some(item);
                }
            }
            if let Some(chunk) = self.current.take() {
                self.retire_chunk(chunk.final_in_slab);
            }
            match self.receive_chunk() {
                Ok(Some(chunk)) => self.current = Some(chunk),
                Ok(None) => return None,
                Err(error) => return Some(Err(error)),
            }
        }
    }
}

impl<T: DecodeRecord> Drop for Pool<T> {
    fn drop(&mut self) {
        drop(self.shutdown());
    }
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::task::Context;
    use std::task::Poll;
    use std::time::Duration;

    use futures::Stream;

    use super::*;

    fn config() -> Config {
        Config {
            workers: 2,
            frame_bytes: 128,
            frame_records: 3,
            decoded_bytes: 1024,
            decoded_records: 2,
            heap_mode: super::super::HeapMode::Reference,
            compact: false,
            phases: true,
            expected_run: None,
        }
    }

    fn frame(number: usize) -> BytesMut {
        let progress = buck2_cli_proto::CommandProgress {
            progress: Some(buck2_cli_proto::command_progress::Progress::PartialResult(
                Box::new(buck2_cli_proto::PartialResult {
                    partial_result: Some(
                        buck2_cli_proto::partial_result::PartialResult::StdoutBytes(
                            buck2_cli_proto::StdoutBytes {
                                data: number.to_le_bytes().to_vec(),
                            },
                        ),
                    ),
                }),
            )),
        };
        BytesMut::from(progress.encode_length_delimited_to_vec().as_slice())
    }

    fn observation(item: LogItem) -> Result<serde_json::Value, String> {
        item.map(|v| serde_json::to_value(v).unwrap())
            .map_err(|e| format!("{e:#}"))
    }

    // Exercise join-failure precedence even in panic=abort test profiles,
    // where deliberately panicking a worker terminates the test process.
    fn inject_join_failure<T: DecodeRecord>(pool: &mut Pool<T>) {
        pool.threads.push(Thread {
            name: "fixture speculative worker".to_owned(),
            handle: std::thread::spawn(|| Err("fixture join failure".to_owned())),
        });
    }

    #[test]
    fn workers_preserve_order_errors_and_post_error_records() {
        for workers in [1, 2, 4, 8] {
            let make = || {
                (0..103).map(|n| match n {
                    7 => Err(buck2_error::buck2_error!(
                        buck2_error::ErrorTag::Input,
                        "raw fixture error"
                    )),
                    11 => Ok(BytesMut::from(&b"\xff"[..])),
                    19 => Ok(BytesMut::from(
                        buck2_cli_proto::CommandProgress { progress: None }
                            .encode_length_delimited_to_vec()
                            .as_slice(),
                    )),
                    _ => Ok(frame(n)),
                })
            };
            let expected: Vec<_> = make()
                .map(|r| observation(r.and_then(|b| decode_frame(b.freeze()))))
                .collect();
            let mut pool = Pool::new(
                futures::stream::iter(make()).boxed(),
                Config {
                    workers,
                    ..config()
                },
            )
            .unwrap();
            let metrics = pool.stats_handle();
            let actual: Vec<_> = pool.by_ref().map(observation).collect();
            assert_eq!(actual, expected);
            assert!(pool.next().is_none());
            let stats = metrics.snapshot();
            assert_eq!(stats.consumer.records, 103);
            assert_eq!(stats.consumer.error_records, 3);
            assert_eq!(stats.consumer.thread_failures, 0);
            assert!(stats.framer.input_completed);
            assert!(
                stats
                    .workers
                    .iter()
                    .all(|w| w.largest_chunk_records <= config().decoded_records)
            );
        }
    }

    #[test]
    fn batches_match_scalar_order_errors_and_post_error_records() {
        for workers in [1, 2, 4, 8] {
            let make = || {
                (0..103).map(|n| match n {
                    7 => Err(buck2_error::buck2_error!(
                        buck2_error::ErrorTag::Input,
                        "raw fixture error"
                    )),
                    11 => Ok(BytesMut::from(&b"\xff"[..])),
                    19 => Ok(BytesMut::from(
                        buck2_cli_proto::CommandProgress { progress: None }
                            .encode_length_delimited_to_vec()
                            .as_slice(),
                    )),
                    _ => Ok(frame(n)),
                })
            };
            let config = Config {
                workers,
                ..config()
            };
            let expected: Vec<_> = Pool::new(futures::stream::iter(make()).boxed(), config)
                .unwrap()
                .map(observation)
                .collect();
            let mut pool = Pool::new(futures::stream::iter(make()).boxed(), config).unwrap();
            let metrics = pool.stats_handle();
            let mut actual = Vec::new();
            while let Some(batch) = pool.next_batch() {
                assert!(batch.len() > 0);
                // Error flushing is the invariant that permits a constant-
                // time terminal-error check before handing off each batch.
                let items = batch.as_slice();
                assert!(items[..items.len() - 1].iter().all(Result::is_ok));
                actual.extend(batch.map(observation));
            }
            assert_eq!(actual, expected);
            assert!(pool.next().is_none());
            assert!(pool.next_batch().is_none());
            let stats = metrics.snapshot();
            assert_eq!(stats.consumer.records, 103);
            assert_eq!(stats.consumer.batch_records, 103);
            assert_eq!(stats.consumer.error_records, 3);
            assert_eq!(stats.consumer.batch_error_records, 3);
            assert_eq!(stats.consumer.batches, stats.consumer.chunks);
            assert_eq!(stats.consumer.thread_failures, 0);
        }
    }

    #[test]
    fn scalar_and_batch_calls_share_the_same_remaining_prefix() {
        let mut pool = Pool::new(
            futures::stream::iter((0..8).map(|n| Ok(frame(n)))).boxed(),
            Config {
                frame_bytes: 4096,
                frame_records: 4,
                decoded_bytes: 4096,
                decoded_records: 4,
                ..config()
            },
        )
        .unwrap();
        let metrics = pool.stats_handle();
        let mut actual = vec![observation(pool.next().unwrap())];
        let batch = pool.next_batch().unwrap();
        assert_eq!(batch.len(), 3);
        actual.extend(batch.map(observation));
        actual.push(observation(pool.next().unwrap()));
        let batch = pool.next_batch().unwrap();
        assert_eq!(batch.len(), 3);
        actual.extend(batch.map(observation));
        assert!(pool.next().is_none());
        assert!(pool.next_batch().is_none());
        let expected: Vec<_> = (0..8)
            .map(|n| observation(decode_frame(frame(n).freeze())))
            .collect();
        assert_eq!(actual, expected);
        let stats = metrics.snapshot();
        assert_eq!(stats.consumer.records, 8);
        assert_eq!(stats.consumer.batch_records, 6);
        assert_eq!(stats.consumer.batches, 2);

        // A batch call must also retire a chunk exhausted by scalar calls.
        let mut pool = Pool::new(
            futures::stream::iter((0..2).map(|n| Ok(frame(n)))).boxed(),
            Config {
                decoded_records: 1,
                ..config()
            },
        )
        .unwrap();
        assert_eq!(
            observation(pool.next().unwrap()),
            observation(decode_frame(frame(0).freeze()))
        );
        assert_eq!(
            pool.next_batch()
                .unwrap()
                .map(observation)
                .collect::<Vec<_>>(),
            vec![observation(decode_frame(frame(1).freeze()))]
        );
        assert!(pool.next_batch().is_none());
    }

    #[test]
    fn batch_delivery_does_not_claim_reduction_and_drop_cancels_pending_input() {
        let input = futures::stream::iter(vec![Ok(frame(0)), Ok(frame(1))])
            .chain(futures::stream::pending())
            .boxed();
        let mut pool = Pool::new(
            input,
            Config {
                frame_records: 2,
                decoded_records: 8,
                ..config()
            },
        )
        .unwrap();
        let metrics = pool.stats_handle();
        let mut batch = pool.next_batch().unwrap();
        assert_eq!(batch.len(), 2);
        assert!(batch.next().unwrap().is_ok());
        // Model a schema refusal after the first record. The second record
        // was handed off but did not reach the reducer.
        drop(batch);
        drop(pool);
        let stats = metrics.snapshot();
        assert_eq!(stats.consumer.records, 2);
        assert_eq!(stats.consumer.batch_records, 2);
        assert_eq!(stats.consumer.thread_failures, 0);
    }

    #[test]
    fn batch_empty_input_and_join_failures_remain_distinct() {
        let mut empty = Pool::new(futures::stream::empty().boxed(), config()).unwrap();
        assert!(empty.next_batch().is_none());

        let mut failed = Pool::new(futures::stream::empty().boxed(), config()).unwrap();
        inject_join_failure(&mut failed);
        let mut batch = failed.next_batch().expect("failure must not be EOF");
        assert_eq!(batch.len(), 1);
        let error = observation(batch.next().unwrap()).unwrap_err();
        assert!(error.contains("fixture join failure"), "{error}");
        drop(batch);
        assert!(failed.next_batch().is_none());
        assert!(failed.next().is_none());
        let stats = failed.stats_handle().snapshot();
        assert_eq!(stats.consumer.records, 0);
        assert_eq!(stats.consumer.batches, 1);
        assert_eq!(stats.consumer.thread_failures, 1);
    }

    #[test]
    fn normalized_oversized_record_is_delivered() {
        use super::super::normalized;

        let start = buck2_data::ActionExecutionStart {
            name: Some(buck2_data::ActionName {
                category: "large".to_owned(),
                identifier: "x".repeat(32 * 1024),
            }),
            ..Default::default()
        };
        let progress = buck2_cli_proto::CommandProgress {
            progress: Some(buck2_cli_proto::command_progress::Progress::Event(
                Box::new(buck2_data::BuckEvent {
                    timestamp: Some(prost_types::Timestamp {
                        seconds: 1,
                        nanos: 0,
                    }),
                    span_id: 17,
                    parent_id: u64::MAX,
                    data: Some(buck2_data::buck_event::Data::SpanStart(
                        buck2_data::SpanStartEvent {
                            data: Some(buck2_data::span_start_event::Data::ActionExecution(start)),
                        },
                    )),
                    ..Default::default()
                }),
            )),
        };
        let bytes = progress.encode_length_delimited_to_vec();
        let config = Config {
            heap_mode: super::super::HeapMode::Verify,
            ..config()
        };
        let slot = size_of::<LogItem<normalized::Record>>();
        let owned = |record: &normalized::Record| {
            slot.saturating_add(allocative::size_of_unique_allocated_data(record))
        };
        let projected = normalized::project(decode_frame(Bytes::copy_from_slice(&bytes)).unwrap());
        assert!(owned(&projected) > config.decoded_bytes);
        drop(projected);
        let mut pool = Pool::new_normalized(
            futures::stream::iter(vec![Ok(BytesMut::from(bytes.as_slice()))]).boxed(),
            config,
        )
        .unwrap();
        let mut batch = pool.next_batch().unwrap();
        assert_eq!(batch.len(), 1);
        let start = match batch.next().unwrap().unwrap() {
            super::super::normalized::Record::Start(start) => start,
            _ => panic!("expected normalized start"),
        };
        assert_eq!(
            (start.sid, start.parent, start.ts),
            (17, u64::MAX, 1_000_000)
        );
        assert!(start.full_target.len() > 32 * 1024);
        drop(batch);
        assert!(pool.next_batch().is_none());
        assert!(pool.next().is_none());
        let stats = pool.stats_handle().snapshot();
        assert_eq!(stats.consumer.records, 1);
        assert_eq!(stats.consumer.thread_failures, 0);
        assert_eq!(
            stats
                .workers
                .iter()
                .map(|w| w.normalized_action_starts)
                .sum::<u64>(),
            1
        );
        assert_eq!(
            stats
                .workers
                .iter()
                .map(|w| w.oversized_records)
                .sum::<u64>(),
            1
        );
        assert_eq!(
            stats.workers.iter().map(|w| w.error_records).sum::<u64>(),
            0
        );
    }

    #[test]
    fn a_slow_first_slab_does_not_change_first_error_or_prefix() {
        let hook: BeforeDecode = Arc::new(|_, ordinal| {
            if ordinal == 0 {
                std::thread::sleep(Duration::from_millis(30));
            }
        });
        let input = futures::stream::iter((0..6).map(|n| {
            if n == 2 || n == 4 {
                Err(buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Input,
                    "error at {n}"
                ))
            } else {
                Ok(frame(n))
            }
        }))
        .boxed();
        let mut pool = Pool::new_inner(
            input,
            Config {
                frame_records: 1,
                ..config()
            },
            Some(hook),
        )
        .unwrap();
        for n in [0, 1] {
            assert_eq!(
                observation(pool.next().unwrap()),
                observation(decode_frame(frame(n).freeze()))
            );
        }
        assert_eq!(observation(pool.next().unwrap()).unwrap_err(), "error at 2");
        assert_eq!(
            observation(pool.next().unwrap()),
            observation(decode_frame(frame(3).freeze()))
        );
        assert_eq!(observation(pool.next().unwrap()).unwrap_err(), "error at 4");
    }

    #[test]
    fn oversized_records_are_preserved_not_rejected() {
        let progress = buck2_cli_proto::CommandProgress {
            progress: Some(buck2_cli_proto::command_progress::Progress::PartialResult(
                Box::new(buck2_cli_proto::PartialResult {
                    partial_result: Some(
                        buck2_cli_proto::partial_result::PartialResult::StdoutBytes(
                            buck2_cli_proto::StdoutBytes {
                                data: vec![7; 1024 * 1024],
                            },
                        ),
                    ),
                }),
            )),
        };
        let bytes = progress.encode_length_delimited_to_vec();
        let expected = observation(decode_frame(Bytes::copy_from_slice(&bytes)));
        let input = futures::stream::iter(vec![Ok(BytesMut::from(bytes.as_slice()))]).boxed();
        let mut pool = Pool::new(input, config()).unwrap();
        let metrics = pool.stats_handle();
        assert_eq!(observation(pool.next().unwrap()), expected);
        assert!(pool.next().is_none());
        let stats = metrics.snapshot();
        assert_eq!(stats.framer.oversized_frames, 1);
        assert_eq!(
            stats
                .workers
                .iter()
                .map(|w| w.oversized_records)
                .sum::<u64>(),
            1
        );
        assert!(stats.framer.largest_slab_owned_bytes >= bytes.len());
        assert!(stats.modeled_decoded_owned_bytes_bound >= 1024 * 1024);
    }

    #[test]
    fn empty_input_and_worker_failures_do_not_look_the_same() {
        let mut empty = Pool::new(futures::stream::empty().boxed(), config()).unwrap();
        assert!(empty.next().is_none());

        let mut failed = Pool::new(futures::stream::empty().boxed(), config()).unwrap();
        inject_join_failure(&mut failed);
        let error = observation(failed.next().expect("failure must not be EOF")).unwrap_err();
        assert!(error.contains("fixture join failure"), "{error}");
        assert!(failed.next().is_none());
        assert_eq!(failed.stats_handle().snapshot().consumer.thread_failures, 1);

        #[cfg(panic = "unwind")]
        {
            let hook: BeforeDecode =
                Arc::new(|_, ordinal| assert!(ordinal != 1, "decoder fixture panic"));
            let mut pool = Pool::new_inner(
                futures::stream::iter((0..8).map(|n| Ok(frame(n)))).boxed(),
                Config {
                    workers: 1,
                    frame_records: 1,
                    ..config()
                },
                Some(hook),
            )
            .unwrap();
            assert!(pool.next().unwrap().is_ok());
            let error = observation(pool.next().expect("panic must not be EOF")).unwrap_err();
            assert!(error.contains("panicked: decoder fixture panic"), "{error}");
            assert!(pool.next().is_none());
            assert!(pool.stats_handle().snapshot().consumer.thread_failures > 0);
        }
    }

    #[test]
    #[cfg(panic = "unwind")]
    fn several_workers_preserve_the_prefix_before_a_mid_stream_panic() {
        let (done, result) = mpsc::channel();
        std::thread::spawn(move || {
            let hook: BeforeDecode = Arc::new(|_, ordinal| {
                assert_ne!(ordinal, 5, "mid-stream decoder panic");
            });
            let mut pool = Pool::new_inner(
                futures::stream::iter((0..40).map(|n| Ok(frame(n)))).boxed(),
                Config {
                    workers: 4,
                    frame_records: 1,
                    decoded_records: 1,
                    ..config()
                },
                Some(hook),
            )
            .unwrap();
            for n in 0..5 {
                assert_eq!(
                    observation(pool.next().unwrap()),
                    observation(decode_frame(frame(n).freeze()))
                );
            }
            let error = pool
                .next()
                .expect("panic is not EOF")
                .err()
                .expect("worker panic must be an error");
            assert!(error.has_tag(buck2_error::ErrorTag::Tier0));
            assert!(error.to_string().contains("panicked"));
            assert!(pool.next().is_none());
            done.send(()).unwrap();
        });
        result
            .recv_timeout(Duration::from_secs(60))
            .expect("worker shutdown did not finish");
    }

    #[test]
    #[cfg(panic = "unwind")]
    fn framer_panic_is_an_internal_failure_not_clean_eof() {
        let (done, result) = mpsc::channel();
        std::thread::spawn(move || {
            let mut n = 0;
            let input = futures::stream::poll_fn(move |_| {
                n += 1;
                assert!(n == 1, "framer fixture panic");
                Poll::Ready(Some(Ok(frame(0))))
            })
            .boxed();
            let mut pool = Pool::new(
                input,
                Config {
                    frame_records: 1,
                    ..config()
                },
            )
            .unwrap();
            assert!(pool.next().unwrap().is_ok());
            let error = pool
                .next()
                .expect("panic is not EOF")
                .err()
                .expect("framer panic must be an error");
            assert!(error.has_tag(buck2_error::ErrorTag::Tier0));
            assert!(error.to_string().contains("framer fixture panic"));
            assert!(pool.next().is_none());
            done.send(()).unwrap();
        });
        result
            .recv_timeout(Duration::from_secs(60))
            .expect("framer shutdown did not finish");
    }

    // panic=abort cannot recover or test an unwinding worker panic: that
    // profile deliberately loses these two tests, not the nonpanic join-error
    // and full-channel shutdown tests immediately around them.
    #[test]
    fn drop_unblocks_full_worker_channels_and_pending_input() {
        let (filled, signal) = mpsc::channel();
        let input = futures::stream::iter((0..100).map(move |n| {
            if n == 4 {
                filled.send(()).unwrap();
            }
            Ok(frame(n))
        }))
        .boxed();
        let pool = Pool::new(
            input,
            Config {
                frame_records: 1,
                ..config()
            },
        )
        .unwrap();
        signal.recv_timeout(Duration::from_secs(60)).unwrap();
        drop(pool);

        struct Pending {
            polled: Option<mpsc::Sender<()>>,
            dropped: Arc<AtomicBool>,
        }
        impl Stream for Pending {
            type Item = buck2_error::Result<BytesMut>;
            fn poll_next(
                mut self: Pin<&mut Self>,
                _: &mut Context<'_>,
            ) -> Poll<Option<Self::Item>> {
                if let Some(polled) = self.polled.take() {
                    polled.send(()).unwrap();
                }
                Poll::Pending
            }
        }
        impl Drop for Pending {
            fn drop(&mut self) {
                self.dropped.store(true, Ordering::Relaxed);
            }
        }
        let (polled, signal) = mpsc::channel();
        let dropped = Arc::new(AtomicBool::new(false));
        let pool = Pool::new(
            Pending {
                polled: Some(polled),
                dropped: dropped.clone(),
            }
            .boxed(),
            config(),
        )
        .unwrap();
        signal.recv_timeout(Duration::from_secs(60)).unwrap();
        drop(pool);
        assert!(dropped.load(Ordering::Relaxed));
    }

    #[test]
    fn invalid_configuration_is_rejected_before_threads_start() {
        for config in [
            Config {
                workers: 0,
                ..config()
            },
            Config {
                workers: 9,
                ..config()
            },
            Config {
                frame_bytes: 0,
                ..config()
            },
            Config {
                decoded_records: 0,
                ..config()
            },
            Config {
                frame_records: usize::MAX,
                ..config()
            },
        ] {
            assert!(Pool::new(futures::stream::empty().boxed(), config).is_err());
        }
    }
}
