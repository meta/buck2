/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;
use std::time::Duration;

use allocative::Allocative;
use buck2_core::execution_types::executor_config::CacheUploadBehavior;
use buck2_core::execution_types::executor_config::CommandExecutorConfig;
use buck2_core::execution_types::executor_config::CommandGenerationOptions;
use buck2_core::execution_types::executor_config::Executor;
use buck2_core::execution_types::executor_config::ExecutorNetworkAccess;
use buck2_core::execution_types::executor_config::HybridExecutionLevel;
use buck2_core::execution_types::executor_config::ImagePackageIdentifier;
use buck2_core::execution_types::executor_config::LocalExecutorOptions;
use buck2_core::execution_types::executor_config::MetaInternalExtraParams;
use buck2_core::execution_types::executor_config::PathSeparatorKind;
use buck2_core::execution_types::executor_config::ReGang;
use buck2_core::execution_types::executor_config::ReGangLocality;
use buck2_core::execution_types::executor_config::ReGangWorker;
use buck2_core::execution_types::executor_config::RePlatformFields;
use buck2_core::execution_types::executor_config::RemoteEnabledExecutor;
use buck2_core::execution_types::executor_config::RemoteEnabledExecutorOptions;
use buck2_core::execution_types::executor_config::RemoteExecutionPolicy;
use buck2_core::execution_types::executor_config::RemoteExecutorCafFbpkg;
use buck2_core::execution_types::executor_config::RemoteExecutorCustomImage;
use buck2_core::execution_types::executor_config::RemoteExecutorDependency;
use buck2_core::execution_types::executor_config::RemoteExecutorOptions;
use buck2_core::execution_types::executor_config::RemoteExecutorUseCase;
use buck2_core::execution_types::executor_config::parse_network_access;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::internal_error;
use buck2_hash::BuckFixedSortedMap;
use derive_more::Display;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::values::Freeze;
use starlark::values::NoSerialize;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::dict::DictRef;
use starlark::values::dict::DictType;
use starlark::values::list::ListRef;
use starlark::values::list::ListType;
use starlark::values::none::NoneOr;
use starlark::values::none::NoneType;
use starlark::values::starlark_value;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum CommandExecutorConfigErrors {
    #[error("expected a dict, got `{0}` (type `{1}`)")]
    RePropertiesNotADict(String, String),
    #[error("expected `{0}` to be set")]
    MissingField(&'static str),
    #[error("invalid value in `{0}`")]
    InvalidField(&'static str),
    #[error("expected a dict, got `{0}` (type `{1}`)")]
    RePolicyNotADict(String, String),
    #[error("expected an list, got `{0}` (type `{1}`)")]
    ReCafFbpkgsNotAList(String, String),
    #[error("expected an dict, got `{0}` (type `{1}`)")]
    ReCafFbpkgNotADict(String, String),
    #[error("`remote_execution_gang` and `remote_execution_gang_workers` are mutually exclusive")]
    GangAndGangWorkersExclusive,
    #[error(
        "expected a dict for `capabilities` in `remote_execution_gang`, got `{0}` (type `{1}`)"
    )]
    ReGangCapabilitiesNotADict(String, String),
    #[error("expected an integer for `num_of_workers` in `remote_execution_gang`, got `{0}`")]
    ReGangNumOfWorkersNotAnInt(String),
    #[error("expected an integer for `num_sub_groups` in `remote_execution_gang`, got `{0}`")]
    ReGangNumSubGroupsNotAnInt(String),
    #[error("expected an integer for `resource_units` in `remote_execution_gang`, got `{0}`")]
    ReGangResourceUnitsNotAnInt(String),
    #[error(
        "expected a positive integer for `queue_timeout_s` in `remote_execution_policy`, got `{0}`"
    )]
    ReQueueTimeoutNotPositive(String),
}

#[derive(
    Debug,
    Display,
    Freeze,
    NoSerialize,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[display("{:?}", _0)]
