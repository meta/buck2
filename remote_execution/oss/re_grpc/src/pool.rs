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
use std::env::VarError;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::LazyLock;
use std::task::Context;
use std::task::Poll;
use std::time::Duration;

use anyhow::Context as _;
use buck2_re_configuration::Buck2OssReConfiguration;
use http_body::Body;
use http_body::Frame;
use http_body_util::combinators::UnsyncBoxBody;
use hyper_util::client::legacy::connect::HttpConnector;
use prost::bytes::Bytes;
use regex::Regex;
use tokio::sync::Mutex;
use tokio::sync::OwnedSemaphorePermit;
use tokio::sync::Semaphore;
use tokio_util::sync::PollSemaphore;
use tonic::Status;
use tonic::transport::Certificate;
use tonic::transport::Channel;
use tonic::transport::Identity;
use tonic::transport::Uri;
use tonic::transport::channel::ClientTlsConfig;
use tower::Service;

use crate::stats::CountingConnector;

/// Configuration for the connection pool
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Minimum number of connections per host
    pub min_connections: usize,
    /// Maximum number of connections per host
    pub max_connections: usize,
    /// Maximum concurrent streams per connection
    pub max_concurrency_per_connection: usize,
}

/// Simplified configuration needed to start new connections
#[derive(Clone)]
pub struct ChannelConfig {
    tls_config: Option<ClientTlsConfig>,
    grpc_keepalive_time_secs: Option<u64>,
    grpc_keepalive_timeout_secs: Option<u64>,
    grpc_keepalive_while_idle: Option<bool>,
}

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

impl ChannelConfig {
    pub async fn new(opts: &Buck2OssReConfiguration) -> anyhow::Result<Self> {
        let tls_config = if opts.tls {
            Some(Self::create_tls_config(opts).await?)
        } else {
            None
        };

        Ok(Self {
            tls_config,
            grpc_keepalive_time_secs: opts.grpc_keepalive_time_secs,
            grpc_keepalive_timeout_secs: opts.grpc_keepalive_timeout_secs,
            grpc_keepalive_while_idle: opts.grpc_keepalive_while_idle,
        })
    }

    async fn create_tls_config(opts: &Buck2OssReConfiguration) -> anyhow::Result<ClientTlsConfig> {
        let config = ClientTlsConfig::new().with_enabled_roots();

        let config = match opts.tls_ca_certs.as_ref() {
            Some(tls_ca_certs) => {
                let tls_ca_certs =
                    substitute_env_vars(tls_ca_certs).context("Invalid `tls_ca_certs`")?;
                let data = tokio::fs::read(&tls_ca_certs)
                    .await
                    .with_context(|| format!("Error reading `{tls_ca_certs}`"))?;
                config.ca_certificate(Certificate::from_pem(data))
            }
            None => config,
        };

        let config = match opts.tls_client_cert.as_ref() {
            Some(tls_client_cert) => {
                let tls_client_cert =
                    substitute_env_vars(tls_client_cert).context("Invalid `tls_client_cert`")?;
                let data = tokio::fs::read(&tls_client_cert)
                    .await
                    .with_context(|| format!("Error reading `{tls_client_cert}`"))?;
                config.identity(Identity::from_pem(&data, &data))
            }
            None => config,
        };

        Ok(config)
    }
}

fn prepare_uri(uri: Uri, tls: bool) -> anyhow::Result<Uri> {
    // Now do some awkward things with the protocol. Why do we do all this? The reason is
    // because we'd like our configuration to not be super confusing. We don't want to e.g.
    // allow setting the address to `https://foobar` without enabling TLS (or enabling tls
    // and using `http://foobar`), so we restrict ourselves to schemes that are actually
    // remotely valid in GRPC (which is more restrictive than what Tonic allows).

    // This is the GRPC spec for naming: https://github.com/grpc/grpc/blob/master/doc/naming.md
    // Many people (including Bazel), use grpc://, so we tolerate it.

    match uri.scheme_str() {
        Some("grpc") | Some("dns") | Some("ipv4") | Some("ipv6") | None => {}
        Some(scheme) => {
            return Err(anyhow::anyhow!(
                "Invalid URI scheme: `{}` for `{}` (you should omit it)",
                scheme,
                uri,
            ));
        }
    };

    // And now, let's put back a proper scheme for Tonic to be happy with. First, because
    // Tonic will blow up if we don't. Second, so we get port inference.
    let mut parts = uri.into_parts();
    parts.scheme = Some(if tls {
        http::uri::Scheme::HTTPS
    } else {
        http::uri::Scheme::HTTP
    });

    // Is this API actually designed to be unusable? If you've got a scheme, you must
    // have a path_and_query. I'm sure there's a good reason, so we abide:
    if parts.path_and_query.is_none() {
        parts.path_and_query = Some(http::uri::PathAndQuery::from_static("/"));
    }

    Ok(Uri::from_parts(parts)?)
}

