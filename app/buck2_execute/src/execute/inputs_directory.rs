/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_common::file_ops::metadata::FileMetadata;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_directory::directory::entry::DirectoryEntry;
use dupe::Dupe;

use crate::digest_config::DigestConfig;
use crate::directory::ActionDirectoryBuilder;
use crate::directory::ActionDirectoryMember;
use crate::directory::LazyActionDirectoryBuilder;
use crate::execute::request::CommandExecutionInput;

pub fn inputs_directory(
    inputs: &[CommandExecutionInput],
    digest_config: DigestConfig,
    fs: &ArtifactFs,
) -> buck2_error::Result<ActionDirectoryBuilder> {
    let mut builder = LazyActionDirectoryBuilder::empty();
    for input in inputs {
        match input {
            CommandExecutionInput::Artifact(group) => {
                group.add_to_directory(&mut builder, fs)?;
            }
            CommandExecutionInput::ActionMetadata(metadata) => {
                let path = fs
                    .buck_out_path_resolver()
                    .resolve_gen(&metadata.path, Some(&metadata.content_hash))?;
                builder.insert(
                    path.into(),
                    DirectoryEntry::Leaf(ActionDirectoryMember::File(FileMetadata {
                        digest: metadata.digest.dupe(),
                        is_executable: false,
                    })),
                )?;
            }
            CommandExecutionInput::ScratchPath(path) => {
                let path = fs.buck_out_path_resolver().resolve_scratch(path)?;
                // Exhaustively empty is the scratch contract: `prep_scratch_path` wipes the
                // dir before every local run, and RE sandboxes start it fresh.
                builder.insert(
                    path.into(),
                    DirectoryEntry::Dir(digest_config.empty_directory()),
                )?;
            }
            CommandExecutionInput::IncrementalRemoteOutput(path, entry) => match entry {
                DirectoryEntry::Dir(d) => {
                    builder.insert(path.clone().into(), DirectoryEntry::Dir(d.dupe()))?;
                }
                DirectoryEntry::Leaf(m) => {
                    builder.insert(path.clone().into(), DirectoryEntry::Leaf(m.dupe()))?;
                }
            },
        };
    }
    builder.finalize()
}