pub struct StarlarkCommandExecutorConfig(
    #[freeze(identity)]
    #[starlark_pagable(pagable)]
    pub Arc<CommandExecutorConfig>,
);

starlark_simple_value!(StarlarkCommandExecutorConfig);

#[starlark_value(type = "CommandExecutorConfig")]
impl<'v> StarlarkValue<'v> for StarlarkCommandExecutorConfig {}

fn parse_remote_execution_properties(
    value: Value,
) -> buck2_error::Result<Option<RePlatformFields>> {
    if value.is_none() {
        return Ok(None);
    }

    let properties = DictRef::from_value(value.to_value()).ok_or_else(|| {
        buck2_error::Error::from(CommandExecutorConfigErrors::RePropertiesNotADict(
            value.to_repr(),
            value.get_type().to_owned(),
        ))
    })?;

    Ok(Some(RePlatformFields {
        properties: Arc::new(
            properties
                .iter()
                .map(|(name, value)| {
                    let values = ListRef::from_value(value).map_or_else(
                        || vec![value.to_str()],
                        |values| values.iter().map(Value::to_str).collect(),
                    );
                    (name.to_str(), values)
                })
                .collect(),
        ),
    }))
}

#[starlark_module]
pub fn register_command_executor_config(builder: &mut GlobalsBuilder) {
    /// Contains configurations for how actions should be executed
    ///
    /// * `local_enabled` : Whether to use local execution for this execution platform.
    /// If both remote_enabled and local_enabled are `True`, we will use the hybrid executor
    /// * `remote_enabled`: Whether to use remote execution for this execution platform
    /// * `remote_cache_enabled`: Whether to query RE caches
    /// * `remote_execution_properties`: Properties for remote execution for this platform. Each
    /// value may be a single value or a list of alternative values.
    /// * `remote_execution_action_key`: A component to inject into the action key
    /// This should typically used to inject variability into the action key so that
    /// it's different across e.g. build modes (RE uses the action key for things like expected memory utilization)
    /// * `remote_execution_max_input_files_mebibytes`: The maximum input file size (in bytes) that remote execution can support
    /// * `remote_execution_queue_time_threshold_s`: The maximum time in seconds we are willing to wait
    /// in the RE queue for remote execution to start running our action
    /// * `remote_execution_use_case`: The use case to use when communicating with RE
    /// * `use_limited_hybrid`: Whether to use the limited hybrid executor
    /// * `allow_limited_hybrid_fallbacks`: Whether to allow fallbacks
    /// * `allow_hybrid_fallbacks_on_failure`: Whether to allow fallbacks when the result is failure (i.e. the command failed on the primary, but the infra worked)
    /// * `use_windows_path_separators`: Whether to use Windows path separators in command line arguments
    /// * `use_persistent workers`: Whether to use persistent workers for local execution if they are available
    /// * `use_bazel_protocol_remote_persistent_workers`: Whether to use persistent workers for remote execution via the Bazel remote persistent worker protocol if they are available
    /// * `allow_cache_uploads`: Whether to upload local actions to the RE cache
    /// * `max_cache_upload_mebibytes`: Maximum size to upload in cache uploads
    /// * `experimental_low_pass_filter`: Whether to use the experimental low pass filter
    /// * `remote_output_paths`: How to express output paths to RE
    /// * `network_access`: Network access policy for commands using this executor. Supports `all`, `none`, `loopback`, `strict`, and `private`.
    /// * `remote_execution_resource_units`: The resources (eg. GPUs) to use for remote execution
    /// * `remote_execution_dependencies`: Dependencies for remote execution for this platform
    /// * `remote_execution_gang_workers`: Gang workers for gang scheduling in remote execution (enumerated gang spec)
    /// * `remote_execution_custom_image`: Custom Tupperware image for remote execution for this platform
    /// * `meta_internal_extra_params`: Json dict of extra params to pass to RE related to Meta internal infra.
    ///   Supports the following keys:
    ///   - `remote_execution_policy`: Policy settings for remote execution. A dict with optional keys
    ///     `priority`, `region_preference`, `setup_preference_key` and `queue_timeout_s` (seconds the
    ///     action may wait in the RE queue; RE caps it at the platform's queue timeout)
    ///   - `remote_execution_caf_fbpkgs`: CAF fbpkgs configuration
    ///   - `remote_execution_gang`: Constrained gang for gang scheduling in remote execution. A dict with keys:
    ///     - `capabilities`: A dict of capability key-value pairs (required)
    ///     - `num_of_workers`: Number of workers in the gang (required, integer)
    ///     - `locality`: Optional locality constraint ("region", "datacenter", or "network_domain")
    ///     Note: mutually exclusive with `remote_execution_gang_workers`
    ///   - `cancel_on_client_disconnect`: Directs RE to stop a started execution when buck2
    ///     closes the execution stream (e.g. the command is cancelled or the process exits).
    ///     Defaults to `False`, which lets RE run it to completion and write the result to cache.
    /// * `priority`: The priority for remote execution requests. The exact interpretation is up
    ///   to the RE server. See the Bazel Remote Execution API for recommended interpretation:
    ///   https://github.com/bazelbuild/remote-apis/blob/main/build/bazel/remote/execution/v2/remote_execution.proto#L1499
    #[starlark(as_type = StarlarkCommandExecutorConfig)]
    fn CommandExecutorConfig<'v>(
        #[starlark(require = named)] local_enabled: bool,
        #[starlark(require = named)] remote_enabled: bool,
        #[starlark(default = NoneOr::None, require = named)] remote_cache_enabled: NoneOr<bool>,
        #[starlark(default = false, require = named)] remote_dep_file_cache_enabled: bool,
        #[starlark(default = NoneType, require = named)] remote_execution_properties: Value<'v>,
        #[starlark(default = NoneType, require = named)] remote_execution_action_key: Value<'v>,
        #[starlark(default = NoneOr::None, require = named)]
        remote_execution_max_input_files_mebibytes: NoneOr<i32>,
        #[starlark(default = NoneOr::None, require = named)]
        remote_execution_queue_time_threshold_s: NoneOr<u64>,
        #[starlark(default = NoneType, require = named)] remote_execution_use_case: Value<'v>,
        #[starlark(default = false, require = named)] use_limited_hybrid: bool,
        #[starlark(default = false, require = named)] allow_limited_hybrid_fallbacks: bool,
        #[starlark(default = false, require = named)] allow_hybrid_fallbacks_on_failure: bool,
        #[starlark(default = false, require = named)] use_windows_path_separators: bool,
        #[starlark(default = false, require = named)] use_persistent_workers: bool,
        #[starlark(default = false, require = named)] use_bazel_protocol_remote_persistent_workers: bool,
        #[starlark(default = false, require = named)] allow_cache_uploads: bool,
        #[starlark(default = NoneOr::None, require = named)] max_cache_upload_mebibytes: NoneOr<
            i32,
        >,
        #[starlark(default = false, require = named)] experimental_low_pass_filter: bool,
        #[starlark(default = NoneOr::None, require = named)] remote_output_paths: NoneOr<&str>,
        #[starlark(default = NoneOr::None, require = named)] network_access: NoneOr<&str>,
        #[starlark(default = NoneOr::None, require = named)]
        remote_execution_resource_units: NoneOr<i64>,
        #[starlark(default = ValueOf::empty_list(), require = named)]
        remote_execution_dependencies: ValueOf<
            'v,
            ListType<ValueOf<'v, DictType<&'v str, &'v str>>>,
        >,
        #[starlark(default = ValueOf::empty_list(), require = named)]
        remote_execution_gang_workers: ValueOf<
            'v,
            ListType<ValueOf<'v, DictType<&'v str, &'v str>>>,
        >,
        #[starlark(default = NoneType, require = named)] remote_execution_dynamic_image: Value<'v>,
        #[starlark(default = NoneOr::None, require = named)] meta_internal_extra_params: NoneOr<
            DictRef<'v>,
        >,
        #[starlark(default = NoneOr::None, require = named)] priority: NoneOr<i32>,
    ) -> starlark::Result<StarlarkCommandExecutorConfig> {
        let command_executor_config = {
            let remote_execution_max_input_files_mebibytes: Option<i32> =
                remote_execution_max_input_files_mebibytes.into_option();

            let remote_execution_queue_time_threshold_s =
                remote_execution_queue_time_threshold_s.into_option();

            let re_resource_units = remote_execution_resource_units.into_option();

            let max_cache_upload_mebibytes = max_cache_upload_mebibytes.into_option();

            let re_properties =
                parse_remote_execution_properties(remote_execution_properties.to_value())?;

            let re_dependencies = remote_execution_dependencies
                .try_items()
                .map(|d| {
                    RemoteExecutorDependency::parse(
                        d?.try_items().collect::<starlark::Result<_>>()?,
                    )
                })
                .collect::<buck2_error::Result<Vec<RemoteExecutorDependency>>>()?;

            let re_gang_workers = remote_execution_gang_workers
                .try_items()
                .map(|w| ReGangWorker::parse(w?.try_items().collect::<starlark::Result<_>>()?))
                .collect::<buck2_error::Result<Vec<ReGangWorker>>>()?;

            let re_dynamic_image = parse_custom_re_image(
                "remote_execution_custom_image",
                remote_execution_dynamic_image,
            )?;

            let extra_params =
                parse_meta_internal_extra_params(meta_internal_extra_params.into_option())?;

            if extra_params.gang.is_some() && !re_gang_workers.is_empty() {
                return Err(buck2_error::Error::from(
                    CommandExecutorConfigErrors::GangAndGangWorkersExclusive,
                )
                .into());
            }

            let priority = priority.into_option();

            let re_use_case = if remote_execution_use_case.is_none() {
                None
            } else {
                let re_use_case = remote_execution_use_case
                    .unpack_str()
                    .internal_error("remote_execution_use_case is not a string")?;
                Some(RemoteExecutorUseCase::new(re_use_case.to_owned()))
            };

            let re_action_key = if remote_execution_action_key.is_none() {
                None
            } else {
                let re_action_key = remote_execution_action_key.unpack_str().ok_or_else(|| {
                    internal_error!("remote_execution_action_key is not a string")
                })?;
                Some(re_action_key.to_owned())
            };

            let local_options = if local_enabled {
                Some(LocalExecutorOptions {
                    use_persistent_workers,
                })
            } else {
                None
            };
            let remote_options = if remote_enabled {
                let re_max_input_files_bytes = remote_execution_max_input_files_mebibytes
                    .map(u64::try_from)
                    .transpose()
                    .buck_error_context("remote_execution_max_input_files_mebibytes is negative")?
                    .map(|b| b * 1024 * 1024);

                let re_max_queue_time = buck2_common::self_test_timeout::maybe_cap_timeout(
                    remote_execution_queue_time_threshold_s.map(Duration::from_secs),
                );

                Some(RemoteExecutorOptions {
                    re_max_input_files_bytes,
                    re_max_queue_time,
                    re_resource_units,
                })
            } else {
                None
            };

            let fallback_on_failure = allow_hybrid_fallbacks_on_failure;

            let hybrid_level = match (use_limited_hybrid, allow_limited_hybrid_fallbacks) {
                (true, true) => HybridExecutionLevel::Fallback {
                    fallback_on_failure,
                },
                (true, false) => HybridExecutionLevel::Limited,
                (false, _) => HybridExecutionLevel::Full {
                    fallback_on_failure,
                    low_pass_filter: experimental_low_pass_filter,
                },
            };

            let max_cache_upload_bytes = max_cache_upload_mebibytes
                .map(u64::try_from)
                .transpose()
                .buck_error_context("max_cache_upload_mebibytes is negative")?
                .map(|b| b * 1024 * 1024);

            let cache_upload_behavior = if allow_cache_uploads {
                CacheUploadBehavior::Enabled {
                    max_bytes: max_cache_upload_bytes,
                }
            } else {
                CacheUploadBehavior::Disabled
            };

            // FIXME: This should probably default to `remote_enabled` and not `true`, but
            // historically this has been `true`, so we should probably migrate our defs to set
            // `remote_cache_enabled = True` explicitly first.
            let remote_cache_default = if buck2_core::is_open_source() {
                remote_enabled
            } else {
                true
            };
            let remote_cache_enabled = remote_cache_enabled
                .into_option()
                .unwrap_or(remote_cache_default);

            let executor = match (local_options, remote_options, remote_cache_enabled) {
                (local, Some(remote), remote_cache_enabled) => {
                    let executor = match local {
                        Some(local) => RemoteEnabledExecutor::Hybrid {
                            local,
                            remote,
                            level: hybrid_level,
                        },
                        None => RemoteEnabledExecutor::Remote(remote),
                    };

                    Executor::RemoteEnabled(RemoteEnabledExecutorOptions {
                        executor,
                        re_properties: re_properties.ok_or(buck2_error::Error::from(
                            CommandExecutorConfigErrors::MissingField(
                                "remote_execution_properties",
                            ),
                        ))?,
                        re_use_case: re_use_case.ok_or(buck2_error::Error::from(
                            CommandExecutorConfigErrors::MissingField("re_use_case"),
                        ))?,
                        re_action_key,
                        cache_upload_behavior,
                        remote_cache_enabled,
                        remote_dep_file_cache_enabled,
                        dependencies: re_dependencies,
                        gang_workers: re_gang_workers,
                        custom_image: re_dynamic_image,
                        meta_internal_extra_params: extra_params,
                        priority,
                    })
                }
                (Some(local), None, true) => {
                    Executor::RemoteEnabled(RemoteEnabledExecutorOptions {
                        executor: RemoteEnabledExecutor::Local(local),
                        // FIXME: We need a migration flip the default for remote_cache_enabled to
                        // remote_enabled first.
                        re_properties: re_properties.unwrap_or_default(),
                        re_use_case: re_use_case
                            .unwrap_or_else(RemoteExecutorUseCase::buck2_default),
                        re_action_key,
                        cache_upload_behavior,
                        remote_cache_enabled: true,
                        remote_dep_file_cache_enabled,
                        dependencies: re_dependencies,
                        gang_workers: re_gang_workers,
                        custom_image: re_dynamic_image,
                        meta_internal_extra_params: extra_params,
                        priority,
                    })
                }
                // If remote cache is disabled, also disable the remote dep file cache as well
                (Some(local), None, false) => Executor::Local(local),
                (None, None, _) => Executor::None,
            };

            let output_paths_behavior = remote_output_paths
                .into_option()
                .map(|s| s.parse())
                .transpose()
                .buck_error_context("Invalid remote_output_paths")?
                .unwrap_or_default();

            let network_access = network_access
                .into_option()
                .map(parse_network_access)
                .transpose()
                .buck_error_context("Invalid network_access")?
                .map(ExecutorNetworkAccess::from);

            CommandExecutorConfig {
                executor,
                options: CommandGenerationOptions {
                    path_separator: if use_windows_path_separators {
                        PathSeparatorKind::Windows
                    } else {
                        PathSeparatorKind::Unix
                    },
                    output_paths_behavior,
                    use_bazel_protocol_remote_persistent_workers,
                    network_access,
                },
            }
        };

        Ok(StarlarkCommandExecutorConfig(Arc::new(
            command_executor_config,
        )))
    }
}

