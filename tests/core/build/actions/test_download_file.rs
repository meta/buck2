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
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Context as _;
use buck2_e2e::http::StaticHttpServer;
use buck2_e2e::http::sha1_hex;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::random_string;
use sha2::Digest;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::net::TcpStream;

/// `-c` arguments declaring the download the fixture's targets read from the config.
fn configs(
    url: &str,
    sha1: Option<&str>,
    sha256: Option<&str>,
    size: Option<usize>,
) -> Vec<String> {
    configs_with_sha512(url, sha1, sha256, None, size)
}

/// [`configs`], with a sha512 as well.
fn configs_with_sha512(
    url: &str,
    sha1: Option<&str>,
    sha256: Option<&str>,
    sha512: Option<&str>,
    size: Option<usize>,
) -> Vec<String> {
    let mut args = vec!["-c".to_owned(), format!("test.url={url}")];
    if let Some(sha1) = sha1 {
        args.extend(["-c".to_owned(), format!("test.sha1={sha1}")]);
    }
    if let Some(sha256) = sha256 {
        args.extend(["-c".to_owned(), format!("test.sha256={sha256}")]);
    }
    if let Some(sha512) = sha512 {
        args.extend(["-c".to_owned(), format!("test.sha512={sha512}")]);
    }
    if let Some(size) = size {
        args.extend(["-c".to_owned(), format!("test.size_bytes={size}")]);
    }
    args
}

fn sha256_hex(content: &[u8]) -> String {
    hex::encode(sha2::Sha256::digest(content))
}

fn sha512_hex(content: &[u8]) -> String {
    hex::encode(sha2::Sha512::digest(content))
}

/// `args`, then each of `configs`, as a command takes them.
fn with<'a>(args: &[&'a str], configs: &[&'a [String]]) -> Vec<&'a str> {
    args.iter()
        .copied()
        .chain(configs.iter().flat_map(|c| c.iter().map(String::as_str)))
        .collect()
}

/// fbsource prefers BLAKE3-KEYED and allows SHA1, so a checksum's algorithm is not the one
/// the download would hash with.
fn use_fbsource_digests(buck: &Buck) -> anyhow::Result<()> {
    append_buckconfig(buck, "[buck2]\ndigest_algorithms = BLAKE3-KEYED,SHA1\n")
}

fn append_buckconfig(buck: &Buck, text: &str) -> anyhow::Result<()> {
    std::fs::OpenOptions::new()
        .append(true)
        .open(buck.cwd().join(".buckconfig"))?
        .write_all(text.as_bytes())?;
    Ok(())
}

async fn build_and_read(buck: &Buck, target: &str, args: &[&str]) -> anyhow::Result<Vec<u8>> {
    let mut command = vec![target];
    command.extend(args);
    let result = buck.build(&command).await?;
    Ok(std::fs::read(
        result.build_report()?.output_for_target(target)?,
    )?)
}

/// An app on localhost, served as aiohttp serves its `get` routes: `respond` answers a GET of a
/// path it routes with a status and a body, and a HEAD the same without the body; any other path
/// is a 404. The server stops when dropped.
struct App {
    port: u16,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for App {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve<F, Fut>(respond: F) -> anyhow::Result<App>
where
    F: Fn(String) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Option<(u16, Vec<u8>)>> + Send + 'static,
{
    // A Python socket binds `localhost` as IPv4.
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let respond = Arc::new(respond);
    let task = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(answer(stream, respond.clone()));
        }
    });
    Ok(App { port, task })
}

async fn answer<F, Fut>(mut stream: TcpStream, respond: Arc<F>)
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Option<(u16, Vec<u8>)>>,
{
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
    let path = target.split('?').next().unwrap_or(target).to_owned();
    let (status, body) = match respond(path).await {
        None => (404, Vec::new()),
        Some(_) if method != "GET" && method != "HEAD" => (405, Vec::new()),
        Some(reply) => reply,
    };
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        405 => "Method Not Allowed",
        429 => "Too Many Requests",
        _ => "Internal Server Error",
    };
    let mut response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    if method != "HEAD" {
        response.extend_from_slice(&body);
    }
    let _ignored = stream.write_all(&response).await;
    let _ignored = stream.shutdown().await;
}