/// Create a configured endpoint for the given address. The endpoint can be
/// reused to create multiple channels without re-processing TLS config.
fn create_endpoint(
    config: &ChannelConfig,
    address: &str,
) -> Result<tonic::transport::Endpoint, anyhow::Error> {
    let address = substitute_env_vars(address).context("Invalid address")?;
    let uri = address.parse().context("Invalid address")?;
    let uri = prepare_uri(uri, config.tls_config.is_some()).context("Invalid URI")?;

    let mut endpoint = Channel::builder(uri);
    if let Some(tls_config) = &config.tls_config {
        endpoint = endpoint.tls_config(tls_config.clone())?;
    }

    // Configure gRPC keepalive settings (always enabled with sensible defaults)
    endpoint = endpoint
        .http2_keep_alive_interval(Duration::from_secs(
            config.grpc_keepalive_time_secs.unwrap_or(30),
        ))
        .keep_alive_timeout(Duration::from_secs(
            config.grpc_keepalive_timeout_secs.unwrap_or(10),
        ))
        .keep_alive_while_idle(config.grpc_keepalive_while_idle.unwrap_or(true));

    Ok(endpoint)
}

/// Create a lazy channel from an endpoint.
fn channel_from_endpoint(endpoint: &tonic::transport::Endpoint) -> Channel {
    // Since we are creating the HttpConnector ourselves, any TCP
    // settings (tcp_nodelay, tcp_keepalive, connect_timeout), need to
    // be set here instead of on the endpoint
    let mut http = HttpConnector::new();
    http.enforce_http(false);
    let connector = CountingConnector::new(http);

    // We need to use a lazy channel so the pool isn't blocked waiting for new
    // connection IO
    endpoint.connect_with_connector_lazy(connector)
}

pub fn create_channel(config: &ChannelConfig, address: &str) -> Result<Channel, anyhow::Error> {
    let endpoint = create_endpoint(config, address)?;
    Ok(channel_from_endpoint(&endpoint))
}

/// A response body wrapper that holds a semaphore permit until the body is fully consumed or dropped.
///
/// This ensures that for streaming RPCs, the stream slot is held for the entire
/// duration of the stream, not just until headers arrive.
pub struct PermitBody<B> {
    inner: B,
    _permit: OwnedSemaphorePermit,
}

impl<B> PermitBody<B> {
    fn new(inner: B, permit: OwnedSemaphorePermit) -> Self {
        Self {
            inner,
            _permit: permit,
        }
    }
}

impl<B> Body for PermitBody<B>
where
    B: Body + Unpin,
{
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Pin::new(&mut self.inner).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }
}

/// A channel wrapper that provides back-pressure via semaphore permits.
///
/// The permit is held in the response body, so for streaming RPCs the permit
/// is held until the stream is fully consumed or dropped.
pub struct PooledChannel {
    channel: Channel,
    semaphore: PollSemaphore,
    permit: Option<OwnedSemaphorePermit>,
}

impl PooledChannel {
    fn new(channel: Channel, semaphore: Arc<Semaphore>) -> Self {
        Self {
            channel,
            semaphore: PollSemaphore::new(semaphore),
            permit: None,
        }
    }
}

impl Clone for PooledChannel {
    fn clone(&self) -> Self {
        Self {
            channel: self.channel.clone(),
            semaphore: self.semaphore.clone(),
            permit: None,
        }
    }
}

impl Service<http::Request<tonic::body::Body>> for PooledChannel {
    type Response = http::Response<UnsyncBoxBody<Bytes, Status>>;
    type Error = tonic::transport::Error;
    type Future = futures::future::BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if self.permit.is_none() {
            self.permit = std::task::ready!(self.semaphore.poll_acquire(cx));
        }
        self.channel.poll_ready(cx)
    }

    fn call(&mut self, request: http::Request<tonic::body::Body>) -> Self::Future {
        let permit = self.permit.take();
        let fut = self.channel.call(request);

        Box::pin(async move {
            let response = fut.await?;
            let (parts, body) = response.into_parts();
            match permit {
                Some(permit) => {
                    let permit_body = PermitBody::new(body, permit);
                    Ok(http::Response::from_parts(
                        parts,
                        UnsyncBoxBody::new(permit_body),
                    ))
                }
                None => Ok(http::Response::from_parts(parts, UnsyncBoxBody::new(body))),
            }
        })
    }
}

