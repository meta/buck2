/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_common::invocation_paths::InvocationPaths;
#[cfg(not(fbcode_build))]
use buck2_error::ErrorTag;
#[cfg(not(fbcode_build))]
use buck2_events::sink::remote::BesChannelSettings;
#[cfg(not(fbcode_build))]
use buck2_events::sink::remote::BesEventFormat;
#[cfg(not(fbcode_build))]
use buck2_events::sink::remote::BesTls;
use buck2_events::sink::remote::RemoteEventConfig;

#[cfg(not(fbcode_build))]
struct BuckconfigBesSettings {
    bes_backend: Option<String>,
    bes_headers: Vec<(String, String)>,
    bes_tls: BesTls,
    build_metadata: Vec<(String, String)>,
    bes_event_format: Option<BesEventFormat>,
    bazel_artifact_upload: Option<bool>,
    upload_successful_action_events: Option<bool>,
    bazel_artifact_upload_backend: Option<String>,
    re_client_cas_address: Option<String>,
    bazel_artifact_upload_instance_name: Option<String>,
    re_client_instance_name: Option<String>,
    bazel_artifact_uri_authority: Option<String>,
    bazel_artifact_upload_max_bytes: Option<usize>,
    bes_results_url: Option<String>,
    channel: BesChannelSettings,
    replay: buck2_re_configuration::BesReplaySettings,
}

#[cfg(not(fbcode_build))]
pub fn with_buckconfig_overrides(
    paths: Option<&InvocationPaths>,
    mut config: RemoteEventConfig,
) -> RemoteEventConfig {
    match read_buckconfig_bes_settings(paths) {
        Ok(settings) => {
            if let Some(bes_backend) = settings.bes_backend {
                config.bes_backend = Some(bes_backend);
            }
            config.bes_headers = settings.bes_headers;
            config.bes_tls = settings.bes_tls;
            config.build_metadata = settings.build_metadata;
            if let Some(bes_event_format) = settings.bes_event_format {
                config.event_format = bes_event_format;
            }
            if let Some(bazel_artifact_upload) = settings.bazel_artifact_upload {
                config.bazel_artifact_upload = bazel_artifact_upload;
            }
            if let Some(upload_successful_action_events) = settings.upload_successful_action_events
            {
                config.upload_successful_action_events = upload_successful_action_events;
            }
            if let Some(backend) = settings.bazel_artifact_upload_backend {
                config.bazel_artifact_upload_backend = Some(backend);
            }
            if let Some(re_client_cas_address) = settings.re_client_cas_address {
                config.re_client_cas_address = Some(re_client_cas_address);
            }
            if let Some(instance_name) = settings.bazel_artifact_upload_instance_name {
                config.bazel_artifact_upload_instance_name = Some(instance_name);
            }
            if let Some(re_client_instance_name) = settings.re_client_instance_name {
                config.re_client_instance_name = Some(re_client_instance_name);
            }
            if let Some(authority) = settings.bazel_artifact_uri_authority {
                config.bazel_artifact_uri_authority = Some(authority);
            }
            if let Some(max_bytes) = settings.bazel_artifact_upload_max_bytes {
                config.bazel_artifact_upload_max_bytes = max_bytes;
            }
            config.channel = settings.channel;
            if let Some(secs) = settings.replay.retry_window_secs {
                config.retry_window = std::time::Duration::from_secs(secs);
            }
            config
        }
        Err(e) => {
            tracing::warn!(
                "Failed to read BES buckconfig settings, falling back to defaults: {:#}",
                e
            );
            config
        }
    }
}

#[cfg(not(fbcode_build))]
pub fn bes_results_url(paths: Option<&InvocationPaths>) -> Option<String> {
    match read_buckconfig_bes_settings(paths) {
        Ok(settings) => settings.bes_results_url,
        Err(e) => {
            tracing::warn!(
                "Failed to read BES buckconfig settings, falling back to defaults: {:#}",
                e
            );
            None
        }
    }
}

#[cfg(fbcode_build)]
pub fn with_buckconfig_overrides(
    _paths: Option<&InvocationPaths>,
    config: RemoteEventConfig,
) -> RemoteEventConfig {
    config
}

