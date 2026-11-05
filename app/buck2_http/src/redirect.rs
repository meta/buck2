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

use buck2_error::BuckErrorContext;
use bytes::Bytes;
use http::HeaderMap;
use http::Method;
use http::Uri;
use hyper::Request;
use hyper::Response;
use hyper::StatusCode;

use crate::HttpError;

trait UriWithRedirect {
    fn is_cross_host(&self, other: &Uri) -> bool;
}

/// Resolves the value of a `Location` header against the request URI (RFC 3986 section 5.2).
/// Returns `None` when the value does not resolve to a URI the client can request.
fn resolve_location(base: &Uri, location: &[u8]) -> Option<Uri> {
    let location = std::str::from_utf8(location).ok()?;
    // A fragment is never sent to the server.
    let location = location.split('#').next().unwrap_or("");
    if let Ok(uri) = Uri::try_from(location) {
        if uri.scheme().is_some() && uri.authority().is_some() {
            return Some(uri);
        }
    }
    let scheme = base.scheme_str()?;
    if let Some(rest) = location.strip_prefix("//") {
        return Uri::try_from(format!("{scheme}://{rest}")).ok();
    }
    // A first segment with a colon is a scheme (RFC 3986 section 4.2), so this was a malformed
    // absolute URI rather than a relative path.
    let first_segment = location.split(['/', '?']).next().unwrap_or("");
    if first_segment.contains(':') {
        return None;
    }
    let authority = base.authority()?.as_str();
    let (path, query) = match location.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (location, None),
    };
    let base_path = base.path();
    let path = if path.is_empty() {
        base_path.to_owned()
    } else if path.starts_with('/') {
        remove_dot_segments(path)
    } else {
        let dir = &base_path[..base_path.rfind('/').map_or(0, |i| i + 1)];
        remove_dot_segments(&format!("{dir}{path}"))
    };
    let query = if location.is_empty() {
        base.query()
    } else {
        query
    };
    let target = match query {
        Some(query) => format!("{scheme}://{authority}{path}?{query}"),
        None => format!("{scheme}://{authority}{path}"),
    };
    Uri::try_from(target).ok()
}

/// RFC 3986 section 5.2.4 on an absolute path.
fn remove_dot_segments(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut trailing_slash = path.ends_with('/');
    for segment in path.split('/').skip(1) {
        match segment {
            "." => trailing_slash = true,
            ".." => {
                out.pop();
                trailing_slash = true;
            }
            segment => {
                out.push(segment);
                trailing_slash = segment.is_empty();
            }
        }
    }
    let mut result = format!("/{}", out.join("/"));
    if trailing_slash && !result.ends_with('/') {
        result.push('/');
    }
    result
}

impl UriWithRedirect for Uri {
    /// Returns whether this Uri is the same host as represented by 'other'.
    fn is_cross_host(&self, other: &Uri) -> bool {
        self.host() != other.host() || self.port_u16() != other.port_u16()
    }
}

/// Intermediate type to store aspects of a request we need to mutate during
/// redirect state machine.
/// This is mostly because hyper::Request cannot be cloned.
pub(super) struct PendingRequest {
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
}

impl PendingRequest {
    pub(super) fn from_request(request: &Request<Bytes>) -> Self {
        Self {
            method: request.method().clone(),
            uri: request.uri().clone(),
            headers: request.headers().clone(),
            body: request.body().clone(),
        }
    }

    pub(super) fn to_request(&self) -> buck2_error::Result<Request<Bytes>> {
        let mut builder = Request::builder()
            .method(self.method.clone())
            .uri(self.uri.clone());
        *builder
            .headers_mut()
            .expect("Request builder should not error here") = self.headers.clone();
        builder
            .body(self.body.clone())
            .buck_error_context("building redirected request")
    }
}

/// A simple state machine that drives following redirects. Much of this is derived
/// from how [`reqwest` handles redirects](https://docs.rs/reqwest/latest/src/reqwest/redirect.rs.html#1-337)
/// as well as the [`follow-redirects`](https://github.com/srijs/rust-follow-redirects) crate.
/// Unfortunately, the latter is abandoned; until and unless it's maintained by someone
/// (preferably the hyper folks), let's roll our own.
pub(super) struct RedirectEngine<B> {
    processed_redirects: usize,
    max_redirects: usize,
    pending_request: PendingRequest,
    response: Response<B>,
}

impl<B> RedirectEngine<B> {
    pub(super) fn new(
        max_redirects: usize,
        pending_request: PendingRequest,
        response: Response<B>,
    ) -> Self {
        Self {
            processed_redirects: 0,
            max_redirects,
            pending_request,
            response,
        }
    }