async fn downloads_declared_each_way(buck: &Buck, shape: &str) -> anyhow::Result<()> {
    let content = random_string().into_bytes();
    let sha1 = sha1_hex(&content);
    let sha256 = sha256_hex(&content);
    let sha512 = sha512_hex(&content);
    let declared = match shape {
        "sha1" => configs("", Some(&sha1), None, None),
        "sha256" => configs("", None, Some(&sha256), None),
        "sha512" => configs_with_sha512("", None, None, Some(&sha512), None),
        "both" => configs("", Some(&sha1), Some(&sha256), None),
        "both_with_size" => configs("", Some(&sha1), Some(&sha256), Some(content.len())),
        "sha512_with_size" => {
            configs_with_sha512("", None, None, Some(&sha512), Some(content.len()))
        }
        "all_three" => configs_with_sha512("", Some(&sha1), Some(&sha256), Some(&sha512), None),
        "sha1_uppercase" => configs("", Some(&sha1.to_uppercase()), None, None),
        "sha512_uppercase" => {
            configs_with_sha512("", None, None, Some(&sha512.to_uppercase()), None)
        }
        _ => anyhow::bail!("Unknown shape `{shape}`"),
    };
    let declared = &declared[2..];
    let server = StaticHttpServer::start([("/file", content.clone())]).await?;
    assert_eq!(
        build_and_read(
            buck,
            "//:copy",
            &with(
                &["--local-only"],
                &[&configs(&server.url("/file"), None, None, None), declared]
            ),
        )
        .await?,
        content
    );
    assert_eq!(server.count("GET", "/file"), 1);
    Ok(())
}

#[buck_test(data_dir = "download", skip_for_os = ["windows"])]
async fn test_downloads_declared_each_way_sha1(buck: &Buck) -> anyhow::Result<()> {
    downloads_declared_each_way(buck, "sha1").await
}

#[buck_test(data_dir = "download", skip_for_os = ["windows"])]
async fn test_downloads_declared_each_way_sha256(buck: &Buck) -> anyhow::Result<()> {
    downloads_declared_each_way(buck, "sha256").await
}

#[buck_test(data_dir = "download", skip_for_os = ["windows"])]
async fn test_downloads_declared_each_way_both(buck: &Buck) -> anyhow::Result<()> {
    downloads_declared_each_way(buck, "both").await
}

#[buck_test(data_dir = "download", skip_for_os = ["windows"])]
async fn test_downloads_declared_each_way_both_with_size(buck: &Buck) -> anyhow::Result<()> {
    downloads_declared_each_way(buck, "both_with_size").await
}

#[buck_test(data_dir = "download", skip_for_os = ["windows"])]
async fn test_downloads_declared_each_way_sha512(buck: &Buck) -> anyhow::Result<()> {
    downloads_declared_each_way(buck, "sha512").await
}

#[buck_test(data_dir = "download", skip_for_os = ["windows"])]
async fn test_downloads_declared_each_way_sha512_with_size(buck: &Buck) -> anyhow::Result<()> {
    downloads_declared_each_way(buck, "sha512_with_size").await
}

#[buck_test(data_dir = "download", skip_for_os = ["windows"])]
async fn test_downloads_declared_each_way_all_three(buck: &Buck) -> anyhow::Result<()> {
    downloads_declared_each_way(buck, "all_three").await
}

#[buck_test(data_dir = "download", skip_for_os = ["windows"])]
async fn test_downloads_declared_each_way_sha512_uppercase(buck: &Buck) -> anyhow::Result<()> {
    downloads_declared_each_way(buck, "sha512_uppercase").await
}

