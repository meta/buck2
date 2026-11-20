/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! A producer of Scribe messages, which connects to a remote Scribe daemon and sends messages to it.
//!
//! There is expected to be at most one Scribe producer in any given process. The Scribe producer synchronously
//! receives messages from the process, which it places in a queue. As messages queue up, it periodically retires the
//! queue by flushing to the remote Scribe producer service.

use std::convert::TryInto;
use std::net::IpAddr;
use std::net::Ipv6Addr;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::sync::atomic;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use crossbeam::queue::ArrayQueue;
use fbinit::FacebookInit;
use fbthrift::NonthrowingFunctionError;
use scribe_message_metadata::MessageMetadata;
use scribe_producer_service::WriteMessage;
use scribe_producer_service::WriteMessageResultCode;
use scribe_producer_service::WriteMessagesRequest;
use scribe_producer_service::consts::DEFAULT_PRODUCER_SERVICE_PORT;
use scribe_producer_service_clients::ProducerServiceClient;
use scribe_producer_service_thriftclients::build_ProducerService_client;
use scribe_use_cases::UseCase;
use thriftclient::ThriftChannelBuilder;
use thriftclient::TransportType;
use tokio_retry::strategy::FibonacciBackoff;

/// A message, destined to be delivered to Scribe.
#[derive(Clone, Debug, Default)]
pub struct Message {
    /// The category to publish this Scribe message to.
    pub category: String,

    /// The message payload itself.
    pub message: Vec<u8>,

    /// A message key to provide to Scribe. The message key is a 64-bit integer that Scribe will use when sharding a
    /// category. To Scribe, a message key is the atomic unit of partitioning for a stream. All messages with a given
    /// message key are guaranteed by Scribe to be processed by the same consumer shard.
    ///
    /// The number itself is arbitrary; for example, Buck2 hashes the trace ID and uses that as the message key.
    pub message_key: Option<i64>,
}

impl From<Message> for WriteMessage {
    fn from(message: Message) -> WriteMessage {
        let metadata = MessageMetadata {
            messageKey: message.message_key,
            ..Default::default()
        };

        WriteMessage {
            category: message.category,
            message: message.message,
            metadata,
            // TODO(swgillespie) add a buck2 use case?
            useCase: UseCase::DEFAULT_RUST,
            ..Default::default()
        }
    }
}

struct MessageSendState {
    message: Message,
    error: Option<WriteMessageError>,
}

/// These counters are a snapshot of the current state of this Scribe client.
#[derive(Debug, Default, Clone)]
pub struct ProducerCounters {
    // Successful submissions to scribe.
    pub successes: u64,
    // How many messages failed to be submitted to scribe by error type.
    pub failures_invalid_request: u64,
    pub failures_unauthorized: u64,
    pub failures_rate_limited: u64,
    pub failures_pushed_back: u64,
    pub failures_enqueue_failed: u64,
    pub failures_internal_error: u64,
    pub failures_timed_out: u64,
    pub failures_unknown: u64,
    // Depth of the queue, e.g. how many messages need to be processed.
    pub queue_depth: u64,
    // How many messages were dropped before we even enqueued them (e.g. because the internal buffer is full).
    pub dropped: u64,
    /// How many bytes were written into this sink.
    pub bytes_written: u64,
}

impl ProducerCounters {
    pub fn failures(&self) -> u64 {
        let ProducerCounters {
            successes: _,
            failures_invalid_request,
            failures_unauthorized,
            failures_rate_limited,
            failures_pushed_back,
            failures_enqueue_failed,
            failures_internal_error,
            failures_timed_out,
            failures_unknown,
            queue_depth: _,
            dropped: _,
            bytes_written: _,
        } = self;
        *failures_invalid_request
            + *failures_unauthorized
            + *failures_rate_limited
            + *failures_pushed_back
            + *failures_enqueue_failed
            + *failures_internal_error
            + *failures_timed_out
            + *failures_unknown
    }
}

#[derive(Debug, Default)]
struct ProducerCountersData {
    successes: AtomicU64,
    failures_invalid_request: AtomicU64,
    failures_unauthorized: AtomicU64,
    failures_rate_limited: AtomicU64,
    failures_pushed_back: AtomicU64,
    failures_enqueue_failed: AtomicU64,
    failures_internal_error: AtomicU64,
    failures_timed_out: AtomicU64,
    failures_unknown: AtomicU64,
    dropped: AtomicU64,
    bytes_written: AtomicU64,
}

// This congestion control has 2 states (phases) each of which changes its behavior depending on whether
// congestion has occurred. The concept of the algorithm is based on TCP Reno which also consists of the
// two states. Our implementation is a very-simplified version but we try not to diverge from the original
// concept on purpose.
#[derive(Debug)]
enum CongestionControlPhase {
    /// Every message batch begins with EarlyFail phase where, if it fails to queue,
    /// the batch is cut by half until the length that is successfully queued is found.
    EarlyFail,
    /// In this phase, the client tries recovering back the batch size linearly so it can send
    /// as many messages as possible while keeping from causing congestion. If it fails to push
    /// the batch to the queue, it cuts the size down to `(current_cutoff + cliff_bottom) / 2`
    /// where `cliff_bottom` is the successful cutoff we memorize when the last cutoff happened.
    FastRecovery { step: usize },
}

#[derive(Debug)]
struct CongestionControlState {
    initial_cutoff: usize,
    phase: CongestionControlPhase,
    current_cutoff: usize,

    // If we draw a line chart with the send attempt count on x-axis and cutoff size on y-axis, the line will
    // be going upward linearly while the traffic isn't congested, and it looks like a cliff when a congestion
    // happens in this algorithm. We need to store the top and bottom value of the cliff since they'll be used
    // in computing the next cutoff when a congestion happens again.
    // https://fburl.com/px/le1398zy and https://fburl.com/gsheet/de3aa4ux are the illustrations that help
    // visually understand it using the example of the value changes in `normal_cutoff_computations` test.
    cliff_top: usize,
    cliff_bottom: usize,
}

