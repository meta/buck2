/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use buck2_common::legacy_configs::key::BuckconfigKeyRef;
use buck2_common::legacy_configs::view::LegacyBuckConfigView;
use buck2_core::execution_types::executor_config::RemoteExecutorUseCase;
use dice::UserComputationData;
use dupe::Dupe;

/// The RE use case a buck2 invocation talks to the CAS as on its own behalf, unless
/// overridden.
pub const DEFAULT_RE_USE_CASE_KEY: BuckconfigKeyRef<'static> = BuckconfigKeyRef {
    section: "build",
    property: "default_remote_execution_use_case",
};

/// Replaces every RE use case in the daemon when set: the executors' and the invocation's own.
pub const RE_USE_CASE_OVERRIDE_KEY: BuckconfigKeyRef<'static> = BuckconfigKeyRef {
    section: "buck2_re_client",
    property: "override_use_case",
};

/// How a buck2 invocation talks to the CAS on its own behalf, which is everything except an
/// action's RE request and the uploads that feed it: those run under the action's executor
/// configuration.
#[derive(Clone, Copy, Dupe, Debug, Allocative)]
pub struct InvocationReSettings {
    /// The use case for the invocation's own CAS traffic: materialize requests, probes for content
    /// an action could skip producing, TTL refresh.
    pub use_case: RemoteExecutorUseCase,
    /// Whether there is a CAS to talk to at all. Independent of any executor configuration: a
    /// build that runs nothing remotely still has one.
    pub cas_configured: bool,
    /// Whether an action may ask the CAS whether it already holds content it would otherwise
    /// produce: false without a CAS, and under `--local-only` and `--no-remote-cache`, which forgo
    /// what remote caches hold. Says nothing about materialization: content tracked as the CAS's
    /// is fetched from it either way.
    pub may_probe_cas: bool,
    /// `--no-remote-cache`. Unlike `--local-only`, it also keeps `download_file` from asking the
    /// CAS for its content: a cache that is not up yet must not hold up the build that starts it.
    pub skip_cache_read: bool,
}

/// The invocation's own RE use case as configured: [`RE_USE_CASE_OVERRIDE_KEY`] if set, else
/// [`DEFAULT_RE_USE_CASE_KEY`], else buck2's default.
pub fn invocation_re_use_case(
    mut config: impl LegacyBuckConfigView,
) -> buck2_error::Result<RemoteExecutorUseCase> {
    let override_use_case: Option<RemoteExecutorUseCase> =
        config.parse(RE_USE_CASE_OVERRIDE_KEY)?;
    Ok(match override_use_case {
        Some(use_case) => use_case,
        None => config
            .parse(DEFAULT_RE_USE_CASE_KEY)?
            .unwrap_or_else(RemoteExecutorUseCase::buck2_default),
    })
}

pub trait SetInvocationReSettings {
    fn set_invocation_re_settings(&mut self, settings: InvocationReSettings);
}

pub trait HasInvocationReSettings {
    fn get_invocation_re_settings(&self) -> InvocationReSettings;
}

impl SetInvocationReSettings for UserComputationData {
    fn set_invocation_re_settings(&mut self, settings: InvocationReSettings) {
        self.data.set(settings);
    }
}

impl HasInvocationReSettings for UserComputationData {
    fn get_invocation_re_settings(&self) -> InvocationReSettings {
        *self
            .data
            .get::<InvocationReSettings>()
            .expect("InvocationReSettings should be set")
    }
}
