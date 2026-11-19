/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::str::FromStr;

use allocative::Allocative;
use buck2_common::legacy_configs::configs::LegacyBuckConfig;
use buck2_common::legacy_configs::key::BuckconfigKeyRef;
use buck2_core::rollout_percentage::RolloutPercentage;

static BUCK2_RE_CLIENT_CFG_SECTION: &str = "buck2_re_client";

/// We put functions here that both things need to implement for code that isn't gated behind a
/// fbcode_build or not(fbcode_build)
pub trait RemoteExecutionStaticMetadataImpl: Sized {
    fn from_legacy_config(legacy_config: &LegacyBuckConfig) -> buck2_error::Result<Self>;
    /// Whether there is a CAS to talk to at all. Independent of any executor configuration: a
    /// command that runs nothing remotely still has one, and asks it about blobs.
    fn cas_configured(&self) -> bool;
    fn cas_semaphore_size(&self) -> usize;
    fn exec_semaphore_size(&self) -> usize;
    fn action_cache_semaphore_size(&self) -> usize;
    /// How many CAS presence-check RPCs may be in flight at once.
    fn presence_check_concurrency(&self) -> usize;
    /// How many digests one CAS presence-check RPC carries at most.
    fn presence_check_max_batch(&self) -> usize;
}

#[derive(Clone, Debug, Allocative)]
pub enum CASdAddress {
    Tcp(u16),
    Uds(String),
}

impl FromStr for CASdAddress {
    type Err = buck2_error::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(path) = s.strip_prefix("unix://") {
            Ok(CASdAddress::Uds(path.to_owned()))
        } else {
            Ok(CASdAddress::Tcp(s.parse()?))
        }
    }
}

#[derive(Clone, Debug, Allocative)]
pub enum CASdMode {
    LocalWithSync,
    LocalWithoutSync,
    Remote,
}

impl FromStr for CASdMode {
    type Err = buck2_error::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "local_with_sync" => Ok(CASdMode::LocalWithSync),
            "local_without_sync" => Ok(CASdMode::LocalWithoutSync),
            "remote" => Ok(CASdMode::Remote),
            _ => Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::Input,
                "Invalid CASd mode: {}",
                s
            )),
        }
    }
}

#[derive(Clone, Debug, Allocative)]
pub enum CopyPolicy {
    Copy,
    Reflink,
    Hybrid,
}

impl FromStr for CopyPolicy {
    type Err = buck2_error::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "hybrid" => Ok(CopyPolicy::Hybrid),
            "reflink" => Ok(CopyPolicy::Reflink),
            _ => Ok(CopyPolicy::Copy),
        }
    }
}

#[allow(unused)]
mod fbcode {
    use buck2_common::legacy_configs::key::BuckconfigKeyRef;

    use super::*;

    /// Metadata that doesn't change between executions
    #[derive(Clone, Debug, Default, Allocative)]
    pub struct RemoteExecutionStaticMetadata {
        // gRPC settings
        pub cas_address: Option<String>,
        pub cas_connection_count: i32,
        pub presence_check_concurrency: usize,
        pub presence_check_max_batch: usize,
        pub shared_casd_cache_path: Option<String>,
        pub legacy_shared_casd_mode: Option<String>,
        pub shared_casd_mode_small_files: Option<CASdMode>,
        pub shared_casd_mode_large_files: Option<CASdMode>,
        pub shared_casd_cache_sync_wal_files_count: Option<u8>,
        pub shared_casd_cache_sync_wal_file_max_size: Option<u64>,
        pub shared_casd_cache_sync_max_batch_size: Option<u32>,
        pub shared_casd_cache_sync_max_delay_ms: Option<u32>,
        pub shared_casd_copy_policy: Option<CopyPolicy>,
        pub shared_casd_address: Option<CASdAddress>,
        pub shared_casd_use_tls: Option<bool>,
        pub cas_client_label: Option<String>,
        /// Client-side cap on concurrent action cache RPCs (reads and writes).
        pub action_cache_semaphore_size: Option<usize>,
        // End gRPC settings
        pub verbose_logging: bool,