#[buck_test(data_dir = "download", skip_for_os = ["windows"])]
async fn test_downloads_declared_each_way_sha1_uppercase(buck: &Buck) -> anyhow::Result<()> {
    downloads_declared_each_way(buck, "sha1_uppercase").await
}

#[buck_test(data_dir = "download")]
async fn test_retries_transient_errors(buck: &Buck) -> anyhow::Result<()> {
    let attempt = Arc::new(AtomicUsize::new(0));
    let body = random_string().into_bytes();

    let app = {
        let (attempt, body) = (attempt.clone(), body.clone());
        serve(move |path| {
            let (attempt, body) = (attempt.clone(), body.clone());
            async move {
                if path != "/" {
                    return None;
                }
                let attempt = attempt.fetch_add(1, Ordering::SeqCst) + 1;
                if attempt > 2 {
                    return Some((200, body));
                }
                if attempt > 1 {
                    return Some((500, Vec::new()));
                }
                Some((429, Vec::new()))
            }
        })
        .await?
    };

    buck.build(&with(
        &["//:download"],
        &[&configs(
            &format!("http://localhost:{}", app.port),
            Some(&sha1_hex(&body)),
            None,
            None,
        )],
    ))
    .await?;

    drop(app);

    // HEAD, then the GET's two retried errors and its success.
    assert_eq!(attempt.load(Ordering::SeqCst), 4);
    Ok(())
}

#[buck_test(data_dir = "download")]
async fn test_a_server_without_head_support(buck: &Buck) -> anyhow::Result<()> {
    let content = random_string().into_bytes();
    let server = StaticHttpServer::builder([("/file", content.clone())])
        .allow_head(false)
        .start()
        .await?;
    assert_eq!(
        build_and_read(
            buck,
            "//:download",
            &with(
                &[],
                &[&configs(
                    &server.url("/file"),
                    Some(&sha1_hex(&content)),
                    None,
                    None
                )]
            ),
        )
        .await?,
        content
    );
    assert_eq!(server.count("HEAD", "/file"), 1);
    assert_eq!(server.count("GET", "/file"), 1);
    Ok(())
}

/// A relative `Location` is resolved against the request URI (RFC 3986 section 5.2), so the
/// redirect stays on the server instead of going to a host named after the file.
#[buck_test(data_dir = "download")]
async fn test_follows_a_relative_location(buck: &Buck) -> anyhow::Result<()> {
    let content = random_string().into_bytes();
    let server = StaticHttpServer::builder([("/dir/artifact.bin", content.clone())])
        .redirects([("/dir/start", "artifact.bin")])
        .start()
        .await?;
    assert_eq!(
        build_and_read(
            buck,
            "//:download",
            &with(
                &[],
                &[&configs(
                    &server.url("/dir/start"),
                    Some(&sha1_hex(&content)),
                    None,
                    None
                )]
            ),
        )
        .await?,
        content
    );
    assert_eq!(server.count("GET", "/dir/start"), 1);
    // The HEAD and the GET of `/dir/start` were both redirected to the file.
    assert!(server.count("GET", "/dir/artifact.bin") >= 1);
    Ok(())
}

#[buck_test(data_dir = "download")]
async fn test_times_out_after_retries(buck: &Buck) -> anyhow::Result<()> {
    let body = random_string().into_bytes();
    let sha1 = sha1_hex(&body);

    let attempt = Arc::new(AtomicUsize::new(0));

    let app = {
        let (attempt, body) = (attempt.clone(), body.clone());
        serve(move |path| {
            let (attempt, body) = (attempt.clone(), body.clone());
            async move {
                match path.as_str() {
                    "/always_times_out" => {
                        tokio::time::sleep(Duration::from_secs(3)).await;
                        Some((200, body))
                    }
                    "/times_out_twice" => {
                        let attempt = attempt.fetch_add(1, Ordering::SeqCst) + 1;
                        if attempt > 2 {
                            return Some((200, body));
                        }
                        tokio::time::sleep(Duration::from_secs(3)).await;
                        Some((200, body))
                    }
                    _ => None,
                }
            }
        })
        .await?
    };

    let url = format!("http://localhost:{}", app.port);

    // These are daemon startup configs, need these to be written in a buckconfig rather
    // than passed as an invocation config.
    //
    // Add an aggressive read timeout.
    append_buckconfig(buck, "[http]\nread_timeout_ms = 50\n")?;

    buck.build(&with(
        &["//:download"],
        &[&configs(
            &format!("{url}/always_times_out"),
            Some(&sha1),
            None,
            None,
        )],
    ))
    .await
    .expect_failure()
    .assert_stderr_matches("Timed out while making request to");

    buck.build(&with(
        &["//:download"],
        &[&configs(
            &format!("{url}/times_out_twice"),
            Some(&sha1),
            None,
            None,
        )],
    ))
    .await?;

    drop(app);
    Ok(())
}

