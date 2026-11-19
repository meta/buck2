/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use buck2_certs::certs::find_internal_cert;
#[cfg(fbcode_build)]
use buck2_certs::certs::set_internal_and_system_roots;
use buck2_certs::certs::supports_vpnless;
use buck2_certs::certs::tls_config_with_single_cert;
use buck2_certs::certs::tls_config_with_system_roots;
use buck2_error::BuckErrorOptionContext;
#[cfg(fbcode_build)]
use http::uri::Scheme;
use hyper::Uri;
use hyper_http_proxy::Proxy;
use hyper_http_proxy::ProxyConnector;
use hyper_rustls::HttpsConnector;
use hyper_rustls::HttpsConnectorBuilder;
use hyper_timeout::TimeoutConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use rustls::ClientConfig;
use tokio::sync::Semaphore;
use tokio_rustls::TlsConnector;
use tower_service::Service as TowerService;

use super::HttpClient;
use super::RequestClient;
use crate::client::dns::DnsResolver;
use crate::proxy;
use crate::stats::HttpNetworkStats;
use crate::x2p;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TimeoutConfig {
    connect_timeout: Option<Duration>,
    read_timeout: Option<Duration>,
    write_timeout: Option<Duration>,
}

impl TimeoutConfig {
    fn to_connector<C>(&self, connector: C) -> TimeoutConnector<C>
    where
        C: TowerService<Uri> + Send,
        C::Response: hyper::rt::Read + hyper::rt::Write + Send + Unpin,
        C::Future: Send + 'static,
        C::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let mut timeout_connector = TimeoutConnector::new(connector);
        timeout_connector.set_connect_timeout(self.connect_timeout);
        timeout_connector.set_read_timeout(self.read_timeout);
        timeout_connector.set_write_timeout(self.write_timeout);
        timeout_connector
    }
}

pub struct HttpClientBuilder {
    tls_config: ClientConfig,
    proxies: Vec<Proxy>,
    max_redirects: Option<usize>,
    supports_vpnless: bool,
    absolute_dns_names: bool,
    http2: bool,
    timeout_config: Option<TimeoutConfig>,
    max_concurrent_requests: Option<usize>,
}

impl HttpClientBuilder {
    /// Builds an http client compatible with OSS usage.
    pub async fn oss() -> buck2_error::Result<Self> {
        tracing::debug!("Using OSS client");
        let mut builder = Self::https_with_system_roots().await?;
        builder.with_proxy_from_env()?;
        Ok(builder)
    }

    /// Builds an http client compatible with internal Meta usage.
    pub async fn internal() -> buck2_error::Result<Self> {
        let mut builder = Self::https_with_system_roots().await?;
        if supports_vpnless() {
            tracing::debug!("Using vpnless client");
            let proxy = x2p::find_proxy()?.internal_error("Expected unix domain socket or http proxy port for x2p client but did not find either")?;
            builder.with_x2p_proxy(proxy);
        } else if let Some(cert_path) = find_internal_cert() {
            tracing::debug!("Using internal https client");
            builder.with_client_auth_cert(cert_path).await?;
        } else {
            tracing::debug!("Using default https client");
        }

        Ok(builder)
    }

    /// Creates a barebones https client using system roots for TLS authentication.
    pub async fn https_with_system_roots() -> buck2_error::Result<Self> {
        let tls_config = tls_config_with_system_roots().await?;
        Ok(Self {
            tls_config,
            proxies: Vec::new(),
            max_redirects: None,
            supports_vpnless: false,
            absolute_dns_names: false,
            http2: true,
            timeout_config: None,
            max_concurrent_requests: None,
        })
    }

    pub fn with_tls_config(&mut self, tls_config: ClientConfig) -> &mut Self {
        self.tls_config = tls_config;
        self
    }

    /// Configure environment proxies only for allowed destination hosts, trusting
    /// internal and native system roots when an environment proxy is added.
    #[cfg(fbcode_build)]
    pub async fn with_internal_proxy_from_env(
        &mut self,
        allowlist: &proxy::ProxyHostAllowlist,
    ) -> buck2_error::Result<&mut Self> {
        if allowlist.is_empty() {
            return Ok(self);
        }
        let previous_proxy_count = self.proxies.len();
        for (name, scheme) in [("HTTPS_PROXY", Scheme::HTTPS), ("HTTP_PROXY", Scheme::HTTP)] {
            if let Some(proxy) = proxy::proxy_from_env(name, scheme, Some(allowlist.clone()))? {
                self.with_proxy(proxy);
            }
        }
        if self.proxies.len() > previous_proxy_count {
            set_internal_and_system_roots(&mut self.tls_config).await?;
        }
        Ok(self)
    }