pub fn parse_custom_re_image(
    field_name: &'static str,
    value: Value,
) -> buck2_error::Result<Option<Box<RemoteExecutorCustomImage>>> {
    if value.is_none() {
        return Ok(None);
    }

    fn dict_ref<'v>(
        field_name: &'static str,
        value: Value<'v>,
    ) -> buck2_error::Result<DictRef<'v>> {
        match DictRef::from_value(value) {
            Some(dict_ref) => Ok(dict_ref),
            None => Err(CommandExecutorConfigErrors::InvalidField(field_name).into()),
        }
    }

    fn list_ref<'v>(
        field_name: &'static str,
        value: Value<'v>,
    ) -> buck2_error::Result<&'v ListRef<'v>> {
        match ListRef::from_value(value) {
            Some(list_ref) => Ok(list_ref),
            None => Err(CommandExecutorConfigErrors::InvalidField(field_name).into()),
        }
    }

    fn get_value<'v>(dict_ref: &DictRef<'v>, name: &'static str) -> buck2_error::Result<Value<'v>> {
        if let Some(value) = dict_ref.get_str(name) {
            if value.is_none() {
                Err(CommandExecutorConfigErrors::InvalidField(name).into())
            } else {
                Ok(value)
            }
        } else {
            Err(CommandExecutorConfigErrors::MissingField(name).into())
        }
    }

    fn get_string<'v>(dict_ref: &DictRef<'v>, name: &'static str) -> buck2_error::Result<String> {
        let value = get_value(dict_ref, name)?;
        Ok(value.to_str())
    }

    let dict = dict_ref(field_name, value)?;
    let identifier_value = get_value(&dict, "identifier")?;
    let identifier = if let Some(identifier_dict) = DictRef::from_value(identifier_value) {
        ImagePackageIdentifier {
            name: get_string(&identifier_dict, "name")?,
            uuid: get_string(&identifier_dict, "uuid")?,
        }
    } else {
        let identifier = identifier_value.to_str();
        if let Some(index) = identifier.rfind(':') {
            ImagePackageIdentifier {
                name: identifier[..index].to_owned(),
                uuid: identifier[(index + 1)..].to_owned(),
            }
        } else {
            ImagePackageIdentifier {
                name: identifier,
                uuid: "".to_owned(),
            }
        }
    };

    let mount_globs_value = get_value(&dict, "drop_host_mount_globs")?;
    let mount_globs = list_ref("drop_host_mount_globs", mount_globs_value)?;
    let mut drop_host_mount_globs = vec![];
    for item in mount_globs.iter() {
        drop_host_mount_globs.push(item.to_str());
    }

    Ok(Some(Box::new(RemoteExecutorCustomImage {
        identifier,
        drop_host_mount_globs,
    })))
}