/// The number of steps for a message batch to get recovered to the original length.
/// The smaller it is, the sooner the recovery will be, but the less chance it has to
/// find a proper and stable batch length.
const FAST_RECOVERY_STEPS: usize = 10;

impl CongestionControlState {
    fn new(initial_cutoff: usize) -> Self {
        Self {
            initial_cutoff,
            phase: CongestionControlPhase::EarlyFail,
            current_cutoff: initial_cutoff,
            cliff_top: 0,
            cliff_bottom: 0,
        }
    }

    /// Update cutoff based on the current conditions
    fn update_cutoff(&mut self, congested: bool) {
        match self.phase {
            CongestionControlPhase::EarlyFail => {
                if congested {
                    // The +1 avoids the situations where 1) cutting down to zero and
                    // 2) making 1-leftover for odd numbers that adds one redundant iteration
                    let new_cutoff = self.current_cutoff / 2 + 1;
                    self.cliff_top = self.current_cutoff;
                    self.cliff_bottom = new_cutoff;
                    self.current_cutoff = new_cutoff;
                } else {
                    self.phase = CongestionControlPhase::FastRecovery {
                        step: self.compute_recovery_amount(),
                    };
                }
            }
            CongestionControlPhase::FastRecovery { step } => {
                if congested {
                    let mut new_cutoff = (self.current_cutoff + self.cliff_bottom) / 2 + 1;
                    if new_cutoff == self.current_cutoff || new_cutoff == self.current_cutoff + 1 {
                        // Congested but reached the bottom of cliff. Exploring down deeper.
                        self.cliff_bottom /= 2;
                        new_cutoff = (self.current_cutoff + self.cliff_bottom) / 2 + 1;
                    }
                    self.cliff_top = self.current_cutoff;
                    self.cliff_bottom = new_cutoff;
                    self.current_cutoff = new_cutoff;
                    self.phase = CongestionControlPhase::FastRecovery {
                        step: self.compute_recovery_amount(),
                    };
                } else {
                    self.current_cutoff += step;
                }
            }
        }
    }

    /// A request of `sent` messages was pushed back as a whole: cut the next request below it.
    /// The state's cutoff may be above `sent` (a stale `last_cutoff`, or a batch shorter than
    /// the cutoff), and halving with `+ 1` does not move below 2 by itself.
    fn congested(&mut self, sent: usize) {
        self.current_cutoff = sent;
        self.update_cutoff(true);
        if self.current_cutoff >= sent {
            self.current_cutoff = sent.saturating_sub(1).max(1);
            self.cliff_bottom = self.current_cutoff;
        }
    }

    /// Compute and return the recovery amount of each step
    fn compute_recovery_amount(&self) -> usize {
        self.cliff_top.saturating_sub(self.current_cutoff) / FAST_RECOVERY_STEPS + 1
    }

    fn load_last_cutoff(&mut self, last_cutoff: usize, init_cliff_top: usize) {
        self.current_cutoff = last_cutoff;
        self.cliff_top = init_cliff_top;
        self.phase = CongestionControlPhase::FastRecovery {
            step: self.compute_recovery_amount(),
        };
    }

    fn compute_last_cutoff(&self) -> Option<usize> {
        if self.current_cutoff < self.initial_cutoff {
            Some(self.current_cutoff)
        } else {
            None
        }
    }
}

/// Upper bound on the payload bytes of one `WriteMessages` request.
///
/// A request that times out at the transport is resent whole, but Scribe may already have
/// accepted it, so every message in it can land twice. Bounding the request bounds both how
/// long Scribe takes to answer, keeping it well under `thrift_timeout`, and how many messages
/// one timeout can duplicate. A local Scribe daemon has been measured at 12-40 ms per MiB.
const MAX_REQUEST_BYTES: usize = 1024 * 1024;

/// The number of leading `messages` to send in the next request: at most `max_messages`, and
/// at most `MAX_REQUEST_BYTES` of payload unless a single message is larger.
fn request_len(messages: &[MessageSendState], max_messages: usize) -> usize {
    let mut bytes = 0;
    let fitting = messages
        .iter()
        .take(max_messages)
        .take_while(|m| {
            bytes += m.message.message.len();
            bytes <= MAX_REQUEST_BYTES
        })
        .count();
    fitting.max(1).min(messages.len())
}

/// A client of the Scribe ProducerService that buffers, retries, and sends messages to a remote Scribe daemon.
pub(crate) struct ScribeProducer {
    fb: FacebookInit,
    client: tokio::sync::Mutex<ProducerServiceClient>,
    queue: ArrayQueue<Message>,
    counters: ProducerCountersData,
    retry_backoff: Duration,
    retry_attempts: usize,
    message_batch_size: Option<usize>,
    last_cutoff: Mutex<Option<usize>>,
    thrift_timeout: Duration,
}

pub struct ScribeConfig {
    pub buffer_size: usize,
    pub retry_backoff: Duration,
    pub retry_attempts: usize,
    pub message_batch_size: Option<usize>,
    pub thrift_timeout: Duration,
}

impl Default for ScribeConfig {
    fn default() -> Self {
        Self {
            buffer_size: 100,
            retry_backoff: Duration::from_millis(500),
            retry_attempts: 5,
            message_batch_size: None,
            thrift_timeout: Duration::from_secs(1),
        }
    }
}

impl ScribeProducer {
    pub(crate) fn new(fb: FacebookInit, config: ScribeConfig) -> anyhow::Result<ScribeProducer> {
        let client = connect(fb, config.thrift_timeout)?;
        let queue = ArrayQueue::new(config.buffer_size);
        Ok(ScribeProducer {
            fb,
            client: tokio::sync::Mutex::new(client),
            queue,
            counters: ProducerCountersData::default(),
            retry_backoff: config.retry_backoff,
            retry_attempts: config.retry_attempts,
            message_batch_size: config.message_batch_size,
            last_cutoff: Mutex::new(None),
            thrift_timeout: config.thrift_timeout,
        })
    }

    /// Offers a message to this Scribe producer. Does not block.
    pub(crate) fn offer(&self, message: Message) {
        if self.queue.push(message).is_err() {
            tracing::debug!("Scribe producer dropping message due to full buffer");
            self.counters
                .dropped
                .fetch_add(1, atomic::Ordering::Relaxed);
        }
    }

