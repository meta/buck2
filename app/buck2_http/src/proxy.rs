/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::net::IpAddr;
use std::str::FromStr;

use allocative::Allocative;
use buck2_error::BuckErrorContext;
use http::Uri;
use http::uri::InvalidUri;
use http::uri::PathAndQuery;
use http::uri::Scheme;
use hyper_http_proxy::Intercept;
use hyper_http_proxy::Proxy;
use ipnetwork::IpNetwork;
use serde::Deserialize;
use serde::Serialize;

/// Exact destination hosts allowed to use environment proxies.
#[derive(
    Allocative,
    Clone,
    Debug,
    Default,
    Deserialize,
    Eq,
    PartialEq,
    Serialize
)]
pub struct ProxyHostAllowlist {
    hosts: Vec<String>,
}

impl TryFrom<Vec<String>> for ProxyHostAllowlist {
    type Error = buck2_error::Error;

    fn try_from(hosts: Vec<String>) -> buck2_error::Result<Self> {
        if matches!(hosts.as_slice(), [host] if host.trim().is_empty()) {
            return Ok(Self::default());
        }
        let mut hosts = hosts
            .into_iter()
            .map(|host| {
                normalize_proxy_host(host.trim()).ok_or_else(|| {
                    buck2_error::buck2_error!(
                        buck2_error::ErrorTag::Input,
                        "Invalid http.proxy_env_allowlist host `{host}`: expected an exact hostname or IP address, without a scheme, port, path, or wildcard"
                    )
                })
            })
            .collect::<buck2_error::Result<Vec<_>>>()?;
        hosts.sort_unstable();
        hosts.dedup();
        Ok(Self { hosts })
    }
}

impl ProxyHostAllowlist {
    #[allow(unused)] // outside fbcode_build
    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }

    fn into_proxy_intercept(self, scheme: Scheme, no_proxy: Option<NoProxy>) -> Intercept {
        let should_proxy =
            move |destination_scheme: Option<&str>, host: Option<&str>, _port: Option<u16>| {
                destination_scheme == Some(scheme.as_str())
                    && host.and_then(normalize_proxy_host).is_some_and(|host| {
                        self.hosts.binary_search(&host).is_ok()
                            && !no_proxy.as_ref().is_some_and(|no_proxy| {
                                no_proxy.should_bypass_proxy_for_host(&host)
                            })
                    })
            };
        should_proxy.into()
    }
}

fn normalize_proxy_host(host: &str) -> Option<String> {
    let address = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    if let Ok(address) = address.parse::<IpAddr>() {
        return Some(address.to_string());
    }
    let domain = host.strip_suffix('.').unwrap_or(host);
    if domain.len() > 253
        || !domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return None;
    }
    Some(domain.to_ascii_lowercase())
}

/// Lookup environment variable and return string value. Checks first for uppercase
/// and falls back to lowercase if unset.
fn env_to_string(env: &'static str) -> buck2_error::Result<Option<String>> {
    std::env::var_os(env)
        .or_else(|| std::env::var_os(env.to_lowercase()))
        .map(|s| s.into_string())
        .transpose()
        .map_err(|original| {
            buck2_error::buck2_error!(
                buck2_error::ErrorTag::Tier0,
                "Invalid utf8 string: '{:?}'",
                original
            )
        })
}

fn noproxy_from_env(scheme: Scheme) -> buck2_error::Result<Option<NoProxy>> {
    Ok(env_to_string("NO_PROXY")?.map(|no_proxy| NoProxy::new(scheme, no_proxy)))
}

/// Returns a hyper_http_proxy::Proxy struct that proxies connections to the uri at
/// $HTTPS_PROXY (or $https_proxy if the former is unset). Respects $NO_PROXY.
pub(super) fn https_proxy_from_env() -> buck2_error::Result<Option<Proxy>> {
    proxy_from_env("HTTPS_PROXY", Scheme::HTTPS, None)
}

/// Returns a hyper_http_proxy::Proxy struct that proxies connections to the uri at
/// $HTTP_PROXY (or $http_proxy if the former is unset). Respects $NO_PROXY.
pub(super) fn http_proxy_from_env() -> buck2_error::Result<Option<Proxy>> {
    proxy_from_env("HTTP_PROXY", Scheme::HTTP, None)
}

