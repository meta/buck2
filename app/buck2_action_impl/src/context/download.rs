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

use buck2_artifact::artifact::output_type::OutputType;
use buck2_build_api::interpreter::rule_defs::artifact::associated::AssociatedArtifacts;
use buck2_build_api::interpreter::rule_defs::artifact::output_artifact_like::OutputArtifactArg;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_declared_artifact::StarlarkDeclaredArtifact;
use buck2_build_api::interpreter::rule_defs::context::AnalysisActions;
use buck2_common::cas_digest::CasDigest;
use buck2_core::execution_types::executor_config::RemoteExecutorUseCase;
use buck2_error::BuckErrorContext;
use buck2_execute::materialize::http::Checksum;
use buck2_hash::buck_mut_insertion_set;
use starlark::environment::MethodsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::ValueTyped;
use starlark::values::none::NoneOr;

use crate::actions::impls::cas_artifact::ArtifactKind;
use crate::actions::impls::cas_artifact::DirectoryKind;
use crate::actions::impls::cas_artifact::UnregisteredCasArtifactAction;
use crate::actions::impls::download_file::UnregisteredDownloadFileAction;

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Tier0)]
enum CasArtifactError {
    #[error("is_tree and is_directory are mutually exclusive")]
    TreeAndDirectory,
    #[error("Out-of-range value `{0}` for expires_after_timestamp")]
    #[buck2(tag = Input)]
    InvalidExpiration(i64),
}

#[starlark_module]
pub(crate) fn analysis_actions_methods_download(methods: &mut MethodsBuilder) {
    /// Downloads a URL to an output (filename as string or output artifact). The file at the URL
    /// must have the given sha1 or the command will fail. The optional parameter is_executable
    /// indicates whether the resulting file should be marked with executable permissions.
    /// (Meta-internal) The optional parameter vpnless_url indicates a url from which this resource
    /// can be downloaded off VPN; this has the same restrictions as `url` above.
    fn download_file<'v>(
        this: &AnalysisActions<'v>,
        #[starlark(require = pos)] output: OutputArtifactArg<'v>,
        #[starlark(require = pos)] url: &str,
        #[starlark(require = named, default = NoneOr::None)] vpnless_url: NoneOr<&str>,
        #[starlark(require = named, default = NoneOr::None)] sha1: NoneOr<&str>,
        #[starlark(require = named, default = NoneOr::None)] sha256: NoneOr<&str>,
        #[starlark(require = named, default = NoneOr::None)] size_bytes: NoneOr<u64>,
        #[starlark(require = named, default = false)] is_executable: bool,
        #[starlark(require = named, default = NoneOr::None)] has_content_based_path: NoneOr<bool>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<ValueTyped<'v, StarlarkDeclaredArtifact<'v>>> {
        let mut this = this.state()?;
        let (declaration, output_artifact) = this.get_or_declare_output(
            eval,
            output,
            OutputType::File,
            has_content_based_path.into_option(),
        )?;

        let checksum = Checksum::new(sha1.into_option(), sha256.into_option())?;

        this.register_action(
            buck_mut_insertion_set![output_artifact],
            UnregisteredDownloadFileAction::new(
                checksum,
                size_bytes.into_option(),
                Arc::from(url),
                vpnless_url.into_option().map(Arc::from),
                is_executable,
            ),
            None,
            None,
        )?;

        Ok(declaration.into_declared_artifact(AssociatedArtifacts::new()))
    }

    /// Downloads a CAS artifact to an output
    ///
    /// * `digest`: must look like `SHA1:SIZE`
    /// * `use_case`: your RE use case
    /// * `expires_after_timestamp`: must be a UNIX timestamp. Your digest's TTL must exceed this
    ///   timestamp. Your build will break once the digest expires, so make sure the expiry is long
    ///   enough (preferably, in years).
    /// * `is_executable`: indicates the resulting file should be marked with executable
    ///   permissions
    /// * `is_tree`: digest must point to a blob of type
    ///   [RE.Tree](https://fburl.com/code/95rqgju0)
    /// * `is_directory`: digest must point to a blob of type
    ///   [RE.Directory](https://fburl.com/code/4eg40nnp)
    fn cas_artifact<'v>(
        this: &AnalysisActions<'v>,
        #[starlark(require = pos)] output: OutputArtifactArg<'v>,
        #[starlark(require = pos)] digest: &str,
        #[starlark(require = pos)] use_case: &str,
        #[starlark(require = named)] expires_after_timestamp: i64,
        #[starlark(require = named, default = false)] is_executable: bool,
        #[starlark(require = named, default = false)] is_tree: bool,
        #[starlark(require = named, default = false)] is_directory: bool,
        #[starlark(require = named, default = NoneOr::Other(true))] has_content_based_path: NoneOr<
            bool,
        >,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<ValueTyped<'v, StarlarkDeclaredArtifact<'v>>> {
        let mut registry = this.state()?;

        let digest = CasDigest::parse_digest(digest, this.digest_config.cas_digest_config())
            .with_buck_error_context(|| format!("Not a valid RE digest: `{}`", digest))?
            .0;

        let use_case = RemoteExecutorUseCase::new(use_case.to_owned());

        let expires_after_timestamp = jiff::Timestamp::from_second(expires_after_timestamp)
            .map_err(|_| {
                buck2_error::Error::from(CasArtifactError::InvalidExpiration(
                    expires_after_timestamp,
                ))
            })?;

        let kind = match (is_tree, is_directory) {
            (true, true) => {
                return Err(buck2_error::Error::from(CasArtifactError::TreeAndDirectory).into());
            }
            (false, true) => ArtifactKind::Directory(DirectoryKind::Directory),
            (true, false) => ArtifactKind::Directory(DirectoryKind::Tree),
            (false, false) => ArtifactKind::File,
        };

        let output_type = match kind {
            ArtifactKind::Directory(_) => OutputType::Directory,
            ArtifactKind::File => OutputType::File,
        };
        let (output_value, output_artifact) = registry.get_or_declare_output(
            eval,
            output,
            output_type,
            has_content_based_path.into_option(),
        )?;

        registry.register_action(
            buck_mut_insertion_set![output_artifact],
            UnregisteredCasArtifactAction {
                digest,
                re_use_case: use_case,
                expires_after: expires_after_timestamp,
                executable: is_executable,
                kind,
            },
            None,
            None,
        )?;

        Ok(output_value.into_declared_artifact(AssociatedArtifacts::new()))
    }
}