    /// Scrape counters for reporting to upstream event logging.
    pub(crate) fn export_counters(&self) -> ProducerCounters {
        ProducerCounters {
            successes: self.counters.successes.load(atomic::Ordering::Relaxed),
            failures_invalid_request: self
                .counters
                .failures_invalid_request
                .load(atomic::Ordering::Relaxed),
            failures_unauthorized: self
                .counters
                .failures_unauthorized
                .load(atomic::Ordering::Relaxed),
            failures_rate_limited: self
                .counters
                .failures_rate_limited
                .load(atomic::Ordering::Relaxed),
            failures_pushed_back: self
                .counters
                .failures_pushed_back
                .load(atomic::Ordering::Relaxed),
            failures_enqueue_failed: self
                .counters
                .failures_enqueue_failed
                .load(atomic::Ordering::Relaxed),
            failures_internal_error: self
                .counters
                .failures_internal_error
                .load(atomic::Ordering::Relaxed),
            failures_timed_out: self
                .counters
                .failures_timed_out
                .load(atomic::Ordering::Relaxed),
            failures_unknown: self
                .counters
                .failures_unknown
                .load(atomic::Ordering::Relaxed),
            dropped: self.counters.dropped.load(atomic::Ordering::Relaxed),
            bytes_written: self.counters.bytes_written.load(atomic::Ordering::Relaxed),
            // So we get an accurate snapshot of the queue depth when scraping
            // metrics, do this here and now rather than in the background.
            queue_depth: self.queue.len() as u64,
        }
    }

    /// Sends all messages in `messages` now (bypass internal message queue.)
    pub(crate) async fn send_messages_now(&self, messages: Vec<Message>) -> Result<(), WriteError> {
        self.send_impl(messages).await
    }

    async fn refresh_connection(
        &self,
        client_: &mut tokio::sync::MutexGuard<'_, ProducerServiceClient>,
    ) -> anyhow::Result<()> {
        let new_client = connect(self.fb, self.thrift_timeout)?;
        **client_ = new_client;
        Ok(())
    }

    fn make_retry_intervals(&self) -> Vec<Duration> {
        // Why not use tokio_retry::Retry? We don't want to wholesale re-run the
        // entire request because we need to mutate the request to filter out
        // successful messages.
        let retry_backoff_ms: u64 = self.retry_backoff.as_millis().try_into().unwrap_or(0);
        std::iter::once(Duration::from_millis(0))
            .chain(FibonacciBackoff::from_millis(retry_backoff_ms))
            .take(self.retry_attempts)
            .collect()
    }

    async fn send_impl(&self, messages: Vec<Message>) -> Result<(), WriteError> {
        if messages.is_empty() {
            return Ok(());
        }

        let mut messages: Vec<MessageSendState> = messages
            .into_iter()
            .map(|message| MessageSendState {
                message,
                error: None,
            })
            .collect();

        let mut cc_state = CongestionControlState::new(messages.len());

        if let Some(last_cutoff) = *self.last_cutoff.lock().unwrap() {
            cc_state.load_last_cutoff(last_cutoff, messages.len());
        }

        let mut first_request_error: Option<RequestError> = None;

        let retry_intervals: Vec<Duration> = self.make_retry_intervals();
        // Failed requests in a row. A request that fully succeeds resets it, so a send gives up only
        // after `retry_attempts` consecutive failures. Every reset delivers at least one message, so
        // the loop ends.
        let mut retry_count = 0;
        while retry_count < retry_intervals.len() {
            let cutoff_len = request_len(&messages, cc_state.current_cutoff);

            tracing::debug!(
                "retry_count={}, interval={:?}, current_cutoff={}, {} remained in batch",
                retry_count,
                retry_intervals[retry_count],
                cc_state.current_cutoff,
                messages.len()
            );

            tokio::time::sleep(retry_intervals[retry_count]).await;

            let req = WriteMessagesRequest {
                messages: messages[..cutoff_len]
                    .iter()
                    .map(|message| message.message.clone().into())
                    .collect(),
                ..Default::default()
            };

            // Mutex block for `self.client`
            let results = {
                let mut client_ = self.client.lock().await;
                match client_.WriteMessages(&req).await {
                    Ok(result) => result.results,
                    Err(e) => {
                        tracing::debug!(
                            "scribe_producer: received fatal error from scribe, will retry: {:#}",
                            e,
                        );
                        let refresh_error = match e {
                            NonthrowingFunctionError::ThriftError(ref te) => {
                                // Error on Thrift transport layer. The channel likely got EOF due to any of
                                // server hitting connection limit, connection age timeout, server connection
                                // idle timeout, or server crashes and the endpoint is voided.
                                if te
                                    .to_string()
                                    .contains("apache::thrift::transport::TTransportException")
                                {
                                    tracing::debug!(
                                        "The existing connection reached EOF. Reconnecting."
                                    );
                                    let res = self.refresh_connection(&mut client_).await;
                                    res.err()
                                } else {
                                    None
                                }
                            }
                            NonthrowingFunctionError::ApplicationException(_) => {
                                // Error on Thrift application layer. Rarely happens as long as we use an
                                // official thrift library. No special action is taken but just retry it.
                                None
                            }
                        };
                        if first_request_error.is_none() {
                            first_request_error = Some(RequestError {
                                error: e,
                                refresh_error,
                            });
                        }
                        retry_count += 1;
                        continue;
                    }
                }
            };

            let mut success_count = 0;
            let mut retryable_error_count = 0;
            let mut write_result_iter = results.iter();
            // Filter successful messages.
            messages.retain_mut(|message| {
                write_result_iter.next().is_none_or(|result| {
                    match result_code_into_result(result.code) {
                        Ok(_) => {
                            self.counters
                                .successes
                                .fetch_add(1, atomic::Ordering::Relaxed);

                            // Unwrap safety: an individual message cant't be so large its length
                            // can't be representable as 64 bits.
                            self.counters.bytes_written.fetch_add(
                                message.message.message.len().try_into().unwrap(),
                                atomic::Ordering::Relaxed,
                            );
                            success_count += 1;
                            false
                        }
                        Err(e) => {
                            if e.is_retryable() {
                                if message.error.is_none() {
                                    message.error = Some(e);
                                }
                                retryable_error_count += 1;
                                true
                            } else {
                                e.inc_counter(&self.counters);
                                false
                            }
                        }
                    }
                })
            });
            if messages.is_empty() {
                break;
            }

            if cutoff_len == retryable_error_count {
                // Congested; all messages were pushed back with retryable errors, which means
                // Scribed/Scribble couldn't prepare enough buffer to hold the message vector.
                cc_state.congested(cutoff_len);
                retry_count += 1;
            } else if cutoff_len == success_count {
                // Success; all the messages up to the cutoff were successfully processed.
                cc_state.update_cutoff(false);
                retry_count = 0;
            } else {
                // Partial Success; there were some unretryable errors, or partial successes, or both
                // not because of congestion. Try again with a longer interval.
                retry_count += 1;
            }
            tracing::debug!(
                "Updated cc_state: {:?}; new cutoff: {}",
                cc_state,
                cc_state.current_cutoff
            );
        }
        *self.last_cutoff.lock().unwrap() = cc_state.compute_last_cutoff();

        for message in &messages {
            match &message.error {
                None => {
                    self.counters
                        .failures_unknown
                        .fetch_add(1, atomic::Ordering::Relaxed);
                }
                Some(e) => e.inc_counter(&self.counters),
            }
        }
        // Any messages leftover after exiting the loop are ones we failed to send after exhausting retries and
        // should be counted as errors.
        if !messages.is_empty() {
            tracing::debug!("scribe_producer: failed to send all messages");
            Err(WriteError::new(first_request_error, messages, retry_count))
        } else {
            Ok(())
        }
    }