pub(super) fn proxy_from_env(
    name: &'static str,
    scheme: Scheme,
    allowlist: Option<ProxyHostAllowlist>,
) -> buck2_error::Result<Option<Proxy>> {
    let Some(value) = env_to_string(name)? else {
        return Ok(None);
    };
    let uri: DefaultSchemeUri = value
        .parse()
        .with_buck_error_context(|| format!("Invalid {name} uri: {value}"))?;
    let no_proxy = noproxy_from_env(scheme.clone())?;
    let intercept = if let Some(allowlist) = allowlist {
        allowlist.into_proxy_intercept(scheme, no_proxy)
    } else if let Some(no_proxy) = no_proxy {
        no_proxy.into_proxy_intercept()
    } else if scheme == Scheme::HTTPS {
        Intercept::Https
    } else {
        Intercept::Http
    };
    Ok(Some(Proxy::new(intercept, uri.into())))
}

/// A wrapped Uri that handles inserting a default scheme (http) if one is not present.
///
/// See https://everything.curl.dev/usingcurl/proxies/type for more information about
/// how curl treats default schemes for e.g. proxy env vars.
pub(super) struct DefaultSchemeUri(pub(super) Uri);

impl FromStr for DefaultSchemeUri {
    type Err = InvalidUri;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<Uri>().map(Self)
    }
}

impl From<DefaultSchemeUri> for Uri {
    fn from(default_scheme_uri: DefaultSchemeUri) -> Self {
        let mut parts = default_scheme_uri.0.into_parts();
        if parts.scheme.is_none() {
            parts.scheme = Some(Scheme::HTTP);
        }
        if parts.path_and_query.is_none() {
            parts.path_and_query = Some(PathAndQuery::from_static("/"));
        }
        Uri::from_parts(parts).expect("Got invalid uri from formerly valid uri")
    }
}

#[derive(Debug)]
struct Domain(String);

impl Domain {
    /// Returns whether this domain "matches" candidate according to Curl's rules
    /// for NO_PROXY.
    ///
    /// See https://github.com/curl/curl/issues/1208 for a bit of discussion about
    /// some of the particulars of subdomain matching.
    fn is_match<S: AsRef<str>>(&self, candidate: S) -> bool {
        let candidate = candidate.as_ref();
        // * unambiguously matches all domains.
        self.0 == "*"
            // Exact match
            || self.0 == candidate
            // .<domain> matches all subdomains, look for exact match
            || self.0.trim_start_matches('.') == candidate
            // Candidate suffixed by domain, only match if candidate is a subdomain of domain
            // Ex: domain=".facebook.com" matches "images.facebook.com" but not "www.thefacebook.com"
            || candidate.trim_end_matches(self.0.as_str().trim_start_matches('.')).ends_with('.')
    }
}

/// Wrapper for the parsed version of Curl's "no proxy" format from the standard
/// NO_PROXY / no_proxy environment variables.
///
/// Uses hyper-proxy's Intercept::Custom to drive matching logic for whether Uris
/// should be proxied or not.
///
/// Incorporates a scheme to proxy connections as well. Connections matching this
/// (scheme, no_proxy_spec) pair will *not* be proxied.
///
/// Matching logic derived from reqwest::NoProxy (e.g. [here](https://github.com/seanmonstar/reqwest/blob/master/src/proxy.rs#L467)).
pub(super) struct NoProxy {
    addresses: Vec<IpAddr>,
    networks: Vec<IpNetwork>,
    domains: Vec<Domain>,
    proxy_scheme: Scheme,
}

