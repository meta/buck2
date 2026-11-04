/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::HashMap;
use std::collections::HashSet;
use std::env::VarError;
use std::io;
use std::io::Cursor;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use async_compression::tokio::bufread::BrotliDecoder;
use async_compression::tokio::bufread::BrotliEncoder;
use async_compression::tokio::bufread::DeflateDecoder;
use async_compression::tokio::bufread::DeflateEncoder;
use async_compression::tokio::bufread::ZstdDecoder;
use async_compression::tokio::bufread::ZstdEncoder;
use buck2_re_configuration::Buck2OssReConfiguration;
use buck2_re_configuration::HttpHeader;
use dupe::Dupe;
use futures::Stream;
use futures::future::BoxFuture;
use futures::future::Future;
use futures::stream::BoxStream;
use futures::stream::StreamExt;
use futures::stream::TryStreamExt;
use lru::LruCache;
use prost::Message;
use re_grpc_proto::build::bazel::remote::execution::v2::ActionResult;
use re_grpc_proto::build::bazel::remote::execution::v2::BatchReadBlobsRequest;
use re_grpc_proto::build::bazel::remote::execution::v2::BatchReadBlobsResponse;
use re_grpc_proto::build::bazel::remote::execution::v2::BatchUpdateBlobsRequest;
use re_grpc_proto::build::bazel::remote::execution::v2::BatchUpdateBlobsResponse;
use re_grpc_proto::build::bazel::remote::execution::v2::Digest;
use re_grpc_proto::build::bazel::remote::execution::v2::ExecuteOperationMetadata;
use re_grpc_proto::build::bazel::remote::execution::v2::ExecuteRequest as GExecuteRequest;
use re_grpc_proto::build::bazel::remote::execution::v2::ExecuteResponse as GExecuteResponse;
use re_grpc_proto::build::bazel::remote::execution::v2::ExecutedActionMetadata;
use re_grpc_proto::build::bazel::remote::execution::v2::ExecutionPolicy;
use re_grpc_proto::build::bazel::remote::execution::v2::FindMissingBlobsRequest;
use re_grpc_proto::build::bazel::remote::execution::v2::FindMissingBlobsResponse;
use re_grpc_proto::build::bazel::remote::execution::v2::GetActionResultRequest;
use re_grpc_proto::build::bazel::remote::execution::v2::GetCapabilitiesRequest;
use re_grpc_proto::build::bazel::remote::execution::v2::OutputDirectory;
use re_grpc_proto::build::bazel::remote::execution::v2::OutputFile;
use re_grpc_proto::build::bazel::remote::execution::v2::OutputSymlink;
use re_grpc_proto::build::bazel::remote::execution::v2::RequestMetadata;
use re_grpc_proto::build::bazel::remote::execution::v2::ResultsCachePolicy;
use re_grpc_proto::build::bazel::remote::execution::v2::ToolDetails;
use re_grpc_proto::build::bazel::remote::execution::v2::UpdateActionResultRequest;
use re_grpc_proto::build::bazel::remote::execution::v2::WaitExecutionRequest;
use re_grpc_proto::build::bazel::remote::execution::v2::action_cache_client::ActionCacheClient;
use re_grpc_proto::build::bazel::remote::execution::v2::batch_update_blobs_request::Request;
use re_grpc_proto::build::bazel::remote::execution::v2::capabilities_client::CapabilitiesClient;
use re_grpc_proto::build::bazel::remote::execution::v2::compressor;
use re_grpc_proto::build::bazel::remote::execution::v2::content_addressable_storage_client::ContentAddressableStorageClient;
use re_grpc_proto::build::bazel::remote::execution::v2::execution_client::ExecutionClient;
use re_grpc_proto::build::bazel::remote::execution::v2::execution_stage;
use re_grpc_proto::google::bytestream::ReadRequest;
use re_grpc_proto::google::bytestream::ReadResponse;
use re_grpc_proto::google::bytestream::WriteRequest;
use re_grpc_proto::google::bytestream::WriteResponse;
use re_grpc_proto::google::bytestream::byte_stream_client::ByteStreamClient;
use re_grpc_proto::google::longrunning::Operation;
use re_grpc_proto::google::longrunning::operation::Result as OpResult;
use re_grpc_proto::google::rpc::Code;
use re_grpc_proto::google::rpc::Status;
use regex::Regex;
use tokio::fs::OpenOptions;
use tokio::io::AsyncBufRead;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio_util::io::StreamReader;
use tonic::codegen::InterceptedService;
use tonic::metadata;
use tonic::metadata::MetadataKey;
use tonic::metadata::MetadataValue;
use tonic::service::Interceptor;
use tonic::transport::Channel;

use crate::error::*;
use crate::metadata::*;
use crate::pool::ChannelConfig;
use crate::pool::ChannelPool;
use crate::pool::PoolConfig;
use crate::pool::PooledChannel;
use crate::pool::create_channel;
use crate::request::*;
use crate::response::*;

const DEFAULT_MAX_TOTAL_BATCH_SIZE: usize = 4 * 1000 * 1000;

fn tdigest_to(tdigest: &TDigest) -> Digest {
    Digest {
        hash: tdigest.hash.clone(),
        size_bytes: tdigest.size_in_bytes,
    }
}

fn tdigest_from(digest: Digest) -> TDigest {
    TDigest {
        hash: digest.hash,
        size_in_bytes: digest.size_bytes,
        ..Default::default()
    }
}

fn tstatus_ok() -> TStatus {
    TStatus {
        code: TCode::OK,
        message: "".to_owned(),
        ..Default::default()
    }
}

fn check_status(status: Status) -> Result<(), REClientError> {
    if status.code == 0 {
        return Ok(());
    }

    Err(REClientError {
        code: TCode(status.code),
        message: status.message,
        group: TCodeReasonGroup::UNKNOWN,
    })
}

fn ttimestamp_to(ts: &TTimestamp) -> ::prost_types::Timestamp {
    ::prost_types::Timestamp {
        seconds: ts.seconds,
        nanos: ts.nanos,
    }
}

fn ttimestamp_from(ts: Option<::prost_types::Timestamp>) -> TTimestamp {
    match ts {
        Some(timestamp) => TTimestamp {
            seconds: timestamp.seconds,
            nanos: timestamp.nanos,
            ..Default::default()
        },
        None => TTimestamp::unix_epoch(),
    }
}

fn tany_to(any: &TAny) -> ::prost_types::Any {
    ::prost_types::Any {
        type_url: any.type_url.clone(),
        value: any.value.clone(),
    }
}

fn tany_from(any: ::prost_types::Any) -> TAny {
    TAny {
        type_url: any.type_url,
        value: any.value,
        ..Default::default()
    }
}

/// Contains information queried from the Remote Execution Capabilities service.
pub struct RECapabilities {
    /// Largest size of a message before being uploaded using bytestream service.
    /// 0 indicates no limit beyond constraint of underlying transport (which is unknown).
    max_total_batch_size: usize,
    /// Compressors supported by the "compressed-blobs" bytestream resources. Also used to decide
    /// whether we can request zstd-compressed `BatchReadBlobs` responses.
    supported_compressors: Vec<Compressor>,
}

/// Contains runtime options for the remote execution client as set under `buck2_re_client`
pub struct RERuntimeOpts {
    /// Use the Meta version of the request metadata
    use_fbcode_metadata: bool,
    /// Maximum number of concurrent upload requests.
    max_concurrent_uploads_per_action: Option<usize>,
    /// Time that digests are assumed to live in CAS after being touched.
    cas_ttl_secs: i64,
    /// Maximum number of digests per `FindMissingBlobs` RPC.
    find_missing_blobs_batch_size: usize,
    /// Whether to request zstd-compressed `BatchReadBlobs` responses (server supports zstd).
    batch_read_zstd: bool,
}

struct InstanceName(Option<String>);

impl InstanceName {
    fn as_str(&self) -> &str {
        match &self.0 {
            Some(instance_name) => instance_name,
            None => "",
        }
    }

    fn as_resource_prefix(&self) -> String {
        match &self.0 {
            Some(instance_name) => format!("{instance_name}/"),
            None => "".to_owned(),
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum Compressor {
    Zstd,
    Deflate,
    Brotli,
}

impl Compressor {
    fn from_grpc(val: i32) -> Option<Self> {
        if val == compressor::Value::Zstd as i32 {
            Some(Self::Zstd)
        } else if val == compressor::Value::Deflate as i32 {
            Some(Self::Deflate)
        } else if val == compressor::Value::Brotli as i32 {
            Some(Self::Brotli)
        } else {
            None
        }
    }

    /// The compressor name used in compressed-blob resource paths
    fn name(&self) -> &str {
        match self {
            Self::Zstd => "zstd",
            Self::Deflate => "deflate",
            Self::Brotli => "brotli",
        }
    }
}

/// Decompress a zstd-compressed blob returned in a `BatchReadBlobs` response.
async fn zstd_decompress_blob(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut decoder = ZstdDecoder::new(Cursor::new(data));
    decoder.multiple_members(true);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out).await?;
    Ok(out)
}

pub struct REClientBuilder;

impl REClientBuilder {
    pub async fn build_and_connect(opts: &Buck2OssReConfiguration) -> anyhow::Result<REClient> {
        // Create channel config once (reads TLS files)
        let channel_config = ChannelConfig::new(opts)
            .await
            .context("Failed to create channel config")?;

        // Create a single channel for fetching capabilities. Other channels are created
        // on-demand through the connection pool.
        let engine_address = opts.engine_address.as_ref().context("No engine address")?;
        let capabilities_channel = create_channel(&channel_config, engine_address)
            .context("Error creating Capabilities channel")?;

        let interceptor = InjectHeadersInterceptor::new(&opts.http_headers)?;

        let mut capabilities_client =
            CapabilitiesClient::with_interceptor(capabilities_channel, interceptor.dupe());

        if let Some(max_decoding_message_size) = opts.max_decoding_message_size {
            capabilities_client =
                capabilities_client.max_decoding_message_size(max_decoding_message_size);
        }

        let instance_name = InstanceName(opts.instance_name.clone());

        let capabilities = if opts.capabilities.unwrap_or(true) {
            Self::fetch_rbe_capabilities(
                &mut capabilities_client,
                &instance_name,
                opts.max_total_batch_size,
            )
            .await?
        } else {
            RECapabilities {
                max_total_batch_size: DEFAULT_MAX_TOTAL_BATCH_SIZE,
                supported_compressors: Vec::new(),
            }
        };

        let max_decoding_msg_size = opts
            .max_decoding_message_size
            .unwrap_or(capabilities.max_total_batch_size * 2);

        if max_decoding_msg_size < capabilities.max_total_batch_size {
            return Err(anyhow::anyhow!(
                "Attribute `max_decoding_message_size` must always be equal or higher to `max_total_batch_size`"
            ));
        }

        // Choose a ByteStream compressor
        let bystream_compressor = if capabilities
            .supported_compressors
            .contains(&Compressor::Zstd)
        {
            Some(Compressor::Zstd)
        } else if capabilities
            .supported_compressors
            .contains(&Compressor::Brotli)
        {
            Some(Compressor::Brotli)
        } else if capabilities
            .supported_compressors
            .contains(&Compressor::Deflate)
        {
            Some(Compressor::Deflate)
        } else {
            None
        };

        // Extract addresses
        let cas_address = opts.cas_address.clone().context("No CAS address")?;
        let action_cache_address = opts
            .action_cache_address
            .clone()
            .context("No action cache address")?;

        // Create connection pool
        let min_connections = opts.min_connections.unwrap_or(1).max(1);
        let max_connections = opts.max_connections.unwrap_or(100).max(min_connections);
        let pool_config = PoolConfig {
            min_connections,
            max_connections,
            max_concurrency_per_connection: opts.max_concurrency_per_connection.unwrap_or(100),
        };
        let pool = ChannelPool::new(pool_config, channel_config);

        Ok(REClient::new(
            RERuntimeOpts {
                use_fbcode_metadata: opts.use_fbcode_metadata,
                max_concurrent_uploads_per_action: opts.max_concurrent_uploads_per_action,
                // NOTE: This is an arbitrary number because RBE does not return information
                // on the TTL of the remote blob.
                cas_ttl_secs: opts.cas_ttl_secs.unwrap_or(3 * 60 * 60),
                find_missing_blobs_batch_size: opts.find_missing_blobs_batch_size.unwrap_or(100),
                batch_read_zstd: capabilities
                    .supported_compressors
                    .contains(&Compressor::Zstd),
            },
            capabilities,
            instance_name,
            bystream_compressor,
            pool,
            max_decoding_msg_size,
            interceptor,
            cas_address,
            engine_address.clone(),
            action_cache_address,
        ))
    }

    async fn fetch_rbe_capabilities(
        client: &mut CapabilitiesClient<InterceptedService<Channel, InjectHeadersInterceptor>>,
        instance_name: &InstanceName,
        max_total_batch_size: Option<usize>,
    ) -> anyhow::Result<RECapabilities> {
        // TODO use more of the capabilities of the remote build executor

        let resp = client
            .get_capabilities(GetCapabilitiesRequest {
                instance_name: instance_name.as_str().to_owned(),
            })
            .await
            .context("Failed to query capabilities of remote")
            .map_err(normalize_re_error)?
            .into_inner();

        let supported_compressors = if let Some(cache_cap) = &resp.cache_capabilities {
            cache_cap
                .supported_compressors
                .iter()
                .copied()
                .filter_map(Compressor::from_grpc)
                .collect()
        } else {
            Vec::new()
        };

        let max_total_batch_size_from_capabilities: Option<usize> =
            if let Some(cache_cap) = resp.cache_capabilities {
                let size = cache_cap.max_batch_total_size_bytes as usize;
                // A value of 0 means no limit is set
                if size != 0 { Some(size) } else { None }
            } else {
                None
            };

        let max_total_batch_size =
            match (max_total_batch_size_from_capabilities, max_total_batch_size) {
                (Some(cap), Some(config)) => std::cmp::min(cap, config),
                (Some(cap), None) => cap,
                (None, Some(config)) => config,
                (None, None) => DEFAULT_MAX_TOTAL_BATCH_SIZE,
            };

        Ok(RECapabilities {
            max_total_batch_size,
            supported_compressors,
        })
    }
}

#[derive(Clone, Dupe)]
struct InjectHeadersInterceptor {
    headers: Arc<Vec<(MetadataKey<metadata::Ascii>, MetadataValue<metadata::Ascii>)>>,
}

impl InjectHeadersInterceptor {
    pub fn new(headers: &[HttpHeader]) -> anyhow::Result<Self> {
        let headers = headers
            .iter()
            .map(|h| {
                // This means we can't have `$` in a header key or value, which isn't great. On the
                // flip side, env vars are good for things like credentials, which those headers
                // are likely to contain. In time, we should allow escaping.
                let key = substitute_env_vars(&h.key)?;
                let value = substitute_env_vars(&h.value)?;

                let key = MetadataKey::<metadata::Ascii>::from_bytes(key.as_bytes())
                    .with_context(|| format!("Invalid key in header: `{key}: {value}`"))?;

                let value = MetadataValue::try_from(&value)
                    .with_context(|| format!("Invalid value in header: `{key}: {value}`"))?;

                anyhow::Ok((key, value))
            })
            .collect::<Result<_, _>>()
            .context("Error converting headers")?;

        Ok(Self {
            headers: Arc::new(headers),
        })
    }
}

impl Interceptor for InjectHeadersInterceptor {
    fn call(
        &mut self,
        mut request: tonic::Request<()>,
    ) -> Result<tonic::Request<()>, tonic::Status> {
        for (k, v) in self.headers.iter() {
            request.metadata_mut().insert(k.clone(), v.clone());
        }
        Ok(request)
    }
}

type GrpcService = InterceptedService<PooledChannel, InjectHeadersInterceptor>;

#[derive(Debug, Copy, Clone)]
enum DigestRemoteState {
    ExistsOnRemote,
    Missing,
}

struct FindMissingCache {
    cache: LruCache<TDigest, DigestRemoteState>,
    /// To avoid a situation where we cache that an artifact is available remotely, but the artifact then expires
    /// we clear our local cache once every `ttl`.
    ttl: Duration,
    last_check: Instant,
}

impl FindMissingCache {
    fn clear_if_ttl_expires(&mut self) {
        if self.last_check.elapsed() > self.ttl {
            self.cache.clear();
            self.last_check = Instant::now();
        }
    }

    pub fn get(&mut self, digest: &TDigest) -> Option<DigestRemoteState> {
        self.clear_if_ttl_expires();
        self.cache.get(digest).copied()
    }

    pub fn put(&mut self, digest: TDigest, state: DigestRemoteState) {
        self.clear_if_ttl_expires();
        self.cache.put(digest, state);
    }
}

pub struct REClient {
    runtime_opts: RERuntimeOpts,
    pool: Arc<ChannelPool>,
    capabilities: RECapabilities,
    instance_name: InstanceName,
    // buck2 calls find_missing for same blobs
    find_missing_cache: Mutex<FindMissingCache>,
    bystream_compressor: Option<Compressor>,
    max_decoding_msg_size: usize,
    interceptor: InjectHeadersInterceptor,
    cas_address: String,
    engine_address: String,
    action_cache_address: String,
}

impl Drop for REClient {
    fn drop(&mut self) {
        // Important we have a drop implementation since the real one does, and we
        // don't want errors coming from the stub not having one
    }
}

/// Information on components of a batch upload.
/// Used to defer reading of NamedDigest contents till
/// actual execution of upload and prevent opening too many
/// files at the same time.
enum BatchUploadRequest {
    Blob(InlinedBlobWithDigest),
    File(NamedDigest),
}

/// Builds up a vector of batch upload requests based upon the maximum allowed message size.
#[derive(Default)]
struct BatchUploadReqAggregator {
    max_msg_size: i64,
    curr_req: Vec<BatchUploadRequest>,
    requests: Vec<Vec<BatchUploadRequest>>,
    curr_request_size: i64,
}

impl BatchUploadReqAggregator {
    pub fn new(max_msg_size: usize) -> Self {
        BatchUploadReqAggregator {
            max_msg_size: max_msg_size as i64,
            ..Default::default()
        }
    }

    pub fn push(&mut self, req: BatchUploadRequest) {
        let size_in_bytes = match &req {
            BatchUploadRequest::Blob(blob) => blob.digest.size_in_bytes,
            BatchUploadRequest::File(file) => file.digest.size_in_bytes,
        };

        // As an optimization, we can silently skip uploading empty blobs
        if size_in_bytes == 0 {
            return;
        }

        self.curr_request_size += size_in_bytes;

        if self.curr_request_size >= self.max_msg_size {
            self.requests.push(std::mem::take(&mut self.curr_req));
            self.curr_request_size = size_in_bytes;
        }
        self.curr_req.push(req);
    }

    pub fn done(mut self) -> Vec<Vec<BatchUploadRequest>> {
        if !self.curr_req.is_empty() {
            self.requests.push(std::mem::take(&mut self.curr_req));
        }
        self.requests
    }
}

/// Returns true if an `io::Error` indicates a transport-level disruption that
/// a fresh connection can recover from.
fn is_transient_io_error(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::TimedOut
    )
}

/// Returns true if an error is a transient connection/transport error worth
/// retrying. Walks the error chain checking for:
///   - `tonic::Status` with codes the gRPC retry policy treats as transient
///     (Unavailable, ResourceExhausted, Aborted, Cancelled)
///   - `io::Error` of a kind that indicates a transport-level disruption
///     (BrokenPipe, ConnectionReset/Aborted, UnexpectedEof, TimedOut)
///   - `h2::Error` wrapping such an `io::Error`. `h2::Error` does not
///     implement `source()`, so the inner `io::Error` never appears as its
///     own link in the chain; tonic also maps these errors to
///     `Code::Unknown`. Body errors on an established connection (for
///     example a TCP reset during a `BatchReadBlobs` response) take this
///     path and need the explicit downcast.
///
/// `tonic::transport::Error` is not matched directly — its transient subset
/// surfaces as an `io::Error` somewhere in the chain, which we catch above.
/// Non-transient transport errors (TLS handshake, invalid URI) do not have
/// an `io::Error` source, so they correctly do not retry.
fn is_retryable(err: &anyhow::Error) -> bool {
    for cause in err.chain() {
        if let Some(status) = cause.downcast_ref::<tonic::Status>() {
            match status.code() {
                tonic::Code::Unavailable
                | tonic::Code::ResourceExhausted
                | tonic::Code::Aborted
                | tonic::Code::Cancelled => return true,
                _ => {}
            }
        }
        if let Some(io_err) = cause.downcast_ref::<std::io::Error>() {
            if is_transient_io_error(io_err) {
                return true;
            }
        }
        if let Some(h2_err) = cause.downcast_ref::<h2::Error>() {
            if let Some(io_err) = h2_err.get_io() {
                if is_transient_io_error(io_err) {
                    return true;
                }
            }
        }
    }
    false
}

/// Attaches an RE classification to gRPC errors, preserving the original chain
/// and any existing `REClientError` classification.
fn normalize_re_error(err: anyhow::Error) -> anyhow::Error {
    if err.downcast_ref::<REClientError>().is_some() {
        return err;
    }
    let Some(status) = err.chain().find_map(|cause| {
        cause.downcast_ref::<tonic::Status>().or_else(|| {
            // `io::Error::source()` skips the wrapped error itself.
            cause
                .downcast_ref::<std::io::Error>()?
                .get_ref()?
                .downcast_ref::<tonic::Status>()
        })
    }) else {
        return err;
    };
    let re_error = REClientError {
        code: TCode(status.code() as i32),
        message: status.message().to_owned(),
        group: TCodeReasonGroup::UNKNOWN,
    };
    err.context(re_error)
}

const INITIAL_DELAY: Duration = Duration::from_millis(100);
const MAX_DELAY: Duration = Duration::from_secs(5);
/// Upper bound of the jitter added to every delay, to avoid synchronized retry
/// storms across concurrent in-flight requests.
const MAX_JITTER: Duration = Duration::from_millis(50);

fn delay_with_jitter(delay: Duration) -> Duration {
    use rand::RngExt;

    let jitter = rand::rng().random_range(0..MAX_JITTER.as_millis() as u64);
    delay + Duration::from_millis(jitter)
}

/// Retry an RPC on transient connection errors and classify its final error.
///
/// On retryable failure, the closure is called again from scratch — acquiring
/// a fresh connection from the pool, rebuilding the request, etc. Up to 5
/// attempts with exponential backoff (100ms, 200ms, 400ms, 800ms — capped at
/// 5s) plus 0–50ms of jitter to avoid synchronized retry storms across
/// concurrent in-flight requests.
async fn retry<F, Fut, T>(f: F) -> anyhow::Result<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    const MAX_ATTEMPTS: u32 = 5;

