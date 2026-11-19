/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::future::Ready;
use std::future::ready;
use std::io;
use std::task::Context;
use std::task::Poll;

use futures::future::Either;
use hyper_util::client::legacy::connect::dns::GaiResolver;
use hyper_util::client::legacy::connect::dns::Name;
use tower_service::Service;

#[derive(Clone)]
pub(crate) struct DnsResolver<R = GaiResolver> {
    inner: R,
    absolute_names: bool,
}

impl DnsResolver {
    pub(crate) fn new(absolute_names: bool) -> Self {
        Self {
            inner: GaiResolver::new(),
            absolute_names,
        }
    }
}

impl<R> DnsResolver<R> {
    fn lookup_name(&self, name: Name) -> io::Result<Name> {
        if !self.absolute_names || name.as_str().ends_with('.') {
            return Ok(name);
        }
        format!("{}.", name.as_str())
            .parse()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
    }
}

impl<R> Service<Name> for DnsResolver<R>
where
    R: Service<Name, Error = io::Error>,
{
    type Response = R::Response;
    type Error = io::Error;
    type Future = Either<R::Future, Ready<io::Result<R::Response>>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, name: Name) -> Self::Future {
        match self.lookup_name(name) {
            Ok(name) => Either::Left(self.inner.call(name)),
            Err(error) => Either::Right(ready(Err(error))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::iter::Once;
    use std::iter::once;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::Mutex;

    use bytes::Bytes;
    use dupe::Dupe;
    use http::Method;
    use http_body_util::BodyExt;
    use http_body_util::Full;
    use httptest::Expectation;
    use httptest::matchers::*;
    use httptest::responders;
    use hyper::Request;
    use hyper_util::client::legacy::Client;
    use hyper_util::client::legacy::connect::HttpConnector;
    use hyper_util::rt::TokioExecutor;

    use super::*;

    #[derive(Clone)]
    struct RecordingResolver {
        addr: SocketAddr,
        names: Arc<Mutex<Vec<String>>>,
    }

    impl Service<Name> for RecordingResolver {
        type Response = Once<SocketAddr>;
        type Error = io::Error;
        type Future = Ready<io::Result<Self::Response>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, name: Name) -> Self::Future {
            self.names.lock().unwrap().push(name.as_str().to_owned());
            ready(Ok(once(self.addr)))
        }
    }

    #[tokio::test]
    async fn test_absolute_dns_preserves_upload_host() -> buck2_error::Result<()> {
        let server = httptest::Server::run();
        let names = Arc::new(Mutex::new(Vec::new()));
        let resolver = DnsResolver {
            inner: RecordingResolver {
                addr: server.addr(),
                names: names.dupe(),
            },
            absolute_names: true,
        };
        let connector = HttpConnector::new_with_resolver(resolver);
        let client = Client::builder(TokioExecutor::new()).build(connector);
        let authority = format!("manifold.facebook.net:{}", server.addr().port());
        let uri = format!("http://{authority}/upload");
        for method in [Method::PUT, Method::POST] {
            server.expect(
                Expectation::matching(all_of![
                    request::method_path(method.as_str().to_owned(), "/upload"),
                    request::headers(contains(("host", authority.clone()))),
                ])
                .respond_with(responders::status_code(200)),
            );
            let request = Request::builder()
                .method(method)
                .uri(&uri)
                .body(Full::new(Bytes::from_static(b"chunk")))?;
            let response = client.request(request).await?;
            assert_eq!(response.status(), 200);
            response.into_body().collect().await?;
        }
        let names = names.lock().unwrap();
        assert!(!names.is_empty());
        assert!(names.iter().all(|name| name == "manifold.facebook.net."));
        Ok(())
    }
}