    /// Handle any redirects we get in the course of sending the request (up to
    /// self.max_redirects).
    pub(super) async fn handle_redirects<S, F>(
        mut self,
        sender_func: S,
    ) -> Result<Response<B>, HttpError>
    where
        F: Future<Output = Result<Response<B>, HttpError>>,
        S: Fn(Request<Bytes>) -> F,
    {
        let initial_uri = self.pending_request.uri.clone();
        loop {
            if !self.should_redirect() {
                break;
            }
            if self.processed_redirects >= self.max_redirects {
                return Err(HttpError::TooManyRedirects {
                    uri: initial_uri.to_string(),
                    max_redirects: self.max_redirects,
                });
            }
            tracing::debug!(
                "http: processing redirect request ({}) for {}",
                self.response.status(),
                self.pending_request.uri,
            );

            if let Some(redirect_request) = self
                .update_and_create_request()
                .map_err(HttpError::MutateRequest)?
            {
                self.response = sender_func(redirect_request).await?;
                self.processed_redirects += 1;
            } else {
                break;
            }
        }

        Ok(self.response)
    }

    /// Whether we should redirect this response.
    fn should_redirect(&self) -> bool {
        let got_redirect_status_code = matches!(
            self.response.status(),
            StatusCode::MOVED_PERMANENTLY
                | StatusCode::FOUND
                | StatusCode::SEE_OTHER
                | StatusCode::TEMPORARY_REDIRECT
                | StatusCode::PERMANENT_REDIRECT
        );

        got_redirect_status_code && self.extract_redirect_location_from_response().is_some()
    }

    /// Updates the request in place to send to the redirect location.
    fn update_and_create_request(&mut self) -> buck2_error::Result<Option<Request<Bytes>>> {
        let redirect_location =
            if let Some(location) = self.extract_redirect_location_from_response() {
                location
            } else {
                return Ok(None);
            };

        let redirect_uri = redirect_location;
        let is_cross_host = redirect_uri.is_cross_host(&self.pending_request.uri);
        self.pending_request.uri = redirect_uri;

        if is_cross_host {
            for sensitive_header in &[
                hyper::header::AUTHORIZATION,
                hyper::header::COOKIE,
                hyper::header::PROXY_AUTHORIZATION,
                hyper::header::WWW_AUTHENTICATE,
            ] {
                self.pending_request.headers.remove(sensitive_header);
            }
        }

        match self.response.status() {
            StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND | StatusCode::SEE_OTHER => {
                for sensitive_header in &[
                    hyper::header::TRANSFER_ENCODING,
                    hyper::header::CONTENT_ENCODING,
                    hyper::header::CONTENT_TYPE,
                    hyper::header::CONTENT_LENGTH,
                ] {
                    self.pending_request.headers.remove(sensitive_header);
                }

                if self.pending_request.method == Method::HEAD {
                    self.pending_request.method = Method::GET;
                }
            }
            _ => {}
        }

        Some(self.pending_request.to_request()).transpose()
    }

    /// Extracts the location header from the current response and resolves it against the
    /// request URI.
    fn extract_redirect_location_from_response(&self) -> Option<Uri> {
        self.response
            .headers()
            .get(hyper::header::LOCATION)
            .and_then(|location| resolve_location(&self.pending_request.uri, location.as_bytes()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `resolve_location` follows RFC 3986 section 5.2 for the reference forms a `Location`
    /// header can take.
    #[test]
    fn test_resolve_location() {
        let base = Uri::try_from("http://h/dir/start?q").unwrap();
        let resolve = |location: &str| {
            resolve_location(&base, location.as_bytes())
                .map(|uri| uri.to_string())
                .unwrap_or_else(|| "<none>".to_owned())
        };
        assert_eq!("http://h/dir/artifact.tar.gz", resolve("artifact.tar.gz"));
        assert_eq!("http://h/dir/sub/next", resolve("sub/next"));
        assert_eq!("http://h/dir/sub/next?x=1", resolve("sub/next?x=1"));
        assert_eq!("http://h/x", resolve("../x"));
        assert_eq!("http://h/x", resolve("../../x"));
        assert_eq!("http://h/dir/", resolve("./"));
        assert_eq!("http://h/dir/start?page=2", resolve("?page=2"));
        assert_eq!("http://h/dir/start?q", resolve(""));
        assert_eq!("http://h/dir/start?q", resolve("#frag"));
        assert_eq!("http://h/abs?x", resolve("/abs?x"));
        assert_eq!("http://h/b/", resolve("/a/../b/./"));
        assert_eq!("http://mirror.example/f", resolve("//mirror.example/f"));
        assert_eq!("https://other/p", resolve("https://other/p"));
        assert_eq!("<none>", resolve("http://"));
        assert_eq!("<none>", resolve("a b"));
    }
}
