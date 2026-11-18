/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! An HTTP server for tests of downloads.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use sha1::Digest;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::net::TcpStream;

/// What a [`StaticHttpServer`] answers with.
pub enum Files {
    /// This content, whatever the client asks for, as a server standing in for a proxy usually
    /// wants.
    All(Vec<u8>),
    /// The content for each request target or path. A request target is matched first, so a proxy
    /// can be given one entry per absolute URL it will be asked to fetch.
    ByTarget(HashMap<String, Vec<u8>>),
}

impl<K: Into<String>, V: Into<Vec<u8>>, const N: usize> From<[(K, V); N]> for Files {
    fn from(files: [(K, V); N]) -> Self {
        Files::ByTarget(
            files
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        )
    }
}

impl From<Vec<u8>> for Files {
    fn from(content: Vec<u8>) -> Self {
        Files::All(content)
    }
}

/// Serves fixed content on localhost and records every request, so a test can assert on what a
/// download fetched. Recorded requests are `(method, target)`, where the target is what the client
/// asked for: a path for a direct request, an absolute URL for one addressed to a proxy. The server
/// stops when dropped.
///
/// ```ignore
/// let server = StaticHttpServer::start([("/file", content)]).await?;
/// buck.build(&["//:download", "-c", &format!("test.url={}", server.url("/file"))]).await?;
/// assert_eq!(server.count("GET", "/file"), 1);
/// ```
pub struct StaticHttpServer {
    host: String,
    port: u16,
    requests: Arc<Mutex<Vec<(String, String)>>>,
    task: tokio::task::JoinHandle<()>,
}

/// How a [`StaticHttpServer`] serves, before it starts.
pub struct StaticHttpServerBuilder {
    files: Files,
    allow_head: bool,
    host: String,
    redirects: HashMap<String, String>,
}

impl StaticHttpServer {
    /// Starts a server answering with `files`.
    pub async fn start(files: impl Into<Files>) -> std::io::Result<Self> {
        Self::builder(files).start().await
    }

    /// A server answering with `files`, to set up further before it starts.
    pub fn builder(files: impl Into<Files>) -> StaticHttpServerBuilder {
        StaticHttpServerBuilder {
            files: files.into(),
            allow_head: true,
            host: "localhost".to_owned(),
            redirects: HashMap::new(),
        }
    }

    /// The URL of `path` on this server.
    pub fn url(&self, path: &str) -> String {
        format!("http://{}:{}{path}", self.host, self.port)
    }

    /// The requests the server got so far, as `(method, target)`.
    pub fn requests(&self) -> Vec<(String, String)> {
        self.requests.lock().unwrap().clone()
    }

    /// How many requests with `method` for `target` the server got.
    pub fn count(&self, method: &str, target: &str) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, t)| m == method && t == target)
            .count()
    }
}

impl Drop for StaticHttpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl StaticHttpServerBuilder {
    /// Whether the server answers HEAD requests; without, it answers them with 405, like servers
    /// that do not implement HEAD.
    pub fn allow_head(mut self, allow_head: bool) -> Self {
        self.allow_head = allow_head;
        self
    }

    /// The host the server listens on and its URLs name.
    pub fn host(mut self, host: &str) -> Self {
        self.host = host.to_owned();
        self
    }

    /// Request targets to answer with a redirect to a `Location` instead.
    pub fn redirects<'a>(
        mut self,
        redirects: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Self {
        self.redirects.extend(
            redirects
                .into_iter()
                .map(|(t, l)| (t.to_owned(), l.to_owned())),
        );
        self
    }

    /// Starts serving.
    pub async fn start(self) -> std::io::Result<StaticHttpServer> {
        // `localhost` would resolve to `::1` first on some hosts, where clients may well connect to
        // `127.0.0.1`.
        let bind_host = if self.host == "localhost" {
            "127.0.0.1"
        } else {
            self.host.as_str()
        };
        let listener = TcpListener::bind((bind_host, 0)).await?;
        let port = listener.local_addr()?.port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let host = self.host.clone();
        let config = Arc::new(self);
        let recorded = requests.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve(stream, config.clone(), recorded.clone()));
            }
        });
        Ok(StaticHttpServer {
            host,
            port,
            requests,
            task,
        })
    }
}

async fn serve(
    mut stream: TcpStream,
    config: Arc<StaticHttpServerBuilder>,
    requests: Arc<Mutex<Vec<(String, String)>>>,
) {
    let mut head = Vec::new();
    let mut buffer = [0; 4096];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(n) => head.extend_from_slice(&buffer[..n]),
        }
    }
    let head = String::from_utf8_lossy(&head);
    let mut request_line = head.lines().next().unwrap_or_default().split(' ');
    let (Some(method), Some(target)) = (request_line.next(), request_line.next()) else {
        return;
    };
    requests
        .lock()
        .unwrap()
        .push((method.to_owned(), target.to_owned()));
    let response = respond(&config, method, target);
    let _ignored = stream.write_all(&response).await;
    let _ignored = stream.shutdown().await;
}

