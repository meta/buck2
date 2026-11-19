/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use async_compression::tokio::bufread::ZstdEncoder;
use buck2_common::manifold::Bucket;
use buck2_common::manifold::ManifoldClient;
use buck2_fs::async_fs_util;
use buck2_fs::error::IoResultExt;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use tokio::io::BufReader;

pub async fn upload_re_logs(
    manifold: &ManifoldClient,
    bucket: Bucket,
    re_logs_dir: &AbsNormPath,
    session_id: &str,
    bucket_path: &str,
) -> buck2_error::Result<()> {
    let logs_path = re_logs_dir
        .join(ForwardRelativePath::new(session_id)?)
        .join(ForwardRelativePath::new("REClientFolly.log")?);
    let file = async_fs_util::open(&logs_path)
        .await
        .categorize_internal()?;
    let mut encoder =
        ZstdEncoder::with_quality(BufReader::new(file), async_compression::Level::Default);

    manifold
        .read_and_upload(bucket, bucket_path, Default::default(), &mut encoder)
        .await?;

    Ok(())
}
