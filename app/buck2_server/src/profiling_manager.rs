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

use buck2_cli_proto::client_context::ProfilePatternOptions;
use buck2_common::manifold::Bucket;
use buck2_common::manifold::ManifoldClient;
use buck2_error::BuckErrorContext;
use buck2_events::dispatch::EventDispatcher;
use buck2_fs::paths::abs_path::AbsPath;
use buck2_fs::paths::abs_path::AbsPathBuf;
use buck2_interpreter::starlark_profiler::config::ProfileRegex;
use buck2_interpreter::starlark_profiler::config::StarlarkProfilerConfiguration;
use buck2_profile::proto_to_profile_mode;
use buck2_wrapper_common::invocation_id::TraceId;
use dupe::Dupe;

use crate::profile_patterns::FileWritingProfileEventListener;

#[derive(buck2_error::Error)]
pub enum BuckdServerError {
    #[error(
        "server received multiple profiler configurations. this is due to using both --profile-patterns and one of the command specific profiling args"
    )]
    #[buck2(tag=buck2_error::ErrorTag::Input)]
    StarlarkProfilerConfigurationConflict,
}

pub struct StarlarkProfilingManager {
    pub(crate) configuration: StarlarkProfilerConfiguration,
    pub(crate) profile_event_listener: Option<Arc<FileWritingProfileEventListener>>,
    trace_id: TraceId,
}

impl StarlarkProfilingManager {
    pub fn new(
        profile_pattern_opts: Option<&ProfilePatternOptions>,
        starlark_profile_override: StarlarkProfilerConfiguration,
        events: &EventDispatcher,
    ) -> buck2_error::Result<Self> {
        let starlark_profile_patterns_opts = match profile_pattern_opts {
            Some(ProfilePatternOptions {
                profile_patterns,
                profile_mode,
                profile_output,
            }) => Some(StarlarkProfilerConfiguration::ProfilePattern(
                proto_to_profile_mode(
                    (*profile_mode)
                        .try_into()
                        .internal_error("invalid profile mode enum value")?,
                ),
                ProfileRegex::new(profile_patterns)?,
                AbsPathBuf::try_from(profile_output.to_owned())?,
            )),
            None => None,
        };
        let configuration = match (starlark_profile_patterns_opts, starlark_profile_override) {
            (None, v) => v,
            (Some(v), StarlarkProfilerConfiguration::None) => v,
            (Some(_), _) => Err(BuckdServerError::StarlarkProfilerConfigurationConflict)?,
        };

        let profile_event_listener = if let StarlarkProfilerConfiguration::ProfilePattern(
            _,
            _,
            output_path,
        ) = &configuration
        {
            Some(Arc::new(FileWritingProfileEventListener::new(
                output_path.join(format!(
                    "{}-{}",
                    jiff::Zoned::now().strftime("%Y-%m-%d-%H-%M"),
                    events.trace_id()
                )),
            )))
        } else {
            None
        };

        Ok(Self {
            configuration,
            profile_event_listener,
            trace_id: events.trace_id().dupe(),
        })
    }

    pub async fn finalize(&self, events: &EventDispatcher) -> buck2_error::Result<()> {
        if let Some(v) = &self.profile_event_listener {
            let merged_svg_path = v.finalize()?;
            if let Some(svg_path) = merged_svg_path {
                match self.upload_profile_to_manifold(&svg_path).await {
                    Ok(url) => {
                        events.console_message(format!(
                            "Merged flamegraph uploaded to manifold: {url}"
                        ));
                    }
                    Err(e) => {
                        events.console_message(format!(
                            "Warning: failed to upload profile to manifold: {e:#}"
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    async fn upload_profile_to_manifold(&self, svg_path: &AbsPath) -> buck2_error::Result<String> {
        let manifold = ManifoldClient::new().await?;
        let manifold_filename = format!("flat/{}-profile-merged.svg", self.trace_id);
        manifold
            .upload_file(
                svg_path,
                manifold_filename,
                Bucket::EVENT_LOGS,
                Default::default(),
            )
            .await
    }
}