        pub use_manifold_rich_client: bool,
        pub use_zippy_rich_client: bool,
        pub use_p2p: bool,

        pub cas_thread_count: i32,
        pub cas_thread_count_ratio: f32,

        pub rich_client_channels_per_blob: Option<i32>,
        pub rich_client_attempt_timeout_ms: Option<i32>,
        pub rich_client_retries_count: Option<i32>,
        pub force_enable_deduplicate_find_missing: Option<bool>,

        pub features_config_path: Option<String>,
        pub client_config_path: Option<String>,

        // curl reactor
        pub curl_reactor_max_number_of_retries: Option<i32>,
        pub curl_reactor_connection_timeout_ms: Option<i32>,
        pub curl_reactor_request_timeout_ms: Option<i32>,

        // ttl management
        pub minimal_blob_ttl_seconds: Option<i64>,
        // When less than (X*100)% of TTL remains, refresh data in the store
        pub remaining_ttl_fraction_refresh_threshold: Option<f32>,
        // Adds a randomness to when refresh the TTL
        pub remaining_ttl_random_extra_threshold: Option<f32>,

        pub disable_fallocate: bool,
        pub respect_file_symlinks: bool,

        // Thrift settings
        pub execution_concurrency_limit: i32,
        pub engine_tier: Option<String>,
        pub engine_host: Option<String>,
        pub engine_port: Option<i32>,
        // End Thrift settings
        /// When set to True, allows for cancellation of RE downloads when futures are dropped
        pub enable_download_cancellation: bool,
    }