    pub async fn with_client_auth_cert<P: AsRef<Path>>(
        &mut self,
        path: P,
    ) -> buck2_error::Result<&mut Self> {
        let tls_config = tls_config_with_single_cert(path.as_ref(), path.as_ref()).await?;
        Ok(self.with_tls_config(tls_config))
    }

    pub fn with_proxy(&mut self, proxy: Proxy) -> &mut Self {
        self.proxies.push(proxy);
        self
    }

    pub fn with_x2p_proxy(&mut self, proxy: Proxy) -> &mut Self {
        self.with_proxy(proxy).with_supports_vpnless();
        self
    }

    pub fn with_proxy_from_env(&mut self) -> buck2_error::Result<&mut Self> {
        if let Some(proxy) = proxy::https_proxy_from_env()? {
            self.with_proxy(proxy);
        }
        if let Some(proxy) = proxy::http_proxy_from_env()? {
            self.with_proxy(proxy);
        }
        Ok(self)
    }

    pub fn with_connect_timeout(&mut self, connect_timeout: Option<Duration>) -> &mut Self {
        if let Some(timeout_config) = &mut self.timeout_config {
            timeout_config.connect_timeout = connect_timeout;
        } else {
            self.timeout_config = Some(TimeoutConfig {
                connect_timeout,
                read_timeout: None,
                write_timeout: None,
            });
        }
        self
    }

    pub fn connect_timeout(&self) -> Option<Duration> {
        self.timeout_config.as_ref().and_then(|c| c.connect_timeout)
    }

    pub fn with_read_timeout(&mut self, read_timeout: Option<Duration>) -> &mut Self {
        if let Some(timeout_config) = &mut self.timeout_config {
            timeout_config.read_timeout = read_timeout;
        } else {
            self.timeout_config = Some(TimeoutConfig {
                read_timeout,
                connect_timeout: None,
                write_timeout: None,
            });
        }
        self
    }

    pub fn read_timeout(&self) -> Option<Duration> {
        self.timeout_config.as_ref().and_then(|c| c.read_timeout)
    }

    pub fn with_write_timeout(&mut self, write_timeout: Option<Duration>) -> &mut Self {
        if let Some(timeout_config) = &mut self.timeout_config {
            timeout_config.write_timeout = write_timeout;
        } else {
            self.timeout_config = Some(TimeoutConfig {
                write_timeout,
                connect_timeout: None,
                read_timeout: None,
            });
        }
        self
    }

    pub fn write_timeout(&self) -> Option<Duration> {
        self.timeout_config.as_ref().and_then(|c| c.write_timeout)
    }

    pub fn with_max_redirects(&mut self, max_redirects: usize) -> &mut Self {
        self.max_redirects = Some(max_redirects);
        self
    }

    pub fn max_redirects(&self) -> Option<usize> {
        self.max_redirects
    }

    pub fn with_supports_vpnless(&mut self) -> &mut Self {
        self.supports_vpnless = true;
        self
    }

    /// Resolve local DNS lookups as absolute names without changing HTTP or TLS hostnames.
    /// Destination lookup through a proxy remains the proxy's responsibility.
    pub fn with_absolute_dns_names(&mut self) -> &mut Self {
        self.absolute_dns_names = true;
        self
    }

    pub fn with_http2(&mut self, http2: bool) -> &mut Self {
        self.http2 = http2;
        self
    }

    pub fn supports_vpnless(&self) -> bool {
        self.supports_vpnless
    }

    pub fn with_max_concurrent_requests(
        &mut self,
        max_concurrent_requests: Option<usize>,
    ) -> &mut Self {
        self.max_concurrent_requests = max_concurrent_requests;
        self
    }