    let mut delay = INITIAL_DELAY;
    for attempt in 1..=MAX_ATTEMPTS {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) if is_retryable(&e) && attempt < MAX_ATTEMPTS => {
                let sleep_for = delay_with_jitter(delay);
                tracing::debug!(
                    "Transient error (attempt {}/{}), retrying in {:?}: {:#}",
                    attempt,
                    MAX_ATTEMPTS,
                    sleep_for,
                    e
                );
                tokio::time::sleep(sleep_for).await;
                delay = (delay * 2).min(MAX_DELAY);
            }
            Err(e) => return Err(normalize_re_error(e)),
        }
    }
    unreachable!()
}

/// Retries attempted after a failure on the execution stream or of the execution
/// itself, matching Bazel's `--remote_retries` default. A resume is cheap —
/// `WaitExecution` reattaches rather than re-running — but a restart re-runs the
/// action, so one the server keeps failing runs one more time than this.
const MAX_EXECUTION_RETRIES: u32 = 5;

/// Free function so a resumed execution stream can open its own call without
/// borrowing an `REClient`.
async fn execution_client_for(
    pool: &ChannelPool,
    engine_address: &str,
    interceptor: &InjectHeadersInterceptor,
) -> anyhow::Result<ExecutionClient<GrpcService>> {
    let channel = pool.get(engine_address).await?;
    Ok(ExecutionClient::new(InterceptedService::new(
        channel,
        interceptor.dupe(),
    )))
}

/// Resumes that report nothing before the stream is treated as broken.
const MAX_EMPTY_RESUMES: u32 = 5;

/// Decodes one `Operation` from an execution stream into a progress event.
fn decode_execute_operation(
    msg: Operation,
    cas_ttl_secs: i64,
) -> anyhow::Result<ExecuteWithProgressResponse> {
    if !msg.done {
        let meta = ExecuteOperationMetadata::decode(&msg.metadata.unwrap_or_default().value[..])?;

        let stage = match execution_stage::Value::try_from(meta.stage) {
            Ok(execution_stage::Value::Unknown) => Stage::UNKNOWN,
            Ok(execution_stage::Value::CacheCheck) => Stage::CACHE_CHECK,
            Ok(execution_stage::Value::Queued) => Stage::QUEUED,
            Ok(execution_stage::Value::Executing) => Stage::EXECUTING,
            Ok(execution_stage::Value::Completed) => Stage::COMPLETED,
            _ => Stage::UNKNOWN,
        };

        return Ok(ExecuteWithProgressResponse {
            stage,
            execute_response: None,
            ..Default::default()
        });
    }

    match msg
        .result
        .context("Missing `result` when message was `done`")?
    {
        OpResult::Error(rpc_status) => Err(REClientError {
            code: TCode(rpc_status.code),
            message: rpc_status.message,
            group: TCodeReasonGroup::UNKNOWN,
        }
        .into()),
        OpResult::Response(any) => {
            let execute_response_grpc: GExecuteResponse = GExecuteResponse::decode(&any.value[..])?;

            check_status(execute_response_grpc.status.unwrap_or_default())?;

            let action_result = execute_response_grpc
                .result
                .with_context(|| "The action result is not defined.")?;

            let action_result = convert_action_result(action_result, cas_ttl_secs)?;

            let execute_response = ExecuteResponse {
                action_result,
                action_result_digest: TDigest::default(),
                action_result_ttl: cas_ttl_secs,
                status: TStatus {
                    code: TCode::OK,
                    message: execute_response_grpc.message,
                    ..Default::default()
                },
                cached_result: execute_response_grpc.cached_result,
                action_digest: Default::default(), // Filled in by the caller.
            };

            Ok(ExecuteWithProgressResponse {
                stage: Stage::COMPLETED,
                execute_response: Some(execute_response),
                ..Default::default()
            })
        }
    }
}

/// Returns true if a server finishing an operation with `code` is worth running
/// the action again for. A finished operation's status can be the action's own:
/// `DEADLINE_EXCEEDED` is its timeout and would recur, and `RESOURCE_EXHAUSTED`
/// can be the action asking for more than any worker offers, where running it
/// again only adds load. A command that exits non-zero never gets here — REAPI
/// reports it with an `OK` status.
fn is_transient_execution_status(code: TCode) -> bool {
    matches!(
        code,
        TCode::UNAVAILABLE | TCode::ABORTED | TCode::UNKNOWN | TCode::INTERNAL
    )
}

struct ExecutionStreamState<F> {
    /// Opens the message stream for the operation it is given.
    open: F,
    /// Which operation to open a stream for: `None` starts a fresh `Execute`,
    /// `Some(name)` resumes that one with `WaitExecution`. Empty until the server
    /// names the operation, and emptied again if the server loses it or finishes
    /// it with a transient failure.
    operation: Option<String>,
    /// Per-action and never replenished, so a long enough action can exhaust it
    /// on unrelated failures it made progress between.
    retries_left: u32,
    /// Resumes since the last message arrived. `WaitExecution` must respond
    /// immediately with the operation's current status, so a resume that yields
    /// nothing at all means the server is misbehaving, and only those are capped.
    /// A long action closing and resuming repeatedly resets this every time.
    empty_resumes: u32,
    delay: Duration,
    /// The response has been delivered, so there is nothing left to read and
    /// reopening the call would only produce a second one.
    done: bool,
}

/// Streams progress events for one action, resuming across a broken connection.
///
/// Follows Bazel's `GrpcRemoteExecutor`: `Execute` starts the action, and once
/// the server has named the operation, `WaitExecution` resumes it rather than
/// re-queueing. `NOT_FOUND` means the server lost the operation, so start over
/// with `Execute`. A stream that ends cleanly without a response is not an
/// error — REAPI allows it when execution outlives a connection — so resuming
/// from one does not spend the retry budget, only the separate cap on resumes
/// that report nothing at all. An operation the server finishes with a
/// transient failure is also started over with `Execute`, and that does spend
/// the budget.
fn execution_progress_stream<S, F, Fut>(
    open: F,
    cas_ttl_secs: i64,
) -> impl Stream<Item = anyhow::Result<ExecuteWithProgressResponse>>
where
    S: Stream<Item = Result<Operation, tonic::Status>> + Unpin,
    F: Fn(Option<String>) -> Fut,
    Fut: Future<Output = anyhow::Result<S>>,
{
    let state = ExecutionStreamState {
        open,
        operation: None,
        retries_left: MAX_EXECUTION_RETRIES,
        empty_resumes: 0,
        delay: INITIAL_DELAY,
        done: false,
    };

    futures::stream::try_unfold(
        (None::<S>, state),
        move |(mut stream, mut state)| async move {
            loop {
                if state.done {
                    return Ok(None);
                }

                let live = match &mut stream {
                    Some(live) => live,
                    None => match (state.open)(state.operation.clone()).await {
                        Ok(opened) => stream.insert(opened),
                        Err(err) => {
                            let err = match &state.operation {
                                Some(name) => {
                                    err.context(format!("Failed to resume remote execution {name}"))
                                }
                                None => err.context("Failed to start remote execution"),
                            };
                            state.wait_to_retry(err).await?;
                            continue;
                        }
                    },
                };

                match live.try_next().await {
                    Ok(Some(msg)) => {
                        if !msg.name.is_empty() {
                            state.operation = Some(msg.name.clone());
                        }
                        state.empty_resumes = 0;
                        match decode_execute_operation(msg, cas_ttl_secs) {
                            Ok(response) => {
                                state.done = response.execute_response.is_some();
                                return Ok(Some((response, (stream, state))));
                            }
                            Err(err) => {
                                let Some(code) = err
                                    .downcast_ref::<REClientError>()
                                    .map(|e| e.code)
                                    .filter(|&code| is_transient_execution_status(code))
                                else {
                                    return Err(err);
                                };
                                // Resuming a finished operation would only report the
                                // same failure.
                                state.operation = None;
                                stream = None;
                                let err = err.context(format!(
                                    "Server failed the execution with status {:?}",
                                    tonic::Code::from(code.0)
                                ));
                                state.back_off(err).await?;
                            }
                        }
                    }
                    Ok(None) => {
                        let Some(operation) = &state.operation else {
                            return Err(anyhow::anyhow!(
                                "Remote server error: execution request terminated with no result"
                            ));
                        };
                        // Not charged to the retry budget: REAPI allows execution to
                        // outlive a connection, so closing and resuming is normal for
                        // a long action and budgeting it would kill exactly the
                        // actions it exists for.
                        if state.empty_resumes == MAX_EMPTY_RESUMES {
                            return Err(anyhow::anyhow!(
                                "Remote server error: execution of {operation} was resumed \
                                 {MAX_EMPTY_RESUMES} times without the server reporting anything"
                            ));
                        }
                        state.empty_resumes += 1;
                        tracing::debug!(
                            "Remote execution stream for {} closed before completing, resuming",
                            operation
                        );
                        stream = None;
                    }
                    Err(status) => {
                        let err = anyhow::Error::new(status).context("RE channel error");
                        stream = None;
                        state.wait_to_retry(err).await?;
                    }
                }
            }
        },
    )
}