    impl RemoteExecutionStaticMetadataImpl for RemoteExecutionStaticMetadata {
        fn from_legacy_config(legacy_config: &LegacyBuckConfig) -> buck2_error::Result<Self> {
            Ok(Self {
                cas_address: legacy_config.parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "cas_address",
                })?,
                cas_connection_count: legacy_config
                    .parse(BuckconfigKeyRef {
                        section: BUCK2_RE_CLIENT_CFG_SECTION,
                        property: "cas_connection_count",
                    })?
                    .unwrap_or(16),
                // Well under the CAS client's own limit on these RPCs (an internal semaphore
                // of 5000 at the time of writing), and above what a build's uploads had in
                // flight before buck2 bounded them at all.
                presence_check_concurrency: legacy_config
                    .parse(BuckconfigKeyRef {
                        section: BUCK2_RE_CLIENT_CFG_SECTION,
                        property: "presence_check_concurrency",
                    })?
                    .unwrap_or(10),
                // The internal CAS client accepts up to 500 per RPC; we see significant
                // improvements from larger batching
                presence_check_max_batch: legacy_config
                    .parse(BuckconfigKeyRef {
                        section: BUCK2_RE_CLIENT_CFG_SECTION,
                        property: "presence_check_max_batch",
                    })?
                    .unwrap_or(500),
                shared_casd_cache_path: legacy_config.parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "cas_shared_cache",
                })?,
                legacy_shared_casd_mode: legacy_config.parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "cas_shared_cache_mode",
                })?,
                shared_casd_mode_small_files: legacy_config.parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "cas_shared_cache_mode_small_files_v2",
                })?,
                shared_casd_mode_large_files: legacy_config.parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "cas_shared_cache_mode_large_files_v2",
                })?,
                shared_casd_cache_sync_wal_files_count: legacy_config.parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "cas_shared_cache_sync_wal_files_count_v2",
                })?,
                shared_casd_cache_sync_wal_file_max_size: legacy_config.parse(
                    BuckconfigKeyRef {
                        section: BUCK2_RE_CLIENT_CFG_SECTION,
                        property: "cas_shared_cache_sync_wal_file_max_size_v2",
                    },
                )?,
                shared_casd_cache_sync_max_batch_size: legacy_config.parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "cas_shared_cache_sync_max_batch_size_v2",
                })?,
                shared_casd_cache_sync_max_delay_ms: legacy_config.parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "cas_shared_cache_sync_max_delay_ms_v2",
                })?,
                shared_casd_copy_policy: legacy_config.parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "cas_shared_cache_copy_policy_v2",
                })?,
                shared_casd_address: {
                    let port_result = legacy_config.parse(BuckconfigKeyRef {
                        section: BUCK2_RE_CLIENT_CFG_SECTION,
                        property: "cas_shared_cache_port",
                    });
                    match port_result {
                        Ok(Some(port)) => Some(port),
                        _ => legacy_config.parse(BuckconfigKeyRef {
                            section: BUCK2_RE_CLIENT_CFG_SECTION,
                            property: "cas_shared_cache_address_v2",
                        })?,
                    }
                },
                shared_casd_use_tls: legacy_config.parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "cas_shared_cache_tls",
                })?,
                cas_client_label: legacy_config.parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "cas_client_label_v2",
                })?,
                action_cache_semaphore_size: legacy_config.parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "action_cache_semaphore_size",
                })?,
                verbose_logging: legacy_config
                    .parse(BuckconfigKeyRef {
                        section: BUCK2_RE_CLIENT_CFG_SECTION,
                        property: "verbose_logging",
                    })?
                    .unwrap_or(false),
                cas_thread_count: legacy_config
                    .parse(BuckconfigKeyRef {
                        section: BUCK2_RE_CLIENT_CFG_SECTION,
                        property: "cas_thread_count",
                    })?
                    .unwrap_or(4),
                cas_thread_count_ratio: legacy_config
                    .parse(BuckconfigKeyRef {
                        section: BUCK2_RE_CLIENT_CFG_SECTION,
                        property: "cas_thread_count_ratio",
                    })?
                    .unwrap_or(0.0),
                use_manifold_rich_client: legacy_config
                    .parse(BuckconfigKeyRef {
                        section: BUCK2_RE_CLIENT_CFG_SECTION,
                        property: "use_manifold_rich_client_new",
                    })?
                    .unwrap_or(true),
                use_zippy_rich_client: legacy_config
                    .parse(BuckconfigKeyRef {
                        section: BUCK2_RE_CLIENT_CFG_SECTION,
                        property: "use_zippy_rich_client",
                    })?
                    .unwrap_or(false),
                use_p2p: legacy_config
                    .parse(BuckconfigKeyRef {
                        section: BUCK2_RE_CLIENT_CFG_SECTION,
                        property: "use_p2p",
                    })?
                    .unwrap_or(false),
                rich_client_channels_per_blob: legacy_config.parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "rich_client_channels_per_blob",
                })?,
                rich_client_attempt_timeout_ms: legacy_config.parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "rich_client_attempt_timeout_ms",
                })?,
                rich_client_retries_count: legacy_config.parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "rich_client_retries_count",
                })?,
                force_enable_deduplicate_find_missing: legacy_config.parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "force_enable_deduplicate_find_missing",
                })?,
                features_config_path: legacy_config.parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "features_config_path",
                })?,
                client_config_path: legacy_config.parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "client_config_path",
                })?,
                curl_reactor_max_number_of_retries: legacy_config.parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "curl_reactor_max_number_of_retries",
                })?,
                curl_reactor_connection_timeout_ms: legacy_config.parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "curl_reactor_connection_timeout_ms",
                })?,
                curl_reactor_request_timeout_ms: legacy_config.parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "curl_reactor_request_timeout_ms",
                })?,
                minimal_blob_ttl_seconds: legacy_config.parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "minimal_blob_ttl_seconds",
                })?,
                disable_fallocate: legacy_config
                    .parse::<RolloutPercentage>(BuckconfigKeyRef {
                        section: BUCK2_RE_CLIENT_CFG_SECTION,
                        property: "disable_fallocate",
                    })?
                    .unwrap_or(RolloutPercentage::never())
                    .roll(),
                remaining_ttl_fraction_refresh_threshold: legacy_config.parse(
                    BuckconfigKeyRef {
                        section: BUCK2_RE_CLIENT_CFG_SECTION,
                        property: "remaining_ttl_fraction_refresh_threshold",
                    },
                )?,
                remaining_ttl_random_extra_threshold: legacy_config.parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "remaining_ttl_random_extra_threshold",
                })?,
                respect_file_symlinks: legacy_config
                    .parse::<RolloutPercentage>(BuckconfigKeyRef {
                        section: BUCK2_RE_CLIENT_CFG_SECTION,
                        property: "respect_file_symlinks",
                    })?
                    .unwrap_or(RolloutPercentage::never())
                    .roll(),
                execution_concurrency_limit: legacy_config
                    .parse(BuckconfigKeyRef {
                        section: BUCK2_RE_CLIENT_CFG_SECTION,
                        property: "execution_concurrency_limit",
                    })?
                    .unwrap_or(4000),
                engine_tier: legacy_config.parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "engine_tier",
                })?,
                engine_host: legacy_config.parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "engine_host",
                })?,
                engine_port: legacy_config.parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "engine_port",
                })?,
                enable_download_cancellation: legacy_config
                    .parse(BuckconfigKeyRef {
                        section: BUCK2_RE_CLIENT_CFG_SECTION,
                        property: "enable_download_cancellation",
                    })?
                    .unwrap_or(false),
            })
        }

        fn cas_configured(&self) -> bool {
            // `cas_address` only picks which CAS; internal builds always have one.
            true
        }

        fn cas_semaphore_size(&self) -> usize {
            self.cas_connection_count as usize * 30
        }

        fn action_cache_semaphore_size(&self) -> usize {
            // See D116018045 for data showing that scaling this much higher doesn't really matter
            // at data center latency, and hurts Max RSS; on non-dc networks though, somewhat larger
            // numbers are needed
            self.action_cache_semaphore_size.unwrap_or(5000)
        }

        fn exec_semaphore_size(&self) -> usize {
            self.execution_concurrency_limit as usize
        }

        fn presence_check_concurrency(&self) -> usize {
            self.presence_check_concurrency
        }

        fn presence_check_max_batch(&self) -> usize {
            self.presence_check_max_batch
        }
    }
}