    fn build_inner(&self) -> Arc<dyn RequestClient> {
        match (self.proxies.as_slice(), &self.timeout_config) {
            // Construct x2p unix socket client.
            // Note: This ignores (and does not require) the TLS config.
            (proxies @ [_, ..], Some(timeout_config)) => {
                #[cfg(unix)]
                if let Some(unix_socket) = find_unix_proxy(proxies) {
                    let timeout_connector = timeout_config.to_connector(hyperlocal::UnixConnector);
                    let proxy_connector = build_proxy_connector(
                        std::slice::from_ref(unix_socket),
                        timeout_connector,
                        None,
                    );
                    return Arc::new(Client::builder(TokioExecutor::new()).build(proxy_connector));
                }
                if self.supports_vpnless {
                    let mut http_connector = HttpConnector::new();
                    // When talking to local x2pagent proxy, only http is supported.
                    http_connector.enforce_http(true);
                    let timeout_connector = timeout_config.to_connector(http_connector);
                    let proxy_connector = build_proxy_connector(proxies, timeout_connector, None);
                    return Arc::new(Client::builder(TokioExecutor::new()).build(proxy_connector));
                }

                // Proxied http client with TLS.
                let https_connector = build_https_connector(
                    self.tls_config.clone(),
                    self.http2,
                    self.absolute_dns_names,
                );
                let timeout_connector = timeout_config.to_connector(https_connector);
                // Re-use TLS config from https connection for communication with proxies.
                let proxy_connector = build_proxy_connector(
                    proxies,
                    timeout_connector,
                    Some(self.tls_config.clone()),
                );
                Arc::new(Client::builder(TokioExecutor::new()).build(proxy_connector))
            }
            (proxies @ [_, ..], None) => {
                #[cfg(unix)]
                if let Some(unix_socket) = find_unix_proxy(proxies) {
                    let proxy_connector = build_proxy_connector(
                        std::slice::from_ref(unix_socket),
                        hyperlocal::UnixConnector,
                        None,
                    );
                    return Arc::new(Client::builder(TokioExecutor::new()).build(proxy_connector));
                }
                if self.supports_vpnless {
                    let mut http_connector = HttpConnector::new();
                    // When talking to local x2pagent proxy, only http is supported.
                    http_connector.enforce_http(true);
                    let proxy_connector = build_proxy_connector(proxies, http_connector, None);
                    return Arc::new(Client::builder(TokioExecutor::new()).build(proxy_connector));
                }

                let https_connector = build_https_connector(
                    self.tls_config.clone(),
                    self.http2,
                    self.absolute_dns_names,
                );
                let proxy_connector =
                    build_proxy_connector(proxies, https_connector, Some(self.tls_config.clone()));
                Arc::new(Client::builder(TokioExecutor::new()).build(proxy_connector))
            }

            // Client with TLS only.
            ([], Some(timeout_config)) => {
                let https_connector = build_https_connector(
                    self.tls_config.clone(),
                    self.http2,
                    self.absolute_dns_names,
                );
                let timeout_connector = timeout_config.to_connector(https_connector);
                Arc::new(Client::builder(TokioExecutor::new()).build(timeout_connector))
            }
            ([], None) => {
                let https_connector = build_https_connector(
                    self.tls_config.clone(),
                    self.http2,
                    self.absolute_dns_names,
                );
                Arc::new(Client::builder(TokioExecutor::new()).build(https_connector))
            }
        }
    }

    pub fn build(&self) -> HttpClient {
        HttpClient {
            inner: self.build_inner(),
            max_redirects: self.max_redirects,
            supports_vpnless: self.supports_vpnless,
            http2: self.http2,
            stats: HttpNetworkStats::new(),
            concurrent_requests_budget: self
                .max_concurrent_requests
                .map(|v| Arc::new(Semaphore::new(v))),
        }
    }
}

fn build_https_connector(
    tls_config: ClientConfig,
    http2: bool,
    absolute_dns_names: bool,
) -> HttpsConnector<HttpConnector<DnsResolver>> {
    let mut http_connector = HttpConnector::new_with_resolver(DnsResolver::new(absolute_dns_names));
    http_connector.enforce_http(false);
    let builder = HttpsConnectorBuilder::new()
        .with_tls_config(tls_config)
        .https_or_http()
        .enable_http1();

    if http2 {
        builder.enable_http2().wrap_connector(http_connector)
    } else {
        builder.wrap_connector(http_connector)
    }
}

