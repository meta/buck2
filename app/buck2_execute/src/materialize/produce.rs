/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Content an action makes itself, put on disk the way a local run's output is, under a
//! materializer that tracks disk state (`Materializer::is_disk_state`): the path is asked for
//! holding the content first, and only where it does not is the content made there and
//! reported. A previous build's write is found rather than made again, and a previous daemon's
//! is found and backed again, since its store went with it.

use buck2_common::file_ops::metadata::FileMetadata;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use dupe::Dupe;

use crate::artifact_value::ArtifactValue;
use crate::digest_config::DigestConfig;
use crate::execute::blocking::BlockingExecutor;
use crate::materialize::materializer::DeclareArtifactPayload;
use crate::materialize::materializer::HeldContent;
use crate::materialize::materializer::Materializer;
use crate::materialize::materializer::WriteRequest;
use crate::materialize::write::maybe_locked_write;

/// What became of the content: found at its path already, or made there now.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Produced {
    Found,
    Made,
}

/// Puts `value`'s content at `path`, running `make` only where the path does not hold it
/// already: `hold_or_take`, then `make` under the write lease, inline on the blocking executor,
/// and the report that makes the content the artifact at the path, as a local run's output is
/// reported. `make` writes the whole of `path`, whatever is there.
pub async fn produce_locally(
    materializer: &dyn Materializer,
    blocking_executor: &dyn BlockingExecutor,
    path: ProjectRelativePathBuf,
    value: ArtifactValue,
    make: impl FnOnce() -> buck2_error::Result<()> + Send,
) -> buck2_error::Result<Produced> {
    let lease = match materializer
        .hold_or_take(path.clone(), value.dupe())
        .await?
    {
        HeldContent::Found(_lease) => return Ok(Produced::Found),
        HeldContent::Missing(lease) => lease,
    };
    blocking_executor.execute_io_inline(make).await?;
    materializer
        .declare_existing(
            &lease,
            vec![DeclareArtifactPayload {
                path,
                artifact: value,
            }],
        )
        .await?;
    Ok(Produced::Made)
}

/// `produce_locally` for a write: the value is the content's digest, and making it is the
/// write itself, atomic at the path.
pub async fn write_locally(
    materializer: &dyn Materializer,
    blocking_executor: &dyn BlockingExecutor,
    fs: &ProjectRoot,
    digest_config: DigestConfig,
    request: WriteRequest,
) -> buck2_error::Result<(ArtifactValue, Produced)> {
    let WriteRequest {
        path,
        content,
        is_executable,
        path_kind,
    } = request;
    let value = ArtifactValue::file(FileMetadata {
        digest: TrackedFileDigest::from_content(&content, digest_config.cas_digest_config()),
        is_executable,
    });
    let (fs, write_path) = (fs.dupe(), path.clone());
    let produced = produce_locally(
        materializer,
        blocking_executor,
        path,
        value.dupe(),
        move || maybe_locked_write(&fs, &write_path, &content, is_executable, path_kind),
    )
    .await?;
    Ok((value, produced))
}