#[allow(unused)]
mod not_fbcode {
    use super::*;

    /// Metadata that doesn't change between executions
    #[derive(Clone, Debug, Default, Allocative)]
    pub struct RemoteExecutionStaticMetadata(pub Buck2OssReConfiguration);

    impl RemoteExecutionStaticMetadataImpl for RemoteExecutionStaticMetadata {
        fn from_legacy_config(legacy_config: &LegacyBuckConfig) -> buck2_error::Result<Self> {
            Ok(Self(Buck2OssReConfiguration::from_legacy_config(
                legacy_config,
            )?))
        }

        fn cas_configured(&self) -> bool {
            self.0.cas_address.is_some()
        }

        fn cas_semaphore_size(&self) -> usize {
            // FIXME: make this configurable?
            1024
        }

        fn action_cache_semaphore_size(&self) -> usize {
            // FIXME: make this configurable?
            1024
        }

        fn exec_semaphore_size(&self) -> usize {
            self.0.execution_concurrency_limit.unwrap_or(400)
        }

        fn presence_check_concurrency(&self) -> usize {
            100
        }

        fn presence_check_max_batch(&self) -> usize {
            50
        }
    }
}

/// A configuration used only in our OSS builds. We still compile this always, which lets us
/// gate less code behind fbcode_build.
#[derive(Clone, Debug, Default, Allocative)]
pub struct Buck2OssReConfiguration {
    /// Address for RBE Content Addresable Storage service (including bytestream uploads service).
    pub cas_address: Option<String>,
    /// Address for RBE Engine service (including capabilities service).
    pub engine_address: Option<String>,
    /// Address for RBE Action Cache service.
    pub action_cache_address: Option<String>,
    /// Whether to use TLS to interact with remote execution.
    pub tls: bool,
    /// Path to a CA certificates bundle. This must be PEM-encoded. If none is set, a default
    /// bundle will be used.
    ///
    /// This can contain environment variables using shell interpolation syntax (i.e. $VAR). They
    /// will be substituted before using the value.
    pub tls_ca_certs: Option<String>,
    /// Path to a client certificate (and intermediate chain), as well as its associated private
    /// key. This must be PEM-encoded.
    ///
    /// This can contain environment variables using shell interpolation syntax (i.e. $VAR). They
    /// will be substituted before using the value.
    pub tls_client_cert: Option<String>,
    /// HTTP headers to inject in all requests to RE. This is a comma-separated list of `Header:
    /// Value` pairs. Minimal validation of those headers is done here.
    ///
    /// This can contain environment variables using shell interpolation syntax (i.e. $VAR). They
    /// will be substituted before using the value.
    pub http_headers: Vec<HttpHeader>,
    /// Whether to query capabilities from the RBE backend.
    pub capabilities: Option<bool>,
    /// The instance name to use in requests.
    pub instance_name: Option<String>,
    /// Use the Meta version of the request metadata
    pub use_fbcode_metadata: bool,
    /// The max size for a GRPC message to be decoded.
    pub max_decoding_message_size: Option<usize>,
    /// The max cumulative blob size for `Read` and `BatchReadBlobs` methods.
    pub max_total_batch_size: Option<usize>,
    /// Maximum number of concurrent upload requests for each action.
    pub max_concurrent_uploads_per_action: Option<usize>,
    /// Maximum number of digests to ask about in a single
    /// `FindMissingBlobs` (a.k.a. `GetDigestsTtl`) RPC. Larger values
    /// reduce per-call wall-clock latency by issuing fewer round-trips,
    /// at the cost of bigger requests and more concurrent server load
    /// when many actions issue independent calls.
    pub find_missing_blobs_batch_size: Option<usize>,
    /// Time that digests are assumed to live in CAS after being touched.
    pub cas_ttl_secs: Option<i64>,
    /// Interval in seconds for HTTP/2 ping frames to detect stale connections.
    pub grpc_keepalive_time_secs: Option<u64>,
    /// Timeout in seconds for receiving HTTP/2 ping acknowledgement.
    pub grpc_keepalive_timeout_secs: Option<u64>,
    /// Whether to send HTTP/2 pings when connection is idle.
    pub grpc_keepalive_while_idle: Option<bool>,
    /// Maximum number of concurrent execution requests.
    pub execution_concurrency_limit: Option<usize>,
    /// Minimum number of HTTP/2 connections per host in the connection pool.
    pub min_connections: Option<usize>,
    /// Maximum number of HTTP/2 connections per host in the connection pool.
    pub max_connections: Option<usize>,
    /// Maximum concurrent streams per connection.
    pub max_concurrency_per_connection: Option<usize>,
}