/// Shared state for a pooled channel (used for channel selection)
struct PooledChannelState {
    channel: Channel,
    semaphore: Arc<Semaphore>,
}

impl PooledChannelState {
    fn new(channel: Channel, max_streams: usize) -> Self {
        Self {
            channel,
            semaphore: Arc::new(Semaphore::new(max_streams)),
        }
    }

    fn has_capacity(&self) -> bool {
        self.semaphore.available_permits() > 0
    }
}

struct HostPoolInner {
    channels: Vec<PooledChannelState>,
    next_index: usize,
}

impl HostPoolInner {
    fn next_channel(&mut self) -> &PooledChannelState {
        let idx = self.next_index % self.channels.len();
        self.next_index = idx + 1;
        &self.channels[idx]
    }
}

/// Pool of channels for a single host
struct HostPool {
    inner: Mutex<HostPoolInner>,
    endpoint: tonic::transport::Endpoint,
    max_connections: usize,
    max_concurrency_per_connection: usize,
}

impl HostPool {
    fn new(
        endpoint: tonic::transport::Endpoint,
        min_connections: usize,
        max_connections: usize,
        max_concurrency_per_connection: usize,
    ) -> Self {
        let channel_states: Vec<_> = (0..min_connections)
            .map(|_| {
                let channel = channel_from_endpoint(&endpoint);
                PooledChannelState::new(channel, max_concurrency_per_connection)
            })
            .collect();

        Self {
            inner: Mutex::new(HostPoolInner {
                channels: channel_states,
                next_index: 0,
            }),
            endpoint,
            max_connections,
            max_concurrency_per_connection,
        }
    }

    /// Get a pooled channel from this pool.
    ///
    /// This method:
    /// 1. Tries to find a channel with available capacity (round-robin)
    /// 2. If all channels are at capacity and we haven't hit max, creates a new channel
    /// 3. If at max channels, picks the next channel (back-pressure happens in poll_ready)
    async fn get(&self) -> PooledChannel {
        let mut inner = self.inner.lock().await;
        let num_channels = inner.channels.len();

        // Round-robin through existing channels, prefer one with capacity
        for _ in 0..num_channels {
            let state = inner.next_channel();
            if state.has_capacity() {
                return PooledChannel::new(state.channel.clone(), state.semaphore.clone());
            }
        }

        // All channels at capacity - create new if allowed
        if num_channels < self.max_connections {
            let channel = channel_from_endpoint(&self.endpoint);
            let state = PooledChannelState::new(channel, self.max_concurrency_per_connection);
            let pooled = PooledChannel::new(state.channel.clone(), state.semaphore.clone());
            inner.channels.push(state);

            return pooled;
        }

        // At max channels - just return next one, back-pressure happens in poll_ready
        let state = inner.next_channel();
        PooledChannel::new(state.channel.clone(), state.semaphore.clone())
    }
}

/// A pool of gRPC channels by host/address.
///
/// All channels in the pool share a single `ChannelConfig`. The address-keyed
/// cache assumes this: if per-call config variation is ever needed, the cache
/// key must include config identity, not just the address.
pub struct ChannelPool {
    pools: Mutex<HashMap<String, Arc<HostPool>>>,
    config: PoolConfig,
    channel_config: ChannelConfig,
}

impl ChannelPool {
    pub fn new(config: PoolConfig, channel_config: ChannelConfig) -> Self {
        Self {
            pools: Mutex::new(HashMap::new()),
            config,
            channel_config,
        }
    }

    /// Get a pooled channel for the given address
    pub async fn get(&self, address: &str) -> anyhow::Result<PooledChannel> {
        use std::collections::hash_map::Entry;

        let host_pool = {
            let mut pools = self.pools.lock().await;

            match pools.entry(address.to_owned()) {
                Entry::Occupied(e) => Arc::clone(e.get()),
                Entry::Vacant(e) => {
                    let endpoint = create_endpoint(&self.channel_config, address)
                        .with_context(|| format!("Failed to create endpoint for {}", address))?;
                    let host_pool = HostPool::new(
                        endpoint,
                        self.config.min_connections,
                        self.config.max_connections,
                        self.config.max_concurrency_per_connection,
                    );
                    Arc::clone(e.insert(Arc::new(host_pool)))
                }
            }
        };

        Ok(host_pool.get().await)
    }
}

#[cfg(test)]
mod tests {
    use tonic::transport::Uri;

    use super::prepare_uri;

    #[test]
    fn prepare_uri_adds_root_path_when_missing() {
        let uri = Uri::builder()
            .authority("example.com:1234")
            .build()
            .unwrap();

        let uri = prepare_uri(uri, false).unwrap();

        assert_eq!(uri.to_string(), "http://example.com:1234/");
    }
}