#[buck_test(data_dir = "download")]
async fn test_a_missing_url_and_a_dead_server(buck: &Buck) -> anyhow::Result<()> {
    let content = random_string().into_bytes();
    let server = StaticHttpServer::start([("/file", content.clone())]).await?;
    buck.build(&with(
        &["//:download"],
        &[&configs(
            &server.url("/gone"),
            Some(&sha1_hex(&content)),
            None,
            None,
        )],
    ))
    .await
    .expect_failure()
    .assert_stderr_matches("404 Not Found");
    // Nothing listens on port 1.
    buck.build(&with(
        &["//:download"],
        &[&configs(
            "http://localhost:1/file",
            Some(&sha1_hex(&content)),
            None,
            None,
        )],
    ))
    .await
    .expect_failure()
    .assert_stderr_matches("Error performing http_download request|Connection refused");
    Ok(())
}

#[buck_test(data_dir = "download")]
async fn test_declarations_that_cannot_be_right(buck: &Buck) -> anyhow::Result<()> {
    let content = random_string().into_bytes();
    let server = StaticHttpServer::start([("/file", content.clone())]).await?;
    let url = server.url("/file");
    buck.build(&with(&["//:download"], &[&configs(&url, None, None, None)]))
        .await
        .expect_failure()
        .assert_stderr_matches("Must pass in at least one checksum");
    buck.build(&with(
        &["//:download"],
        &[&configs(&url, Some("xxxxxx"), None, None)],
    ))
    .await
    .expect_failure()
    .assert_stderr_matches("Invalid digest for `sha1` argument");
    // A sha512 is 128 hex characters: a sha256 is too short, and `z` is not hex.
    let sha256 = sha256_hex(&content);
    buck.build(&with(
        &["//:download"],
        &[&configs_with_sha512(&url, None, None, Some(&sha256), None)],
    ))
    .await
    .expect_failure()
    .assert_stderr_matches(&format!(
        "Invalid digest for `sha512` argument, expected length of 128 but got 64, digest `{sha256}`"
    ));
    buck.build(&with(
        &["//:download"],
        &[&configs_with_sha512(
            &url,
            None,
            None,
            Some(&"z".repeat(128)),
            None,
        )],
    ))
    .await
    .expect_failure()
    .assert_stderr_matches(
        "Invalid digest for `sha512` argument, expected 0-9 a-z hex characters, but got `z`",
    );
    // Nothing was fetched for those.
    assert_eq!(server.count("GET", "/file"), 0);
    Ok(())
}

