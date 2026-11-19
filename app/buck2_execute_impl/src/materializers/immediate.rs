/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_directory::directory::directory::Directory;
use buck2_directory::directory::directory_iterator::DirectoryIterator;
use buck2_directory::directory::directory_iterator::DirectoryIteratorPathStack;
use buck2_directory::directory::entry::DirectoryEntry;
use buck2_directory::directory::walk::unordered_entry_walk;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::digest::CasDigestToReExt;
use buck2_execute::directory::ActionDirectoryMember;
use buck2_execute::execute::blocking::BlockingExecutor;
use buck2_execute::execute::clean_output_paths::CleanOutputPaths;
use buck2_execute::materialize::materializer::CasDownloadInfo;
use buck2_execute::re::manager::ReConnectionManager;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use remote_execution::NamedDigest;
use remote_execution::NamedDigestWithPermissions;

use crate::materializers::io::MaterializeTreeStructure;

pub async fn cas_download(
    fs: &ProjectRoot,
    io: &dyn BlockingExecutor,
    re: &ReConnectionManager,
    info: &CasDownloadInfo,
    artifacts: Vec<(ProjectRelativePathBuf, ArtifactValue)>,
    cancellations: &CancellationContext,
) -> buck2_error::Result<()> {
    io.execute_io(
        Box::new(CleanOutputPaths {
            paths: artifacts
                .iter()
                .map(|(p, _)| p.to_owned())
                .collect::<Vec<_>>(),
        }),
        cancellations,
    )
    .await?;

    for (path, value) in artifacts.iter() {
        io.execute_io(
            Box::new(MaterializeTreeStructure {
                path: path.to_owned(),
                entry: value.entry().dupe(),
            }),
            cancellations,
        )
        .await?;
    }

    let mut files = Vec::new();
    for (path, value) in artifacts.iter() {
        let mut walk = unordered_entry_walk(value.entry().as_ref().map_dir(Directory::as_ref));
        while let Some((entry_path, entry)) = walk.next() {
            if let DirectoryEntry::Leaf(ActionDirectoryMember::File(m)) = entry {
                files.push(NamedDigestWithPermissions {
                    named_digest: NamedDigest {
                        digest: m.digest.to_re(),
                        name: fs
                            .resolve(path.join(entry_path.get()))
                            .as_maybe_relativized_str()?
                            .to_owned(),
                        ..Default::default()
                    },
                    is_executable: m.is_executable,
                    ..Default::default()
                });
            }
        }
    }

    let re_conn = re.get_re_connection();
    let re_client = re_conn.get_client().with_use_case(info.re_use_case);
    cancellations
        .critical_section(|| re_client.materialize_files(files, info))
        .await?;
    Ok(())
}