#[cfg(fbcode_build)]
pub fn bes_results_url(_paths: Option<&InvocationPaths>) -> Option<String> {
    None
}

#[cfg(not(fbcode_build))]
fn read_buckconfig_bes_settings(
    paths: Option<&InvocationPaths>,
) -> buck2_error::Result<BuckconfigBesSettings> {
    use buck2_common::legacy_configs::cells::BuckConfigBasedCells;
    use buck2_common::legacy_configs::key::BuckconfigKeyRef;

    let Some(paths) = paths else {
        return Ok(BuckconfigBesSettings {
            bes_backend: None,
            bes_headers: Vec::new(),
            bes_tls: BesTls::default(),
            build_metadata: Vec::new(),
            bes_event_format: None,
            bazel_artifact_upload: None,
            upload_successful_action_events: None,
            bazel_artifact_upload_backend: None,
            re_client_cas_address: None,
            bazel_artifact_upload_instance_name: None,
            re_client_instance_name: None,
            bazel_artifact_uri_authority: None,
            bazel_artifact_upload_max_bytes: None,
            bes_results_url: None,
            channel: BesChannelSettings::default(),
            replay: Default::default(),
        });
    };
    let fs = paths.project_root();

    let legacy_cells =
        futures::executor::block_on(BuckConfigBasedCells::parse_with_config_args(fs, &[]))?;
    let cells = &legacy_cells.cell_resolver;
    let root_config =
        futures::executor::block_on(legacy_cells.parse_single_cell(cells.root_cell(), fs))?;

    let bes_results_url = root_config
        .get(BuckconfigKeyRef {
            section: "bes",
            property: "results_url",
        })
        .or_else(|| {
            root_config.get(BuckconfigKeyRef {
                section: "bes",
                property: "result_url",
            })
        })
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let re_client_cas_address = buck2_re_configuration::bes_cas_address(&root_config)?;

    // `[bes] connection = re_client` reuses the remote execution client's endpoint, headers
    // and TLS identity. Explicit `backend` and `header` keys still win.
    let connection = match root_config
        .get(BuckconfigKeyRef {
            section: "bes",
            property: "connection",
        })
        .map(str::trim)
    {
        Some("re_client") => buck2_re_configuration::BesConnection::from_re_client(&root_config)?,
        Some("") | None => None,
        Some(other) => {
            return Err(buck2_error::buck2_error!(
                ErrorTag::Input,
                "Invalid `bes.connection` `{}` (expected `re_client`)",
                other
            ));
        }
    };
    let (mut bes_headers, missing_header_env) =
        parse_bes_headers(root_config.parse_list::<String>(BuckconfigKeyRef {
            section: "bes",
            property: "header",
        })?)?;
    if bes_headers.is_empty() {
        if let Some(connection) = &connection {
            bes_headers = connection.headers.clone();
        }
    }
    let bes_tls = connection
        .as_ref()
        .map(|connection| BesTls {
            client_cert: connection.tls_client_cert.clone(),
            ca_certs: connection.tls_ca_certs.clone(),
        })
        .unwrap_or_default();

    let bes_backend = root_config
        .get(BuckconfigKeyRef {
            section: "bes",
            property: "backend",
        })
        .map(str::to_owned)
        .or_else(|| {
            connection
                .as_ref()
                .map(|connection| connection.backend.clone())
        });
    // An empty API key reads as none at all to the server, so a header whose variable is unset
    // turns BES off here as it does in the daemon (buck2_server daemon/state.rs).
    let bes_backend = if missing_header_env.is_empty() {
        bes_backend
    } else {
        tracing::warn!(
            "{}",
            buck2_events::sink::remote::missing_bes_header_env_warning(
                &missing_header_env,
                "the client's"
            )
        );
        None
    };
    // No backend, no sink: a results URL would name a page that never gets written.
    let bes_results_url = bes_results_url.filter(|_| bes_backend.is_some());

    Ok(BuckconfigBesSettings {
        bes_backend: bes_backend.clone(),
        bes_headers,
        bes_tls,
        build_metadata: parse_bes_build_metadata(root_config.parse_list::<String>(
            BuckconfigKeyRef {
                section: "bes",
                property: "build_metadata",
            },
        )?)?,
        bes_event_format: root_config.parse::<BesEventFormat>(BuckconfigKeyRef {
            section: "bes",
            property: "event_format",
        })?,
        bazel_artifact_upload: root_config.parse::<bool>(BuckconfigKeyRef {
            section: "bes",
            property: "bazel_artifact_upload",
        })?,
        upload_successful_action_events: root_config.parse::<bool>(BuckconfigKeyRef {
            section: "bes",
            property: "upload_successful_action_events",
        })?,
        bazel_artifact_upload_backend: root_config
            .get(BuckconfigKeyRef {
                section: "bes",
                property: "bazel_artifact_upload_backend",
            })
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        re_client_cas_address,
        bazel_artifact_upload_instance_name: root_config
            .get(BuckconfigKeyRef {
                section: "bes",
                property: "bazel_artifact_upload_instance_name",
            })
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        re_client_instance_name: root_config.parse(BuckconfigKeyRef {
            section: "buck2_re_client",
            property: "instance_name",
        })?,
        bazel_artifact_uri_authority: root_config
            .get(BuckconfigKeyRef {
                section: "bes",
                property: "bazel_artifact_uri_authority",
            })
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        bazel_artifact_upload_max_bytes: root_config.parse::<usize>(BuckconfigKeyRef {
            section: "bes",
            property: "bazel_artifact_upload_max_bytes",
        })?,
        bes_results_url,
        channel: BesChannelSettings {
            grpc_keepalive_time_secs: root_config.parse(BuckconfigKeyRef {
                section: "buck2_re_client",
                property: "grpc_keepalive_time_secs",
            })?,
            grpc_keepalive_timeout_secs: root_config.parse(BuckconfigKeyRef {
                section: "buck2_re_client",
                property: "grpc_keepalive_timeout_secs",
            })?,
            grpc_keepalive_while_idle: root_config.parse(BuckconfigKeyRef {
                section: "buck2_re_client",
                property: "grpc_keepalive_while_idle",
            })?,
            tcp_keepalive_secs: root_config.parse(BuckconfigKeyRef {
                section: "buck2_re_client",
                property: "tcp_keepalive_secs",
            })?,
            bytestream_progress_timeout_secs: root_config.parse(BuckconfigKeyRef {
                section: "buck2_re_client",
                property: "bytestream_progress_timeout_secs",
            })?,
        },
        replay: buck2_re_configuration::BesReplaySettings::from_legacy_config(&root_config)?,
    })
}