fn parse_remote_execution_policy(
    policy: Option<Value>,
) -> buck2_error::Result<RemoteExecutionPolicy> {
    match policy {
        None => Ok(RemoteExecutionPolicy::default()),
        Some(policy) => {
            let re_policy_dict = DictRef::from_value(policy.to_value()).ok_or_else(|| {
                buck2_error::Error::from(CommandExecutorConfigErrors::RePolicyNotADict(
                    policy.to_value().to_repr(),
                    policy.to_value().get_type().to_owned(),
                ))
            })?;

            let queue_timeout_s = match re_policy_dict.get_str("queue_timeout_s") {
                None => None,
                Some(v) => match v.unpack_i32() {
                    Some(seconds) if seconds > 0 => Some(seconds),
                    _ => {
                        return Err(CommandExecutorConfigErrors::ReQueueTimeoutNotPositive(
                            v.to_repr(),
                        )
                        .into());
                    }
                },
            };

            Ok(RemoteExecutionPolicy {
                setup_preference_key: re_policy_dict
                    .get_str("setup_preference_key")
                    .and_then(|v| v.unpack_str())
                    .map(|s| s.to_owned()),
                region_preference: re_policy_dict
                    .get_str("region_preference")
                    .and_then(|v| v.unpack_str())
                    .map(|s| s.to_owned()),
                priority: re_policy_dict
                    .get_str("priority")
                    .and_then(|v| v.unpack_i32())
                    .map(|i| i.to_owned()),
                queue_timeout_s,
            })
        }
    }
}

