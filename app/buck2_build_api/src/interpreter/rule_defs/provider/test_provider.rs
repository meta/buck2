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

use buck2_common::tenting::TentingStatus;
use buck2_core::cells::name::CellName;
use buck2_test_api::data::ConfiguredTarget;
use buck2_test_api::data::ExternalRunnerSpec;
use buck2_test_api::data::ExternalRunnerSpecValue;
use buck2_test_api::protocol::TestExecutor;
use futures::future::BoxFuture;
use futures::future::FutureExt;
use itertools::Itertools;

use crate::interpreter::rule_defs::cmd_args::CommandLineArtifactVisitor;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::ExternalRunnerTestInfo;
use crate::interpreter::rule_defs::provider::builtin::external_runner_test_info::TestCommandMember;
use crate::interpreter::rule_defs::provider::builtin::internal_runner_test_info::InternalRunnerTestInfo;
use crate::interpreter::rule_defs::provider::collection::ProviderCollection;

pub trait TestProvider<'v> {
    fn visit_artifacts(
        &self,
        visitor: &mut dyn CommandLineArtifactVisitor<'v>,
    ) -> buck2_error::Result<()>;

    fn labels(&self) -> buck2_error::Result<Vec<&'v str>>;

    fn dispatch<'exec>(
        &self,
        target: ConfiguredTarget,
        executor: Arc<dyn TestExecutor + 'exec>,
        working_dir_cell: CellName,
        tenting_acl_names: TentingStatus,
    ) -> BoxFuture<'exec, buck2_error::Result<()>>;
}

/// Build an `ExternalRunnerSpec` from the common test provider fields.
/// Used by both `ExternalRunnerTestInfo` and `InternalRunnerTestInfo`
/// to dispatch to the external test executor (TPX).
pub fn build_external_runner_spec<'a>(
    command: impl Iterator<Item = buck2_error::Result<TestCommandMember<'a>>>,
    env_keys: impl Iterator<Item = buck2_error::Result<&'a str>>,
    test_type: &str,
    labels: impl Iterator<Item = buck2_error::Result<&'a str>>,
    contacts: impl Iterator<Item = buck2_error::Result<&'a str>>,
    target: ConfiguredTarget,
    working_dir_cell: CellName,
    tenting_status: &TentingStatus,
) -> buck2_error::Result<ExternalRunnerSpec> {
    let mut handle_index = 0;

    let command = command
        .map(|c| {
            Ok(match c? {
                TestCommandMember::Literal(l) => ExternalRunnerSpecValue::Verbatim(l.to_owned()),
                TestCommandMember::Arglike(_) => {
                    // We assign indices to handles, which Tpx can use to reference them later.
                    // We don't count literals in here since Tpx won't use handles to
                    // communicate those (it would just use a literal instead).
                    let handle = ExternalRunnerSpecValue::ArgHandle(handle_index.into());
                    handle_index += 1;
                    handle
                }
            })
        })
        .collect::<buck2_error::Result<_>>()?;

    let env = env_keys
        .map(|k| {
            let k = k?;
            Ok((
                k.to_owned(),
                ExternalRunnerSpecValue::EnvHandle(k.to_owned().into()),
            ))
        })
        .collect::<buck2_error::Result<_>>()?;
    let package_oncall = target.package_oncall.clone();
    let mut labels: Vec<String> = labels
        .map(|l| Ok(l?.to_owned()))
        .collect::<buck2_error::Result<_>>()?;
    // Single JSON-array label, carried verbatim through TPX -> RR -> WWW. The three
    // wire states let WWW tell "not tented" from "not reported":
    //   Tented -> ["acl",..]    NotTented -> []    Unknown -> no label
    match tenting_status {
        TentingStatus::Tented(acl_names) => {
            if let Ok(json) = serde_json::to_string(acl_names) {
                labels.push(format!("tpx_test_config::tenting_acl_names={}", json));
            }
        }
        TentingStatus::NotTented => {
            labels.push("tpx_test_config::tenting_acl_names=[]".to_owned());
        }
        TentingStatus::Unknown => {}
    }
    let contacts: Vec<String> = contacts
        .map(|l| Ok(l?.to_owned()))
        .collect::<buck2_error::Result<_>>()?;
    let oncall = contacts
        .iter()
        .exactly_one()
        .ok()
        .cloned()
        .or(package_oncall);

    Ok(ExternalRunnerSpec {
        target,
        test_type: test_type.to_owned(),
        command,
        env,
        labels,
        contacts,
        oncall,
        working_dir_cell,
    })
}

impl<'v> TestProvider<'v> for ExternalRunnerTestInfo<'v> {
    fn visit_artifacts(
        &self,
        visitor: &mut dyn CommandLineArtifactVisitor<'v>,
    ) -> buck2_error::Result<()> {
        ExternalRunnerTestInfo::visit_artifacts(self, visitor)
    }

    fn labels(&self) -> buck2_error::Result<Vec<&'v str>> {
        ExternalRunnerTestInfo::labels(self).collect()
    }

    fn dispatch<'exec>(
        &self,
        target: ConfiguredTarget,
        executor: Arc<dyn TestExecutor + 'exec>,
        working_dir_cell: CellName,
        tenting_acl_names: TentingStatus,
    ) -> BoxFuture<'exec, buck2_error::Result<()>> {
        let spec = build_external_runner_spec(
            self.command(),
            self.env().map(|entry| Ok(entry?.0)),
            self.test_type(),
            self.labels(),
            self.contacts(),
            target,
            working_dir_cell,
            &tenting_acl_names,
        );
        async move { executor.external_runner_spec(spec?).await }.boxed()
    }
}

impl<'v> TestProvider<'v> for InternalRunnerTestInfo<'v> {
    fn visit_artifacts(
        &self,
        visitor: &mut dyn CommandLineArtifactVisitor<'v>,
    ) -> buck2_error::Result<()> {
        InternalRunnerTestInfo::visit_artifacts(self, visitor)
    }

    fn labels(&self) -> buck2_error::Result<Vec<&'v str>> {
        InternalRunnerTestInfo::labels(self).collect()
    }

    // NOTE: This dispatch() sends the spec to the external test runner (TPX).
    // In practice, test_target() in command.rs intercepts InternalRunnerTestInfo
    // before dispatch() is called, routing it to the in-process runner instead.
    // This impl exists so InternalRunnerTestInfo satisfies the TestProvider
    // trait for contexts that use visit_artifacts() and labels().
    fn dispatch<'exec>(
        &self,
        target: ConfiguredTarget,
        executor: Arc<dyn TestExecutor + 'exec>,
        working_dir_cell: CellName,
        tenting_acl_names: TentingStatus,
    ) -> BoxFuture<'exec, buck2_error::Result<()>> {
        let spec = build_external_runner_spec(
            self.command(),
            self.env().map(|entry| Ok(entry?.0)),
            self.test_type(),
            self.labels(),
            self.contacts(),
            target,
            working_dir_cell,
            &tenting_acl_names,
        );
        async move { executor.external_runner_spec(spec?).await }.boxed()
    }
}

pub fn test_provider_from_collection<'v>(
    providers: &ProviderCollection<'v>,
) -> buck2_error::Result<Option<&'v dyn TestProvider<'v>>> {
    // Check for InternalRunnerTestInfo first
    if let Some(provider) = providers.builtin_provider::<InternalRunnerTestInfo>()? {
        return Ok(Some(provider.as_ref()));
    }

    if let Some(provider) = providers.builtin_provider::<ExternalRunnerTestInfo>()? {
        return Ok(Some(provider.as_ref()));
    }

    Ok(None)
}