#[cfg(not(fbcode_build))]
/// The headers, and the variables they name that are unset or empty.
fn parse_bes_headers(
    raw_headers: Option<Vec<String>>,
) -> buck2_error::Result<buck2_events::sink::remote::BesHeadersAndMissingVars> {
    parse_bes_headers_with_env(raw_headers, |name| std::env::var(name).ok())
}

#[cfg(not(fbcode_build))]
fn parse_bes_headers_with_env<F>(
    raw_headers: Option<Vec<String>>,
    mut env: F,
) -> buck2_error::Result<buck2_events::sink::remote::BesHeadersAndMissingVars>
where
    F: FnMut(&str) -> Option<String>,
{
    let mut headers = Vec::new();
    let mut missing = Vec::new();
    for raw_header in raw_headers.unwrap_or_default() {
        let (raw_header, unset) =
            buck2_events::sink::remote::expand_bes_config_env_vars_reporting(&raw_header, &mut env);
        missing.extend(unset);
        let (key, value) = raw_header.split_once('=').ok_or_else(|| {
            buck2_error::buck2_error!(
                ErrorTag::Input,
                "Invalid `bes.header` entry `{}` (expected `NAME=VALUE`)",
                raw_header
            )
        })?;
        let key = key.trim();
        if key.is_empty() {
            return Err(buck2_error::buck2_error!(
                ErrorTag::Input,
                "Invalid `bes.header` entry `{}` (header name is empty)",
                raw_header
            ));
        }
        headers.push((key.to_owned(), value.to_owned()));
    }
    Ok((headers, missing))
}

#[cfg(not(fbcode_build))]
fn parse_bes_build_metadata(
    raw_entries: Option<Vec<String>>,
) -> buck2_error::Result<Vec<(String, String)>> {
    parse_bes_build_metadata_with_env(raw_entries, |name| std::env::var(name).ok())
}