#[buck_test(data_dir = "download")]
async fn test_checksums_the_content_does_not_match(buck: &Buck) -> anyhow::Result<()> {
    let content = random_string().into_bytes();
    let zeros40 = "0".repeat(40);
    let zeros64 = "0".repeat(64);
    let server = StaticHttpServer::start([("/file", content.clone())]).await?;
    let url = server.url("/file");
    buck.build(&with(
        &["//:download"],
        &[&configs(&url, Some(&zeros40), None, None)],
    ))
    .await
    .expect_failure()
    .assert_stderr_matches("Invalid sha1 digest");
    buck.build(&with(
        &["//:download"],
        &[&configs(&url, Some(&zeros40), None, Some(content.len()))],
    ))
    .await
    .expect_failure()
    .assert_stderr_matches("Invalid sha1 digest");
    buck.build(&with(
        &["//:download"],
        &[&configs(&url, None, Some(&zeros64), None)],
    ))
    .await
    .expect_failure()
    .assert_stderr_matches("Invalid sha256 digest");
    buck.build(&with(
        &["//:download"],
        &[&configs_with_sha512(
            &url,
            None,
            None,
            Some(&"0".repeat(128)),
            None,
        )],
    ))
    .await
    .expect_failure()
    .assert_stderr_matches(&format!(
        "Invalid sha512 digest. Expected {}, got {}. URL: {url}",
        "0".repeat(128),
        sha512_hex(&content)
    ));
    // Content the CAS does not have is downloaded, and every checksum named is held
    // against it.
    buck.build(&with(
        &["//:download"],
        &[&configs(
            &url,
            Some(&sha1_hex(&content)),
            Some(&zeros64),
            None,
        )],
    ))
    .await
    .expect_failure()
    .assert_stderr_matches("Invalid sha256 digest");
    // Each of the three is enforced when the other two are right.
    for wrong in ["sha1", "sha256", "sha512"] {
        let digest = |algorithm: &str, right: String| {
            if algorithm == wrong {
                "0".repeat(right.len())
            } else {
                right
            }
        };
        buck.build(&with(
            &["//:download"],
            &[&configs_with_sha512(
                &url,
                Some(&digest("sha1", sha1_hex(&content))),
                Some(&digest("sha256", sha256_hex(&content))),
                Some(&digest("sha512", sha512_hex(&content))),
                None,
            )],
        ))
        .await
        .expect_failure()
        .assert_stderr_matches(&format!("Invalid {wrong} digest"));
    }
    Ok(())
}

#[buck_test(data_dir = "download")]
async fn test_a_declared_size_of_zero_is_reported_by_the_download(
    buck: &Buck,
) -> anyhow::Result<()> {
    let content = random_string().into_bytes();
    let server = StaticHttpServer::start([("/file", content.clone())]).await?;
    // A checksum paired with a size of 0 is not the empty file, so the CAS is not asked
    // about it: the download runs and reports the mismatch like any other wrong size.
    buck.build(&with(
        &["//:download"],
        &[&configs(
            &server.url("/file"),
            Some(&sha1_hex(&content)),
            None,
            Some(0),
        )],
    ))
    .await
    .expect_failure()
    .assert_stderr_matches(&format!(
        "Downloaded size \\({}\\) does not match expected size \\(0\\)",
        content.len()
    ));
    assert_eq!(server.count("GET", "/file"), 1);
    Ok(())
}

#[buck_test(data_dir = "download")]
async fn test_a_wrong_size_is_reported_by_the_download(buck: &Buck) -> anyhow::Result<()> {
    let content = random_string().into_bytes();
    let server = StaticHttpServer::start([("/file", content.clone())]).await?;
    buck.build(&with(
        &["//:download"],
        &[&configs(
            &server.url("/file"),
            Some(&sha1_hex(&content)),
            None,
            Some(content.len() + 1),
        )],
    ))
    .await
    .expect_failure()
    .assert_stderr_matches(&format!(
        "Downloaded size \\({}\\) does not match expected size \\({}\\)",
        content.len(),
        content.len() + 1
    ));
    // A sha256 under a digest config that only accepts sha1 declares no metadata, and
    // `size_bytes` is still checked.
    use_fbsource_digests(buck)?;
    buck.build(&with(
        &["//:download"],
        &[&configs(
            &server.url("/file"),
            None,
            Some(&sha256_hex(&content)),
            Some(content.len() + 1),
        )],
    ))
    .await
    .expect_failure()
    .assert_stderr_matches(&format!(
        "Downloaded size \\({}\\) does not match expected size \\({}\\)",
        content.len(),
        content.len() + 1
    ));
    // A sha512 alone does not name a CAS digest, so `size_bytes` is all there is to check the
    // size against.
    buck.build(&with(
        &["//:download"],
        &[&configs_with_sha512(
            &server.url("/file"),
            None,
            None,
            Some(&sha512_hex(&content)),
            Some(content.len() + 1),
        )],
    ))
    .await
    .expect_failure()
    .assert_stderr_matches(&format!(
        "Downloaded size \\({}\\) does not match expected size \\({}\\)",
        content.len(),
        content.len() + 1
    ));
    Ok(())
}