impl<F> ExecutionStreamState<F> {
    /// Spends one retry and waits out the backoff, or gives the error back to the
    /// caller if the budget is spent or the failure is not worth another attempt.
    /// Retrying itself is the caller's job.
    async fn wait_to_retry(&mut self, err: anyhow::Error) -> anyhow::Result<()> {
        let code = err
            .chain()
            .find_map(|cause| cause.downcast_ref::<tonic::Status>())
            .map(|status| status.code());

        // Only meaningful while resuming, where it means the server no longer
        // knows the operation and REAPI says to start over. Answering a fresh
        // `Execute` this way is permanent — a bad instance name, or an endpoint
        // with no execution service — and retrying cannot change it.
        let operation_lost = self.operation.is_some()
            && matches!(
                code,
                Some(tonic::Code::NotFound | tonic::Code::Unimplemented)
            );
        if operation_lost {
            self.operation = None;
        }

        let retryable = operation_lost
            || is_retryable(&err)
            // Only while resuming: starting over re-runs the action, and REAPI
            // defines `DEADLINE_EXCEEDED` there as the action's own timeout.
            || (self.operation.is_some() && code == Some(tonic::Code::DeadlineExceeded));

        if !retryable {
            return Err(err);
        }
        self.back_off(err).await
    }

    /// Spends one retry and waits out the backoff, or gives the error back to the
    /// caller once the budget is spent.
    async fn back_off(&mut self, err: anyhow::Error) -> anyhow::Result<()> {
        if self.retries_left == 0 {
            return Err(err);
        }
        self.retries_left -= 1;

        let sleep_for = delay_with_jitter(self.delay);
        self.delay = (self.delay * 2).min(MAX_DELAY);
        tracing::warn!(
            "Remote execution failed (retry {}/{}), retrying in {:?}: {:#}",
            MAX_EXECUTION_RETRIES - self.retries_left,
            MAX_EXECUTION_RETRIES,
            sleep_for,
            err
        );
        tokio::time::sleep(sleep_for).await;
        Ok(())
    }
}

impl REClient {
    fn new(
        runtime_opts: RERuntimeOpts,
        capabilities: RECapabilities,
        instance_name: InstanceName,
        bystream_compressor: Option<Compressor>,
        pool: ChannelPool,
        max_decoding_msg_size: usize,
        interceptor: InjectHeadersInterceptor,
        cas_address: String,
        engine_address: String,
        action_cache_address: String,
    ) -> Self {
        let cas_ttl_secs = runtime_opts.cas_ttl_secs;
        REClient {
            runtime_opts,
            pool: Arc::new(pool),
            capabilities,
            instance_name,
            find_missing_cache: Mutex::new(FindMissingCache {
                cache: LruCache::new(NonZeroUsize::new(500_000).unwrap()),
                ttl: Duration::from_secs(cas_ttl_secs.max(0) as u64),
                last_check: Instant::now(),
            }),
            bystream_compressor,
            max_decoding_msg_size,
            interceptor,
            cas_address,
            engine_address,
            action_cache_address,
        }
    }