fn parse_remote_execution_caf_fbpkgs(
    caf_fbpkgs: Option<Value>,
) -> buck2_error::Result<Vec<RemoteExecutorCafFbpkg>> {
    match caf_fbpkgs {
        None => Ok(vec![]),
        Some(caf_fbpkgs) => {
            let re_caf_fbpkgs_list =
                ListRef::from_value(caf_fbpkgs.to_value()).ok_or_else(|| {
                    buck2_error::Error::from(CommandExecutorConfigErrors::ReCafFbpkgsNotAList(
                        caf_fbpkgs.to_value().to_repr(),
                        caf_fbpkgs.to_value().get_type().to_owned(),
                    ))
                })?;

            Ok(re_caf_fbpkgs_list
                .iter()
                .map(|caf_fbpkg| match DictRef::from_value(caf_fbpkg) {
                    Some(dict_ref) => Ok(RemoteExecutorCafFbpkg {
                        name: dict_ref
                            .get_str("name")
                            .ok_or(CommandExecutorConfigErrors::MissingField("name"))?
                            .to_str(),
                        uuid: dict_ref
                            .get_str("uuid")
                            .ok_or(CommandExecutorConfigErrors::MissingField("uuid"))?
                            .to_str(),
                        tag: dict_ref.get_str("tag").map(|v| v.to_str()),
                        permissions: dict_ref.get_str("permissions").map(|v| v.to_str()),
                    }),
                    None => Err(buck2_error::Error::from(
                        CommandExecutorConfigErrors::ReCafFbpkgNotADict(
                            caf_fbpkg.to_repr(),
                            caf_fbpkg.get_type().to_owned(),
                        ),
                    )),
                })
                .collect::<buck2_error::Result<Vec<RemoteExecutorCafFbpkg>>>()?)
        }
    }
}