fn respond(config: &StaticHttpServerBuilder, method: &str, target: &str) -> Vec<u8> {
    if let Some(location) = config.redirects.get(target) {
        return response(302, &[("Location", location)], b"", true);
    }
    let content = match &config.files {
        Files::All(content) => content,
        Files::ByTarget(files) => match files.get(target).or_else(|| files.get(path_of(target))) {
            Some(content) => content,
            None => return response(404, &[], b"", true),
        },
    };
    match method {
        "HEAD" if !config.allow_head => response(405, &[], b"", true),
        // A HEAD response has the GET response's `Content-Length`, but no body.
        "HEAD" => response(200, &[], content, false),
        "GET" => response(200, &[], content, true),
        _ => response(405, &[], b"", true),
    }
}

/// The path of a request target: without the scheme and authority of an absolute URL, and without
/// the query.
fn path_of(target: &str) -> &str {
    let path = match target.split_once("://") {
        Some((_, rest)) => rest.find('/').map_or("/", |i| &rest[i..]),
        None => target,
    };
    path.split('?').next().unwrap_or(path)
}

fn response(status: u16, headers: &[(&str, &str)], content: &[u8], with_body: bool) -> Vec<u8> {
    let reason = match status {
        200 => "OK",
        302 => "Found",
        404 => "Not Found",
        _ => "Method Not Allowed",
    };
    let mut response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n",
        content.len()
    );
    for (name, value) in headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str("\r\n");
    let mut response = response.into_bytes();
    if with_body {
        response.extend_from_slice(content);
    }
    response
}

/// The sha1 `download_file` takes as a checksum: a protocol, not a security choice.
pub fn sha1_hex(content: &[u8]) -> String {
    // @lint-ignore PATTERNLINT poor-choice-of-hash-function
    hex::encode(sha1::Sha1::digest(content))
}

/// `-c` arguments for fixtures whose `download_file` rules read `test.url`, `test.sha1` and
/// `test.sha256` from the config.
pub fn download_configs(url: &str, content: &[u8]) -> Vec<String> {
    vec![
        "-c".to_owned(),
        format!("test.url={url}"),
        "-c".to_owned(),
        format!("test.sha1={}", sha1_hex(content)),
        "-c".to_owned(),
        format!("test.sha256={}", hex::encode(sha2::Sha256::digest(content))),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn get(url: &str, method: &str) -> String {
        let (host_port, path) = url
            .strip_prefix("http://")
            .unwrap()
            .split_once('/')
            .unwrap();
        let mut stream = TcpStream::connect(host_port).await.unwrap();
        stream
            .write_all(format!("{method} /{path} HTTP/1.1\r\nHost: {host_port}\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        response
    }

    #[tokio::test]
    async fn test_serves_files_and_records_requests() {
        let server = StaticHttpServer::start([("/file", "content")])
            .await
            .unwrap();
        let response = get(&server.url("/file"), "GET").await;
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        assert!(response.ends_with("\r\n\r\ncontent"), "{response}");
        assert!(
            get(&server.url("/other"), "GET")
                .await
                .starts_with("HTTP/1.1 404")
        );
        assert_eq!(server.count("GET", "/file"), 1);
        assert_eq!(server.requests().len(), 2);
    }

    #[tokio::test]
    async fn test_head_requests_get_no_body_or_405() {
        let server = StaticHttpServer::start([("/file", "content")])
            .await
            .unwrap();
        let response = get(&server.url("/file"), "HEAD").await;
        assert!(response.contains("Content-Length: 7\r\n"), "{response}");
        assert!(response.ends_with("\r\n\r\n"), "{response}");
        let server = StaticHttpServer::builder([("/file", "content")])
            .allow_head(false)
            .start()
            .await
            .unwrap();
        assert!(
            get(&server.url("/file"), "HEAD")
                .await
                .starts_with("HTTP/1.1 405")
        );
    }

    #[tokio::test]
    async fn test_redirects_answer_with_a_location() {
        let server = StaticHttpServer::builder(b"content".to_vec())
            .redirects([("/old", "/new")])
            .start()
            .await
            .unwrap();
        let response = get(&server.url("/old"), "GET").await;
        assert!(response.starts_with("HTTP/1.1 302"), "{response}");
        assert!(response.contains("Location: /new\r\n"), "{response}");
    }

    #[test]
    fn test_a_proxy_target_matches_by_path() {
        assert_eq!(path_of("http://example.com/a/b?c"), "/a/b");
        assert_eq!(path_of("/a?b"), "/a");
    }

    #[test]
    fn test_checksums_are_hex() {
        assert_eq!(sha1_hex(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    }
}
