/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::time::Duration;

use anyhow::Context;
use buck2_e2e::http::Files;
use buck2_e2e::http::StaticHttpServer;
use buck2_e2e::prelude::*;
use sha2::Digest;

const PROXY_ENV_VARS: [&str; 6] = [
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "NO_PROXY",
    "no_proxy",
];

/// What every test starts with: none of the proxy variables of the test's own environment.
fn set_up(buck: &mut Buck) {
    for name in PROXY_ENV_VARS {
        buck.remove_env(name);
    }
}

fn make_payload() -> Vec<u8> {
    format!("download contents {}\n", uuid::Uuid::new_v4()).into_bytes()
}

/// A server on the host this file's allowlists name, answering every request with `payload`, or
/// routing by what was asked for when given a mapping.
async fn serve(payload: impl Into<Files>) -> anyhow::Result<StaticHttpServer> {
    serve_on(payload, "127.0.0.1", &[]).await
}

async fn serve_on(
    payload: impl Into<Files>,
    host: &str,
    redirects: &[(&str, &str)],
) -> anyhow::Result<StaticHttpServer> {
    Ok(StaticHttpServer::builder(payload)
        .host(host)
        .redirects(redirects.iter().copied())
        .start()
        .await?)
}

fn configure(buck: &Buck, allowlist: Option<&str>, digest: &str) -> anyhow::Result<()> {
    for name in PROXY_ENV_VARS {
        assert!(
            buck.env_var(name).is_none(),
            "{name} leaked into the test environment"
        );
    }
    let mut config = format!("[buck2]\ndigest_algorithms = {digest}\n");
    if let Some(allowlist) = allowlist {
        config.push_str(&format!("[http]\nproxy_env_allowlist = {allowlist}\n"));
    }
    std::fs::write(buck.cwd().join(".buckconfig.local"), config)?;
    Ok(())
}

fn build_args(payload: &[u8], origin: &StaticHttpServer, size: bool) -> Vec<String> {
    let mut args = vec![
        "--local-only".to_owned(),
        "--no-remote-cache".to_owned(),
        "-c".to_owned(),
        format!("test.url={}", origin.url("/download")),
        "-c".to_owned(),
        format!("test.sha256={}", hex::encode(sha2::Sha256::digest(payload))),
    ];
    if size {
        args.extend([
            "-c".to_owned(),
            format!("test.size_bytes={}", payload.len()),
        ]);
    }
    args
}

/// `buck2 build //:download <args>` with `env`.
async fn build(
    buck: &Buck,
    args: &[String],
    env: &[(&str, String)],
) -> anyhow::Result<buck2_e2e::BuckOutput> {
    let mut command: Vec<&str> = vec!["//:download"];
    command.extend(args.iter().map(String::as_str));
    let mut build = buck.build(&command);
    for (key, value) in env {
        build = build.env(key, value);
    }
    Ok(tokio::time::timeout(Duration::from_secs(90), build).await??)
}

async fn build_download(
    buck: &Buck,
    payload: &[u8],
    origin: &StaticHttpServer,
    env: &[(&str, String)],
    size: bool,
) -> anyhow::Result<()> {
    let result = build(buck, &build_args(payload, origin, size), env).await?;
    let output = result
        .build_report()?
        .output_for_target("root//:download")?;
    assert_eq!(std::fs::read(output)?, payload);
    Ok(())
}

async fn daemon_pid(buck: &Buck, env: &[(&str, String)]) -> anyhow::Result<u64> {
    let mut status = buck.status(&[]);
    for (key, value) in env {
        status = status.env(key, value);
    }
    let result = tokio::time::timeout(Duration::from_secs(30), status).await??;
    serde_json::from_str::<serde_json::Value>(&result.stdout)?["process_info"]["pid"]
        .as_u64()
        .context("No `process_info.pid` in `buck2 status`")
}

/// Requests as `StaticHttpServer::requests` lists them.
fn requests(requests: &[(&str, &str)]) -> Vec<(String, String)> {
    requests
        .iter()
        .map(|(method, target)| ((*method).to_owned(), (*target).to_owned()))
        .collect()
}