#[buck_test(data_dir = "download", skip_for_os = ["windows"])]
async fn test_a_content_based_path_under_a_different_preferred_digest(
    buck: &Buck,
) -> anyhow::Result<()> {
    // The output's value is what consumers resolve a content-based path from, so it has to be
    // the value the path was built from: the declared one, under the checksum's algorithm.
    use_fbsource_digests(buck)?;
    let content = random_string().into_bytes();
    let server = StaticHttpServer::start([("/file", content.clone())]).await?;
    assert_eq!(
        build_and_read(
            buck,
            "//:copy_content_based",
            &with(
                &["--local-only"],
                &[&configs(
                    &server.url("/file"),
                    Some(&sha1_hex(&content)),
                    None,
                    None
                )]
            ),
        )
        .await?,
        content
    );
    Ok(())
}

async fn fetches_once_across_restarts(buck: &Buck, digest_algorithm: &str) -> anyhow::Result<()> {
    append_buckconfig(
        buck,
        &format!(
            "[buck2]\ndigest_algorithms = {digest_algorithm}\nsqlite_materializer_state = true\n"
        ),
    )?;

    let content = random_string().into_bytes();
    let server = StaticHttpServer::start([("/file", content.clone())]).await?;
    let declared = configs(
        &server.url("/file"),
        Some(&sha1_hex(&content)),
        Some(&sha256_hex(&content)),
        None,
    );
    let target = "//:download";

    let res = buck.build(&with(&[target], &[&declared])).await?;
    let output = res.build_report()?.output_for_target(target)?;
    assert_eq!(std::fs::read(&output)?, content);
    assert_eq!(server.count("GET", "/file"), 1);

    // The file is still on disk, so a new daemon has no reason to fetch it again.
    buck.kill().await?;
    buck.build(&with(&[target], &[&declared])).await?;
    assert_eq!(std::fs::read(&output)?, content);
    assert_eq!(server.count("GET", "/file"), 1);
    Ok(())
}

#[buck_test(data_dir = "download")]
#[expect(non_snake_case, reason = "the Python harness's name for the case")]
async fn test_fetches_once_across_restarts_SHA1(buck: &Buck) -> anyhow::Result<()> {
    fetches_once_across_restarts(buck, "SHA1").await
}

#[buck_test(data_dir = "download")]
#[expect(non_snake_case, reason = "the Python harness's name for the case")]
async fn test_fetches_once_across_restarts_SHA256(buck: &Buck) -> anyhow::Result<()> {
    fetches_once_across_restarts(buck, "SHA256").await
}

#[buck_test(data_dir = "download")]
async fn test_is_uploaded_from_disk(buck: &mut Buck) -> anyhow::Result<()> {
    let content = random_string().into_bytes();
    // Force the uploader to treat the download as missing from the CAS even if it is there.
    buck.set_env(
        "BUCK2_TEST_INJECTED_MISSING_DIGESTS",
        format!("{}:{}", sha1_hex(&content), content.len()),
    );
    let server = StaticHttpServer::start([("/file", content.clone())]).await?;
    buck.build(&with(
        &["//:copy", "--remote-only", "--no-remote-cache"],
        &[&configs(
            &server.url("/file"),
            Some(&sha1_hex(&content)),
            None,
            None,
        )],
    ))
    .await?;
    assert_eq!(server.count("GET", "/file"), 1);
    Ok(())
}