    pub fn get_action_result<'a>(
        &'a self,
        metadata: &RemoteExecutionMetadata,
        request: ActionResultRequest,
    ) -> impl Future<Output = anyhow::Result<ActionResultResponse>> + Send + use<'a> {
        let action_digest = tdigest_to(&request.digest);
        let headers = with_re_metadata((), metadata, self.runtime_opts.use_fbcode_metadata)
            .into_parts()
            .0;
        async move {
            retry(|| async {
                let res = self
                    .action_cache_client()
                    .await?
                    .get_action_result(tonic::Request::from_parts(
                        headers.clone(),
                        Default::default(),
                        GetActionResultRequest {
                            instance_name: self.instance_name.as_str().to_owned(),
                            action_digest: Some(action_digest.clone()),
                            ..Default::default()
                        },
                    ))
                    .await?;

                Ok(ActionResultResponse {
                    action_result: convert_action_result(
                        res.into_inner(),
                        self.runtime_opts.cas_ttl_secs,
                    )?,
                    ttl: self.runtime_opts.cas_ttl_secs,
                })
            })
            .await
        }
    }

    pub fn write_action_result<'a>(
        &'a self,
        metadata: &RemoteExecutionMetadata,
        request: &WriteActionResultRequest,
    ) -> impl Future<Output = anyhow::Result<WriteActionResultResponse>> + Send + use<'a> {
        let action_result = convert_t_action_result2(&request.action_result);
        let action_digest = request.action_digest.clone();
        let headers = with_re_metadata((), metadata, self.runtime_opts.use_fbcode_metadata)
            .into_parts()
            .0;
        async move {
            let action_result = action_result?;
            retry(|| async {
                let res = self
                    .action_cache_client()
                    .await?
                    .update_action_result(tonic::Request::from_parts(
                        headers.clone(),
                        Default::default(),
                        UpdateActionResultRequest {
                            instance_name: self.instance_name.as_str().to_owned(),
                            action_digest: Some(tdigest_to(&action_digest)),
                            action_result: Some(action_result.clone()),
                            results_cache_policy: None,
                            ..Default::default()
                        },
                    ))
                    .await?;

                Ok(WriteActionResultResponse {
                    actual_action_result: convert_action_result(
                        res.into_inner(),
                        self.runtime_opts.cas_ttl_secs,
                    )?,
                    ttl_seconds: self.runtime_opts.cas_ttl_secs,
                })
            })
            .await
        }
    }

    pub async fn execute_with_progress(
        &self,
        metadata: &RemoteExecutionMetadata,
        mut execute_request: ExecuteRequest,
    ) -> anyhow::Result<BoxStream<'static, anyhow::Result<ExecuteWithProgressResponse>>> {
        // TODO(aloiscochard): Map those properly in the request
        // use crate::proto::build::bazel::remote::execution::v2::ExecutionPolicy;

        let action_digest = tdigest_to(&execute_request.action_digest);
        let priority = execute_request
            .execution_policy
            .map(|ep| ep.priority)
            .unwrap_or_default();

        let pool = self.pool.dupe();
        let interceptor = self.interceptor.dupe();
        let engine_address = self.engine_address.clone();
        let use_fbcode_metadata = self.runtime_opts.use_fbcode_metadata;
        let request = GExecuteRequest {
            instance_name: self.instance_name.as_str().to_owned(),
            skip_cache_lookup: execute_request.skip_cache_lookup,
            execution_policy: Some(ExecutionPolicy { priority }),
            results_cache_policy: Some(ResultsCachePolicy { priority: 0 }),
            action_digest: Some(action_digest),
            ..Default::default()
        };

        let owned_metadata = metadata.clone();
        let open = move |resume: Option<String>| {
            let pool = pool.dupe();
            let interceptor = interceptor.dupe();
            let engine_address = engine_address.clone();
            let metadata = owned_metadata.clone();
            let request = request.clone();

            async move {
                let mut client = execution_client_for(&pool, &engine_address, &interceptor).await?;

                let stream = match resume {
                    Some(name) => client
                        .wait_execution(with_re_metadata(
                            WaitExecutionRequest { name },
                            &metadata,
                            use_fbcode_metadata,
                        ))
                        .await?
                        .into_inner(),
                    None => client
                        .execute(with_re_metadata(request, &metadata, use_fbcode_metadata))
                        .await?
                        .into_inner(),
                };

                anyhow::Ok(stream)
            }
        };

        let stream = execution_progress_stream(open, self.runtime_opts.cas_ttl_secs)
            .map_err(normalize_re_error);

        // We fill in the action digest a little later here. We do it this way so we don't have to
        // clone the execute_request into every future we create above.

        let stream = stream.map(move |mut r| {
            match &mut r {
                Ok(ExecuteWithProgressResponse {
                    execute_response: Some(response),
                    ..
                }) => {
                    response.action_digest = std::mem::take(&mut execute_request.action_digest);
                }
                _ => {}
            };

            r
        });

        Ok(stream.boxed())
    }

    pub fn upload<'a>(
        &'a self,
        metadata: &RemoteExecutionMetadata,
        request: UploadRequest,
    ) -> impl Future<Output = anyhow::Result<UploadResponse>> + Send + use<'a> {
        let headers = with_re_metadata((), metadata, self.runtime_opts.use_fbcode_metadata)
            .into_parts()
            .0;
        async move {
            upload_impl(
                &self.instance_name,
                request,
                self.bystream_compressor,
                self.capabilities.max_total_batch_size,
                self.runtime_opts.max_concurrent_uploads_per_action,
                |re_request| {
                    let headers = headers.clone();
                    async move {
                        let resp = self
                            .cas_client()
                            .await?
                            .batch_update_blobs(tonic::Request::from_parts(
                                headers,
                                Default::default(),
                                re_request,
                            ))
                            .await?;
                        Ok(resp.into_inner())
                    }
                },
                |segments| {
                    let headers = headers.clone();
                    async move {
                        let resp = self
                            .bytestream_client()
                            .await?
                            .write(tonic::Request::from_parts(
                                headers,
                                Default::default(),
                                futures::stream::iter(segments),
                            ))
                            .await?;
                        Ok(resp.into_inner())
                    }
                },
            )
            .await
        }
    }

    pub fn upload_blob_with_digest<'a>(
        &'a self,
        blob: Vec<u8>,
        digest: TDigest,
        metadata: &RemoteExecutionMetadata,
    ) -> impl Future<Output = anyhow::Result<TDigest>> + Send + use<'a> {
        let blob = InlinedBlobWithDigest {
            digest: digest.clone(),
            blob,
            ..Default::default()
        };
        let upload = self.upload(
            metadata,
            UploadRequest {
                inlined_blobs_with_digest: Some(vec![blob]),
                files_with_digest: None,
                directories: None,
                upload_only_missing: false,
                ..Default::default()
            },
        );
        async move {
            upload.await?;
            Ok(digest)
        }
    }

    pub fn download<'a>(
        &'a self,
        metadata: &RemoteExecutionMetadata,
        request: DownloadRequest,
    ) -> impl Future<Output = anyhow::Result<DownloadResponse>> + Send + use<'a> {
        let headers = with_re_metadata((), metadata, self.runtime_opts.use_fbcode_metadata)
            .into_parts()
            .0;
        async move {
            download_impl(
                &self.runtime_opts,
                &self.instance_name,
                request,
                self.bystream_compressor,
                self.capabilities.max_total_batch_size,
                |re_request| {
                    let headers = headers.clone();
                    async move {
                        let resp = self
                            .cas_client()
                            .await?
                            .batch_read_blobs(tonic::Request::from_parts(
                                headers,
                                Default::default(),
                                re_request,
                            ))
                            .await?;
                        Ok(resp.into_inner())
                    }
                },
                |read_request| {
                    let headers = headers.clone();
                    async move {
                        let response = self
                            .bytestream_client()
                            .await?
                            .read(tonic::Request::from_parts(
                                headers,
                                Default::default(),
                                read_request,
                            ))
                            .await?
                            .into_inner();
                        Ok(Box::pin(response.into_stream()))
                    }
                },
            )
            .await
        }
    }

    pub async fn get_digests_ttl(
        &self,
        metadata: &RemoteExecutionMetadata,
        request: GetDigestsTtlRequest,
    ) -> anyhow::Result<GetDigestsTtlResponse> {
        let mut remote_results: HashMap<TDigest, DigestRemoteState> = HashMap::new();
        let mut digests_to_check: Vec<TDigest> = Vec::new();

        let batch_size = self.runtime_opts.find_missing_blobs_batch_size;
        let mut digest_iter = request.digests.iter();
        while digest_iter.len() > 0 {
            // Sort our blobs based on what action we need to take
            {
                let mut find_missing_cache = self.find_missing_cache.lock().unwrap();
                for digest in digest_iter.by_ref() {
                    if let Some(rs) = find_missing_cache.get(digest) {
                        // We have our final result already cached
                        remote_results.insert(digest.clone(), rs);
                    } else {
                        // We can check this blob
                        digests_to_check.push(digest.clone());
                    }
                    if digests_to_check.len() >= batch_size {
                        break;
                    }
                }
            }

            // Send a request and notify others of the result
            if !digests_to_check.is_empty() {
                tracing::debug!(num_digests = digests_to_check.len(), "FindMissingBlobs");
                let blob_digests: Vec<_> =
                    digests_to_check.iter().map(tdigest_to).collect::<Vec<_>>();
                let resp: FindMissingBlobsResponse = retry(|| async {
                    let resp = self
                        .cas_client()
                        .await?
                        .find_missing_blobs(with_re_metadata(
                            FindMissingBlobsRequest {
                                instance_name: self.instance_name.as_str().to_owned(),
                                blob_digests: blob_digests.clone(),
                                ..Default::default()
                            },
                            metadata,
                            self.runtime_opts.use_fbcode_metadata,
                        ))
                        .await
                        .context("Failed to request what blobs are not present on remote")?;
                    Ok(resp.into_inner())
                })
                .await?;

                // Build the set of missing digests first, so we don't
                // prematurely cache them as ExistsOnRemote.
                let missing: HashSet<TDigest> = resp
                    .missing_blob_digests
                    .iter()
                    .map(|d| tdigest_from(d.clone()))
                    .collect();

                let mut find_missing_cache = self.find_missing_cache.lock().unwrap();
                for digest in &digests_to_check {
                    if missing.contains(digest) {
                        // Missing is a transient state: the blob may be uploaded
                        // or produced by an RE action at any time.  Do NOT cache
                        // it -- a stale "Missing" entry would prevent future
                        // FindMissingBlobs RPCs, and a premature "ExistsOnRemote"
                        // entry would cause concurrent actions to skip uploading
                        // a blob that hasn't been uploaded yet.
                        remote_results.insert(digest.clone(), DigestRemoteState::Missing);
                    } else {
                        remote_results.insert(digest.clone(), DigestRemoteState::ExistsOnRemote);
                        find_missing_cache.put(digest.clone(), DigestRemoteState::ExistsOnRemote);
                    }
                }
                digests_to_check.clear();
            }
        }

        Ok(GetDigestsTtlResponse {
            digests_with_ttl: remote_results
                .iter()
                .map(|(digest, rs)| match rs {
                    DigestRemoteState::Missing => DigestWithTtl {
                        digest: digest.clone(),
                        ttl: 0,
                    },
                    DigestRemoteState::ExistsOnRemote => DigestWithTtl {
                        digest: digest.clone(),
                        ttl: self.runtime_opts.cas_ttl_secs,
                    },
                })
                .collect::<Vec<DigestWithTtl>>(),
        })
    }

    pub async fn extend_digest_ttl(
        &self,
        _metadata: &RemoteExecutionMetadata,
        _request: ExtendDigestsTtlRequest,
    ) -> anyhow::Result<TDigest> {
        // TODO(arr)
        Err(anyhow::anyhow!("Not implemented (RE extend_digest_ttl)"))
    }

    pub fn get_execution_client(&self) -> &Self {
        self
    }

    pub fn get_cas_client(&self) -> &Self {
        self
    }

    pub fn get_action_cache_client(&self) -> &Self {
        self
    }

    async fn cas_client(&self) -> anyhow::Result<ContentAddressableStorageClient<GrpcService>> {
        let channel = self.pool.get(&self.cas_address).await?;
        Ok(
            ContentAddressableStorageClient::new(InterceptedService::new(
                channel,
                self.interceptor.dupe(),
            ))
            .max_decoding_message_size(self.max_decoding_msg_size),
        )
    }

    async fn bytestream_client(&self) -> anyhow::Result<ByteStreamClient<GrpcService>> {
        let channel = self.pool.get(&self.cas_address).await?;
        Ok(
            ByteStreamClient::new(InterceptedService::new(channel, self.interceptor.dupe()))
                .max_decoding_message_size(self.max_decoding_msg_size),
        )
    }

    async fn action_cache_client(&self) -> anyhow::Result<ActionCacheClient<GrpcService>> {
        let channel = self.pool.get(&self.action_cache_address).await?;
        Ok(ActionCacheClient::new(InterceptedService::new(
            channel,
            self.interceptor.dupe(),
        )))
    }

    pub fn get_metrics_client(&self) -> &Self {
        self
    }

    pub fn get_session_id(&self) -> &str {
        // TODO(aloiscochard): Return a unique ID, ideally from the GRPC client
        "GRPC-SESSION-ID"
    }

    pub fn get_experiment_name(&self) -> anyhow::Result<Option<String>> {
        Ok(None)
    }

    pub fn release_temporary_memory(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn convert_action_result(
    action_result: ActionResult,
    cas_ttl_secs: i64,
) -> anyhow::Result<TActionResult2> {
    let execution_metadata = action_result
        .execution_metadata
        .with_context(|| "The execution metadata are not defined.")?;

    let output_files = action_result
        .output_files
        .into_iter()
        .map(|output_file| {
            let output_file_digest = output_file.digest.with_context(|| "Digest not found.")?;

            anyhow::Ok(TFile {
                digest: DigestWithStatus {
                    status: tstatus_ok(),
                    digest: tdigest_from(output_file_digest),
                    _dot_dot_default: (),
                },
                name: output_file.path,
                existed: false,
                executable: output_file.is_executable,
                ttl: cas_ttl_secs,
                _dot_dot_default: (),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let output_symlinks = action_result
        .output_symlinks
        .into_iter()
        .map(|output_symlink| {
            anyhow::Ok(TSymlink {
                name: output_symlink.path,
                target: output_symlink.target,
                _dot_dot_default: (),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let output_directories = action_result
        .output_directories
        .into_iter()
        .map(|output_directory| {
            let digest = tdigest_from(
                output_directory
                    .tree_digest
                    .with_context(|| "Tree digest not defined.")?,
            );
            anyhow::Ok(TDirectory2 {
                path: output_directory.path,
                tree_digest: digest.clone(),
                root_directory_digest: digest,
                _dot_dot_default: (),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let action_result = TActionResult2 {
        output_files,
        output_symlinks,
        output_directories,
        exit_code: action_result.exit_code,
        stdout_raw: Some(action_result.stdout_raw),
        stdout_digest: action_result.stdout_digest.map(tdigest_from),
        stderr_raw: Some(action_result.stderr_raw),
        stderr_digest: action_result.stderr_digest.map(tdigest_from),

        execution_metadata: Box::new(TExecutedActionMetadata {
            worker: execution_metadata.worker,
            queued_timestamp: ttimestamp_from(execution_metadata.queued_timestamp),
            worker_start_timestamp: ttimestamp_from(execution_metadata.worker_start_timestamp),
            worker_completed_timestamp: ttimestamp_from(
                execution_metadata.worker_completed_timestamp,
            ),
            input_fetch_start_timestamp: ttimestamp_from(
                execution_metadata.input_fetch_start_timestamp,
            ),
            input_fetch_completed_timestamp: ttimestamp_from(
                execution_metadata.input_fetch_completed_timestamp,
            ),
            execution_start_timestamp: ttimestamp_from(
                execution_metadata.execution_start_timestamp,
            ),
            execution_completed_timestamp: ttimestamp_from(
                execution_metadata.execution_completed_timestamp,
            ),
            output_upload_start_timestamp: ttimestamp_from(
                execution_metadata.output_upload_start_timestamp,
            ),
            output_upload_completed_timestamp: ttimestamp_from(
                execution_metadata.output_upload_completed_timestamp,
            ),
            auxiliary_metadata: execution_metadata
                .auxiliary_metadata
                .into_iter()
                .map(tany_from)
                .collect::<Vec<_>>(),
            input_analyzing_start_timestamp: Default::default(),
            input_analyzing_completed_timestamp: Default::default(),
            execution_dir: "".to_owned(),
            execution_attempts: 0,
            last_queued_timestamp: Default::default(),
            ..Default::default()
        }),
        ..Default::default()
    };

    Ok(action_result)
}

fn convert_t_action_result2(t_action_result: &TActionResult2) -> anyhow::Result<ActionResult> {
    let t_execution_metadata = &t_action_result.execution_metadata;
    let virtual_execution_duration = prost_types::Duration::try_from(
        t_execution_metadata
            .execution_completed_timestamp
            .saturating_duration_since(&t_execution_metadata.execution_start_timestamp),
    )?;
    let execution_metadata = Some(ExecutedActionMetadata {
        worker: t_execution_metadata.worker.clone(),
        queued_timestamp: Some(ttimestamp_to(&t_execution_metadata.queued_timestamp)),
        worker_start_timestamp: Some(ttimestamp_to(&t_execution_metadata.worker_start_timestamp)),
        worker_completed_timestamp: Some(ttimestamp_to(
            &t_execution_metadata.worker_completed_timestamp,
        )),
        input_fetch_start_timestamp: Some(ttimestamp_to(
            &t_execution_metadata.input_fetch_start_timestamp,
        )),
        input_fetch_completed_timestamp: Some(ttimestamp_to(
            &t_execution_metadata.input_fetch_completed_timestamp,
        )),
        execution_start_timestamp: Some(ttimestamp_to(
            &t_execution_metadata.execution_start_timestamp,
        )),
        execution_completed_timestamp: Some(ttimestamp_to(
            &t_execution_metadata.execution_completed_timestamp,
        )),
        virtual_execution_duration: Some(virtual_execution_duration),
        output_upload_start_timestamp: Some(ttimestamp_to(
            &t_execution_metadata.output_upload_start_timestamp,
        )),
        output_upload_completed_timestamp: Some(ttimestamp_to(
            &t_execution_metadata.output_upload_completed_timestamp,
        )),
        auxiliary_metadata: t_execution_metadata
            .auxiliary_metadata
            .iter()
            .map(tany_to)
            .collect::<Vec<_>>(),
    });

    let output_files = t_action_result
        .output_files
        .iter()
        .map(|output_file| OutputFile {
            path: output_file.name.clone(),
            digest: Some(tdigest_to(&output_file.digest.digest)),
            is_executable: output_file.executable,
            contents: Vec::new(),
            node_properties: None,
        })
        .collect::<Vec<_>>();

    let output_symlinks = t_action_result
        .output_symlinks
        .iter()
        .map(|output_symlink| OutputSymlink {
            path: output_symlink.name.clone(),
            target: output_symlink.target.clone(),
            node_properties: None,
        })
        .collect::<Vec<_>>();

    let output_directories = t_action_result
        .output_directories
        .iter()
        .map(|output_directory| {
            let digest = tdigest_to(&output_directory.tree_digest);
            OutputDirectory {
                path: output_directory.path.clone(),
                tree_digest: Some(digest.clone()),
                is_topologically_sorted: false,
                root_directory_digest: None,
            }
        })
        .collect::<Vec<_>>();

    let action_result = ActionResult {
        output_files,
        output_symlinks,
        output_directories,
        exit_code: t_action_result.exit_code,
        stdout_raw: Vec::new(),
        stdout_digest: t_action_result.stdout_digest.as_ref().map(tdigest_to),
        stderr_raw: Vec::new(),
        stderr_digest: t_action_result.stderr_digest.as_ref().map(tdigest_to),
        execution_metadata,
        ..Default::default()
    };

    Ok(action_result)
}

async fn download_impl<Byt, BytRet, Cas>(
    opts: &RERuntimeOpts,
    instance_name: &InstanceName,
    request: DownloadRequest,
    bystream_compressor: Option<Compressor>,
    max_total_batch_size: usize,
    cas_f: impl Fn(BatchReadBlobsRequest) -> Cas,
    bystream_fut: impl Fn(ReadRequest) -> Byt + Sync + Send + Copy,
) -> anyhow::Result<DownloadResponse>
where
    Byt: Future<Output = anyhow::Result<Pin<Box<BytRet>>>>,
    BytRet: Stream<Item = Result<ReadResponse, tonic::Status>> + Send,
    Cas: Future<Output = anyhow::Result<BatchReadBlobsResponse>>,
{
    fn resource_name(
        instance_name: &InstanceName,
        compressor: Option<Compressor>,
        digest: &TDigest,
    ) -> String {
        if let Some(compressor) = compressor {
            format!(
                "{}compressed-blobs/{}/{}/{}",
                instance_name.as_resource_prefix(),
                compressor.name(),
                digest.hash,
                digest.size_in_bytes,
            )
        } else {
            format!(
                "{}blobs/{}/{}",
                instance_name.as_resource_prefix(),
                digest.hash,
                digest.size_in_bytes,
            )
        }
    }

    let bystream_fut = |digest: TDigest| async move {
        let resource_name = resource_name(instance_name, bystream_compressor, &digest);

        bystream_fut(ReadRequest {
            resource_name: resource_name.clone(),
            read_offset: 0,
            read_limit: 0,
        })
        .await
        // adapt the tokio Stream of ReadResponse into a StreamReader
        .map(|p| {
            let blob_reader = StreamReader::new(
                p.map(|r| r.map(|rr| Cursor::new(rr.data)).map_err(io::Error::other)),
            );
            let reader: Pin<Box<dyn AsyncRead + Unpin + Send>> = match bystream_compressor {
                None => Pin::new(Box::new(blob_reader)),
                Some(Compressor::Zstd) => {
                    let mut decoder = ZstdDecoder::new(blob_reader);
                    decoder.multiple_members(true);
                    Pin::new(Box::new(decoder))
                }
                Some(Compressor::Deflate) => {
                    let mut decoder = DeflateDecoder::new(blob_reader);
                    decoder.multiple_members(true);
                    Pin::new(Box::new(decoder))
                }
                Some(Compressor::Brotli) => {
                    let mut decoder = BrotliDecoder::new(blob_reader);
                    decoder.multiple_members(true);
                    Pin::new(Box::new(decoder))
                }
            };

            reader
        })
        .with_context(|| format!("Failed to read {resource_name} from Bytestream service"))
    };

    let inlined_digests = request.inlined_digests.unwrap_or_default();
    let file_digests = request.file_digests.unwrap_or_default();

    // Advertise zstd for BatchReadBlobs responses when the server supports it; Identity is always
    // acceptable as a fallback and the server decides per-blob what to actually return.
    let acceptable_compressors = if opts.batch_read_zstd {
        vec![
            compressor::Value::Zstd as i32,
            compressor::Value::Identity as i32,
        ]
    } else {
        vec![compressor::Value::Identity as i32]
    };

    let mut curr_size = 0;
    let mut requests = vec![];
    let mut curr_digests = vec![];
    for digest in file_digests
        .iter()
        .map(|req| &req.named_digest.digest)
        .chain(inlined_digests.iter())
        .map(tdigest_to)
        .filter(|d| d.size_bytes > 0)
    {
        if digest.size_bytes as usize >= max_total_batch_size {
            // digest is too big to download in a BatchReadBlobsRequest
            // need to use the bytstream api
            continue;
        }
        curr_size += digest.size_bytes;
        if curr_size >= max_total_batch_size as i64 {
            let read_blob_req = BatchReadBlobsRequest {
                instance_name: instance_name.as_str().to_owned(),
                digests: std::mem::take(&mut curr_digests),
                acceptable_compressors: acceptable_compressors.clone(),
                ..Default::default()
            };
            requests.push(read_blob_req);
            curr_size = digest.size_bytes;
        }
        curr_digests.push(digest.clone());
    }

    if !curr_digests.is_empty() {
        let read_blob_req = BatchReadBlobsRequest {
            instance_name: instance_name.as_str().to_owned(),
            digests: std::mem::take(&mut curr_digests),
            acceptable_compressors: acceptable_compressors.clone(),
            ..Default::default()
        };
        requests.push(read_blob_req);
    }

    let mut batched_blobs_response = HashMap::new();
    for read_blob_req in requests {
        let resp = retry(|| async {
            cas_f(read_blob_req.clone())
                .await
                .context("Failed to make BatchReadBlobs request")
        })
        .await?;
        for r in resp.responses.into_iter() {
            let digest = tdigest_from(r.digest.context("Response digest not found.")?);
            check_status(r.status.unwrap_or_default())?;
            let data = if r.compressor == compressor::Value::Identity as i32 {
                r.data
            } else if r.compressor == compressor::Value::Zstd as i32 {
                zstd_decompress_blob(&r.data)
                    .await
                    .context("Failed to decompress zstd BatchReadBlobs response")?
            } else {
                return Err(anyhow::anyhow!(
                    "BatchReadBlobs response for `{}` used an unsupported compressor: {}",
                    digest,
                    r.compressor
                ));
            };
            batched_blobs_response.insert(digest, data);
        }
    }

    let get = |digest: &TDigest| -> anyhow::Result<Vec<u8>> {
        if digest.size_in_bytes == 0 {
            return Ok(Vec::new());
        }

        Ok(batched_blobs_response
            .get(digest)
            .with_context(|| format!("Did not receive digest data for `{digest}`"))?
            .clone())
    };

    let mut inlined_blobs = vec![];
    for digest in inlined_digests {
        let data = if digest.size_in_bytes as usize >= max_total_batch_size {
            retry(|| async {
                let mut accum = vec![];
                let mut reader = bystream_fut(digest.clone()).await?;
                tokio::io::copy(&mut reader, &mut accum).await?;
                Ok(accum)
            })
            .await?
        } else {
            get(&digest)?
        };
        inlined_blobs.push(InlinedDigestWithStatus {
            digest,
            status: tstatus_ok(),
            blob: data,
        })
    }

    let writes = file_digests.iter().map(|req| async {
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            if req.is_executable {
                opts.mode(0o755);
            } else {
                opts.mode(0o644);
            }
        }

        retry(|| async {
            let mut file = opts
                .open(&req.named_digest.name)
                .await
                .context("Error opening")?;

            // If the data is small enough to be transferred in a batch
            // blob update, write it all at once to the file. Otherwise, it'll
            // be streamed in chunks as the remote responds.
            if req.named_digest.digest.size_in_bytes < max_total_batch_size as i64 {
                let data = get(&req.named_digest.digest)?;
                file.write_all(&data)
                    .await
                    .with_context(|| format!("Error writing: {}", req.named_digest.digest))?;
            } else {
                let mut reader = bystream_fut(req.named_digest.digest.clone()).await?;
                tokio::io::copy(&mut reader, &mut file)
                    .await
                    .with_context(|| {
                        format!("Error writing chunk of: {}", req.named_digest.digest)
                    })?;
            }
            file.flush().await.context("Error flushing")?;
            anyhow::Ok(())
        })
        .await
        .with_context(|| {
            format!(
                "Error downloading digest `{}` to `{}`",
                req.named_digest.digest, req.named_digest.name,
            )
        })
    });

    buck2_util::future::try_join_all(writes).await?;

    Ok(DownloadResponse {
        inlined_blobs: Some(inlined_blobs),
        directories: None,
        local_cache_stats: Default::default(),
    })
}

async fn upload_impl<Byt, Cas>(
    instance_name: &InstanceName,
    request: UploadRequest,
    bystream_compressor: Option<Compressor>,
    max_total_batch_size: usize,
    max_concurrent_uploads: Option<usize>,
    cas_f: impl Fn(BatchUpdateBlobsRequest) -> Cas + Sync + Send + Copy,
    bystream_fut: impl Fn(Vec<WriteRequest>) -> Byt + Sync + Send + Copy,
) -> anyhow::Result<UploadResponse>
where
    Cas: Future<Output = anyhow::Result<BatchUpdateBlobsResponse>> + Send,
    Byt: Future<Output = anyhow::Result<WriteResponse>> + Send,
{
    fn resource_name(
        instance_name: &InstanceName,
        client_uuid: &str,
        compressor: Option<Compressor>,
        digest: &TDigest,
    ) -> String {
        if let Some(compressor) = compressor {
            format!(
                "{}uploads/{}/compressed-blobs/{}/{}/{}",
                instance_name.as_resource_prefix(),
                client_uuid,
                compressor.name(),
                digest.hash,
                digest.size_in_bytes,
            )
        } else {
            format!(
                "{}uploads/{}/blobs/{}/{}",
                instance_name.as_resource_prefix(),
                client_uuid,
                digest.hash,
                digest.size_in_bytes,
            )
        }
    }

    // NOTE if we stop recording blob_hashes, we can drop out a lot of allocations.
    let mut upload_futures: Vec<BoxFuture<anyhow::Result<Vec<String>>>> = vec![];

    // For small file uploads the client should group them together and call `BatchUpdateBlobs`
    // https://github.com/bazelbuild/remote-apis/blob/main/build/bazel/remote/execution/v2/remote_execution.proto#L205
    let mut batched_blob_updates = BatchUploadReqAggregator::new(max_total_batch_size);

    // Adapt the given bystream_fut to take in an AsyncBufRead
    let bystream_fut = |resource_name: String, reader: Box<dyn AsyncBufRead + Unpin + Send>| async move {
        let mut reader: Pin<Box<dyn AsyncRead + Unpin + Send>> = match bystream_compressor {
            None => Pin::new(Box::new(reader)),
            Some(Compressor::Zstd) => Pin::new(Box::new(ZstdEncoder::new(reader))),
            Some(Compressor::Deflate) => Pin::new(Box::new(DeflateEncoder::new(reader))),
            Some(Compressor::Brotli) => Pin::new(Box::new(BrotliEncoder::new(reader))),
        };

        let mut current_offset = 0;
        let mut upload_segments = Vec::new();
        let mut buf = vec![0; max_total_batch_size];
        loop {
            let n_read = reader.read(&mut buf).await?;
            if n_read == 0 {
                break;
            }
            upload_segments.push(WriteRequest {
                resource_name: resource_name.clone(),
                write_offset: current_offset,
                finish_write: false,
                data: buf[0..n_read].to_vec(),
            });
            current_offset += n_read as i64;
        }
        if let Some(last_segment) = upload_segments.last_mut() {
            last_segment.finish_write = true;
        }

        if upload_segments.is_empty() {
            // As an optimization, we can silently skip uploading empty blobs
            return Ok(());
        }

        let response = bystream_fut(upload_segments).await?;
        if response.committed_size != current_offset && response.committed_size != -1 {
            return Err(anyhow::anyhow!(
                "Failed to upload `{resource_name}`: invalid committed_size from WriteResponse"
            ));
        }

        Ok(())
    };

    // Create futures for any blobs that need uploading.
    for blob in request.inlined_blobs_with_digest.unwrap_or_default() {
        let hash = blob.digest.hash.clone();
        let size = blob.digest.size_in_bytes;

        if size < max_total_batch_size as i64 {
            batched_blob_updates.push(BatchUploadRequest::Blob(blob));
            continue;
        }

        let data = prost::bytes::Bytes::from(blob.blob);
        let client_uuid = uuid::Uuid::new_v4().to_string();
        let resource_name = resource_name(
            instance_name,
            &client_uuid,
            bystream_compressor,
            &blob.digest,
        );
        let fut = async move {
            retry(|| async {
                bystream_fut(resource_name.clone(), Box::new(Cursor::new(data.clone()))).await?;
                Ok(vec![hash.clone()])
            })
            .await
        };
        upload_futures.push(Box::pin(fut));
    }

    // Create futures for any files that needs uploading.
    for file in request.files_with_digest.unwrap_or_default() {
        let hash = file.digest.hash.clone();
        let size = file.digest.size_in_bytes;
        let name = file.name.clone();
        if size < max_total_batch_size as i64 {
            batched_blob_updates.push(BatchUploadRequest::File(file));
            continue;
        }
        let client_uuid = uuid::Uuid::new_v4().to_string();
        let resource_name = resource_name(
            instance_name,
            &client_uuid,
            bystream_compressor,
            &file.digest,
        );

        let fut = async move {
            retry(|| async {
                let file = tokio::fs::File::open(&name)
                    .await
                    .with_context(|| format!("Opening `{name}` for reading failed"))?;

                bystream_fut(resource_name.clone(), Box::new(BufReader::new(file))).await?;
                Ok(vec![hash.clone()])
            })
            .await
        };
        upload_futures.push(Box::pin(fut));
    }

    // Create futures for any files small enough that they
    // should be uploaded in batches.
    let batched_blob_updates = batched_blob_updates.done();
    for batch in batched_blob_updates {
        let fut = async move {
            let mut re_request = BatchUpdateBlobsRequest {
                instance_name: instance_name.as_str().to_owned(),
                requests: vec![],
                ..Default::default()
            };
            for blob in batch {
                match blob {
                    BatchUploadRequest::Blob(blob) => {
                        re_request.requests.push(Request {
                            digest: Some(tdigest_to(&blob.digest)),
                            data: blob.blob.clone(),
                            compressor: compressor::Value::Identity as i32,
                        });
                    }
                    BatchUploadRequest::File(file) => {
                        // These should be small files, so no need to use a buffered reader.
                        let mut fin = tokio::fs::File::open(&file.name)
                            .await
                            .with_context(|| format!("Opening {} for reading failed", file.name))?;
                        let mut data = vec![];
                        fin.read_to_end(&mut data).await?;

                        re_request.requests.push(Request {
                            digest: Some(tdigest_to(&file.digest)),
                            data,
                            compressor: compressor::Value::Identity as i32,
                        });
                    }
                }
            }
            let blob_hashes = re_request
                .requests
                .iter()
                .map(|x| x.digest.as_ref().unwrap().hash.clone())
                .collect::<Vec<String>>();

            let response = retry(|| async { cas_f(re_request.clone()).await }).await?;
            let failures: Vec<String> = response
                .responses
                .iter()
                .filter_map(|r| {
                    r.status.as_ref().and_then(|s| {
                        if s.code == (Code::Ok as i32) {
                            None
                        } else {
                            Some(format!(
                                "Unable to upload blob '{}', rpc status code: {}, message: \"{}\"",
                                r.digest.as_ref().map_or("N/A", |d| &d.hash),
                                s.code,
                                s.message
                            ))
                        }
                    })
                })
                .collect();

            if !failures.is_empty() {
                return Err(anyhow::anyhow!("Batch upload failed: {:?}", failures));
            }
            Ok(blob_hashes)
        };
        upload_futures.push(Box::pin(fut));
    }

    let blob_hashes = if let Some(concurrency_limit) = max_concurrent_uploads {
        futures::stream::iter(upload_futures)
            .buffer_unordered(concurrency_limit)
            .try_collect::<Vec<Vec<String>>>()
            .await?
    } else {
        futures::future::try_join_all(upload_futures).await?
    };

    tracing::debug!("uploaded: {:?}", blob_hashes);
    Ok(UploadResponse {})
}

fn with_re_metadata<T>(
    t: T,
    metadata: &RemoteExecutionMetadata,
    use_fbcode_metadata: bool,
) -> tonic::Request<T> {
    // This creates a new Tonic request with attached metadata for the RE
    // backend. There are two cases here we need to support:
    //
    //   - Servers that abide by the remote execution apis defined with Bazel,
    //     AKA the "OSS RE API", which this package implements
    //   - The internal RE solution used at Meta, which uses a different API,
    //     but is compatible with the OSS RE API to some extent.
    //
    // The second case is supported only through attaching some metadata to the
    // request, which the fbcode RE service understands; and the reason for all
    // of this is that it allows this OSS client package to be tested inside of
    // fbcode builds within Meta. So there doesn't need to be a separate CI
    // check.
    //
    // However, we don't need it for FOSS builds of Buck2. And in theory we
    // could test the OSS Bazel API in the upstream GitHub CI, but doing it this
    // way is only a little ugly, it's hidden, and it helps ensure the internal
    // Meta builds catch those issues earlier.

    let mut msg = tonic::Request::new(t);

    if use_fbcode_metadata {
        // This is pretty ugly, but the protobuf spec that defines this is
        // internal, so considering field numbers need to be stable anyway (=
        // low risk), and this is not used in prod (= low impact if this goes
        // wrong), we just inline it here. This is a small hack that lets us use
        // our internal RE using this GRPC client for testing.
        //
        // This is defined in `fbcode/remote_execution/re_cas_common/grpc/proto/metadata.proto`.
        #[derive(prost::Message)]
        struct Metadata {
            #[prost(message, optional, tag = "15")]
            platform: Option<crate::grpc::Platform>,
            #[prost(string, optional, tag = "18")]
            use_case_id: Option<String>,
        }

        let mut encoded = Vec::new();
        Metadata {
            platform: metadata.platform.clone(),
            use_case_id: Some(metadata.use_case_id.clone()),
        }
        .encode(&mut encoded)
        .expect("Encoding into a Vec cannot not fail");

        msg.metadata_mut()
            .insert_bin("re-metadata-bin", MetadataValue::from_bytes(&encoded));
    } else {
        let mut encoded = Vec::new();
        let RemoteExecutionMetadata {
            correlated_invocations_id,
            buck_info,
            action_id,
            action_mnemonic,
            target_id,
            configuration_id,
            ..
        } = metadata.clone();

        let correlated_invocations_id = correlated_invocations_id
            .filter(|id| !id.is_empty())
            .or_else(|| {
                buck_info
                    .as_ref()
                    .map(|b| b.build_id.clone())
                    .filter(|id| !id.is_empty())
            })
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let (tool_invocation_id, tool_version) = match buck_info {
            Some(buck_info) => {
                let tool_version = if buck_info.version.is_empty() {
                    "dev".to_owned()
                } else {
                    buck_info.version
                };
                (buck_info.build_id, tool_version)
            }
            None => (String::new(), "dev".to_owned()),
        };

        RequestMetadata {
            tool_details: Some(ToolDetails {
                tool_name: "buck2".to_owned(),
                tool_version,
            }),
            action_id: action_id.unwrap_or_default(),
            tool_invocation_id,
            correlated_invocations_id,
            action_mnemonic: action_mnemonic.unwrap_or_default(),
            target_id: target_id.unwrap_or_default(),
            configuration_id: configuration_id.unwrap_or_default(),
        }
        .encode(&mut encoded)
        .expect("Encoding into a Vec cannot not fail");

        msg.metadata_mut().insert_bin(
            "build.bazel.remote.execution.v2.requestmetadata-bin",
            MetadataValue::from_bytes(&encoded),
        );
    };
    msg
}

/// Replace occurrences of $FOO in a string with the value of the env var $FOO.
fn substitute_env_vars(s: &str) -> anyhow::Result<String> {
    substitute_env_vars_impl(s, |v| std::env::var(v))
}

fn substitute_env_vars_impl(
    s: &str,
    getter: impl Fn(&str) -> Result<String, VarError>,
) -> anyhow::Result<String> {
    static ENV_REGEX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new("\\$[a-zA-Z_][a-zA-Z_0-9]*").unwrap());

    let mut out = String::with_capacity(s.len());
    let mut last_idx = 0;

    for mat in ENV_REGEX.find_iter(s) {
        out.push_str(&s[last_idx..mat.start()]);
        let var = &mat.as_str()[1..];
        let val = getter(var).with_context(|| format!("Error substituting `{}`", mat.as_str()))?;
        out.push_str(&val);
        last_idx = mat.end();
    }

    if last_idx < s.len() {
        out.push_str(&s[last_idx..s.len()]);
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::Ordering;
    use std::collections::VecDeque;
    use std::sync::atomic::AtomicU16;

    use futures::FutureExt;
    use re_grpc_proto::build::bazel::remote::execution::v2::batch_read_blobs_response;
    use re_grpc_proto::build::bazel::remote::execution::v2::batch_update_blobs_response;

    use super::*;

    const OP: &str = "operations/action-1";

    type ScriptedItem = Result<Operation, tonic::Status>;

    /// One scripted call to the opener: either a stream of messages, or a failure
    /// of the call itself. The distinction matters — tonic reports a server that
    /// rejects a streaming RPC outright when the call is opened, not when the
    /// stream is first read.
    type ScriptedCall = Result<Vec<ScriptedItem>, tonic::Status>;

    fn progress_op(stage: execution_stage::Value) -> Operation {
        Operation {
            name: OP.to_owned(),
            metadata: Some(prost_types::Any {
                type_url: String::new(),
                value: ExecuteOperationMetadata {
                    stage: stage as i32,
                    ..Default::default()
                }
                .encode_to_vec(),
            }),
            ..Default::default()
        }
    }

    fn done_op() -> Operation {
        response_op(GExecuteResponse {
            result: Some(ActionResult {
                execution_metadata: Some(ExecutedActionMetadata::default()),
                ..Default::default()
            }),
            ..Default::default()
        })
    }

    /// A finished operation whose `ExecuteResponse.status` is `code`.
    fn failed_response_op(code: TCode, message: &str) -> Operation {
        response_op(GExecuteResponse {
            status: Some(Status {
                code: code.0,
                message: message.to_owned(),
                ..Default::default()
            }),
            result: Some(ActionResult {
                execution_metadata: Some(ExecutedActionMetadata::default()),
                ..Default::default()
            }),
            ..Default::default()
        })
    }

    /// A finished operation that carries `code` in `Operation.error` instead of a
    /// response.
    fn error_op(code: TCode, message: &str) -> Operation {
        Operation {
            name: OP.to_owned(),
            done: true,
            result: Some(OpResult::Error(Status {
                code: code.0,
                message: message.to_owned(),
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    fn response_op(response: GExecuteResponse) -> Operation {
        Operation {
            name: OP.to_owned(),
            done: true,
            result: Some(OpResult::Response(prost_types::Any {
                type_url: String::new(),
                value: response.encode_to_vec(),
            })),
            ..Default::default()
        }
    }

    /// Drives `execution_progress_stream` over one scripted stream per call to
    /// the opener, returning the events yielded and the `resume` argument each
    /// call was made with — `None` for `Execute`, `Some(name)` for
    /// `WaitExecution`.
    async fn run_scripted(
        attempts: Vec<ScriptedCall>,
    ) -> (
        Vec<anyhow::Result<ExecuteWithProgressResponse>>,
        Vec<Option<String>>,
    ) {
        let attempts = Arc::new(Mutex::new(VecDeque::from(attempts)));
        let calls = Arc::new(Mutex::new(Vec::new()));

        let open = {
            let attempts = attempts.dupe();
            let calls = calls.dupe();
            move |resume: Option<String>| {
                let attempts = attempts.dupe();
                let calls = calls.dupe();
                async move {
                    calls.lock().unwrap().push(resume);
                    let script = attempts
                        .lock()
                        .unwrap()
                        .pop_front()
                        .context("opener called more often than scripted")?;
                    anyhow::Ok(futures::stream::iter(script?).boxed())
                }
                .boxed()
            }
        };

        let events = execution_progress_stream(open, 0).collect().await;
        let calls = calls.lock().unwrap().clone();
        (events, calls)
    }

    fn stages(events: &[anyhow::Result<ExecuteWithProgressResponse>]) -> Vec<Stage> {
        events
            .iter()
            .map(|e| e.as_ref().unwrap().stage)
            .collect::<Vec<_>>()
    }

    #[tokio::test(start_paused = true)]
    async fn execution_resumes_a_named_operation_with_wait_execution() {
        let (events, calls) = run_scripted(vec![
            Ok(vec![
                Ok(progress_op(execution_stage::Value::Queued)),
                Err(tonic::Status::unavailable("connection reset")),
            ]),
            Ok(vec![Ok(done_op())]),
        ])
        .await;

        assert_eq!(calls, vec![None, Some(OP.to_owned())]);
        assert_eq!(stages(&events), vec![Stage::QUEUED, Stage::COMPLETED]);
    }

    #[tokio::test(start_paused = true)]
    async fn execution_restarts_when_it_fails_before_the_operation_is_named() {
        let (events, calls) = run_scripted(vec![
            Ok(vec![Err(tonic::Status::unavailable("no backends"))]),
            Ok(vec![Ok(done_op())]),
        ])
        .await;

        assert_eq!(calls, vec![None, None]);
        assert_eq!(stages(&events), vec![Stage::COMPLETED]);
    }

    #[tokio::test(start_paused = true)]
    async fn execution_retries_a_call_that_fails_when_opened() {
        let (events, calls) = run_scripted(vec![
            Err(tonic::Status::unavailable("no backends")),
            Ok(vec![Ok(done_op())]),
        ])
        .await;

        assert_eq!(calls, vec![None, None]);
        assert_eq!(stages(&events), vec![Stage::COMPLETED]);
    }

    /// The realistic shape: a server that no longer knows the operation rejects
    /// `WaitExecution` when the call is opened.
    #[tokio::test(start_paused = true)]
    async fn execution_restarts_when_resuming_is_rejected_as_not_found() {
        let (events, calls) = run_scripted(vec![
            Ok(vec![
                Ok(progress_op(execution_stage::Value::Executing)),
                Err(tonic::Status::unavailable("goaway")),
            ]),
            Err(tonic::Status::not_found("unknown operation")),
            Ok(vec![Ok(done_op())]),
        ])
        .await;

        assert_eq!(calls, vec![None, Some(OP.to_owned()), None]);
        assert_eq!(stages(&events), vec![Stage::EXECUTING, Stage::COMPLETED]);
    }

    /// A server without `WaitExecution` support has to fall back the same way.
    #[tokio::test(start_paused = true)]
    async fn execution_restarts_when_resuming_is_unimplemented() {
        let (events, calls) = run_scripted(vec![
            Ok(vec![
                Ok(progress_op(execution_stage::Value::Executing)),
                Err(tonic::Status::unavailable("goaway")),
            ]),
            Err(tonic::Status::unimplemented("WaitExecution")),
            Ok(vec![Ok(done_op())]),
        ])
        .await;

        assert_eq!(calls, vec![None, Some(OP.to_owned()), None]);
        assert_eq!(stages(&events), vec![Stage::EXECUTING, Stage::COMPLETED]);
    }

    #[tokio::test(start_paused = true)]
    async fn execution_restarts_when_the_server_lost_the_operation_mid_stream() {
        let (events, calls) = run_scripted(vec![
            Ok(vec![
                Ok(progress_op(execution_stage::Value::Executing)),
                Err(tonic::Status::unavailable("goaway")),
            ]),
            Ok(vec![Err(tonic::Status::not_found("unknown operation"))]),
            Ok(vec![Ok(done_op())]),
        ])
        .await;

        assert_eq!(calls, vec![None, Some(OP.to_owned()), None]);
        assert_eq!(stages(&events), vec![Stage::EXECUTING, Stage::COMPLETED]);
    }

    /// REAPI allows a stream to end without a response when execution outlives a
    /// connection, so an action can close and resume far more often than either
    /// limit allows — and still have its whole retry budget for a real failure.
    #[tokio::test(start_paused = true)]
    async fn execution_resumes_after_a_clean_close_without_spending_retries() {
        let closes = (MAX_EMPTY_RESUMES + MAX_EXECUTION_RETRIES) * 3;

        let mut attempts = Vec::new();
        for _ in 0..=closes {
            attempts.push(Ok(vec![Ok(progress_op(execution_stage::Value::Executing))]));
        }
        for _ in 0..MAX_EXECUTION_RETRIES {
            attempts.push(Ok(vec![Err(tonic::Status::unavailable(
                "connection reset",
            ))]));
        }
        attempts.push(Ok(vec![Ok(done_op())]));

        let (events, calls) = run_scripted(attempts).await;

        assert_eq!(calls.len() as u32, closes + MAX_EXECUTION_RETRIES + 2);
        assert!(calls[1..].iter().all(|c| c.as_deref() == Some(OP)));
        assert_eq!(
            events.last().unwrap().as_ref().unwrap().stage,
            Stage::COMPLETED,
            "the retry budget survived every resume"
        );
    }

    /// `WaitExecution` must report current status immediately, so a resume that
    /// reports nothing is a broken server and must not loop forever.
    #[tokio::test(start_paused = true)]
    async fn execution_gives_up_on_a_server_that_resumes_without_reporting() {
        let mut attempts = vec![Ok(vec![Ok(progress_op(execution_stage::Value::Executing))])];
        for _ in 0..(MAX_EMPTY_RESUMES + 1) {
            attempts.push(Ok(vec![]));
        }

        let (events, calls) = run_scripted(attempts).await;

        assert_eq!(calls.len() as u32, MAX_EMPTY_RESUMES + 1);
        let Err(err) = events.last().unwrap() else {
            panic!("expected the resume loop to give up")
        };
        assert!(format!("{err:#}").contains("without the server reporting anything"));
    }

    /// Permanent for a fresh `Execute` — a bad instance name, or an endpoint with
    /// no execution service — so it must not buy the whole retry budget.
    #[tokio::test(start_paused = true)]
    async fn execution_does_not_retry_not_found_from_a_fresh_execute() {
        let (events, calls) =
            run_scripted(vec![Err(tonic::Status::not_found("no such instance"))]).await;

        assert_eq!(calls, vec![None]);
        let Err(err) = &events[0] else {
            panic!("expected the status to be surfaced")
        };
        assert!(format!("{err:#}").contains("Failed to start remote execution"));
    }

    #[tokio::test(start_paused = true)]
    async fn execution_does_not_retry_unimplemented_from_a_fresh_execute() {
        let (_events, calls) = run_scripted(vec![Err(tonic::Status::unimplemented(
            "no Execution service",
        ))])
        .await;

        assert_eq!(calls, vec![None]);
    }

    #[tokio::test(start_paused = true)]
    async fn execution_fails_when_it_closes_before_naming_an_operation() {
        let (events, calls) = run_scripted(vec![Ok(vec![])]).await;

        assert_eq!(calls, vec![None]);
        let Err(err) = &events[0] else {
            panic!("expected a hard error")
        };
        assert!(format!("{err:#}").contains("terminated with no result"));
    }

    /// Progress is forwarded verbatim; keeping the caller's view consistent is
    /// the caller's job.
    #[tokio::test(start_paused = true)]
    async fn execution_forwards_replayed_stages() {
        let (events, _calls) = run_scripted(vec![
            Ok(vec![
                Ok(progress_op(execution_stage::Value::Queued)),
                Ok(progress_op(execution_stage::Value::Executing)),
                Err(tonic::Status::unavailable("goaway")),
            ]),
            Ok(vec![
                Ok(progress_op(execution_stage::Value::Queued)),
                Ok(progress_op(execution_stage::Value::Executing)),
                Ok(done_op()),
            ]),
        ])
        .await;

        assert_eq!(
            stages(&events),
            vec![
                Stage::QUEUED,
                Stage::EXECUTING,
                Stage::QUEUED,
                Stage::EXECUTING,
                Stage::COMPLETED
            ]
        );
    }

    /// A command that exits non-zero ran to completion: REAPI reports it with an
    /// `OK` status, and it reaches the caller as a response, not an error.
    #[tokio::test(start_paused = true)]
    async fn execution_does_not_retry_a_nonzero_exit_code() {
        let op = response_op(GExecuteResponse {
            result: Some(ActionResult {
                exit_code: 1,
                execution_metadata: Some(ExecutedActionMetadata::default()),
                ..Default::default()
            }),
            ..Default::default()
        });

        let (events, calls) = run_scripted(vec![Ok(vec![Ok(op)])]).await;

        assert_eq!(calls, vec![None]);
        let event = events[0]
            .as_ref()
            .expect("expected the response to be delivered");
        assert_eq!(
            event
                .execute_response
                .as_ref()
                .map(|r| r.action_result.exit_code),
            Some(1)
        );
    }

    /// The timeout is the action's own verdict, and running it again would only
    /// time out again, even though the server attached a result.
    #[tokio::test(start_paused = true)]
    async fn execution_does_not_retry_a_timed_out_action() {
        let op = failed_response_op(TCode::DEADLINE_EXCEEDED, "action timed out");

        let (events, calls) = run_scripted(vec![Ok(vec![Ok(op)])]).await;

        assert_eq!(calls, vec![None]);
        let Err(err) = &events[0] else {
            panic!("expected the timeout to be surfaced")
        };
        assert!(format!("{err:#}").contains("action timed out"));
    }

    #[tokio::test(start_paused = true)]
    async fn execution_does_not_retry_a_permanent_operation_error() {
        let op = error_op(TCode::INVALID_ARGUMENT, "operation rejected");

        let (events, calls) = run_scripted(vec![Ok(vec![Ok(op)])]).await;

        assert_eq!(calls, vec![None]);
        let Err(err) = &events[0] else {
            panic!("expected the operation error to be surfaced")
        };
        assert!(format!("{err:#}").contains("operation rejected"));
    }

    /// It can be the action asking for more than any worker offers, where running
    /// it again only adds load.
    #[tokio::test(start_paused = true)]
    async fn execution_does_not_retry_resource_exhaustion() {
        let op = failed_response_op(TCode::RESOURCE_EXHAUSTED, "out of quota");

        let (events, calls) = run_scripted(vec![Ok(vec![Ok(op)])]).await;

        assert_eq!(calls, vec![None]);
        let Err(err) = &events[0] else {
            panic!("expected the exhaustion to be surfaced")
        };
        assert!(format!("{err:#}").contains("out of quota"));
    }

    /// The operation is over, so resuming it with `WaitExecution` would only
    /// report the same failure.
    #[tokio::test(start_paused = true)]
    async fn execution_restarts_after_a_transient_operation_error() {
        let (events, calls) = run_scripted(vec![
            Ok(vec![Ok(error_op(TCode::UNAVAILABLE, "io exception"))]),
            Ok(vec![Ok(done_op())]),
        ])
        .await;

        assert_eq!(calls, vec![None, None]);
        assert_eq!(stages(&events), vec![Stage::COMPLETED]);
    }

    #[tokio::test(start_paused = true)]
    async fn execution_restarts_after_an_internal_action_status() {
        let (events, calls) = run_scripted(vec![
            Ok(vec![Ok(failed_response_op(TCode::INTERNAL, "worker lost"))]),
            Ok(vec![Ok(done_op())]),
        ])
        .await;

        assert_eq!(calls, vec![None, None]);
        assert_eq!(stages(&events), vec![Stage::COMPLETED]);
    }

    /// Every restart runs the action again, so restarts draw on the same budget as
    /// reconnecting does.
    #[tokio::test(start_paused = true)]
    async fn execution_stops_restarting_once_the_budget_is_spent() {
        let attempts = (0..=MAX_EXECUTION_RETRIES)
            .map(|_| Ok(vec![Ok(error_op(TCode::UNAVAILABLE, "io exception"))]))
            .collect();

        let (events, calls) = run_scripted(attempts).await;

        assert_eq!(calls.len() as u32, MAX_EXECUTION_RETRIES + 1);
        let Err(err) = &events[0] else {
            panic!("expected the last operation error to be surfaced")
        };
        assert_eq!(
            err.downcast_ref::<REClientError>().map(|e| e.code),
            Some(TCode::UNAVAILABLE)
        );
    }

    #[test]
    fn execution_restarts_only_on_transient_statuses() {
        let expected = [
            (TCode::CANCELLED, false),
            (TCode::UNKNOWN, true),
            (TCode::INVALID_ARGUMENT, false),
            (TCode::DEADLINE_EXCEEDED, false),
            (TCode::NOT_FOUND, false),
            (TCode::ALREADY_EXISTS, false),
            (TCode::PERMISSION_DENIED, false),
            (TCode::RESOURCE_EXHAUSTED, false),
            (TCode::FAILED_PRECONDITION, false),
            (TCode::ABORTED, true),
            (TCode::OUT_OF_RANGE, false),
            (TCode::UNIMPLEMENTED, false),
            (TCode::INTERNAL, true),
            (TCode::UNAVAILABLE, true),
            (TCode::DATA_LOSS, false),
            (TCode::UNAUTHENTICATED, false),
        ];

        let actual = expected.map(|(code, _)| (code, is_transient_execution_status(code)));

        assert_eq!(actual, expected);
    }

    /// Restarting would re-run an action that already used up its timeout, but
    /// resuming a named operation costs nothing.
    #[tokio::test(start_paused = true)]
    async fn execution_only_retries_deadline_exceeded_when_it_can_resume() {
        let (_events, calls) = run_scripted(vec![Ok(vec![Err(tonic::Status::deadline_exceeded(
            "too slow",
        ))])])
        .await;
        assert_eq!(calls, vec![None]);

        let (events, calls) = run_scripted(vec![
            Ok(vec![
                Ok(progress_op(execution_stage::Value::Executing)),
                Err(tonic::Status::deadline_exceeded("too slow")),
            ]),
            Ok(vec![Ok(done_op())]),
        ])
        .await;
        assert_eq!(calls, vec![None, Some(OP.to_owned())]);
        assert_eq!(stages(&events), vec![Stage::EXECUTING, Stage::COMPLETED]);
    }

    #[tokio::test(start_paused = true)]
    async fn execution_gives_up_once_the_budget_is_spent() {
        let attempts = (0..=MAX_EXECUTION_RETRIES)
            .map(|_| Ok(vec![Err(tonic::Status::unavailable("connection reset"))]))
            .collect();

        let (events, calls) = run_scripted(attempts).await;

        assert_eq!(calls.len() as u32, MAX_EXECUTION_RETRIES + 1);
        let Err(err) = &events[0] else {
            panic!("expected the last status to be surfaced")
        };
        assert!(err.chain().any(|c| {
            c.downcast_ref::<tonic::Status>()
                .is_some_and(|s| s.code() == tonic::Code::Unavailable)
        }));
    }

    /// Opening is deferred into the stream, so this is the only thing that still
    /// says a call never got off the ground.
    #[tokio::test(start_paused = true)]
    async fn execution_reports_a_call_that_never_opened() {
        let attempts = (0..=MAX_EXECUTION_RETRIES)
            .map(|_| Err(tonic::Status::unavailable("no backends")))
            .collect();

        let (events, calls) = run_scripted(attempts).await;

        assert_eq!(calls.len() as u32, MAX_EXECUTION_RETRIES + 1);
        let Err(err) = &events[0] else {
            panic!("expected the open failure to be surfaced")
        };
        assert!(format!("{err:#}").contains("Failed to start remote execution"));
    }

    #[tokio::test(start_paused = true)]
    async fn execution_stops_once_a_response_arrives() {
        let (events, calls) = run_scripted(vec![Ok(vec![Ok(done_op())])]).await;

        assert_eq!(calls, vec![None]);
        assert_eq!(stages(&events), vec![Stage::COMPLETED]);
    }

    /// Whatever the connection says after the response is moot — resuming would
    /// only fetch the same completed operation again.
    #[tokio::test(start_paused = true)]
    async fn execution_stops_even_if_the_stream_then_errors() {
        let (events, calls) = run_scripted(vec![
            Ok(vec![
                Ok(done_op()),
                Err(tonic::Status::unavailable("connection reset")),
            ]),
            Ok(vec![Ok(done_op())]),
        ])
        .await;

        assert_eq!(calls, vec![None]);
        assert_eq!(stages(&events), vec![Stage::COMPLETED]);
    }

    pub(super) fn test_re_runtime_opts() -> RERuntimeOpts {
        RERuntimeOpts {
            use_fbcode_metadata: false,
            max_concurrent_uploads_per_action: None,
            cas_ttl_secs: 0,
            find_missing_blobs_batch_size: 100,
            batch_read_zstd: false,
        }
    }

    #[tokio::test]
    async fn test_retry_unauthenticated() {
        let attempts = AtomicU16::new(0);
        let err = retry(|| async {
            attempts.fetch_add(1, Ordering::Relaxed);
            Err::<(), _>(
                anyhow::Error::new(tonic::Status::unauthenticated("no creds"))
                    .context("FindMissingBlobs failed"),
            )
        })
        .await
        .unwrap_err();

        assert_eq!(attempts.load(Ordering::Relaxed), 1);
        let re_error = err.downcast_ref::<REClientError>().unwrap();
        assert_eq!(re_error.code, TCode::UNAUTHENTICATED);
        assert_eq!(re_error.group, TCodeReasonGroup::UNKNOWN);
        assert_eq!(re_error.message, "no creds");
        assert!(err.downcast_ref::<tonic::Status>().is_some());
        assert!(format!("{err:#}").contains("FindMissingBlobs failed"));
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        let attempts = AtomicU16::new(0);
        let err = retry(|| async {
            attempts.fetch_add(1, Ordering::Relaxed);
            Err::<(), _>(anyhow::Error::new(tonic::Status::unavailable("offline")))
        })
        .await
        .unwrap_err();

        assert_eq!(attempts.load(Ordering::Relaxed), 5);
        assert_eq!(
            err.downcast_ref::<REClientError>().unwrap().code,
            TCode::UNAVAILABLE
        );
        assert!(err.downcast_ref::<tonic::Status>().is_some());
    }

    #[test]
    fn test_normalize_re_error_io_wrapped() {
        let io = io::Error::other(tonic::Status::unauthenticated("no creds"));
        let err = normalize_re_error(anyhow::Error::new(io).context("stream error"));
        assert_eq!(
            err.downcast_ref::<REClientError>().unwrap().code,
            TCode::UNAUTHENTICATED
        );
        assert!(err.downcast_ref::<io::Error>().is_some());
        assert!(format!("{err:#}").contains("stream error"));
    }

    #[test]
    fn test_normalize_re_error_preserves_classification() {
        let err = anyhow::Error::new(tonic::Status::unauthenticated("no creds"))
            .context(REClientError {
                message: "denied".to_owned(),
                code: TCode::PERMISSION_DENIED,
                group: TCodeReasonGroup::USER_BAD_CERTS,
            })
            .context("upload failed")
            .context("remote action failed");
        let diagnostic = format!("{err:#}");
        let err = normalize_re_error(err);
        let re_error = err.downcast_ref::<REClientError>().unwrap();
        assert_eq!(re_error.code, TCode::PERMISSION_DENIED);
        assert_eq!(re_error.group, TCodeReasonGroup::USER_BAD_CERTS);
        assert_eq!(re_error.message, "denied");
        assert_eq!(format!("{err:#}"), diagnostic);
        assert!(err.downcast_ref::<tonic::Status>().is_some());
    }

    #[test]
    fn test_normalize_re_error_without_status() {
        let err = normalize_re_error(anyhow::anyhow!("plain error with no grpc status"));
        assert!(err.downcast_ref::<REClientError>().is_none());
        assert_eq!(err.to_string(), "plain error with no grpc status");
    }

    #[tokio::test]
    async fn test_download_named() -> anyhow::Result<()> {
        let work = tempfile::tempdir()?;

        let path1 = work.path().join("path1");
        let path1 = path1.to_str().context("tempdir is not utf8")?;

        let path2 = work.path().join("path2");
        let path2 = path2.to_str().context("tempdir is not utf8")?;

        let digest1 = TDigest {
            hash: "aa".to_owned(),
            size_in_bytes: 3,
            ..Default::default()
        };

        let digest2 = TDigest {
            hash: "bb".to_owned(),
            size_in_bytes: 3,
            ..Default::default()
        };

        let req = DownloadRequest {
            file_digests: Some(vec![
                NamedDigestWithPermissions {
                    named_digest: NamedDigest {
                        name: path1.to_owned(),
                        digest: digest1.clone(),
                        ..Default::default()
                    },
                    is_executable: true,
                    ..Default::default()
                },
                NamedDigestWithPermissions {
                    named_digest: NamedDigest {
                        name: path2.to_owned(),
                        digest: digest2.clone(),
                        ..Default::default()
                    },
                    is_executable: false,
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };

        let res = BatchReadBlobsResponse {
            responses: vec![
                // Reply out of order
                batch_read_blobs_response::Response {
                    digest: Some(tdigest_to(&digest2)),
                    data: vec![4, 5, 6],
                    ..Default::default()
                },
                batch_read_blobs_response::Response {
                    digest: Some(tdigest_to(&digest1)),
                    data: vec![1, 2, 3],
                    ..Default::default()
                },
            ],
        };

        download_impl(
            &test_re_runtime_opts(),
            &InstanceName(None),
            req,
            None,
            10000,
            |req| {
                let res = res.clone();
                let digest1 = digest1.clone();
                let digest2 = digest2.clone();
                async move {
                    assert_eq!(req.digests.len(), 2);
                    assert_eq!(req.digests[0], tdigest_to(&digest1));
                    assert_eq!(req.digests[1], tdigest_to(&digest2));
                    Ok(res.clone())
                }
            },
            |_digest| async move { anyhow::Ok(Box::pin(futures::stream::iter(vec![]))) },
        )
        .await?;

        assert_eq!(tokio::fs::read(&path1).await?, vec![1, 2, 3]);
        assert_eq!(tokio::fs::read(&path2).await?, vec![4, 5, 6]);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                tokio::fs::metadata(&path1).await?.permissions().mode() & 0o111,
                0o111
            );
            assert_eq!(
                tokio::fs::metadata(&path2).await?.permissions().mode() & 0o111,
                0o000
            );
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_download_large_named() -> anyhow::Result<()> {
        let work = tempfile::tempdir()?;

        let path1 = work.path().join("path1");
        let path1 = path1.to_str().context("tempdir is not utf8")?;

        let path2 = work.path().join("path2");
        let path2 = path2.to_str().context("tempdir is not utf8")?;

        let digest1 = TDigest {
            hash: "aa".to_owned(),
            size_in_bytes: 3,
            ..Default::default()
        };

        let blob_data = vec![
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18,
        ];

        let digest2 = TDigest {
            hash: "xl".to_owned(),
            size_in_bytes: 18,
            ..Default::default()
        };

        let req = DownloadRequest {
            file_digests: Some(vec![
                NamedDigestWithPermissions {
                    named_digest: NamedDigest {
                        name: path1.to_owned(),
                        digest: digest1.clone(),
                        ..Default::default()
                    },
                    is_executable: true,
                    ..Default::default()
                },
                NamedDigestWithPermissions {
                    named_digest: NamedDigest {
                        name: path2.to_owned(),
                        digest: digest2.clone(),
                        ..Default::default()
                    },
                    is_executable: false,
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };

        let res = BatchReadBlobsResponse {
            responses: vec![
                // Reply out of order
                batch_read_blobs_response::Response {
                    digest: Some(tdigest_to(&digest1)),
                    data: vec![1, 2, 3],
                    ..Default::default()
                },
            ],
        };

        let read_response1 = ReadResponse {
            data: blob_data[..10].to_vec(),
        };
        let read_response2 = ReadResponse {
            data: blob_data[10..].to_vec(),
        };

        download_impl(
            &test_re_runtime_opts(),
            &InstanceName(None),
            req,
            None,
            10, // kept small to simulate a large file download
            |req| {
                let res = res.clone();
                let digest1 = digest1.clone();
                async move {
                    assert_eq!(req.digests.len(), 1);
                    assert_eq!(req.digests[0], tdigest_to(&digest1));
                    Ok(res.clone())
                }
            },
            |req| {
                let read_response1 = read_response1.clone();
                let read_response2 = read_response2.clone();
                async move {
                    assert_eq!(req.resource_name, "blobs/xl/18");
                    anyhow::Ok(Box::pin(futures::stream::iter(vec![
                        Ok(read_response1),
                        Ok(read_response2),
                    ])))
                }
            },
        )
        .await?;

        assert_eq!(tokio::fs::read(&path1).await?, vec![1, 2, 3]);
        assert_eq!(tokio::fs::read(&path2).await?, blob_data);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                tokio::fs::metadata(&path1).await?.permissions().mode() & 0o111,
                0o111
            );
            assert_eq!(
                tokio::fs::metadata(&path2).await?.permissions().mode() & 0o111,
                0o000
            );
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_download_inlined() -> anyhow::Result<()> {
        let digest1 = &TDigest {
            hash: "aa".to_owned(),
            size_in_bytes: 3,
            ..Default::default()
        };

        let digest2 = &TDigest {
            hash: "bb".to_owned(),
            size_in_bytes: 3,
            ..Default::default()
        };

        let req = DownloadRequest {
            inlined_digests: Some(vec![digest1.clone(), digest2.clone()]),
            ..Default::default()
        };

        let res = BatchReadBlobsResponse {
            responses: vec![
                // Reply out of order
                batch_read_blobs_response::Response {
                    digest: Some(tdigest_to(&digest2)),
                    data: vec![4, 5, 6],
                    ..Default::default()
                },
                batch_read_blobs_response::Response {
                    digest: Some(tdigest_to(&digest1)),
                    data: vec![1, 2, 3],
                    ..Default::default()
                },
            ],
        };

        let res = download_impl(
            &test_re_runtime_opts(),
            &InstanceName(None),
            req,
            None,
            100000,
            |req| {
                let res = res.clone();
                let digest1 = digest1.clone();
                let digest2 = digest2.clone();
                async move {
                    assert_eq!(req.digests.len(), 2);
                    assert_eq!(req.digests[0], tdigest_to(&digest1));
                    assert_eq!(req.digests[1], tdigest_to(&digest2));
                    Ok(res)
                }
            },
            |_digest| async move { anyhow::Ok(Box::pin(futures::stream::iter(vec![]))) },
        )
        .await?;

        let inlined_blobs = res.inlined_blobs.unwrap();

        assert_eq!(inlined_blobs.len(), 2);

        assert_eq!(inlined_blobs[0].digest, *digest1);
        assert_eq!(inlined_blobs[0].blob, vec![1, 2, 3]);

        assert_eq!(inlined_blobs[1].digest, *digest2);
        assert_eq!(inlined_blobs[1].blob, vec![4, 5, 6]);

        Ok(())
    }

    #[tokio::test]
    async fn test_download_multiple_batches() -> anyhow::Result<()> {
        let digest1 = &TDigest {
            hash: "aa".to_owned(),
            size_in_bytes: 3,
            ..Default::default()
        };

        let digest2 = &TDigest {
            hash: "bb".to_owned(),
            size_in_bytes: 3,
            ..Default::default()
        };

        let digest3 = &TDigest {
            hash: "cc".to_owned(),
            size_in_bytes: 3,
            ..Default::default()
        };

        let digest4 = &TDigest {
            hash: "dd".to_owned(),
            size_in_bytes: 3,
            ..Default::default()
        };

        let digest5 = &TDigest {
            hash: "dd".to_owned(),
            size_in_bytes: 3,
            ..Default::default()
        };

        let digest6 = &TDigest {
            hash: "dd".to_owned(),
            size_in_bytes: 3,
            ..Default::default()
        };

        let digests = vec![
            digest1.clone(),
            digest2.clone(),
            digest3.clone(),
            digest4.clone(),
            digest5.clone(),
            digest6.clone(),
        ];

        let req = DownloadRequest {
            inlined_digests: Some(digests.clone()),
            ..Default::default()
        };

        let counter = AtomicU16::new(0);

        let res = download_impl(
            &test_re_runtime_opts(),
            &InstanceName(None),
            req,
            None,
            7,
            |req| {
                counter.fetch_add(1, Ordering::Relaxed);
                let res = BatchReadBlobsResponse {
                    responses: req
                        .digests
                        .iter()
                        .map(|d| batch_read_blobs_response::Response {
                            digest: Some(d.clone()),
                            data: vec![0, 1, 2],
                            ..Default::default()
                        })
                        .collect::<Vec<_>>(),
                };
                async { Ok(res) }
            },
            |_digest| async move { anyhow::Ok(Box::pin(futures::stream::iter(vec![]))) },
        )
        .await?;

        let inlined_blobs = res.inlined_blobs.unwrap();

        assert_eq!(inlined_blobs.len(), digests.len());
        assert_eq!(counter.load(Ordering::Relaxed), 3);

        Ok(())
    }

    #[tokio::test]
    async fn test_download_large_inlined() -> anyhow::Result<()> {
        let digest1 = &TDigest {
            hash: "aa".to_owned(),
            size_in_bytes: 3,
            ..Default::default()
        };

        let digest2 = &TDigest {
            hash: "xl".to_owned(),
            size_in_bytes: 18,
            ..Default::default()
        };

        let req = DownloadRequest {
            inlined_digests: Some(vec![digest1.clone(), digest2.clone()]),
            ..Default::default()
        };

        let res = BatchReadBlobsResponse {
            responses: vec![
                // Reply out of order
                batch_read_blobs_response::Response {
                    digest: Some(tdigest_to(&digest1)),
                    data: vec![1, 2, 3],
                    ..Default::default()
                },
            ],
        };

        let blob_data = vec![
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18,
        ];

        let read_response1 = ReadResponse {
            data: blob_data[..10].to_vec(),
        };
        let read_response2 = ReadResponse {
            data: blob_data[10..].to_vec(),
        };

        let res = download_impl(
            &test_re_runtime_opts(),
            &InstanceName(None),
            req,
            None,
            10, // intentionally small value to keep data in the test blobs small
            |req| {
                let res = res.clone();
                let digest1 = digest1.clone();
                async move {
                    assert_eq!(req.digests.len(), 1);
                    assert_eq!(req.digests[0], tdigest_to(&digest1));
                    Ok(res)
                }
            },
            |req| {
                let read_response1 = read_response1.clone();
                let read_response2 = read_response2.clone();
                async move {
                    assert_eq!(req.resource_name, "blobs/xl/18");
                    anyhow::Ok(Box::pin(futures::stream::iter(vec![
                        Ok(read_response1),
                        Ok(read_response2),
                    ])))
                }
            },
        )
        .await?;

        let inlined_blobs = res.inlined_blobs.unwrap();

        assert_eq!(inlined_blobs.len(), 2);

        assert_eq!(inlined_blobs[0].digest, *digest1);
        assert_eq!(inlined_blobs[0].blob, vec![1, 2, 3]);

        assert_eq!(inlined_blobs[1].digest, *digest2);
        assert_eq!(inlined_blobs[1].blob, blob_data);

        Ok(())
    }

    #[tokio::test]
    async fn test_download_empty() -> anyhow::Result<()> {
        let digest1 = &TDigest {
            hash: "aa".to_owned(),
            size_in_bytes: 0,
            ..Default::default()
        };

        let req = DownloadRequest {
            inlined_digests: Some(vec![digest1.clone()]),
            ..Default::default()
        };

        let res = BatchReadBlobsResponse { responses: vec![] };

        let res = download_impl(
            &test_re_runtime_opts(),
            &InstanceName(None),
            req,
            None,
            100000,
            |req| {
                let res = res.clone();
                async move {
                    assert_eq!(req.digests.len(), 0);
                    Ok(res)
                }
            },
            |_digest| async move { anyhow::Ok(Box::pin(futures::stream::iter(vec![]))) },
        )
        .await?;

        let inlined_blobs = res.inlined_blobs.unwrap();

        assert_eq!(inlined_blobs.len(), 1);

        assert_eq!(inlined_blobs[0].digest, *digest1);
        assert!(inlined_blobs[0].blob.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_download_resource_name() -> anyhow::Result<()> {
        let digest1 = &TDigest {
            hash: "aa".to_owned(),
            size_in_bytes: 0,
            ..Default::default()
        };

        let req = DownloadRequest {
            inlined_digests: Some(vec![digest1.clone()]),
            ..Default::default()
        };

        download_impl(
            &test_re_runtime_opts(),
            &InstanceName(Some("instance".to_owned())),
            req,
            None,
            0,
            |_req| async { panic!("not called") },
            |req| async move {
                assert_eq!(req.resource_name, "instance/blobs/aa/0");
                anyhow::Ok(Box::pin(futures::stream::iter(vec![])))
            },
        )
        .await?;

        Ok(())
    }

    #[tokio::test]
    async fn test_upload_named() -> anyhow::Result<()> {
        let work = tempfile::tempdir()?;

        let path1 = work.path().join("path1");
        let path1 = path1.to_str().context("tempdir is not utf8")?;
        tokio::fs::write(path1, "aaa").await?;

        let path2 = work.path().join("path2");
        let path2 = path2.to_str().context("tempdir is not utf8")?;
        tokio::fs::write(path2, "bbb").await?;

        let digest1 = TDigest {
            hash: "aa".to_owned(),
            size_in_bytes: 3,
            ..Default::default()
        };

        let digest2 = TDigest {
            hash: "bb".to_owned(),
            size_in_bytes: 3,
            ..Default::default()
        };

        let req = UploadRequest {
            files_with_digest: Some(vec![
                NamedDigest {
                    name: path1.to_owned(),
                    digest: digest1.clone(),
                    ..Default::default()
                },
                NamedDigest {
                    name: path2.to_owned(),
                    digest: digest2.clone(),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };

        let res = BatchUpdateBlobsResponse {
            responses: vec![
                // Reply out of order
                batch_update_blobs_response::Response {
                    digest: Some(tdigest_to(&digest2)),
                    status: Some(Status::default()),
                },
                batch_update_blobs_response::Response {
                    digest: Some(tdigest_to(&digest1)),
                    status: Some(Status::default()),
                },
            ],
        };

        upload_impl(
            &InstanceName(None),
            req,
            None,
            10000,
            None,
            |req| {
                let res = res.clone();
                let digest1 = digest1.clone();
                let digest2 = digest2.clone();
                async move {
                    assert_eq!(req.requests.len(), 2);
                    assert_eq!(req.requests[0].digest, Some(tdigest_to(&digest1)));
                    assert_eq!(req.requests[0].data, b"aaa");
                    assert_eq!(req.requests[1].digest, Some(tdigest_to(&digest2)));
                    assert_eq!(req.requests[1].data, b"bbb");
                    Ok(res)
                }
            },
            |_req| async { panic!("A Bytestream upload should not be triggered") },
        )
        .await?;

        Ok(())
    }

    #[tokio::test]
    async fn test_batch_download_zstd() -> anyhow::Result<()> {
        // The server returns a zstd-compressed blob; we advertise zstd and decompress it.
        let blob = vec![9u8; 500];
        let digest = TDigest {
            hash: "dd".to_owned(),
            size_in_bytes: blob.len() as i64,
            ..Default::default()
        };

        let mut compressed = vec![];
        ZstdEncoder::new(Cursor::new(blob.clone()))
            .read_to_end(&mut compressed)
            .await
            .unwrap();

        let req = DownloadRequest {
            inlined_digests: Some(vec![digest.clone()]),
            ..Default::default()
        };

        let res = BatchReadBlobsResponse {
            responses: vec![batch_read_blobs_response::Response {
                digest: Some(tdigest_to(&digest)),
                data: compressed.clone(),
                status: Some(Status::default()),
                compressor: compressor::Value::Zstd as i32,
            }],
        };

        let mut opts = test_re_runtime_opts();
        opts.batch_read_zstd = true;

        let expected = blob.clone();
        let d_resp = download_impl(
            &opts,
            &InstanceName(None),
            req,
            None,
            10000,
            |req| {
                let res = res.clone();
                async move {
                    // We advertised zstd as an acceptable response compressor.
                    assert!(
                        req.acceptable_compressors
                            .contains(&(compressor::Value::Zstd as i32))
                    );
                    Ok(res)
                }
            },
            |_digest| async move { anyhow::Ok(Box::pin(futures::stream::iter(vec![]))) },
        )
        .await?;

        assert_eq!(d_resp.inlined_blobs.unwrap()[0].blob, expected);
        Ok(())
    }

    #[tokio::test]
    async fn test_upload_large_named() -> anyhow::Result<()> {
        let blob_data = vec![
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18,
        ];

        let work = tempfile::tempdir()?;

        let path1 = work.path().join("path1");
        let path1 = path1.to_str().context("tempdir is not utf8")?;
        tokio::fs::write(path1, "aaa").await?;

        let path2 = work.path().join("path2");
        let path2 = path2.to_str().context("tempdir is not utf8")?;
        tokio::fs::write(path2, &blob_data).await?;

        let digest1 = TDigest {
            hash: "aa".to_owned(),
            size_in_bytes: 3,
            ..Default::default()
        };

        let digest2 = TDigest {
            hash: "xl".to_owned(),
            size_in_bytes: 18,
            ..Default::default()
        };

        let req = UploadRequest {
            files_with_digest: Some(vec![
                NamedDigest {
                    name: path1.to_owned(),
                    digest: digest1.clone(),
                    ..Default::default()
                },
                NamedDigest {
                    name: path2.to_owned(),
                    digest: digest2.clone(),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };

        let res = BatchUpdateBlobsResponse {
            responses: vec![
                // Reply out of order
                batch_update_blobs_response::Response {
                    digest: Some(tdigest_to(&digest2)),
                    status: Some(Status::default()),
                },
                batch_update_blobs_response::Response {
                    digest: Some(tdigest_to(&digest1)),
                    status: Some(Status::default()),
                },
            ],
        };

        upload_impl(
            &InstanceName(None),
            req,
            None,
            10, // kept small to simulate a large file upload
            None,
            |req| {
                let res = res.clone();
                let digest1 = digest1.clone();
                async move {
                    assert_eq!(req.requests.len(), 1);
                    assert_eq!(req.requests[0].digest, Some(tdigest_to(&digest1)));
                    assert_eq!(req.requests[0].data, b"aaa");
                    Ok(res)
                }
            },
            |write_reqs| {
                let blob_data = blob_data.clone();
                async move {
                    assert_eq!(write_reqs.len(), 2);
                    assert_eq!(write_reqs[0].write_offset, 0);
                    assert!(!write_reqs[0].finish_write);
                    assert_eq!(write_reqs[0].data, blob_data[..10]);
                    assert_eq!(write_reqs[1].write_offset, 10);
                    assert!(write_reqs[1].finish_write);
                    assert_eq!(write_reqs[1].data, blob_data[10..]);
                    anyhow::Ok(WriteResponse { committed_size: 18 })
                }
            },
        )
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_upload_large_inlined() -> anyhow::Result<()> {
        let digest1 = TDigest {
            hash: "aa".to_owned(),
            size_in_bytes: 3,
            ..Default::default()
        };
        let blob_data1 = b"aaa".to_vec();

        let digest2 = TDigest {
            hash: "xl".to_owned(),
            size_in_bytes: 18,
            ..Default::default()
        };
        let blob_data2 = vec![
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18,
        ];

        let req = UploadRequest {
            inlined_blobs_with_digest: Some(vec![
                InlinedBlobWithDigest {
                    blob: blob_data2.clone(),
                    digest: digest2.clone(),
                    ..Default::default()
                },
                InlinedBlobWithDigest {
                    blob: blob_data1.clone(),
                    digest: digest1.clone(),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };

        let res = BatchUpdateBlobsResponse {
            responses: vec![batch_update_blobs_response::Response {
                digest: Some(tdigest_to(&digest2)),
                status: Some(Status::default()),
            }],
        };

        upload_impl(
            &InstanceName(None),
            req,
            None,
            10, // kept small to simulate a large inlined upload
            None,
            |req| {
                let res = res.clone();
                let digest1 = digest1.clone();
                let blob_data1 = blob_data1.clone();
                async move {
                    assert_eq!(req.requests.len(), 1);
                    assert_eq!(req.requests[0].digest, Some(tdigest_to(&digest1)));
                    assert_eq!(req.requests[0].data, blob_data1);
                    Ok(res)
                }
            },
            |write_reqs| {
                let blob_data2 = blob_data2.clone();
                async move {
                    assert_eq!(write_reqs.len(), 2);
                    assert_eq!(write_reqs[0].write_offset, 0);
                    assert!(!write_reqs[0].finish_write);
                    assert_eq!(write_reqs[0].data, blob_data2[..10]);
                    assert_eq!(write_reqs[1].write_offset, 10);
                    assert!(write_reqs[1].finish_write);
                    assert_eq!(write_reqs[1].data, blob_data2[10..]);
                    anyhow::Ok(WriteResponse { committed_size: 18 })
                }
            },
        )
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_upload_invalid_committed_size() -> anyhow::Result<()> {
        let blob_data = vec![
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18,
        ];

        let work = tempfile::tempdir()?;

        let path2 = work.path().join("path2");
        let path2 = path2.to_str().context("tempdir is not utf8")?;
        tokio::fs::write(path2, &blob_data).await?;

        let digest2 = TDigest {
            hash: "xl".to_owned(),
            size_in_bytes: 18,
            ..Default::default()
        };

        let req = UploadRequest {
            files_with_digest: Some(vec![NamedDigest {
                name: path2.to_owned(),
                digest: digest2.clone(),
                ..Default::default()
            }]),
            ..Default::default()
        };

        let resp: Result<UploadResponse, anyhow::Error> = upload_impl(
            &InstanceName(None), // TODO
            req,
            None,
            10,
            None,
            |_req| async move {
                panic!("This should not be called as there are no blobs to upload in batch");
            },
            |_write_reqs| async move {
                // Not the right size
                anyhow::Ok(WriteResponse { committed_size: 10 })
            },
        )
        .await;

        let err: anyhow::Error = resp.unwrap_err();
        // can't compare the full message because tempfile is used
        assert!(
            err.root_cause()
                .to_string()
                .contains("invalid committed_size")
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_upload_exact() -> anyhow::Result<()> {
        let work = tempfile::tempdir()?;

        let path1 = work.path().join("path1");
        let path1 = path1.to_str().context("tempdir is not utf8")?;
        tokio::fs::write(path1, "aaabbb").await?;

        let digest1 = TDigest {
            hash: "aa".to_owned(),
            size_in_bytes: 6,
            ..Default::default()
        };

        let digest2 = TDigest {
            hash: "bb".to_owned(),
            size_in_bytes: 6,
            ..Default::default()
        };
        let blob_data2 = vec![1, 2, 3, 4, 5, 6];

        let req = UploadRequest {
            files_with_digest: Some(vec![NamedDigest {
                name: path1.to_owned(),
                digest: digest1.clone(),
                ..Default::default()
            }]),
            inlined_blobs_with_digest: Some(vec![InlinedBlobWithDigest {
                blob: blob_data2.clone(),
                digest: digest2.clone(),
                ..Default::default()
            }]),
            ..Default::default()
        };

        upload_impl(
            &InstanceName(None),
            req,
            None,
            3,
            None,
            |_req| async move {
                panic!("Not called");
            },
            |write_reqs| async move {
                assert_eq!(write_reqs.len(), 2);
                assert!(write_reqs[1].finish_write);
                anyhow::Ok(WriteResponse { committed_size: 6 })
            },
        )
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_upload_empty() -> anyhow::Result<()> {
        let work = tempfile::tempdir()?;

        let path1 = work.path().join("path1");
        let path1 = path1.to_str().context("tempdir is not utf8")?;
        tokio::fs::write(path1, "").await?;

        let digest1 = TDigest {
            hash: "aa".to_owned(),
            size_in_bytes: 0,
            ..Default::default()
        };

        for compressor in [
            None,
            Some(Compressor::Deflate),
            Some(Compressor::Brotli),
            Some(Compressor::Zstd),
        ] {
            assert!(
                upload_impl(
                    &InstanceName(None),
                    UploadRequest {
                        files_with_digest: Some(vec![NamedDigest {
                            name: path1.to_owned(),
                            digest: digest1.clone(),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    },
                    compressor,
                    0, // max_total_batch_size=0 forces bytestream API
                    None,
                    |_req| async move {
                        panic!("Not called");
                    },
                    |_write_reqs| async move {
                        panic!("Not called");
                    },
                )
                .await
                .is_ok()
            );

            assert!(
                upload_impl(
                    &InstanceName(None),
                    UploadRequest {
                        files_with_digest: Some(vec![NamedDigest {
                            name: path1.to_owned(),
                            digest: digest1.clone(),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    },
                    compressor,
                    1024, // forces the batch API
                    None,
                    |_req| async move {
                        panic!("Not called");
                    },
                    |_write_reqs| async move {
                        panic!("Not called");
                    },
                )
                .await
                .is_ok()
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_upload_resource_name() -> anyhow::Result<()> {
        let digest1 = TDigest {
            hash: "aa".to_owned(),
            size_in_bytes: 3,
            ..Default::default()
        };
        let work = tempfile::tempdir()?;

        let path1 = work.path().join("path1");
        let path1 = path1.to_str().context("tempdir is not utf8")?;
        tokio::fs::write(path1, "aaa").await?;

        let req = UploadRequest {
            inlined_blobs_with_digest: Some(vec![InlinedBlobWithDigest {
                digest: digest1.clone(),
                blob: b"aaa".to_vec(),
                ..Default::default()
            }]),
            files_with_digest: Some(vec![NamedDigest {
                name: path1.to_owned(),
                digest: digest1.clone(),
                ..Default::default()
            }]),
            ..Default::default()
        };

        upload_impl(
            &InstanceName(Some("instance".to_owned())),
            req,
            None,
            1,
            None,
            |_req| async move {
                panic!("Not called");
            },
            |write_reqs| async move {
                assert!(write_reqs[0].resource_name.starts_with("instance/uploads/"));
                assert!(write_reqs[0].resource_name.ends_with("/blobs/aa/3"));
                anyhow::Ok(WriteResponse { committed_size: 3 })
            },
        )
        .await?;

        Ok(())
    }

    #[tokio::test]
    async fn test_upload_resource_name_compressed() -> anyhow::Result<()> {
        let digest1 = TDigest {
            hash: "aa".to_owned(),
            size_in_bytes: 3,
            ..Default::default()
        };
        let work = tempfile::tempdir()?;

        let path1 = work.path().join("path1");
        let path1 = path1.to_str().context("tempdir is not utf8")?;
        tokio::fs::write(path1, "aaa").await?;

        let req = UploadRequest {
            inlined_blobs_with_digest: Some(vec![InlinedBlobWithDigest {
                digest: digest1.clone(),
                blob: b"aaa".to_vec(),
                ..Default::default()
            }]),
            files_with_digest: Some(vec![NamedDigest {
                name: path1.to_owned(),
                digest: digest1.clone(),
                ..Default::default()
            }]),
            ..Default::default()
        };

        upload_impl(
            &InstanceName(Some("instance".to_owned())),
            req,
            Some(Compressor::Zstd),
            1,
            None,
            |_req| async move {
                panic!("Not called");
            },
            |write_reqs| async move {
                assert!(write_reqs[0].resource_name.starts_with("instance/uploads/"));
                assert!(
                    write_reqs[0]
                        .resource_name
                        .ends_with("/compressed-blobs/zstd/aa/3")
                );
                anyhow::Ok(WriteResponse { committed_size: -1 })
            },
        )
        .await?;

        Ok(())
    }

    #[test]
    fn test_substitute_env_vars() {
        let getter = |s: &str| match s {
            "FOO" => Ok("foo_value".to_owned()),
            "BAR" => Ok("bar_value".to_owned()),
            "BAZ" => Err(VarError::NotPresent),
            _ => panic!("Unexpected"),
        };

        assert_eq!(
            substitute_env_vars_impl("$FOO", getter).unwrap(),
            "foo_value"
        );
        assert_eq!(
            substitute_env_vars_impl("$FOO$BAR", getter).unwrap(),
            "foo_valuebar_value"
        );
        assert_eq!(
            substitute_env_vars_impl("some$FOO.bar", getter).unwrap(),
            "somefoo_value.bar"
        );
        assert_eq!(substitute_env_vars_impl("foo", getter).unwrap(), "foo");
        assert_eq!(substitute_env_vars_impl("FOO", getter).unwrap(), "FOO");
        assert!(substitute_env_vars_impl("$FOO$BAZ", getter).is_err());
    }
}

#[tokio::test]
async fn test_upload_compressed() -> anyhow::Result<()> {
    let blob_data = vec![1; 10 * 1024 * 1024];
    let digest1 = TDigest {
        hash: "aa".to_owned(),
        size_in_bytes: blob_data.len() as i64,
        ..Default::default()
    };

    let req = UploadRequest {
        inlined_blobs_with_digest: Some(vec![InlinedBlobWithDigest {
            digest: digest1.clone(),
            blob: blob_data.clone(),
            ..Default::default()
        }]),
        ..Default::default()
    };

    let blob_data_ref = &blob_data;
    upload_impl(
        &InstanceName(Some("instance".to_owned())),
        req,
        Some(Compressor::Zstd),
        1,
        None,
        |_req| async move {
            panic!("Not called");
        },
        {
            |write_reqs| async move {
                let compressed_data: Vec<u8> =
                    write_reqs.iter().flat_map(|wr| wr.data.clone()).collect();
                let mut data = vec![];
                ZstdDecoder::new(Cursor::new(compressed_data))
                    .read_to_end(&mut data)
                    .await
                    .unwrap();
                assert_eq!(&data, blob_data_ref);
                anyhow::Ok(WriteResponse { committed_size: -1 })
            }
        },
    )
    .await?;

    Ok(())
}

#[tokio::test]
async fn test_download_compressed() -> anyhow::Result<()> {
    let blob_data = vec![1; 1024];

    let mut compressed_data = vec![];
    ZstdEncoder::new(Cursor::new(blob_data.clone()))
        .read_to_end(&mut compressed_data)
        .await
        .unwrap();
    let compressed_data_ref = &compressed_data;

    let d_resp = download_impl(
        &tests::test_re_runtime_opts(),
        &InstanceName(None),
        DownloadRequest {
            inlined_digests: Some(vec![TDigest {
                hash: "aa".to_owned(),
                size_in_bytes: blob_data.len() as i64,
                ..Default::default()
            }]),
            file_digests: None,
            ..Default::default()
        },
        Some(Compressor::Zstd),
        10,
        |_req| async { panic!("not called") },
        |_req| async move {
            Ok(Box::pin(futures::stream::iter(
                compressed_data_ref
                    .chunks(10)
                    .map(|d| Result::Ok(ReadResponse { data: d.to_vec() })),
            )))
        },
    )
    .await?;

    assert_eq!(
        d_resp.inlined_blobs.as_ref().unwrap()[0].blob.len(),
        blob_data.len()
    );
    assert_eq!(d_resp.inlined_blobs.unwrap()[0].blob, blob_data);
    Ok(())
}
