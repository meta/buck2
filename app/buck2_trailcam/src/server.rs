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
use std::net::Ipv4Addr;
use std::net::Ipv6Addr;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::http::Uri;
use axum::http::header;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing::get;
use buck2_error::BuckErrorContext;
use buck2_event_log::read::EventLogPathBuf;
use buck2_events::metadata::hostname;
use buck2_hash::BuckMutMap;
use tokio::net::TcpListener;
use tokio_util::io::ReaderStream;

use crate::bundle;
use crate::bundle::Asset;
use crate::invocation;
use crate::invocation::InvocationInfo;
use crate::invocation::LoadedLog;
use crate::invocation::LogBody;

pub struct ServeConfig {
    pub log: EventLogPathBuf,
    /// Port to bind on. None picks the first free port of
    /// `SECURE_WEB_APPS_PORTS`; 0 lets the OS pick.
    pub port: Option<u16>,
}

/// Ports that Secure Web Apps forwards to a dev host over plain HTTP: a
/// browser with the VPNLess WWW extension opens
/// `http://<host>.fbinfra.net:<port>/` with no tunnel. They sit in the
/// ephemeral range, so one may be taken; the server tries them in turn.
const SECURE_WEB_APPS_PORTS: std::ops::RangeInclusive<u16> = 44100..=44109;

/// The `.fbinfra.net` name Secure Web Apps uses for a dev host, or None when
/// this is not a dev host (a laptop, say). The shapes are the ones
/// `nest/libs/next-core/scripts/proxy-hostname.cjs` recognizes, minus the
/// Sandcastle and Twine ones, which do not run interactive viewers.
fn secure_web_apps_host(hostname: &str) -> Option<String> {
    fn numeric(s: &str) -> bool {
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
    }
    let labels: Vec<&str> = hostname.split('.').collect();
    match labels.as_slice() {
        [first, region, "facebook", "com"] if !region.is_empty() => ["devvm", "devgpu", "devbig"]
            .iter()
            .any(|kind| first.strip_prefix(kind).is_some_and(numeric))
            .then(|| format!("{first}.{region}.fbinfra.net")),
        [id, "od", "fbinfra", "net"] if numeric(id) => Some(hostname.to_owned()),
        _ => None,
    }
}

struct AppState {
    assets: BuckMutMap<String, Asset>,
    loaded: LoadedLog,
}

/// Reads the log, binds, prints the URLs, and serves until Ctrl-C.
pub async fn serve(cfg: ServeConfig) -> buck2_error::Result<()> {
    let assets = bundle::load()?;
    eprintln!("trailcam: reading {}", cfg.log.path().display());
    let loaded = invocation::load(&cfg.log).await?;
    let state = Arc::new(AppState { assets, loaded });

    let app = Router::new()
        .route("/", get(index))
        .route("/api/invocation", get(api_invocation))
        .route("/api/event-log", get(api_event_log))
        .fallback(get(asset))
        .with_state(state);

    // On a dev host, listen on every interface so Secure Web Apps can reach
    // the server from the user's laptop; anywhere else stay on loopback.
    let external_host = hostname().and_then(|h| secure_web_apps_host(&h));
    let ip: IpAddr = if external_host.is_some() {
        Ipv6Addr::UNSPECIFIED.into()
    } else {
        Ipv4Addr::LOCALHOST.into()
    };
    let listener = bind(ip, cfg.port).await?;
    let port = listener
        .local_addr()
        .buck_error_context("Reading bound address")?
        .port();
    eprintln!("trailcam: open http://localhost:{port}/ (Ctrl-C to stop)");
    if let Some(host) = &external_host {
        eprintln!("trailcam: from a laptop with the VPNless WWW extension: http://{host}:{port}/");
    }

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            drop(tokio::signal::ctrl_c().await);
        })
        .await
        .buck_error_context("HTTP server error")?;
    Ok(())
}

/// Binds the requested port, or the first free one of the Secure Web Apps
/// range when none was requested.
async fn bind(ip: IpAddr, port: Option<u16>) -> buck2_error::Result<TcpListener> {
    let candidates: Vec<u16> = match port {
        Some(port) => vec![port],
        None => SECURE_WEB_APPS_PORTS.collect(),
    };
    let mut last_error = None;
    for port in candidates {
        let addr = SocketAddr::new(ip, port);
        match TcpListener::bind(addr).await {
            Ok(listener) => return Ok(listener),
            Err(e) => last_error = Some((addr, e)),
        }
    }
    let (addr, e) = last_error.expect("at least one candidate port");
    Err(e).with_buck_error_context(|| format!("Binding to {addr}"))
}

fn asset_response(asset: &Asset, cache_control: &'static str) -> Response {
    let mut response = asset.bytes.clone().into_response();
    let headers = response.headers_mut();
    if let Ok(mime) = HeaderValue::from_str(&asset.mime) {
        headers.insert(header::CONTENT_TYPE, mime);
    }
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );
    response
}

async fn index(State(state): State<Arc<AppState>>) -> Response {
    match state.assets.get("index.html") {
        Some(asset) => asset_response(asset, "no-store"),
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "bundle has no index.html",
        )
            .into_response(),
    }
}

/// Everything under `assets/` carries a content hash in its name, so it can
/// be cached forever.
async fn asset(State(state): State<Arc<AppState>>, uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    match state.assets.get(path) {
        Some(asset) => asset_response(asset, "public, max-age=31536000, immutable"),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn api_invocation(State(state): State<Arc<AppState>>) -> Json<InvocationInfo> {
    Json(state.loaded.info.clone())
}

async fn api_event_log(State(state): State<Arc<AppState>>) -> Response {
    let (body, len) = match &state.loaded.body {
        LogBody::Bytes(bytes) => (Body::from(bytes.clone()), bytes.len() as u64),
        LogBody::File { path, len } => match tokio::fs::File::open(path).await {
            Ok(file) => (Body::from_stream(ReaderStream::new(file)), *len),
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to open {}: {e}", path.display()),
                )
                    .into_response();
            }
        },
    };
    let mut response = body.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_hosts_get_an_fbinfra_name() {
        assert_eq!(
            secure_web_apps_host("devvm21550.cco0.facebook.com").as_deref(),
            Some("devvm21550.cco0.fbinfra.net")
        );
        assert_eq!(
            secure_web_apps_host("devgpu007.prn2.facebook.com").as_deref(),
            Some("devgpu007.prn2.fbinfra.net")
        );
        assert_eq!(
            secure_web_apps_host("12345.od.fbinfra.net").as_deref(),
            Some("12345.od.fbinfra.net")
        );
    }

    #[test]
    fn other_machines_are_not_dev_hosts() {
        for name in [
            "laptop.local",
            "devvm.cco0.facebook.com",
            "devvmx1.cco0.facebook.com",
            "devvm1..facebook.com",
            "abc.od.fbinfra.net",
            "job.twshared1.2.dc.tw.fbinfra.net",
        ] {
            assert_eq!(secure_web_apps_host(name), None, "{name}");
        }
    }
}