/// The requests `server` got for downloads. On macOS, buck2's InternGraph client honors
/// `ALL_PROXY` and tunnels through the proxy with `CONNECT`s that have nothing to do with
/// `download_file`; the Python server never recorded those.
fn download_requests(server: &StaticHttpServer) -> Vec<(String, String)> {
    server
        .requests()
        .into_iter()
        .filter(|(method, _)| method != "CONNECT")
        .collect()
}

async fn requires_client_opt_in(buck: &mut Buck, allowlist: Option<&str>) -> anyhow::Result<()> {
    set_up(buck);
    configure(buck, allowlist, "SHA256")?;
    let payload = make_payload();
    let origin = serve(payload.clone()).await?;
    let proxy = serve(payload.clone()).await?;
    build_download(
        buck,
        &payload,
        &origin,
        &[("HTTP_PROXY", proxy.url(""))],
        false,
    )
    .await?;
    assert_eq!(proxy.requests(), requests(&[]));
    assert_eq!(
        origin.requests(),
        requests(&[("HEAD", "/download"), ("GET", "/download")])
    );
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_requires_client_opt_in_unset(buck: &mut Buck) -> anyhow::Result<()> {
    requires_client_opt_in(buck, None).await
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_requires_client_opt_in_empty(buck: &mut Buck) -> anyhow::Result<()> {
    requires_client_opt_in(buck, Some("")).await
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_requires_client_opt_in_whitespace(buck: &mut Buck) -> anyhow::Result<()> {
    requires_client_opt_in(buck, Some("   ")).await
}

async fn proxy_download_paths(
    buck: &mut Buck,
    digest: &str,
    size: bool,
    methods: &[&str],
) -> anyhow::Result<()> {
    set_up(buck);
    configure(buck, Some("127.0.0.1"), digest)?;
    let payload = make_payload();
    let origin = serve(payload.clone()).await?;
    let proxy = serve(payload.clone()).await?;
    build_download(
        buck,
        &payload,
        &origin,
        &[("HTTP_PROXY", proxy.url(""))],
        size,
    )
    .await?;
    assert_eq!(origin.requests(), requests(&[]));
    let url = origin.url("/download");
    assert_eq!(
        proxy.requests(),
        requests(
            &methods
                .iter()
                .map(|method| (*method, url.as_str()))
                .collect::<Vec<_>>()
        )
    );
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_proxy_download_paths_get_only(buck: &mut Buck) -> anyhow::Result<()> {
    proxy_download_paths(buck, "SHA1", false, &["GET"]).await
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_proxy_download_paths_head_then_get(buck: &mut Buck) -> anyhow::Result<()> {
    proxy_download_paths(buck, "SHA256", false, &["HEAD", "GET"]).await
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_proxy_download_paths_sized_get_only(buck: &mut Buck) -> anyhow::Result<()> {
    proxy_download_paths(buck, "SHA256", true, &["GET"]).await
}

async fn proxy_environment_semantics(
    buck: &mut Buck,
    values: &[(&str, &str)],
    proxied: bool,
) -> anyhow::Result<()> {
    set_up(buck);
    configure(buck, Some("127.0.0.1"), "SHA256")?;
    let payload = make_payload();
    let origin = serve(payload.clone()).await?;
    let proxy = serve(payload.clone()).await?;
    let env: Vec<(&str, String)> = values
        .iter()
        .map(|(name, value)| {
            let value = match *value {
                "origin" => origin.url(""),
                "proxy" => proxy.url(""),
                value => value.to_owned(),
            };
            (*name, value)
        })
        .collect();
    build_download(buck, &payload, &origin, &env, false).await?;
    if proxied {
        assert_eq!(origin.requests(), requests(&[]));
        let url = origin.url("/download");
        assert_eq!(
            download_requests(&proxy),
            requests(&[("HEAD", url.as_str()), ("GET", url.as_str())])
        );
    } else {
        assert_eq!(download_requests(&proxy), requests(&[]));
        assert_eq!(
            origin.requests(),
            requests(&[("HEAD", "/download"), ("GET", "/download")])
        );
    }
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_proxy_environment_semantics_all_unset(buck: &mut Buck) -> anyhow::Result<()> {
    proxy_environment_semantics(buck, &[], false).await
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_proxy_environment_semantics_lowercase(buck: &mut Buck) -> anyhow::Result<()> {
    proxy_environment_semantics(buck, &[("http_proxy", "proxy")], true).await
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_proxy_environment_semantics_uppercase_precedence(
    buck: &mut Buck,
) -> anyhow::Result<()> {
    proxy_environment_semantics(
        buck,
        &[("HTTP_PROXY", "proxy"), ("http_proxy", "origin")],
        true,
    )
    .await
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_proxy_environment_semantics_no_proxy_exclusion(
    buck: &mut Buck,
) -> anyhow::Result<()> {
    proxy_environment_semantics(
        buck,
        &[("HTTP_PROXY", "proxy"), ("NO_PROXY", "127.0.0.1")],
        false,
    )
    .await
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_proxy_environment_semantics_lowercase_no_proxy_exclusion(
    buck: &mut Buck,
) -> anyhow::Result<()> {
    proxy_environment_semantics(
        buck,
        &[("HTTP_PROXY", "proxy"), ("no_proxy", "127.0.0.1")],
        false,
    )
    .await
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_proxy_environment_semantics_no_proxy_alone(buck: &mut Buck) -> anyhow::Result<()> {
    proxy_environment_semantics(buck, &[("NO_PROXY", "127.0.0.1")], false).await
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_proxy_environment_semantics_nonstandard_variables_ignored(
    buck: &mut Buck,
) -> anyhow::Result<()> {
    proxy_environment_semantics(
        buck,
        &[("SDK_PROXY", "proxy"), ("Http_Proxy", "proxy")],
        false,
    )
    .await
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_proxy_environment_semantics_all_proxy_ignored(buck: &mut Buck) -> anyhow::Result<()> {
    proxy_environment_semantics(
        buck,
        &[("ALL_PROXY", "proxy"), ("all_proxy", "proxy")],
        false,
    )
    .await
}

async fn invalid_proxy_values_are_rejected(
    buck: &mut Buck,
    values: &[(&str, &str)],
) -> anyhow::Result<()> {
    set_up(buck);
    configure(buck, Some("127.0.0.1"), "SHA256")?;
    let payload = make_payload();
    let origin = serve(payload.clone()).await?;
    let mut command: Vec<String> = vec!["//:download".to_owned()];
    command.extend(build_args(&payload, &origin, false));
    let command: Vec<&str> = command.iter().map(String::as_str).collect();
    let mut build = buck.build(&command);
    for (key, value) in values {
        build = build.env(key, value);
    }
    tokio::time::timeout(Duration::from_secs(90), build)
        .await?
        .expect_failure()
        .assert_stderr_matches("Invalid (HTTP|HTTPS)_PROXY uri");
    assert_eq!(origin.requests(), requests(&[]));
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_invalid_proxy_values_are_rejected_empty_http(buck: &mut Buck) -> anyhow::Result<()> {
    invalid_proxy_values_are_rejected(buck, &[("HTTP_PROXY", "")]).await
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_invalid_proxy_values_are_rejected_empty_http_lowercase(
    buck: &mut Buck,
) -> anyhow::Result<()> {
    invalid_proxy_values_are_rejected(buck, &[("http_proxy", "")]).await
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_invalid_proxy_values_are_rejected_empty_https(buck: &mut Buck) -> anyhow::Result<()> {
    invalid_proxy_values_are_rejected(buck, &[("HTTPS_PROXY", "")]).await
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_invalid_proxy_values_are_rejected_empty_https_lowercase(
    buck: &mut Buck,
) -> anyhow::Result<()> {
    invalid_proxy_values_are_rejected(buck, &[("https_proxy", "")]).await
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_invalid_proxy_values_are_rejected_malformed(buck: &mut Buck) -> anyhow::Result<()> {
    invalid_proxy_values_are_rejected(buck, &[("HTTP_PROXY", "not a valid proxy")]).await
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_empty_allowlist_ignores_invalid_proxy_values(buck: &mut Buck) -> anyhow::Result<()> {
    set_up(buck);
    configure(buck, Some(""), "SHA256")?;
    let payload = make_payload();
    let origin = serve(payload.clone()).await?;
    build_download(
        buck,
        &payload,
        &origin,
        &[
            ("HTTP_PROXY", "not a valid proxy".to_owned()),
            ("HTTPS_PROXY", String::new()),
        ],
        false,
    )
    .await?;
    assert_eq!(
        origin.requests(),
        requests(&[("HEAD", "/download"), ("GET", "/download")])
    );
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_same_daemon_routes_allowed_and_other_hosts(buck: &mut Buck) -> anyhow::Result<()> {
    set_up(buck);
    configure(buck, Some(" EXAMPLE.com. , 127.0.0.1 "), "SHA256")?;
    let allowed_payload = make_payload();
    let direct_payload = make_payload();
    let allowed_origin = serve(allowed_payload.clone()).await?;
    let direct_origin = serve_on(direct_payload.clone(), "localhost", &[]).await?;
    let proxy = serve(allowed_payload.clone()).await?;

    let env = [("HTTP_PROXY", proxy.url(""))];
    build_download(buck, &allowed_payload, &allowed_origin, &env, false).await?;
    let first_pid = daemon_pid(buck, &env).await?;
    build_download(buck, &direct_payload, &direct_origin, &env, false).await?;
    assert_eq!(daemon_pid(buck, &env).await?, first_pid);
    assert_eq!(allowed_origin.requests(), requests(&[]));
    assert_eq!(
        direct_origin.requests(),
        requests(&[("HEAD", "/download"), ("GET", "/download")])
    );
    let url = allowed_origin.url("/download");
    assert_eq!(
        proxy.requests(),
        requests(&[("HEAD", url.as_str()), ("GET", url.as_str())])
    );
    Ok(())
}

async fn redirect_uses_destination_allowlist(
    buck: &mut Buck,
    initial_allowed: bool,
) -> anyhow::Result<()> {
    set_up(buck);
    configure(buck, Some("127.0.0.1"), "SHA1")?;
    let payload = make_payload();
    let (initial_host, final_host) = if initial_allowed {
        ("127.0.0.1", "localhost")
    } else {
        ("localhost", "127.0.0.1")
    };
    let final_origin = serve_on(payload.clone(), final_host, &[]).await?;
    let final_url = final_origin.url("/download");
    let initial_origin = serve_on(
        payload.clone(),
        initial_host,
        &[("/download", final_url.as_str())],
    )
    .await?;
    let initial_url = initial_origin.url("/download");
    let proxy = serve_on(
        payload.clone(),
        "127.0.0.1",
        &[(initial_url.as_str(), final_url.as_str())],
    )
    .await?;
    build_download(
        buck,
        &payload,
        &initial_origin,
        &[("HTTP_PROXY", proxy.url(""))],
        false,
    )
    .await?;
    assert_eq!(
        initial_origin.requests(),
        if initial_allowed {
            requests(&[])
        } else {
            requests(&[("GET", "/download")])
        }
    );
    assert_eq!(
        final_origin.requests(),
        if initial_allowed {
            requests(&[("GET", "/download")])
        } else {
            requests(&[])
        }
    );
    assert_eq!(
        proxy.requests(),
        requests(&[(
            "GET",
            if initial_allowed {
                initial_url.as_str()
            } else {
                final_url.as_str()
            }
        )])
    );
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
#[expect(non_snake_case, reason = "the Python harness's name for the case")]
async fn test_redirect_uses_destination_allowlist_True(buck: &mut Buck) -> anyhow::Result<()> {
    redirect_uses_destination_allowlist(buck, true).await
}

#[buck_test(skip_for_os = ["windows"])]
#[expect(non_snake_case, reason = "the Python harness's name for the case")]
async fn test_redirect_uses_destination_allowlist_False(buck: &mut Buck) -> anyhow::Result<()> {
    redirect_uses_destination_allowlist(buck, false).await
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_proxy_environment_changes_require_manual_restart(
    buck: &mut Buck,
) -> anyhow::Result<()> {
    set_up(buck);
    configure(buck, Some("127.0.0.1"), "SHA256")?;
    let payloads: Vec<Vec<u8>> = (0..3).map(|_| make_payload()).collect();
    let origin = serve(payloads[0].clone()).await?;
    let second_origin = serve(payloads[1].clone()).await?;
    let third_origin = serve(payloads[2].clone()).await?;
    let origins = [&origin, &second_origin, &third_origin];
    // Either proxy can be asked for any of the three, and answers as that origin would.
    let routes = || -> Files {
        Files::ByTarget(
            origins
                .iter()
                .zip(&payloads)
                .map(|(server, payload)| (server.url("/download"), payload.clone()))
                .collect(),
        )
    };
    let first_proxy = serve(routes()).await?;
    let second_proxy = serve(routes()).await?;

    let first_env = [("HTTP_PROXY", first_proxy.url(""))];
    build_download(buck, &payloads[0], &origin, &first_env, false).await?;
    let first_pid = daemon_pid(buck, &first_env).await?;
    let url = origin.url("/download");
    assert_eq!(
        first_proxy.requests(),
        requests(&[("HEAD", url.as_str()), ("GET", url.as_str())])
    );

    // A download in the same daemon, asked for with a different proxy in the
    // environment, still goes through the proxy the daemon started with.
    let second_env = [("HTTP_PROXY", second_proxy.url(""))];
    build_download(buck, &payloads[1], &second_origin, &second_env, false).await?;
    assert_eq!(daemon_pid(buck, &second_env).await?, first_pid);
    assert_eq!(second_proxy.requests(), requests(&[]));
    let first_proxy_requests = first_proxy.requests();
    let url = second_origin.url("/download");
    assert_eq!(
        first_proxy_requests[first_proxy_requests.len().saturating_sub(2)..],
        requests(&[("HEAD", url.as_str()), ("GET", url.as_str())])
    );

    // Only a restart picks the new one up.
    tokio::time::timeout(Duration::from_secs(30), buck.kill()).await??;
    build_download(buck, &payloads[2], &third_origin, &second_env, false).await?;
    assert_ne!(daemon_pid(buck, &second_env).await?, first_pid);
    let url = third_origin.url("/download");
    assert_eq!(
        second_proxy.requests(),
        requests(&[("HEAD", url.as_str()), ("GET", url.as_str())])
    );
    assert_eq!(
        origins
            .iter()
            .map(|server| server.requests())
            .collect::<Vec<_>>(),
        [requests(&[]), requests(&[]), requests(&[])]
    );
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_checkout_local_allowlist_restarts_existing_daemon(
    buck: &mut Buck,
) -> anyhow::Result<()> {
    set_up(buck);
    configure(buck, Some("127.0.0.2"), "SHA256")?;
    let first_payload = make_payload();
    let payload = make_payload();
    let origin = serve(first_payload.clone()).await?;
    let proxy = serve(payload.clone()).await?;
    let env = [("HTTP_PROXY", proxy.url(""))];
    build_download(buck, &first_payload, &origin, &env, false).await?;
    let disabled_pid = daemon_pid(buck, &env).await?;
    assert_eq!(proxy.requests(), requests(&[]));

    configure(buck, Some("127.0.0.1"), "SHA256")?;
    let result = build(buck, &build_args(&payload, &origin, false), &env).await?;
    let output = result
        .build_report()?
        .output_for_target("root//:download")?;
    assert_eq!(std::fs::read(output)?, payload);
    assert_ne!(daemon_pid(buck, &env).await?, disabled_pid);
    let url = origin.url("/download");
    assert_eq!(
        proxy.requests(),
        requests(&[("HEAD", url.as_str()), ("GET", url.as_str())])
    );
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_empty_allowlist_ignores_proxy_environment_changes(
    buck: &mut Buck,
) -> anyhow::Result<()> {
    set_up(buck);
    configure(buck, Some(""), "SHA256")?;
    let payload = make_payload();
    let origin = serve(payload.clone()).await?;
    let first_proxy = serve(payload.clone()).await?;
    let second_proxy = serve(payload.clone()).await?;
    let first_env = [("HTTP_PROXY", first_proxy.url(""))];
    build_download(buck, &payload, &origin, &first_env, false).await?;
    let first_pid = daemon_pid(buck, &first_env).await?;
    let second_env = [("HTTP_PROXY", second_proxy.url(""))];
    build_download(buck, &payload, &origin, &second_env, false).await?;
    assert_eq!(daemon_pid(buck, &second_env).await?, first_pid);
    assert_eq!(first_proxy.requests(), requests(&[]));
    assert_eq!(second_proxy.requests(), requests(&[]));
    Ok(())
}