    pub(crate) async fn run_once(&self) -> Result<(), WriteError> {
        if self.queue.is_empty() {
            return Ok(());
        }

        let mut messages: Vec<Message> = vec![];
        let count = self.message_batch_size.unwrap_or_else(|| self.queue.len());
        for _ in 0..count {
            match self.queue.pop() {
                Some(msg) => messages.push(msg),
                None => break,
            }
        }

        self.send_impl(messages).await
    }
}

/// Connect to Scribe producer service (local Scribe daemon) via Thrift interface.
fn connect(fb: FacebookInit, timeout: Duration) -> anyhow::Result<ProducerServiceClient> {
    let addr = SocketAddr::new(
        IpAddr::V6(Ipv6Addr::LOCALHOST),
        DEFAULT_PRODUCER_SERVICE_PORT as u16,
    );
    build_ProducerService_client(
        ThriftChannelBuilder::from_sock_addr(fb, addr)?
            .with_conn_timeout(timeout.as_millis().try_into()?)
            .with_recv_timeout(timeout.as_millis().try_into()?)
            .with_channel_pool(false)
            .with_transport_type(TransportType::Header)
            // By default, ThriftChannelBuilder will initiate a TLS handshake with the target server. This works fine
            // on production machines, where Chef has set up certificates that make this successful; on corp machines,
            // this is not the case, and we must turn it off otherwise we will fail to connect to the local Scribe
            // daemon.
            //
            // Disabling TLS on a localhost connection is fine anyway since there's no way this traffic ever leaves the
            // machine.
            .with_secure(false),
    )
}

#[derive(Debug)]
pub struct RequestError {
    error: NonthrowingFunctionError,
    refresh_error: Option<anyhow::Error>,
}

// At least one of each possible error type if any message in a request failed to send.
// Only makes sense to track a single request error since the rest will be from retries.
// May be useful to track one error per message but most likely they will be duplicates and
// right now this is ignored in any cases we send more than a single message (only used for invocation record).
#[derive(Debug)]
pub struct WriteError {
    first_request_error: Option<RequestError>,
    first_message_error: Option<WriteMessageError>,
    failed_message_count: usize,
    failed_message_bytes: usize,
    retry_count: usize,
}

impl WriteError {
    fn new(
        first_request_error: Option<RequestError>,
        failed_messages: Vec<MessageSendState>,
        retry_count: usize,
    ) -> Self {
        let failed_message_count = failed_messages.len();
        let failed_message_bytes = failed_messages
            .iter()
            .fold(0, |acc, m| acc + m.message.message.len());
        Self {
            first_message_error: failed_messages.into_iter().next().and_then(|m| m.error),
            first_request_error,
            failed_message_count,
            failed_message_bytes,
            retry_count,
        }
    }
}

impl From<WriteError> for anyhow::Error {
    fn from(val: WriteError) -> Self {
        let error = anyhow::anyhow!(
            "scribe_producer: failed to send {} message(s) with {} bytes after {} retries",
            val.failed_message_count,
            val.failed_message_bytes,
            val.retry_count
        );
        let error = match val.first_message_error {
            Some(e) => error.context(format!("message error: {e:?}")),
            None => error,
        };
        match val.first_request_error {
            Some(e) => {
                let error = error.context(format!("request error: {:?}", e.error));
                match e.refresh_error {
                    Some(re) => error.context(format!("connection refresh error: {re:?}")),
                    None => error,
                }
            }
            None => error,
        }
    }
}

