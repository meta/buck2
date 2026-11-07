/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::env;
use std::io;

fn main() -> io::Result<()> {
    let proto_files = &["data.proto", "error.proto"];

    let buck_proto_srcs = env::var("BUCK_PROTO_SRCS");
    let includes = if let Ok(path) = &buck_proto_srcs {
        vec![path.as_str()]
    } else {
        vec![".", "../buck2_host_sharing_proto"]
    };

    let builder = buck2_protoc_dev::configure();
    unsafe { builder.setup_protoc() }
        .type_attribute(
            "buck.data.BuckEvent.data",
            "#[allow(clippy::large_enum_variant)]",
        )
        .type_attribute(
            "buck.data.SpanEndEvent.data",
            "#[allow(clippy::large_enum_variant)]",
        )
        .type_attribute(
            "buck.data.InstantEvent.data",
            "#[allow(clippy::large_enum_variant)]",
        )
        .type_attribute(
            "buck.data.BuckEvent.data",
            "#[derive(::derive_more::From, ::variant_name::VariantName)]",
        )
        .type_attribute(
            "buck.data.SpanStartEvent.data",
            "#[derive(::derive_more::From, ::variant_name::VariantName)]",
        )
        .type_attribute(
            "buck.data.SpanEndEvent.data",
            "#[derive(::derive_more::From, ::variant_name::VariantName)]",
        )
        .type_attribute(
            "buck.data.CommandStart.data",
            "#[derive(::derive_more::From, ::variant_name::VariantName)]",
        )
        .type_attribute(
            "buck.data.CommandEnd.data",
            "#[derive(::derive_more::From, ::variant_name::VariantName)]",
        )
        .type_attribute(
            "buck.data.CommandCriticalStart.data",
            "#[derive(::derive_more::From, ::variant_name::VariantName)]",
        )
        .type_attribute(
            "buck.data.CommandCriticalEnd.data",
            "#[derive(::derive_more::From, ::variant_name::VariantName)]",
        )
        .type_attribute(
            "buck.data.InstantEvent.data",
            "#[derive(::derive_more::From, ::variant_name::VariantName)]",
        )
        .type_attribute(
            "buck.data.RecordEvent.data",
            "#[derive(::derive_more::From, ::variant_name::VariantName)]",
        )
        .type_attribute(
            "buck.data.LocalStage.stage",
            "#[derive(::derive_more::From)]",
        )
        .type_attribute("buck.data.ReStage.stage", "#[derive(::derive_more::From)]")
        .type_attribute(
            "buck.data.ExecutorStageStart.stage",
            "#[derive(::derive_more::From)]",
        )
        .type_attribute(
            "buck.data.CommandExecution.status",
            "#[derive(::derive_more::From, ::variant_name::VariantName)]",
        )
        .type_attribute(
            "buck.data.ActionExecutionEnd.error",
            "#[derive(::derive_more::From, ::variant_name::VariantName)]",
        )
        .type_attribute(
            "buck.data.ActionError.error",
            "#[derive(::derive_more::From, ::variant_name::VariantName)]",
        )
        .field_attribute(
            "buck.data.CommandExecutionDetails.cmd_stderr",
            "#[serde(rename = \"stderr\")]",
        )
        .field_attribute(
            "buck.data.CommandExecutionDetails.cmd_stdout",
            "#[serde(rename = \"stdout\")]",
        )
        .type_attribute(
            "buck.data.CommandExecutionDetails.command",
            "#[derive(::derive_more::From, ::variant_name::VariantName)]",
        )
        .type_attribute(
            "buck.data.DynamicLambdaStart.owner",
            "#[derive(::derive_more::From, ::variant_name::VariantName)]",
        )
        .type_attribute(
            "buck.data.DeferredPreparationStageStart.stage",
            "#[derive(::derive_more::From, ::variant_name::VariantName)]",
        )
        .type_attribute(
            "buck.data.AnalysisStart.target",
            "#[derive(::derive_more::From, ::variant_name::VariantName)]",
        )
        .type_attribute(
            "buck.data.AnalysisEnd.target",
            "#[derive(::derive_more::From, ::variant_name::VariantName)]",
        )
        .type_attribute(
            "buck.data.ActionKind",
            "#[derive(::variant_name::VariantName, ::pagable::Pagable)]",
        )
        .type_attribute(
            "buck.data.MaterializationMethod",
            "#[derive(::variant_name::VariantName)]",
        )
        .type_attribute("buck.data.CpuCounter", "#[derive(dupe::Dupe)]")
        .type_attribute("buck.data.CommandExecutionStats", "#[derive(dupe::Dupe)]")
        .type_attribute(".", "#[derive(::serde::Serialize, ::serde::Deserialize)]")
        .type_attribute(".", "#[derive(::allocative::Allocative)]")
        // Keep the JSON event log for ordinary failures as `{"Failure": {}}`.
        .field_attribute(
            "buck.data.CommandExecution.Failure.missing_materialized_inputs",
            "#[serde(default, skip_serializing_if = \"std::ops::Not::not\")]",
        )
        .field_attribute(
            "timestamp",
            "#[serde(with = \"::buck2_proto_serde::serialize_timestamp\")]",
        )
        .field_attribute(
            "start_time",
            "#[serde(default, with = \"::buck2_proto_serde::serialize_timestamp\")]",
        )
        .field_attribute(
            "buck.data.BuildPhaseEnd.end_time",
            "#[serde(default, with = \"::buck2_proto_serde::serialize_timestamp\")]",
        )
        .field_attribute(
            "client_start_time",
            "#[serde(default, with = \"::buck2_proto_serde::serialize_timestamp\")]",
        )
        .field_attribute(
            "event_time",
            "#[serde(default, with = \"::buck2_proto_serde::serialize_timestamp\")]",
        )
        .field_attribute(
            "time_event_generated",
            "#[serde(default, with = \"::buck2_proto_serde::serialize_timestamp\")]",
        )
        .field_attribute(
            "time_collected",
            "#[serde(default, with = \"::buck2_proto_serde::serialize_timestamp\")]",
        )
        .field_attribute(
            "suspend_duration",
            "#[serde(default, with = \"::buck2_proto_serde::serialize_duration_as_micros\")]",
        )
        .field_attribute(
            "duration",
            "#[serde(rename = \"duration_us\", with = \"::buck2_proto_serde::serialize_duration_as_micros\")]",
        )
        .field_attribute(
            "TestRunEnd.timeout",
            "#[serde(default, rename = \"timeout_us\", with = \"::buck2_proto_serde::serialize_duration_as_micros\")]",
        )
        .field_attribute(
            "command_duration",
            "#[serde(rename = \"command_duration_us\", with = \"::buck2_proto_serde::serialize_duration_as_micros\")]",
        )
        .field_attribute(
            "client_walltime",
            "#[serde(rename = \"client_walltime_us\", with = \"::buck2_proto_serde::serialize_duration_as_micros\")]",
        )
        .field_attribute(
            "critical_path_duration",
            "#[serde(rename = \"critical_path_duration_us\", with = \"::buck2_proto_serde::serialize_duration_as_micros\")]",
        )
        .field_attribute(
            "critical_path_page_in",
            "#[serde(rename = \"critical_path_page_in_us\", with = \"::buck2_proto_serde::serialize_duration_as_micros\")]",
        )
        .field_attribute(
            "ActionExecutionEnd.wall_time",
            "#[serde(rename = \"wall_time_us\", with = \"::buck2_proto_serde::serialize_duration_as_micros\")]",
        )
        .field_attribute(
            "ActionKey.id",
            "#[serde(with = \"crate::serialize_bytes\")]",
        )
        // When serializing using Serde we don't want those to just be i32s, since those are
        // meaningless without the Protobuf schema.
        .field_attribute(
            "ActionExecutionStart.kind",
            "#[serde(with = \"crate::serialize_action_kind\")]",
        )
        .field_attribute(
            "ActionExecutionEnd.kind",
            "#[serde(with = \"crate::serialize_action_kind\")]",
        )
        .field_attribute(
            "RemoteCommand.queue_time",
            "#[serde(rename = \"queue_time_us\", with = \"::buck2_proto_serde::serialize_duration_as_micros\")]",
        )
        .field_attribute(
            "concurrent_command_blocking_duration",
            "#[serde(rename = \"concurrent_command_blocking_duration_us\", with = \"::buck2_proto_serde::serialize_duration_as_micros\")]",
        )
        .field_attribute(
            "bxl_ensure_artifacts_duration",
            "#[serde(rename = \"bxl_ensure_artifacts_duration_us\", with = \"::buck2_proto_serde::serialize_duration_as_micros\")]",
        )
        .field_attribute(
            "install_duration",
            "#[serde(rename = \"install_duration_us\", with = \"::buck2_proto_serde::serialize_duration_as_micros\")]",
        )
        .field_attribute(
            "CriticalPathEntry2.user_duration",
            "#[serde(rename = \"user_duration_us\", with = \"::buck2_proto_serde::serialize_duration_as_micros\")]",
        )
        .field_attribute(
            "CriticalPathEntry2.total_duration",
            "#[serde(rename = \"total_duration_us\", with = \"::buck2_proto_serde::serialize_duration_as_micros\")]",
        )
        .field_attribute(
            "CriticalPathEntry2.potential_improvement_duration",
            "#[serde(rename = \"potential_improvement_duration_us\", with = \"::buck2_proto_serde::serialize_duration_as_micros\")]",
        )
        .field_attribute(
            "CriticalPathEntry2.queue_duration",
            "#[serde(rename = \"queue_duration_us\", with = \"::buck2_proto_serde::serialize_duration_as_micros\")]",
        )
        .field_attribute(
            "CriticalPathEntry2.non_critical_path_duration",
            "#[serde(rename = \"non_critical_path_duration_us\", with = \"::buck2_proto_serde::serialize_duration_as_micros\")]",
        )
        .type_attribute(
            "buck.data.CriticalPathEntry2.entry",
            "#[derive(::derive_more::From, ::variant_name::VariantName)]",
        )
        .type_attribute(
            "buck.data.CriticalPathEntry2.Analysis.target",
            "#[derive(::derive_more::From, ::variant_name::VariantName)]",
        )
        .type_attribute(
            "buck.data.CriticalPathEntry2.ActionExecution.owner",
            "#[derive(::derive_more::From, ::variant_name::VariantName)]",
        )
        .type_attribute(
            "buck.data.CriticalPathEntry2.Materialization.owner",
            "#[derive(::derive_more::From, ::variant_name::VariantName)]",
        )
        .type_attribute(
            "buck.data.StarlarkUserMetadataDictValue",
            "#[serde(transparent)]",
        )
        .type_attribute(
            "buck.data.StarlarkUserMetadataListValue",
            "#[serde(transparent)]",
        )
        .type_attribute(
            "buck.data.StarlarkUserMetadataValue",
            "#[serde(transparent)]",
        )
        .type_attribute(
            "buck.data.StarlarkUserMetadataValue.value",
            "#[serde(untagged)]",
        )
        .type_attribute(
            "buck.data.CommandExecutionKind.command",
            "#[derive(::derive_more::From, ::variant_name::VariantName)]",
        )
        .field_attribute(
            "buck.data.Invocation.expanded_command_line_args",
            "#[serde(default)]",
        )
        .field_attribute(
            "buck.data.CommandExecutionMetadata.wall_time",
            "#[serde(rename = \"wall_time_us\", with = \"::buck2_proto_serde::serialize_duration_as_micros\")]",
        )
        .field_attribute(
            "buck.data.CommandExecutionMetadata.execution_time",
            "#[serde(rename = \"execution_time_us\", with = \"::buck2_proto_serde::serialize_duration_as_micros\")]",
        )
        .field_attribute(
            "buck.data.CommandExecutionMetadata.input_materialization_duration",
            "#[serde(rename = \"input_materialization_duration_us\", with = \"::buck2_proto_serde::serialize_duration_as_micros\")]",
        )
        .field_attribute(
            "buck.data.CommandExecutionMetadata.hashing_duration",
            "#[serde(rename = \"hashing_duration_us\", with = \"::buck2_proto_serde::serialize_duration_as_micros\")]",
        )
        .field_attribute(
            "buck.data.CommandExecutionMetadata.queue_duration",
            "#[serde(rename = \"queue_duration_us\", with = \"::buck2_proto_serde::serialize_duration_as_micros\")]",
        )
        .boxed("RecordEvent.data.invocation_record")
        // Putting some of the larger, rarer payloads into a Box reduce the size of every event in the event queue
        .boxed("SpanEndEvent.data.action_execution")
        .boxed("RemoteCommand.re_worker")
        .boxed("SpanEndEvent.data.cache_upload")
        .boxed("SpanEndEvent.data.dep_file_upload")
        .boxed("SpanEndEvent.data.test_discovery")
        .boxed("SpanEndEvent.data.test_run")
        .boxed("CommandEnd.data.clean")
        .boxed("InstantEvent.data.action_error")
        .boxed("InstantEvent.data.clean_stale_result")
        .boxed("InstantEvent.data.page_out_summary")
        .boxed("InstantEvent.data.paging_summary")
        .boxed("InstantEvent.data.rage_result")
        .boxed("InstantEvent.data.resource_control_event")
        .boxed("InstantEvent.data.snapshot")
        .extern_path(".buck.host_sharing", "::buck2_host_sharing_proto")
        .compile(proto_files, &includes)
}
