/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::Cursor;

use buck2_common::manifold::Bucket;
use buck2_common::manifold::ManifoldClient;
use buck2_fs::async_fs_util;
use buck2_fs::error::IoResultExt;
use buck2_fs::paths::abs_path::AbsPath;

pub(crate) fn manifold_leads(bucket: &Bucket, filename: String) -> String {
    let full_path = bucket.path(filename.as_str());
    let command = format!("manifold get {full_path}");
    let url = bucket.intern_url(filename.as_str());
    format!("{command}\n{url}")
}

pub(crate) async fn file_to_manifold(
    manifold: &ManifoldClient,
    path: &AbsPath,
    filename: String,
) -> buck2_error::Result<String> {
    let bucket = Bucket::RAGE_DUMPS;
    // can't use async_fs_util
    // the trait to convert from tokio::fs::File is not implemented for Stdio
    let mut file = async_fs_util::open(&path).await.categorize_internal()?;

    manifold
        .read_and_upload(bucket, &filename, Default::default(), &mut file)
        .await?;

    Ok(manifold_leads(&bucket, filename))
}

pub(crate) async fn buf_to_manifold(
    manifold: &ManifoldClient,
    buf: &[u8],
    filename: String,
) -> buck2_error::Result<String> {
    let bucket = Bucket::RAGE_DUMPS;
    let mut cursor = Cursor::new(buf);

    manifold
        .read_and_upload(bucket, &filename, Default::default(), &mut cursor)
        .await?;

    Ok(manifold_leads(&bucket, filename))
}