// Errors returned from Scribe's Producer API. Each corresponds to a return code
// from WriteMessages.
// From: https://fburl.com/code/2jrnviz0
#[derive(Debug, thiserror::Error)]
pub enum WriteMessageError {
    // Returned if the message is evaluated to be invalid (e.g., invalid message
    // size, unregistered category, invalid application bucket).
    #[error("invalid request")]
    InvalidRequest,
    // Returned if the Thrift client has attempted to write to a category
    // which it lacks permissions for.
    #[error("unauthorized")]
    Unauthorized,
    // Returned if the message is dropped for rate limiting reasons
    // (e.g., the category is blocklisted or sampled).
    //
    // Retrying upon this result code may ingest a message successfully due to
    // sampling or if the category is removed from the blocklist in the meantime.
    // However, if retry is needed all the time, the pushback write enforcement
    // is a better choice.
    //
    // Please see the following links for more information about the blocking,
    // sampling and pushback write enforcement types.
    // - https://fburl.com/wiki/6hvwsuir
    // - https://fburl.com/wiki/4clxpwm6
    // - https://fburl.com/wiki/7b9vr52e
    #[error("ratelimited")]
    RateLimited,
    // Returned if the request is throttled because the category of the message
    // has reached its write rate limit.
    //
    // It is up to the client to retry its request upon this result code after
    // some time.
    //
    // Please see the following links for more information about the pushback
    // write enforcement type:
    // - https://fburl.com/wiki/6hvwsuir
    // - https://fburl.com/wiki/7b9vr52e
    #[error("pushed back")]
    PushedBack,
    // Returned if the message could not be handled by the Scribe service,
    // possibly because clients have been sending too many requests in a short
    // period of time and the Scribe service is overloaded.
    #[error("enqueue failed")]
    EnqueueFailed,
    // Returned if an error occurred inside the Scribe service.
    #[error("internal error")]
    InternalError,
    // Returned in case of the request timeout.
    #[error("timed out")]
    TimedOut,
    // Any other errors that arise.
    #[error("unknown")]
    Unknown,
}

impl WriteMessageError {
    pub fn is_retryable(&self) -> bool {
        std::matches!(
            self,
            Self::RateLimited
                | Self::PushedBack
                | Self::EnqueueFailed
                | Self::InternalError
                | Self::TimedOut
                | Self::Unknown
        )
    }

    fn inc_counter(&self, counter: &ProducerCountersData) {
        let counter = match self {
            WriteMessageError::InvalidRequest => &counter.failures_invalid_request,
            WriteMessageError::Unauthorized => &counter.failures_unauthorized,
            WriteMessageError::RateLimited => &counter.failures_rate_limited,
            WriteMessageError::PushedBack => &counter.failures_pushed_back,
            WriteMessageError::EnqueueFailed => &counter.failures_enqueue_failed,
            WriteMessageError::InternalError => &counter.failures_internal_error,
            WriteMessageError::TimedOut => &counter.failures_timed_out,
            WriteMessageError::Unknown => &counter.failures_unknown,
        };
        counter.fetch_add(1, atomic::Ordering::Relaxed);
    }
}

