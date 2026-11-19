/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;
use std::io::Write;
use std::sync::Arc;

use buck2_build_api::actions::artifact::get_artifact_fs::GetArtifactFs;
use buck2_build_api::audit_dep_files::AUDIT_DEP_FILES;
use buck2_build_api::materialize::invocation_re_use_case;
use buck2_core::category::Category;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_directory::directory::directory::Directory;
use buck2_directory::directory::directory_iterator::DirectoryIterator;
use buck2_error::BuckErrorOptionContext;
use buck2_error::buck2_error;
use buck2_execute::digest_config::HasDigestConfig;
use buck2_execute::materialize::materializer::HasMaterializer;
use buck2_file_watcher::dep_files::HasDepFileCache;
use dice::DiceTransaction;

use crate::actions::impls::run::RunActionKey;
use crate::actions::impls::run::dep_files::StoredFingerprints;
use crate::actions::impls::run::dep_files::get_dep_files;
use crate::actions::impls::run::dep_files::read_dep_files;

pub(crate) fn init_audit_dep_files() {
    AUDIT_DEP_FILES.init(|ctx, label, category, identifier, stdout| {
        Box::pin(audit_dep_files(ctx, label, category, identifier, stdout))
    });
}

async fn audit_dep_files(
    ctx: &DiceTransaction,
    label: ConfiguredTargetLabel,
    category: Category,
    identifier: Option<String>,
    stdout: &mut (dyn Write + Send),
) -> buck2_error::Result<()> {
    let key = RunActionKey::new(
        BaseDeferredKey::TargetLabel(label),
        category,
        identifier.map(Arc::from),
    );

    let state = get_dep_files(ctx.per_transaction_data().get_dep_file_cache(), &key)
        .with_internal_error(|| format!("Failed to find dep files for key `{key}`"))?;

    let declared_dep_files = match state.declared_dep_files() {
        Some(declared_dep_files) => declared_dep_files,
        None => {
            return Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "Trying to audit dep files for an action that doesn't declare any dep files!"
            ));
        }
    };

    let artifact_fs = ctx.ctx().get_artifact_fs().await?;
    let result = state.result();
    let re_use_case = invocation_re_use_case(&ctx.ctx());
    let dep_files = read_dep_files(
        state.has_signatures(),
        declared_dep_files,
        result,
        artifact_fs,
        ctx.per_transaction_data().get_materializer(),
        re_use_case,
    )
    .await?
    .internal_error("Dep files have expired")?;

    let fingerprints = state.locked_compute_fingerprints(
        Cow::Owned(dep_files),
        true,
        ctx.global_data().get_digest_config(),
        artifact_fs,
    )?;

    let dirs = match &*fingerprints {
        StoredFingerprints::Digests(..) => {
            // This is bit awkward but this only for testing right now so that's OK
            return Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "Fingerprints were stored as digests! You probably need to use BUCK2_KEEP_DEP_FILE_DIRECTORIES=true"
            ));
        }
        StoredFingerprints::Dirs(dirs) => dirs,
    };

    for path in dirs.untagged.ordered_walk_leaves().paths() {
        writeln!(stdout, "untagged\t{path}")?;
    }

    for (tag, dir) in dirs.tagged.iter() {
        for path in dir.ordered_walk_leaves().paths() {
            writeln!(stdout, "{tag}\t{path}")?;
        }
    }

    Ok(())
}