impl NoProxy {
    /// Constructs a new NoProxy struct. Connections made to a host matching the
    /// no_proxy spec with a scheme of `proxy_scheme` WILL NOT be proxied (e.g.
    /// requests to these hosts with `proxy_scheme` will go directly to the dest).
    pub(super) fn new<S: AsRef<str>>(proxy_scheme: Scheme, s: S) -> Self {
        let mut addresses = Vec::new();
        let mut networks = Vec::new();
        let mut domains = Vec::new();
        let s = s.as_ref();
        for entity in s.split(',').map(str::trim) {
            if let Ok(network) = entity.parse::<IpNetwork>() {
                networks.push(network);
            } else if let Ok(address) = entity.parse::<IpAddr>() {
                addresses.push(address);
            } else {
                domains.push(Domain(entity.to_owned()));
            }
        }

        Self {
            addresses,
            networks,
            domains,
            proxy_scheme,
        }
    }

    /// Returns whether `host` matches any of the NO_PROXY entities - i.e. that we
    /// should *not* proxy requests to this host.
    fn should_bypass_proxy_for_host<S: AsRef<str>>(&self, host: S) -> bool {
        let host = host.as_ref();
        if let Ok(host_address) = host.parse::<IpAddr>() {
            self.addresses.contains(&host_address)
                || self
                    .networks
                    .iter()
                    .any(|network| network.contains(host_address))
        } else {
            self.domains.iter().any(|domain| domain.is_match(host))
        }
    }