fn result_code_into_result(code: WriteMessageResultCode) -> Result<(), WriteMessageError> {
    match code {
        WriteMessageResultCode::OK => Ok(()),
        WriteMessageResultCode::INVALID_REQUEST => Err(WriteMessageError::InvalidRequest),
        WriteMessageResultCode::UNAUTHORIZED => Err(WriteMessageError::Unauthorized),
        WriteMessageResultCode::RATE_LIMITED => Err(WriteMessageError::RateLimited),
        WriteMessageResultCode::PUSHED_BACK => Err(WriteMessageError::PushedBack),
        WriteMessageResultCode::ENQUEUE_FAILED => Err(WriteMessageError::EnqueueFailed),
        WriteMessageResultCode::INTERNAL_ERROR => Err(WriteMessageError::InternalError),
        WriteMessageResultCode::TIMED_OUT => Err(WriteMessageError::TimedOut),
        _ => Err(WriteMessageError::Unknown),
    }
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use std::sync::Arc;

    use fbthrift::ApplicationExceptionErrorCode;
    use fbthrift::application_exception::ApplicationException;
    use scribe_producer_service::WriteMessageResult as ThriftWriteMessageResult;
    use scribe_producer_service::WriteMessagesResponse;
    use scribe_producer_service_clients::ProducerService;
    use scribe_producer_service_clients::errors::WriteMessagesError;

    use super::*;

    #[allow(non_snake_case)]
    fn make_ScribeProducer(
        fb: FacebookInit,
        client: ProducerServiceClient,
        queue_size: usize,
    ) -> ScribeProducer {
        ScribeProducer {
            fb,
            client: tokio::sync::Mutex::new(client),
            queue: ArrayQueue::new(queue_size),
            counters: ProducerCountersData::default(),
            retry_backoff: Duration::from_millis(0),
            retry_attempts: 5,
            message_batch_size: None,
            last_cutoff: Mutex::new(None),
            thrift_timeout: Duration::from_millis(0),
        }
    }

    #[fbinit::test]
    async fn success_smoke_test(fb: FacebookInit) {
        let client = Arc::new(scribe_producer_service_mocks::new::<dyn ProducerService>());
        client.WriteMessages.mock(|req| {
            assert_eq!(req.messages.len(), 1);
            let msg = &req.messages[0];
            assert_eq!(msg.category, "buck2_events");
            assert_eq!(msg.message, b"hello, world!".to_vec());
            assert_eq!(msg.metadata.messageKey, Some(42));
            WriteMessagesResponse {
                results: vec![ThriftWriteMessageResult {
                    code: WriteMessageResultCode::OK,
                    ..Default::default()
                }],
                ..Default::default()
            }
        });

        let producer = make_ScribeProducer(fb, client, 5);

        let message = Message {
            category: "buck2_events".to_owned(),
            message: b"hello, world!".to_vec(),
            message_key: Some(42),
        };

        producer.offer(message);
        producer.run_once().await.expect("write failure");
        let counters = producer.export_counters();
        println!("counters: {counters:?}");
        assert_eq!(counters.successes, 1);
        assert_eq!(counters.failures(), 0);
    }

    // Make a mock ProducerService that returns the provided codes in the same order
    fn mock_client(
        mut mock_return_code_groups: Vec<Vec<WriteMessageResultCode>>,
    ) -> ProducerServiceClient {
        let client = Arc::new(scribe_producer_service_mocks::new::<dyn ProducerService>());
        mock_return_code_groups.reverse();
        client.WriteMessages.mock(move |req| {
            let mock_return_code_group = mock_return_code_groups.pop().unwrap_or_else(|| {
                panic!("WriteMessages() was called more than the mock ProducerService expected.");
            });
            assert!(
                mock_return_code_group.len() == req.messages.len(),
                "Mock ProducerService received the different number of messages than it expected.",
            );

            WriteMessagesResponse {
                results: mock_return_code_group
                    .iter()
                    .map(|code| ThriftWriteMessageResult {
                        code: *code,
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }
        });

        client
    }

    fn message(contents: &'static str) -> Message {
        Message {
            category: "buck2_events".to_owned(),
            message: contents.as_bytes().to_vec(),
            ..Default::default()
        }
    }

    #[fbinit::test]
    async fn run_once_retries_and_all_succeed(fb: FacebookInit) {
        let codes = vec![
            vec![
                WriteMessageResultCode::OK,
                WriteMessageResultCode::OK,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
            ],
            vec![
                WriteMessageResultCode::OK,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
            ],
            vec![WriteMessageResultCode::OK, WriteMessageResultCode::OK],
        ];
        let client = mock_client(codes.clone());
        let producer = make_ScribeProducer(fb, client, codes[0].len());

        for _ in codes[0].iter() {
            producer.offer(message("hello, world!"));
        }
        producer.run_once().await.expect("write failure");
        let counters = producer.export_counters();
        assert_eq!(counters.successes, 5);
        assert_eq!(counters.failures(), 0);
    }

    #[fbinit::test]
    async fn run_once_does_not_retry_terminal_failures(fb: FacebookInit) {
        let codes = vec![
            vec![
                WriteMessageResultCode::OK,
                WriteMessageResultCode::OK,
                WriteMessageResultCode::UNAUTHORIZED,
                WriteMessageResultCode::INVALID_REQUEST,
                WriteMessageResultCode::ENQUEUE_FAILED,
            ],
            vec![WriteMessageResultCode::OK],
        ];
        let client = mock_client(codes.clone());
        let producer = make_ScribeProducer(fb, client, codes[0].len());

        for _ in codes[0].iter() {
            producer.offer(message("hello, world!"));
        }
        producer.run_once().await.expect("write failure");
        let counters = producer.export_counters();
        println!("counters: {counters:?}");
        assert_eq!(counters.successes, 3);
        assert_eq!(counters.failures(), 2);
    }

    #[fbinit::test]
    async fn run_once_does_not_retry_after_max_retries(fb: FacebookInit) {
        let codes = vec![
            vec![
                WriteMessageResultCode::OK,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
            ],
            vec![
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::OK,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
            ],
            vec![
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::OK,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
            ],
            vec![
                WriteMessageResultCode::OK,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
            ],
            vec![
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::OK,
                WriteMessageResultCode::ENQUEUE_FAILED,
            ],
        ];
        let client = mock_client(codes.clone());
        let producer = make_ScribeProducer(fb, client, codes[0].len());
        for _ in codes[0].iter() {
            producer.offer(message("hello, world!"));
        }

        let res = producer.run_once().await;
        let counters = producer.export_counters();
        assert_eq!(counters.successes, 5);
        assert_eq!(counters.failures(), 2);
        assert_matches!(
            res,
            Err(WriteError {
                first_message_error: Some(WriteMessageError::EnqueueFailed),
                ..
            })
        );
    }

    #[fbinit::test]
    async fn all_messages_are_retried_if_write_messages_fails(fb: FacebookInit) {
        let client = Arc::new(scribe_producer_service_mocks::new::<dyn ProducerService>());
        client.WriteMessages.mock_result(|_| {
            Err(WriteMessagesError::ApplicationException(
                ApplicationException::unknown_method(),
            ))
        });

        let producer = make_ScribeProducer(fb, client, 5);
        for _ in 0..5 {
            producer.offer(message("hello, world!"));
        }
        let res = producer.run_once().await;
        let counters = producer.export_counters();
        assert_eq!(counters.successes, 0);
        assert_eq!(counters.failures(), 5);
        assert_matches!(
            res,
            Err(WriteError {
                first_request_error: Some(RequestError {
                    error: NonthrowingFunctionError::ApplicationException(ApplicationException {
                        message: _,
                        type_: ApplicationExceptionErrorCode::UnknownMethod,
                    },),
                    ..
                }),
                ..
            })
        );
    }

    #[fbinit::test]
    async fn send_one_message_with_cutoff_retries_and_succeeds(fb: FacebookInit) {
        let codes = vec![
            vec![WriteMessageResultCode::ENQUEUE_FAILED],
            // Last one shouldn't be cut off
            vec![WriteMessageResultCode::OK],
        ];
        let client = mock_client(codes.clone());
        let producer = make_ScribeProducer(fb, client, codes[0].len());
        producer
            .send_messages_now(vec![message("hello, world!")])
            .await
            .expect("write failed");
        let counters = producer.export_counters();
        assert_eq!(counters.successes, 1);
        assert_eq!(counters.failures(), 0);
    }

    #[fbinit::test]
    async fn run_once_with_cutoff_retries_and_all_succeed(fb: FacebookInit) {
        let codes = vec![
            vec![
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
            ],
            vec![
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
            ],
            vec![
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
            ],
            vec![WriteMessageResultCode::OK, WriteMessageResultCode::OK],
            vec![WriteMessageResultCode::OK, WriteMessageResultCode::OK],
            vec![
                WriteMessageResultCode::OK,
                WriteMessageResultCode::OK,
                WriteMessageResultCode::OK,
            ],
            vec![WriteMessageResultCode::OK],
        ];
        let client = mock_client(codes.clone());
        let producer = make_ScribeProducer(fb, client, codes[0].len());

        for _ in codes[0].iter() {
            producer.offer(message("hello, world!"));
        }
        producer.run_once().await.expect("write failed");
        let counters = producer.export_counters();
        assert_eq!(counters.successes, 8);
        assert_eq!(counters.failures(), 0);
    }

    #[fbinit::test]
    async fn run_once_with_cutoff_does_not_retry_after_max_retries(fb: FacebookInit) {
        let codes = vec![
            vec![
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
            ],
            vec![
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
            ],
            vec![
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
            ],
            vec![
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
            ],
            vec![WriteMessageResultCode::ENQUEUE_FAILED],
        ];
        let client = mock_client(codes.clone());
        let producer = make_ScribeProducer(fb, client, codes[0].len());
        for _ in codes[0].iter() {
            producer.offer(message("hello, world!"));
        }

        let res = producer.run_once().await;
        let counters = producer.export_counters();
        assert_eq!(counters.successes, 0);
        assert_eq!(counters.failures(), 8);
        assert_matches!(
            res,
            Err(WriteError {
                first_message_error: Some(WriteMessageError::EnqueueFailed),
                ..
            })
        );
    }

    #[fbinit::test]
    async fn last_cutoff_is_memorized(fb: FacebookInit) {
        let codes = vec![
            // The first batch begins with cutoff_index = 8
            vec![
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
                WriteMessageResultCode::ENQUEUE_FAILED,
            ],
            vec![
                WriteMessageResultCode::OK,
                WriteMessageResultCode::OK,
                WriteMessageResultCode::OK,
                WriteMessageResultCode::OK,
                WriteMessageResultCode::OK,
            ],
            vec![
                WriteMessageResultCode::OK,
                WriteMessageResultCode::OK,
                WriteMessageResultCode::OK,
            ],
            // The second batch begins with cutoff_index = 5
            vec![
                WriteMessageResultCode::OK,
                WriteMessageResultCode::OK,
                WriteMessageResultCode::OK,
                WriteMessageResultCode::OK,
                WriteMessageResultCode::OK,
            ],
            vec![
                WriteMessageResultCode::OK,
                WriteMessageResultCode::OK,
                WriteMessageResultCode::OK,
            ],
        ];
        let client = mock_client(codes.clone());
        let producer = make_ScribeProducer(fb, client, 8);
        for _ in 0..8 {
            producer.offer(message("hello, world!"));
        }
        producer.run_once().await.expect("write failed");
        for _ in 0..8 {
            producer.offer(message("hello, world!"));
        }
        producer.run_once().await.expect("write failed");
        let counters = producer.export_counters();
        assert_eq!(counters.successes, 16);
        assert_eq!(counters.failures(), 0);
    }

    fn cap_client(cap: usize, sizes: Arc<Mutex<Vec<usize>>>) -> ProducerServiceClient {
        let client = Arc::new(scribe_producer_service_mocks::new::<dyn ProducerService>());
        client.WriteMessages.mock(move |req| {
            let n = req.messages.len();
            sizes.lock().unwrap().push(n);
            let code = if n <= cap {
                WriteMessageResultCode::OK
            } else {
                WriteMessageResultCode::ENQUEUE_FAILED
            };
            WriteMessagesResponse {
                results: (0..n)
                    .map(|_| ThriftWriteMessageResult {
                        code,
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }
        });
        client
    }

    /// When Scribe queues only one-message requests, the batch is cut down to one message and
    /// every message is delivered.
    #[fbinit::test]
    async fn test_cutoff_with_one_message_capacity(fb: FacebookInit) {
        let sizes = Arc::new(Mutex::new(Vec::new()));
        let producer = make_ScribeProducer(fb, cap_client(1, Arc::clone(&sizes)), 3);
        let res = producer
            .send_messages_now(vec![message("a"), message("b"), message("c")])
            .await;
        assert!(res.is_ok(), "{res:?}");
        assert_eq!(*sizes.lock().unwrap(), vec![3, 2, 1, 1, 1]);
        assert_eq!(producer.export_counters().successes, 3);
    }

    /// A stale `last_cutoff` above the batch size does not keep the requests at the full size:
    /// a congested request is cut from the size that was sent.
    #[fbinit::test]
    async fn test_stale_last_cutoff(fb: FacebookInit) {
        let sizes = Arc::new(Mutex::new(Vec::new()));
        let producer = ScribeProducer {
            last_cutoff: Mutex::new(Some(501)),
            ..make_ScribeProducer(fb, cap_client(2, Arc::clone(&sizes)), 3)
        };
        let res = producer
            .send_messages_now(vec![message("a"), message("b"), message("c")])
            .await;
        assert!(res.is_ok(), "{res:?}");
        assert_eq!(*sizes.lock().unwrap(), vec![3, 2, 1]);
    }

    fn sized_message(len: usize) -> Message {
        Message {
            category: "buck2_events".to_owned(),
            message: vec![0; len],
            ..Default::default()
        }
    }

    /// A queue larger than `MAX_REQUEST_BYTES` is sent as several requests that each fit in it,
    /// and splitting it does not use up retry attempts.
    #[fbinit::test]
    async fn test_requests_are_bounded_by_bytes(fb: FacebookInit) {
        let sizes = Arc::new(Mutex::new(Vec::new()));
        let producer = make_ScribeProducer(fb, cap_client(usize::MAX, Arc::clone(&sizes)), 20);
        let res = producer
            .send_messages_now((0..20).map(|_| sized_message(300 * 1024)).collect())
            .await;
        assert!(res.is_ok(), "{res:?}");
        // 3 * 300 KiB fits in 1 MiB, 4 * 300 KiB does not. 7 requests exceed the 5 attempts.
        assert_eq!(*sizes.lock().unwrap(), vec![3, 3, 3, 3, 3, 3, 2]);
        assert_eq!(producer.export_counters().successes, 20);
    }

    /// A client whose `n`th `WriteMessages` call fails with an exception when `fails(n)`, and
    /// accepts every message otherwise.
    fn flaky_client(
        fails: impl Fn(usize) -> bool + Send + Sync + 'static,
        sizes: Arc<Mutex<Vec<usize>>>,
    ) -> ProducerServiceClient {
        let client = Arc::new(scribe_producer_service_mocks::new::<dyn ProducerService>());
        client.WriteMessages.mock_result(move |req| {
            let mut sizes = sizes.lock().unwrap();
            let call = sizes.len();
            sizes.push(req.messages.len());
            if fails(call) {
                return Err(WriteMessagesError::ApplicationException(
                    ApplicationException::unknown_method(),
                ));
            }
            Ok(WriteMessagesResponse {
                results: (0..req.messages.len())
                    .map(|_| ThriftWriteMessageResult {
                        code: WriteMessageResultCode::OK,
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            })
        });
        client
    }

    /// Failures separated by successful requests do not use up the retry attempts.
    #[fbinit::test]
    async fn test_failures_between_successes_do_not_exhaust_retries(fb: FacebookInit) {
        let sizes = Arc::new(Mutex::new(Vec::new()));
        let producer = make_ScribeProducer(
            fb,
            flaky_client(|call| call % 2 == 0, Arc::clone(&sizes)),
            20,
        );
        let res = producer
            .send_messages_now((0..20).map(|_| sized_message(300 * 1024)).collect())
            .await;
        assert!(res.is_ok(), "{res:?}");
        // Each of the 7 requests fails once and then succeeds: 7 failures, more than the 5 attempts.
        assert_eq!(
            *sizes.lock().unwrap(),
            vec![3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 2, 2]
        );
        assert_eq!(producer.export_counters().successes, 20);
    }

    /// A send gives up after `retry_attempts` consecutive failures.
    #[fbinit::test]
    async fn test_consecutive_failures_give_up(fb: FacebookInit) {
        let sizes = Arc::new(Mutex::new(Vec::new()));
        let producer =
            make_ScribeProducer(fb, flaky_client(|call| call >= 1, Arc::clone(&sizes)), 20);
        let res = producer
            .send_messages_now((0..20).map(|_| sized_message(300 * 1024)).collect())
            .await;
        assert!(
            res.is_err(),
            "the send should give up after 5 failures in a row"
        );
        // The first request succeeds, then the second one fails 5 times.
        assert_eq!(*sizes.lock().unwrap(), vec![3, 3, 3, 3, 3, 3]);
        let counters = producer.export_counters();
        assert_eq!(counters.successes, 3);
        assert_eq!(counters.failures(), 17);
    }

    /// A message larger than `MAX_REQUEST_BYTES` is still sent, alone in its request.
    #[fbinit::test]
    async fn test_oversized_message_is_sent_alone(fb: FacebookInit) {
        let sizes = Arc::new(Mutex::new(Vec::new()));
        let producer = make_ScribeProducer(fb, cap_client(usize::MAX, Arc::clone(&sizes)), 3);
        let res = producer
            .send_messages_now(vec![
                sized_message(2 * MAX_REQUEST_BYTES),
                sized_message(10),
                sized_message(10),
            ])
            .await;
        assert!(res.is_ok(), "{res:?}");
        assert_eq!(*sizes.lock().unwrap(), vec![1, 2]);
        assert_eq!(producer.export_counters().successes, 3);
    }

    #[test]
    fn normal_cutoff_computations() {
        let mut cc_state = CongestionControlState::new(1000);

        // Congested; 1000 -> 501
        cc_state.update_cutoff(true);
        assert_matches!(
            cc_state,
            CongestionControlState {
                phase: CongestionControlPhase::EarlyFail,
                current_cutoff: 501,
                cliff_top: 1000,
                cliff_bottom: 501,
                ..
            }
        );

        // Congested; 501 -> 251
        cc_state.update_cutoff(true);
        assert_matches!(
            cc_state,
            CongestionControlState {
                phase: CongestionControlPhase::EarlyFail,
                current_cutoff: 251,
                cliff_top: 501,
                cliff_bottom: 251,
                ..
            }
        );

        // Not congested; 251 -> 251; phase shifted to FastRecovery
        cc_state.update_cutoff(false);
        assert_matches!(
            cc_state,
            CongestionControlState {
                phase: CongestionControlPhase::FastRecovery { step: 26 },
                current_cutoff: 251,
                cliff_top: 501,
                cliff_bottom: 251,
                ..
            }
        );

        // Not congested; 251 -> 277 (+26)
        cc_state.update_cutoff(false);
        assert_matches!(
            cc_state,
            CongestionControlState {
                phase: CongestionControlPhase::FastRecovery { step: 26 },
                current_cutoff: 277,
                cliff_top: 501,
                cliff_bottom: 251,
                ..
            }
        );

        // Not congested; 277 -> 303 (+26)
        cc_state.update_cutoff(false);
        assert_matches!(
            cc_state,
            CongestionControlState {
                phase: CongestionControlPhase::FastRecovery { step: 26 },
                current_cutoff: 303,
                cliff_top: 501,
                cliff_bottom: 251,
                ..
            }
        );

        // Not congested x10; +26 per step; recovers beyond the cliff top
        for _ in 0..10 {
            cc_state.update_cutoff(false);
        }
        assert_matches!(
            cc_state,
            CongestionControlState {
                phase: CongestionControlPhase::FastRecovery { step: 26 },
                current_cutoff: 563,
                cliff_top: 501,
                cliff_bottom: 251,
                ..
            }
        );

        // Congested; 563 -> 408 (cut down to the mid point of current value and cliff bottom)
        cc_state.update_cutoff(true);
        assert_matches!(
            cc_state,
            CongestionControlState {
                phase: CongestionControlPhase::FastRecovery { step: 16 },
                current_cutoff: 408,
                cliff_top: 563,
                cliff_bottom: 408,
                ..
            }
        );

        // Congested; 408 -> 307 (cut down to below the cliff bottom)
        cc_state.update_cutoff(true);
        assert_matches!(
            cc_state,
            CongestionControlState {
                phase: CongestionControlPhase::FastRecovery { step: 11 },
                current_cutoff: 307,
                cliff_top: 408,
                cliff_bottom: 307,
                ..
            }
        );

        // Not congested; 307 -> 318 (+11)
        cc_state.update_cutoff(false);
        assert_matches!(
            cc_state,
            CongestionControlState {
                phase: CongestionControlPhase::FastRecovery { step: 11 },
                current_cutoff: 318,
                cliff_top: 408,
                cliff_bottom: 307,
                ..
            }
        );
    }

    #[test]
    fn last_one_is_not_cut_off() {
        let mut cc_state = CongestionControlState::new(1);

        cc_state.update_cutoff(true);
        assert_eq!(cc_state.current_cutoff, 1);
        cc_state.update_cutoff(false);
        cc_state.update_cutoff(true);
        assert_eq!(cc_state.current_cutoff, 1);
    }
}