/// Build a proxy connector using `proxies`, wrapping underlying `connector`,
/// and optionally using `tls_config` to secure communications with the proxy.
///
/// Note: Not all proxy connectors built by this client need TLS communication
/// with the proxy, e.g. if the proxy is on localhost.
fn build_proxy_connector<C>(
    proxies: &[Proxy],
    connector: C,
    tls_config: Option<ClientConfig>,
) -> ProxyConnector<C>
where
    C: TowerService<Uri> + Send,
    C::Response: hyper::rt::Read + hyper::rt::Write + Send + Unpin,
    C::Future: Send + 'static,
    C::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    // Note: we use the `unsecured()` constructor here, but all that does is
    // not load the default TLS config. You can optionally pass your own tls
    // config if needed.
    let mut proxy_connector = ProxyConnector::unsecured(connector);
    proxy_connector.extend_proxies(proxies.iter().cloned());
    if let Some(tls_config) = tls_config {
        proxy_connector.set_tls(Some(TlsConnector::from(Arc::new(tls_config))));
    }
    proxy_connector
}

/// Helper function to find any proxies with unix:// as the scheme (which
/// indicates we want to proxy through a unix domain socket).
///
/// Note: This _does_ compile on non-unix, but is only used at runtime in unix;
/// adding this to silence dead code warnings.
#[cfg(unix)]
fn find_unix_proxy(proxies: &[Proxy]) -> Option<&Proxy> {
    proxies
        .iter()
        .find(|proxy| proxy.uri().scheme_str() == Some("unix"))
}

#[cfg(test)]
mod tests {
    use hyper_http_proxy::Intercept;

    use super::*;

    #[tokio::test]
    async fn test_default_builder() -> buck2_error::Result<()> {
        buck2_certs::certs::maybe_setup_cryptography();
        let builder = HttpClientBuilder::https_with_system_roots().await?;

        assert_eq!(None, builder.max_redirects);
        assert!(builder.proxies.is_empty());
        assert!(!builder.supports_vpnless);
        assert!(!builder.absolute_dns_names);
        Ok(())
    }

    #[tokio::test]
    async fn test_supports_vpnless_set_true() -> buck2_error::Result<()> {
        buck2_certs::certs::maybe_setup_cryptography();
        let mut builder = HttpClientBuilder::https_with_system_roots().await?;
        builder.with_supports_vpnless();

        assert!(builder.supports_vpnless);
        Ok(())
    }

    #[tokio::test]
    async fn test_http2_option() -> buck2_error::Result<()> {
        buck2_certs::certs::maybe_setup_cryptography();
        let mut builder = HttpClientBuilder::https_with_system_roots().await?;
        assert!(builder.http2);
        builder.with_http2(false);

        assert!(!builder.http2);
        Ok(())
    }

    #[tokio::test]
    async fn test_with_max_redirects_overrides_default() -> buck2_error::Result<()> {
        buck2_certs::certs::maybe_setup_cryptography();
        let mut builder = HttpClientBuilder::https_with_system_roots().await?;
        builder.with_max_redirects(5);

        assert_eq!(5, builder.max_redirects.unwrap());
        Ok(())
    }

    #[tokio::test]
    async fn test_builder_with_proxy_adds_proxy() -> buck2_error::Result<()> {
        buck2_certs::certs::maybe_setup_cryptography();
        let proxy = Proxy::new(Intercept::All, "http://localhost:12345".try_into()?);
        let mut builder = HttpClientBuilder::https_with_system_roots().await?;
        builder.with_proxy(proxy);

        assert_eq!(1, builder.proxies.len());
        Ok(())
    }

    #[tokio::test]
    async fn test_set_connect_timeout() -> buck2_error::Result<()> {
        buck2_certs::certs::maybe_setup_cryptography();
        let mut builder = HttpClientBuilder::https_with_system_roots().await?;
        builder.with_connect_timeout(Some(Duration::from_millis(1000)));

        assert_eq!(
            Some(TimeoutConfig {
                connect_timeout: Some(Duration::from_millis(1000)),
                read_timeout: None,
                write_timeout: None,
            }),
            builder.timeout_config,
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_set_connect_and_read_timeouts() -> buck2_error::Result<()> {
        buck2_certs::certs::maybe_setup_cryptography();
        let mut builder = HttpClientBuilder::https_with_system_roots().await?;
        builder
            .with_connect_timeout(Some(Duration::from_millis(1000)))
            .with_read_timeout(Some(Duration::from_millis(2000)));
        assert_eq!(
            Some(TimeoutConfig {
                connect_timeout: Some(Duration::from_millis(1000)),
                read_timeout: Some(Duration::from_millis(2000)),
                write_timeout: None,
            }),
            builder.timeout_config,
        );
        Ok(())
    }
}