pub fn parse_meta_internal_extra_params<'v>(
    params: Option<DictRef<'v>>,
) -> buck2_error::Result<Arc<MetaInternalExtraParams>> {
    let Some(params) = params else {
        return Ok(MetaInternalExtraParams::default_arc());
    };

    let gang = params
        .get_str("remote_execution_gang")
        .and_then(DictRef::from_value)
        .map(|dict| parse_remote_execution_gang(Some(dict)))
        .transpose()?
        .flatten();

    let allow_unsandboxed_action_cache_uploads = params
        .get_str("allow_unsandboxed_action_cache_uploads")
        .and_then(|v| v.unpack_bool())
        .unwrap_or(false);

    let cancel_on_client_disconnect = params
        .get_str("cancel_on_client_disconnect")
        .and_then(|v| v.unpack_bool())
        .unwrap_or(false);

    let result = MetaInternalExtraParams {
        remote_execution_policy: parse_remote_execution_policy(
            params.get_str("remote_execution_policy"),
        )?,
        remote_execution_caf_fbpkgs: parse_remote_execution_caf_fbpkgs(
            params.get_str("remote_execution_caf_fbpkgs"),
        )?,
        gang,
        allow_unsandboxed_action_cache_uploads,
        cancel_on_client_disconnect,
    };

    if result == MetaInternalExtraParams::default() {
        Ok(MetaInternalExtraParams::default_arc())
    } else {
        Ok(Arc::new(result))
    }
}
fn parse_remote_execution_gang<'v>(
    gang: Option<DictRef<'v>>,
) -> buck2_error::Result<Option<ReGang>> {
    let Some(gang) = gang else {
        return Ok(None);
    };

    // Parse capabilities as a nested dict
    let capabilities_value = gang
        .get_str("capabilities")
        .ok_or(CommandExecutorConfigErrors::MissingField("capabilities"))?;
    let capabilities_dict = DictRef::from_value(capabilities_value).ok_or_else(|| {
        buck2_error::Error::from(CommandExecutorConfigErrors::ReGangCapabilitiesNotADict(
            capabilities_value.to_repr(),
            capabilities_value.get_type().to_owned(),
        ))
    })?;

    let capabilities: BuckFixedSortedMap<String, String> = capabilities_dict
        .iter()
        .map(|(k, v)| (k.to_str(), v.to_str()))
        .collect();

    if capabilities.is_empty() {
        return Err(CommandExecutorConfigErrors::MissingField("capabilities").into());
    }

    let num_of_workers_value = gang
        .get_str("num_of_workers")
        .ok_or(CommandExecutorConfigErrors::MissingField("num_of_workers"))?;
    let num_of_workers: i32 = num_of_workers_value.unpack_i32().ok_or_else(|| {
        buck2_error::Error::from(CommandExecutorConfigErrors::ReGangNumOfWorkersNotAnInt(
            num_of_workers_value.to_repr(),
        ))
    })?;

    let locality = gang
        .get_str("locality")
        .and_then(|v| v.unpack_str())
        .map(ReGangLocality::parse)
        .transpose()?;

    let num_sub_groups = gang
        .get_str("num_sub_groups")
        .map(|v| {
            v.unpack_i32().ok_or_else(|| {
                buck2_error::Error::from(CommandExecutorConfigErrors::ReGangNumSubGroupsNotAnInt(
                    v.to_repr(),
                ))
            })
        })
        .transpose()?;

    let resource_units = gang
        .get_str("resource_units")
        .map(|v| {
            v.unpack_i32().ok_or_else(|| {
                buck2_error::Error::from(CommandExecutorConfigErrors::ReGangResourceUnitsNotAnInt(
                    v.to_repr(),
                ))
            })
        })
        .transpose()?;

    Ok(Some(ReGang::parse(
        capabilities,
        num_of_workers,
        locality,
        num_sub_groups,
        resource_units,
    )?))
}