#[buck_test(data_dir = "download")]
async fn test_uses_the_cas_when_it_has_the_content(buck: &Buck) -> anyhow::Result<()> {
    let content = random_string().into_bytes();
    let server = StaticHttpServer::start([("/file", content.clone())]).await?;
    let declared = configs(&server.url("/file"), Some(&sha1_hex(&content)), None, None);

    // Running an RE action on the download uploads its content to the CAS.
    buck.build(&with(
        &["//:copy", "--remote-only", "--no-remote-cache"],
        &[&declared],
    ))
    .await?;
    assert_eq!(server.count("GET", "/file"), 1);

    // A second download of the same content finds it in the CAS, so it neither contacts
    // the server nor touches the disk until something needs the file...
    let target = "//:download_again";
    let res = buck
        .build(&with(&[target, "--materializations=none"], &[&declared]))
        .await?;
    let output = res.build_report()?.output_for_target(target)?;
    assert!(!output.exists());
    assert_eq!(server.count("GET", "/file"), 1);

    // ...and then it comes out of the CAS.
    buck.build(&with(&[target], &[&declared])).await?;
    assert_eq!(std::fs::read(&output)?, content);
    assert_eq!(server.count("GET", "/file"), 1);
    Ok(())
}

#[buck_test(data_dir = "download")]
async fn test_content_the_cas_holds_needs_no_url(buck: &Buck) -> anyhow::Result<()> {
    let content = random_string().into_bytes();
    let server = StaticHttpServer::start([("/file", content.clone())]).await?;
    // With a size declared, the download is fully described without contacting the URL.
    let declared = configs(
        &server.url("/file"),
        Some(&sha1_hex(&content)),
        None,
        Some(content.len()),
    );
    buck.build(&with(
        &["//:copy", "--remote-only", "--no-remote-cache"],
        &[&declared],
    ))
    .await?;
    assert_eq!(server.count("GET", "/file"), 1);

    // A new daemon and a URL nothing answers on: the declaration names content the CAS
    // holds, so a remote consumer and a local one both get it without the URL.
    buck.kill().await?;
    let broken = configs(
        "http://localhost:1/file",
        Some(&sha1_hex(&content)),
        None,
        Some(content.len()),
    );
    buck.build(&with(
        &["//:copy", "--remote-only", "--no-remote-cache"],
        &[&broken],
    ))
    .await?;
    assert_eq!(
        build_and_read(buck, "//:copy_again", &with(&["--local-only"], &[&broken])).await?,
        content
    );
    assert_eq!(server.count("GET", "/file"), 1);
    Ok(())
}

#[buck_test(data_dir = "download")]
async fn test_a_second_checksum_is_not_checked_against_content_the_cas_serves(
    buck: &Buck,
) -> anyhow::Result<()> {
    let content = random_string().into_bytes();
    let server = StaticHttpServer::start([("/file", content.clone())]).await?;
    buck.build(&with(
        &["//:copy", "--remote-only", "--no-remote-cache"],
        &[&configs(
            &server.url("/file"),
            Some(&sha1_hex(&content)),
            None,
            None,
        )],
    ))
    .await?;
    assert_eq!(server.count("GET", "/file"), 1);

    // The CAS vouches for the sha1 and serves the content under it; the sha256 the
    // declaration also names is never held against bytes nobody fetches.
    assert_eq!(
        build_and_read(
            buck,
            "//:copy_again",
            &with(
                &["--local-only"],
                &[&configs(
                    &server.url("/file"),
                    Some(&sha1_hex(&content)),
                    Some(&"0".repeat(64)),
                    None
                )]
            ),
        )
        .await?,
        content
    );
    assert_eq!(server.count("GET", "/file"), 1);
    Ok(())
}