    /// Converts this NoProxy spec into a hyper_http_proxy::Intercept::Custom closure
    /// so it can be used to build a new hyper_http_proxy::Proxy.
    ///
    /// Note: There's a tricky bit of logic below. We explicitly *negate* the return
    /// condition of the closure because of the way hyper_http_proxy::Intercept::Custom's
    /// closure works; if it returns `true`, the connection is proxied.
    ///
    /// For NoProxy, we want to *negate* this logic - if a (scheme, host) pair
    /// match our NoProxy instance, we _don't want to proxy_. Therefore we negate
    /// the return conditions below.
    ///
    /// Some examples to clarify:
    ///
    /// NO_PROXY=".facebook.com" for HTTPS
    ///     does not proxy https://www.facebook.com
    ///     does not proxy https://images.facebook.com
    ///     does proxy https://www.thefacebook.com
    ///     does proxy http://www.thefacebook.com
    ///
    /// NO_PROXY="192.168.0.1" for HTTP
    ///     does not proxy http://192.168.0.1
    ///     does proxy https://192.168.0.1
    ///     does proxy http://192.168.0.2
    pub(super) fn into_proxy_intercept(self) -> Intercept {
        let should_proxy = move |scheme: Option<&str>, host: Option<&str>, _port: Option<u16>| {
            // IPv6 addresses are wrapped in [ ] so remove those for equality checks.
            let host = host.map(|h| h.trim_start_matches('[').trim_end_matches(']'));
            let should_bypass_proxy =
                host.is_some_and(|host| self.should_bypass_proxy_for_host(host));

            // Negation happens here - true means we're going to proxy the connection.
            !should_bypass_proxy
                && self.proxy_scheme.as_str() == scheme.unwrap_or(Scheme::HTTP.as_str())
        };
        should_proxy.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(s: &'static str) -> Uri {
        s.parse().unwrap()
    }

    #[test]
    fn test_proxy_allowlist_normalizes_exact_hosts() -> buck2_error::Result<()> {
        let allowlist = ProxyHostAllowlist::try_from(vec![
            " EXAMPLE.com. ".to_owned(),
            "example.com".to_owned(),
            "127.0.0.1".to_owned(),
            " [0:0:0:0:0:0:0:1] ".to_owned(),
        ])?;
        assert_eq!(
            allowlist,
            ProxyHostAllowlist::try_from(vec![
                "::1".to_owned(),
                "127.0.0.1".to_owned(),
                "example.com".to_owned(),
            ])?
        );
        let intercept = allowlist.into_proxy_intercept(Scheme::HTTPS, None);
        for host in ["example.com", "EXAMPLE.COM.", "127.0.0.1", "[::1]"] {
            assert!(
                intercept.matches(&format!("https://{host}/").parse::<Uri>()?),
                "Expected allowed host {host}"
            );
        }
        for host in [
            "sub.example.com",
            "notexample.com",
            "example.com.evil",
            "127.0.0.2",
        ] {
            assert!(
                !intercept.matches(&format!("https://{host}/").parse::<Uri>()?),
                "Unexpected allowed host {host}"
            );
        }
        Ok(())
    }

    #[test]
    fn test_proxy_allowlist_rejects_invalid_entries() {
        for hosts in [
            vec!["*"],
            vec!["*.example.com"],
            vec![".example.com"],
            vec!["https://example.com"],
            vec!["example.com:443"],
            vec!["example.com/path"],
            vec!["user@example.com"],
            vec!["a", "", "b"],
            vec!["", ""],
            vec!["a", ""],
            vec!["a b"],
            vec!["a..b"],
            vec!["-example.com"],
            vec!["example-.com"],
        ] {
            let allowlist = ProxyHostAllowlist::try_from(
                hosts
                    .iter()
                    .map(|host| host.to_string())
                    .collect::<Vec<_>>(),
            );
            assert!(allowlist.is_err(), "Accepted {hosts:?}");
        }
    }

    #[test]
    fn test_proxy_allowlist_interception() -> buck2_error::Result<()> {
        for scheme in [Scheme::HTTP, Scheme::HTTPS] {
            let allowlist =
                ProxyHostAllowlist::try_from(vec!["example.com".to_owned(), "::1".to_owned()])?;
            let intercept = allowlist.into_proxy_intercept(scheme.clone(), None);
            for (url, allowed) in [
                ("http://example.com/path", scheme == Scheme::HTTP),
                ("https://example.com:8443/path", scheme == Scheme::HTTPS),
                ("https://sub.example.com/path", false),
                ("https://[::1]/path", scheme == Scheme::HTTPS),
                ("https://other.example/path", false),
                ("/relative", false),
            ] {
                assert_eq!(allowed, intercept.matches(&uri(url)), "{scheme}: {url}");
            }
            let allowlist =
                ProxyHostAllowlist::try_from(vec!["example.com".to_owned(), "::1".to_owned()])?;
            let intercept = allowlist.into_proxy_intercept(
                scheme.clone(),
                Some(NoProxy::new(scheme, "example.com,::1")),
            );
            assert!(!intercept.matches(&uri("http://example.com/path")));
            assert!(!intercept.matches(&uri("https://example.com/path")));
            assert!(!intercept.matches(&uri("http://EXAMPLE.COM./path")));
            assert!(!intercept.matches(&uri("https://EXAMPLE.COM./path")));
            assert!(!intercept.matches(&uri("https://[::1]/path")));
        }
        for hosts in [vec![], vec![""], vec!["   "]] {
            let allowlist = ProxyHostAllowlist::try_from(
                hosts.into_iter().map(str::to_owned).collect::<Vec<_>>(),
            )?;
            assert!(allowlist.is_empty());
            assert!(
                !allowlist
                    .into_proxy_intercept(Scheme::HTTPS, None)
                    .matches(&uri("https://example.com"))
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_proxy_allowlist_https_uses_connect() -> buck2_error::Result<()> {
        buck2_certs::certs::maybe_setup_cryptography();
        let proxy_server = httptest::Server::run();
        proxy_server.expect(
            httptest::Expectation::matching(httptest::matchers::request::method("CONNECT"))
                .respond_with(httptest::responders::status_code(502)),
        );
        let allowlist = ProxyHostAllowlist::try_from(vec!["allowed.invalid".to_owned()])?;
        let client = crate::HttpClientBuilder::https_with_system_roots()
            .await?
            .with_proxy(Proxy::new(
                allowlist.into_proxy_intercept(Scheme::HTTPS, None),
                proxy_server.url("/"),
            ))
            .build();
        assert!(client.get("https://allowed.invalid/archive").await.is_err());
        Ok(())
    }

    #[test]
    fn test_domain_match() {
        let d = Domain(".facebook.com".to_owned());
        assert!(d.is_match("www.facebook.com"));
        assert!(!d.is_match("boofacebook.com"));

        let d = Domain("facebook.com".to_owned());
        assert!(d.is_match("facebook.com"));
        assert!(d.is_match("www.facebook.com"));

        let d = Domain("photos.facebook.com".to_owned());
        assert!(!d.is_match("facebook.com"));
        assert!(d.is_match("jpg.photos.facebook.com"));

        let d = Domain("*".to_owned());
        assert!(d.is_match("www.facebook.com"));
        assert!(d.is_match("facebook.com"));
    }

    #[test]
    fn test_noproxy_empty_string_does_not_match() {
        let noproxy = NoProxy::new(Scheme::HTTP, "");
        assert!(!noproxy.should_bypass_proxy_for_host("facebook.com"));
    }

    #[test]
    fn test_noproxy_matches_ip_address() {
        let noproxy = NoProxy::new(Scheme::HTTP, "192.168.0.1");
        assert!(noproxy.should_bypass_proxy_for_host("192.168.0.1"));
    }

    #[test]
    fn test_noproxy_matches_ip_network() {
        let noproxy = NoProxy::new(Scheme::HTTP, "192.168.0.0/16");
        assert!(noproxy.should_bypass_proxy_for_host("192.168.0.1"));
    }

    #[test]
    fn test_noproxy_matches_subdomain() {
        let noproxy = NoProxy::new(Scheme::HTTP, ".facebook.com");
        assert!(noproxy.should_bypass_proxy_for_host("images.facebook.com"));
    }

    #[test]
    fn test_noproxy_matches_multiple() {
        let noproxy = NoProxy::new(Scheme::HTTP, ".facebook.com, 192.168.0.0/24, 28.0.0.1");
        assert!(noproxy.should_bypass_proxy_for_host("images.facebook.com"));
        assert!(noproxy.should_bypass_proxy_for_host("192.168.0.1"));
        assert!(!noproxy.should_bypass_proxy_for_host("28.0.0.2"));
    }

    #[test]
    fn test_noproxy_intercept_does_not_proxy_for_ip_addr_match() {
        let noproxy = NoProxy::new(Scheme::HTTPS, "192.168.0.1");
        let intercept = noproxy.into_proxy_intercept();
        // DON'T proxy https connections to 192.168.0.1 because it's an IP match
        assert!(!intercept.matches(&uri("https://192.168.0.1/foo")));
        // DON'T proxy http connections to 192.168.0.1 because it's a different scheme
        assert!(!intercept.matches(&uri("http://192.168.0.1/foo")));
        // DO proxy https connections to 192.168.0.2 because no IP match and schemes match
        assert!(intercept.matches(&uri("https://192.168.0.2/bar")));
    }

    #[test]
    fn test_noproxy_intercept_does_not_proxy_for_ip_net_match() {
        let noproxy = NoProxy::new(Scheme::HTTPS, "192.168.0.0/24");
        let intercept = noproxy.into_proxy_intercept();
        // DON'T proxy https to 192.168.0.1 because IP and scheme match
        assert!(!intercept.matches(&uri("https://192.168.0.1/foo")));
        // DO proxy https to 192.168.1.1 because IP mismatch and scheme match
        assert!(intercept.matches(&uri("https://192.168.1.1/foo")));
        // DON'T proxy http to 192.168.0.1 because scheme mismatch
        assert!(!intercept.matches(&uri("http://192.168.0.1/foo")));
    }

    #[test]
    fn test_noproxy_intercept_does_not_proxy_for_domain_match() {
        let noproxy = NoProxy::new(Scheme::HTTPS, ".facebook.com");
        let intercept = noproxy.into_proxy_intercept();
        // DON'T proxy because scheme matches and is subdomain.
        assert!(!intercept.matches(&uri("https://www.facebook.com/foo/bar")));
        // DO proxy because scheme matches but domain is different.
        assert!(intercept.matches(&uri("https://www.thefacebook.com/foo/bar")));
        // DON'T proxy because scheme mismatch
        assert!(!intercept.matches(&uri("http://www.facebook.com/foo/bar")));
    }

    #[test]
    fn test_noproxy_intercept_does_not_proxy_for_scheme_mismatch() {
        let noproxy = NoProxy::new(Scheme::HTTP, ".facebook.com");
        let intercept = noproxy.into_proxy_intercept();
        assert!(!intercept.matches(&uri("https://www.facebook.com/foo/bar")));
    }
}