#[derive(Clone, Debug, Default, Allocative)]
pub struct HttpHeader {
    pub key: String,
    pub value: String,
}

impl FromStr for HttpHeader {
    type Err = buck2_error::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut iter = s.splitn(2, ':');
        match (iter.next(), iter.next()) {
            (Some(key), Some(value)) => Ok(Self {
                key: key.trim().to_owned(),
                value: value.trim().to_owned(),
            }),
            _ => Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::Input,
                "Invalid header (expect name and value separated by `:`): `{}`",
                s
            )),
        }
    }
}

impl Buck2OssReConfiguration {
    pub fn from_legacy_config(legacy_config: &LegacyBuckConfig) -> buck2_error::Result<Self> {
        // this is used for all three services by default, if given; if one of
        // them has an explicit address given as well though, use that instead
        let default_address: Option<String> = legacy_config.parse(BuckconfigKeyRef {
            section: BUCK2_RE_CLIENT_CFG_SECTION,
            property: "address",
        })?;

        Ok(Self {
            cas_address: legacy_config
                .parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "cas_address",
                })?
                .or(default_address.clone()),
            engine_address: legacy_config
                .parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "engine_address",
                })?
                .or(default_address.clone()),
            action_cache_address: legacy_config
                .parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "action_cache_address",
                })?
                .or(default_address),
            tls: legacy_config
                .parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "tls",
                })?
                .unwrap_or(true),
            tls_ca_certs: legacy_config.parse(BuckconfigKeyRef {
                section: BUCK2_RE_CLIENT_CFG_SECTION,
                property: "tls_ca_certs",
            })?,
            tls_client_cert: legacy_config.parse(BuckconfigKeyRef {
                section: BUCK2_RE_CLIENT_CFG_SECTION,
                property: "tls_client_cert",
            })?,
            http_headers: legacy_config
                .parse_list(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "http_headers",
                })?
                .unwrap_or_default(), // Empty list is as good None.
            capabilities: legacy_config.parse(BuckconfigKeyRef {
                section: BUCK2_RE_CLIENT_CFG_SECTION,
                property: "capabilities",
            })?,
            instance_name: legacy_config.parse(BuckconfigKeyRef {
                section: BUCK2_RE_CLIENT_CFG_SECTION,
                property: "instance_name",
            })?,
            use_fbcode_metadata: legacy_config
                .parse(BuckconfigKeyRef {
                    section: BUCK2_RE_CLIENT_CFG_SECTION,
                    property: "use_fbcode_metadata",
                })?
                .unwrap_or(false),
            max_decoding_message_size: legacy_config.parse(BuckconfigKeyRef {
                section: BUCK2_RE_CLIENT_CFG_SECTION,
                property: "max_decoding_message_size",
            })?,
            max_total_batch_size: legacy_config.parse(BuckconfigKeyRef {
                section: BUCK2_RE_CLIENT_CFG_SECTION,
                property: "max_total_batch_size",
            })?,
            max_concurrent_uploads_per_action: legacy_config.parse(BuckconfigKeyRef {
                section: BUCK2_RE_CLIENT_CFG_SECTION,
                property: "max_concurrent_uploads_per_action",
            })?,
            find_missing_blobs_batch_size: legacy_config.parse(BuckconfigKeyRef {
                section: BUCK2_RE_CLIENT_CFG_SECTION,
                property: "find_missing_blobs_batch_size",
            })?,
            cas_ttl_secs: legacy_config.parse(BuckconfigKeyRef {
                section: BUCK2_RE_CLIENT_CFG_SECTION,
                property: "cas_ttl_secs",
            })?,
            grpc_keepalive_time_secs: legacy_config.parse(BuckconfigKeyRef {
                section: BUCK2_RE_CLIENT_CFG_SECTION,
                property: "grpc_keepalive_time_secs",
            })?,
            grpc_keepalive_timeout_secs: legacy_config.parse(BuckconfigKeyRef {
                section: BUCK2_RE_CLIENT_CFG_SECTION,
                property: "grpc_keepalive_timeout_secs",
            })?,
            grpc_keepalive_while_idle: legacy_config.parse(BuckconfigKeyRef {
                section: BUCK2_RE_CLIENT_CFG_SECTION,
                property: "grpc_keepalive_while_idle",
            })?,
            execution_concurrency_limit: legacy_config.parse(BuckconfigKeyRef {
                section: BUCK2_RE_CLIENT_CFG_SECTION,
                property: "execution_concurrency_limit",
            })?,
            min_connections: legacy_config.parse(BuckconfigKeyRef {
                section: BUCK2_RE_CLIENT_CFG_SECTION,
                property: "min_connections",
            })?,
            max_connections: legacy_config.parse(BuckconfigKeyRef {
                section: BUCK2_RE_CLIENT_CFG_SECTION,
                property: "max_connections",
            })?,
            max_concurrency_per_connection: legacy_config.parse(BuckconfigKeyRef {
                section: BUCK2_RE_CLIENT_CFG_SECTION,
                property: "max_concurrency_per_connection",
            })?,
        })
    }
}

#[cfg(fbcode_build)]
pub use fbcode::RemoteExecutionStaticMetadata;
#[cfg(not(fbcode_build))]
pub use not_fbcode::RemoteExecutionStaticMetadata;