#[buck_test(data_dir = "download")]
async fn test_a_wrong_size_is_a_miss_in_the_cas(buck: &Buck) -> anyhow::Result<()> {
    let content = random_string().into_bytes();
    let server = StaticHttpServer::start([("/file", content.clone())]).await?;
    buck.build(&with(
        &["//:copy", "--remote-only", "--no-remote-cache"],
        &[&configs(
            &server.url("/file"),
            Some(&sha1_hex(&content)),
            None,
            None,
        )],
    ))
    .await?;
    assert_eq!(server.count("GET", "/file"), 1);

    // The CAS names content by hash and size, so the content is not there under the wrong
    // size: the download runs and reports the mismatch.
    buck.build(&with(
        &["//:download_again"],
        &[&configs(
            &server.url("/file"),
            Some(&sha1_hex(&content)),
            None,
            Some(content.len() + 1),
        )],
    ))
    .await
    .expect_failure()
    .assert_stderr_matches(&format!(
        "Downloaded size \\({}\\) does not match expected size \\({}\\)",
        content.len(),
        content.len() + 1
    ));
    assert_eq!(server.count("GET", "/file"), 2);
    Ok(())
}

/// A checksum is part of an action, and an action is part of an analysis result, which can be
/// paged out to disk and back while a daemon runs.
async fn a_paged_out_download_is_read_back(buck: &Buck, shape: &[&str]) -> anyhow::Result<()> {
    std::fs::write(
        buck.settings_home_dir().join(".bucksettings.local.toml"),
        "[hydration]\nenable_paging = true\npage_out_min_free_disk_gb = 0\n",
    )?;
    let content = random_string().into_bytes();
    let digest = |algorithm: &str, hex: fn(&[u8]) -> String| {
        shape.contains(&algorithm).then(|| hex(&content))
    };
    let server = StaticHttpServer::start([("/file", content.clone())]).await?;
    let declared = configs_with_sha512(
        &server.url("/file"),
        digest("sha1", sha1_hex).as_deref(),
        digest("sha256", sha256_hex).as_deref(),
        digest("sha512", sha512_hex).as_deref(),
        None,
    );
    assert_eq!(
        build_and_read(buck, "//:download", &with(&[], &[&declared])).await?,
        content
    );

    buck.debug(&["hydration", "page-out"]).await?;
    let status = buck.debug(&["hydration", "status"]).await?.stdout;
    let paged_out: u64 = regex::Regex::new(r"(\d+) paged out")?
        .captures(&status)
        .with_context(|| format!("unexpected status output:\n{status}"))?[1]
        .parse()?;
    assert!(paged_out > 0, "{status}");

    // A target that reads the download needs its analysis result back, which holds the action
    // and with it the checksum.
    let result = buck
        .build(&with(&["//:copy", "--local-only"], &[&declared]))
        .await?;
    assert_eq!(
        std::fs::read(result.build_report()?.output_for_target("//:copy")?)?,
        content
    );
    let record = result.invocation_record()?;
    let analysis = &record["page_in_by_key_type"]["AnalysisKey"];
    assert!(
        analysis["count"].as_u64().is_some_and(|count| count > 0),
        "{analysis}"
    );
    assert_eq!(server.count("GET", "/file"), 1);
    Ok(())
}

#[buck_test(data_dir = "download", skip_for_os = ["windows"], write_invocation_record = true)]
async fn test_a_paged_out_download_is_read_back_sha1(buck: &Buck) -> anyhow::Result<()> {
    a_paged_out_download_is_read_back(buck, &["sha1"]).await
}

#[buck_test(data_dir = "download", skip_for_os = ["windows"], write_invocation_record = true)]
async fn test_a_paged_out_download_is_read_back_sha512(buck: &Buck) -> anyhow::Result<()> {
    a_paged_out_download_is_read_back(buck, &["sha512"]).await
}

#[buck_test(data_dir = "download", skip_for_os = ["windows"], write_invocation_record = true)]
async fn test_a_paged_out_download_is_read_back_all_three(buck: &Buck) -> anyhow::Result<()> {
    a_paged_out_download_is_read_back(buck, &["sha1", "sha256", "sha512"]).await
}