#[cfg(not(fbcode_build))]
fn parse_bes_build_metadata_with_env<F>(
    raw_entries: Option<Vec<String>>,
    mut env: F,
) -> buck2_error::Result<Vec<(String, String)>>
where
    F: FnMut(&str) -> Option<String>,
{
    let mut metadata = Vec::new();
    for raw_entry in raw_entries.unwrap_or_default() {
        let raw_entry =
            buck2_events::sink::remote::expand_bes_config_env_vars_with(&raw_entry, &mut env);
        let (key, value) = raw_entry.split_once('=').ok_or_else(|| {
            buck2_error::buck2_error!(
                ErrorTag::Input,
                "Invalid bes.build_metadata entry '{}' (expected KEY=VALUE)",
                raw_entry
            )
        })?;
        let key = key.trim();
        if key.is_empty() {
            return Err(buck2_error::buck2_error!(
                ErrorTag::Input,
                "Invalid bes.build_metadata entry '{}' (metadata key is empty)",
                raw_entry
            ));
        }
        metadata.push((key.to_owned(), value.to_owned()));
    }
    Ok(metadata)
}

#[cfg(all(test, not(fbcode_build)))]
mod tests {
    use super::*;

    #[test]
    fn parses_bes_build_metadata_entries() {
        let metadata = parse_bes_build_metadata(Some(vec![
            "PARENT_RUN_ID=workflow-run".to_owned(),
            " PARENT_INVOCATION_ID=workflow-invocation".to_owned(),
            "EXTRA=left=right".to_owned(),
        ]))
        .unwrap();

        assert_eq!(
            metadata,
            vec![
                ("PARENT_RUN_ID".to_owned(), "workflow-run".to_owned()),
                (
                    "PARENT_INVOCATION_ID".to_owned(),
                    "workflow-invocation".to_owned()
                ),
                ("EXTRA".to_owned(), "left=right".to_owned()),
            ]
        );
    }

    #[test]
    fn rejects_invalid_bes_build_metadata_entries() {
        assert!(parse_bes_build_metadata(Some(vec!["PARENT_RUN_ID".to_owned()])).is_err());
        assert!(parse_bes_build_metadata(Some(vec!["=workflow-run".to_owned()])).is_err());
    }

    #[test]
    fn a_header_naming_an_unset_variable_reports_it() {
        let (headers, missing) = parse_bes_headers_with_env(
            Some(vec![
                "x-buildbuddy-api-key=$UNSET_API_KEY".to_owned(),
                "x-literal=value".to_owned(),
            ]),
            test_env,
        )
        .unwrap();
        assert_eq!(missing, vec!["UNSET_API_KEY".to_owned()]);
        assert_eq!(headers[1], ("x-literal".to_owned(), "value".to_owned()));
    }

    #[test]
    fn expands_env_vars_in_bes_headers_and_metadata() {
        let (headers, missing) = parse_bes_headers_with_env(
            Some(vec!["x-buildbuddy-api-key=$BUILDBUDDY_API_KEY".to_owned()]),
            test_env,
        )
        .unwrap();
        assert_eq!(
            headers,
            vec![("x-buildbuddy-api-key".to_owned(), "secret".to_owned())]
        );
        assert!(missing.is_empty());

        let metadata = parse_bes_build_metadata_with_env(
            Some(vec![
                "PARENT_RUN_ID=${BUILDBUDDY_RUN_ID}".to_owned(),
                "MISSING=$MISSING".to_owned(),
                "LITERAL=$9".to_owned(),
            ]),
            test_env,
        )
        .unwrap();
        assert_eq!(
            metadata,
            vec![
                ("PARENT_RUN_ID".to_owned(), "run-id".to_owned()),
                ("MISSING".to_owned(), "".to_owned()),
                ("LITERAL".to_owned(), "$9".to_owned()),
            ]
        );
    }

    fn test_env(name: &str) -> Option<String> {
        match name {
            "BUILDBUDDY_API_KEY" => Some("secret".to_owned()),
            "BUILDBUDDY_RUN_ID" => Some("run-id".to_owned()),
            _ => None,
        }
    }
}
