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

use buck2_error::BuckErrorContext;
use buck2_error::conversion::from_any_with_tag;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::Filtered;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::prelude::*;
use tracing_subscriber::reload;
use tracing_subscriber::reload::Handle;

use crate::buck2_env;

pub mod log_file;

#[cfg(not(fbcode_build))]
pub mod otel;

#[cfg(fbcode_build)]
mod otel_stub;
#[cfg(fbcode_build)]
pub use otel_stub as otel;

pub trait LogConfigurationReloadHandle: Send + Sync + 'static {
    fn update_log_filter(&self, format: &str) -> buck2_error::Result<()>;
}

impl dyn LogConfigurationReloadHandle {
    pub fn noop() -> Arc<dyn LogConfigurationReloadHandle> {
        Arc::new(NoopLogConfigurationReloadHandle) as _
    }
}

struct NoopLogConfigurationReloadHandle;

impl LogConfigurationReloadHandle for NoopLogConfigurationReloadHandle {
    fn update_log_filter(&self, _filter: &str) -> buck2_error::Result<()> {
        Ok(())
    }
}

impl<L, R> LogConfigurationReloadHandle for Handle<Filtered<L, EnvFilter, R>, R>
where
    L: Send + Sync + 'static,
    R: Send + Sync + 'static,
{
    fn update_log_filter(&self, raw: &str) -> buck2_error::Result<()> {
        let filter = EnvFilter::try_new(raw)
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::LogFilter))
            .buck_error_context("Invalid log filter")?;
        self.modify(|layer| *layer.filter_mut() = filter)
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::LogFilter))
            .buck_error_context("Error updating log filter")?;
        tracing::debug!("Log filter was updated to: `{}`", raw);
        Ok(())
    }
}

pub fn init_tracing_for_writer<W>(
    writer: W,
) -> buck2_error::Result<Arc<dyn LogConfigurationReloadHandle>>
where
    W: for<'writer> MakeWriter<'writer> + Send + Sync + 'static,
{
    // By default, show warnings/errors.
    // If the user specifies BUCK_LOG, we want to honour that.
    const ENV_VAR: &str = "BUCK_LOG";

    let filter = match buck2_env!(ENV_VAR)? {
        Some(v) => EnvFilter::try_new(v)
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::LogFilter))
            .with_buck_error_context(|| format!("Failed to parse ${ENV_VAR} as a filter"))?,
        // daemon_listener is all emitted before the client starts tailing, which is why we log
        // those by default.
        None => EnvFilter::new("warn,[daemon_listener]=info"),
    };

    let layer = tracing_subscriber::fmt::layer()
        .with_writer(writer)
        .with_filter(filter);

    let (layer, handle) = reload::Layer::new(layer);

    tracing_subscriber::registry().with(layer).init();

    Ok(Arc::new(handle) as _)
}
