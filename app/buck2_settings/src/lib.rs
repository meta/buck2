/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Typed Buck settings: the configuration buck2 core reads about itself,
//! replacing the buckconfig keys it used to read. Values come from
//! `.bucksettings.toml` files, the wrapper's rollout cache, and `--setting`;
//! every key is declared in this crate with its type, defaults, scope, and
//! override policy, and nothing here is readable from Starlark.

// The declaration macro refers to this crate by name, including from here.
extern crate self as buck2_settings;

mod agent_build;
mod agent_config_change;
pub mod agent_interactive;
mod agent_materialization;
mod analysis;
pub mod args;
pub mod buckconfig;
mod daemon;
pub mod dice;
mod digest;
mod event_log;
mod execution_platforms;
mod health_check;
mod http;
mod hydration;
mod log_download;
mod materializer;
pub mod metadata;
mod package_visibility;
pub mod parser;
mod path;
pub mod report;
mod resource_control;
#[cfg(fbcode_build)]
mod rollouts;
mod section;
pub mod settings;
mod starlark;
pub mod telemetry;

pub use agent_build::AgentBuild;
pub use agent_config_change::AgentConfigChange;
pub use agent_interactive::AgentInteractive;
pub use agent_materialization::AgentMaterialization;
pub use analysis::Analysis;
pub use buck2_settings_derive::buck_settings_section;
pub use buckconfig::BuckconfigSource;
pub use buckconfig::NoBuckconfig;
pub use daemon::Daemon;
pub use daemon::MacosQosClass;
pub use digest::Digest;
pub use event_log::EventLog;
pub use execution_platforms::ExecutionPlatforms;
pub use health_check::HealthCheck;
pub use http::Http;
pub use hydration::Hydration;
pub use hydration::PageOutOnIdleIsolationDirScope;
pub use log_download::LogDownload;
pub use materializer::Materializer;
pub use metadata::SectionReport;
pub use metadata::SettingsSection;
pub use package_visibility::PackageVisibility;
pub use package_visibility::PackageVisibilityDefaultIntersection;
pub use parser::ResolvedSettings;
pub use parser::SettingOrigin;
pub use parser::SettingsLayerReport;
pub use report::SettingsReport;
pub use resource_control::ResourceControl;
pub use section::Section;
pub use settings::BuckSettingChange;
pub use settings::BuckSettings;
pub use settings::DaemonBuckSettings;
pub use settings::DiceBuckSettings;
pub use settings::InvocationBuckSettings;
pub use starlark::Starlark;