#[cfg(test)]
mod tests {
    use buck2_interpreter::dice::starlark_provider::StarlarkEvalKind;
    use buck2_interpreter::factory::BuckStarlarkModule;
    use buck2_interpreter::factory::StarlarkEvaluatorProvider;
    use dice_futures::cancellation::CancellationContext;
    use starlark::values::Heap;
    use starlark::values::dict::AllocDict;
    use starlark::values::dict::DictRef;
    use starlark::values::list::AllocList;

    use crate::interpreter::rule_defs::command_executor_config::parse_meta_internal_extra_params;
    use crate::interpreter::rule_defs::command_executor_config::parse_remote_execution_properties;

    #[test]
    fn test_parse_remote_execution_properties_with_multiple_values() -> buck2_error::Result<()> {
        BuckStarlarkModule::with_profiling(|module| {
            let architectures = module.heap().alloc(AllocList(["x86_64", "aarch64"]));
            let platform = module.heap().alloc("linux-remote-execution");
            let properties = module
                .heap()
                .alloc(AllocDict([("arch", architectures), ("platform", platform)]));

            let properties = parse_remote_execution_properties(properties)
                .expect("Properties should parse")
                .expect("Properties should be present");

            assert_eq!(
                properties.properties.get("arch"),
                Some(&vec!["x86_64".to_owned(), "aarch64".to_owned()]),
            );
            assert_eq!(
                properties.properties.get("platform"),
                Some(&vec!["linux-remote-execution".to_owned()]),
            );

            let provider =
                StarlarkEvaluatorProvider::passthrough(StarlarkEvalKind::Unknown("test".into()));
            let (finished_eval, ()) = provider.with_evaluator(
                &module,
                CancellationContext::testing().into(),
                |_eval, _| Ok(()),
            )?;
            let (profiling, _) = finished_eval.finish()?;
            Ok((profiling, ()))
        })
    }

    #[test]
    fn test_parse_meta_internal_extra_params_cancel_on_client_disconnect() -> buck2_error::Result<()>
    {
        Heap::temp(|heap| {
            let params = heap.alloc(AllocDict([("cancel_on_client_disconnect", true)]));
            let params = parse_meta_internal_extra_params(DictRef::from_value(params))?;
            assert!(params.cancel_on_client_disconnect);

            let params = heap.alloc(AllocDict([(
                "allow_unsandboxed_action_cache_uploads",
                true,
            )]));
            let params = parse_meta_internal_extra_params(DictRef::from_value(params))?;
            assert!(!params.cancel_on_client_disconnect, "defaults to false");
            Ok(())
        })
    }
}
