/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fs::Metadata;
use std::io::Read;
#[cfg(unix)]
use std::time::SystemTime;

use buck2_env::soft_error::capture_soft_error_context;
use buck2_env::soft_error::with_soft_error_context;

use crate::error::IoError;
use crate::fs_util;
use crate::io_counters::IoCounterKey;
use crate::paths::abs_path::AbsPath;

/// Runs blocking work while preserving command-scoped soft-error behavior.
pub fn spawn_blocking<F, R>(func: F) -> tokio::task::JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let context = capture_soft_error_context();
    tokio::task::spawn_blocking(move || with_soft_error_context(context, func))
}

pub async fn open<P: AsRef<AbsPath>>(path: P) -> Result<tokio::fs::File, IoError> {
    let _guard = IoCounterKey::Read.guard();
    let path = path.as_ref();
    tokio::fs::File::open(path.as_maybe_relativized())
        .await
        .map_err(|e| IoError::new_with_path("open", path, e))
}

pub async fn read<P: AsRef<AbsPath>>(path: P, buf: &mut [u8]) -> Result<usize, IoError> {
    let path = path.as_ref().to_owned();
    let mut buffer = buf.to_vec();

    let (n, new_buffer) = spawn_blocking(move || {
        let mut read_guard = fs_util::open_file(&path)?;
        let n = read_guard
            .read(&mut buffer)
            .map_err(|e| IoError::new_with_path("read", &path, e))?;
        Ok::<_, IoError>((n, buffer))
    })
    .await
    .map_err(IoError::from_join)??;

    buf.copy_from_slice(&new_buffer);
    Ok(n)
}

pub async fn write<P: AsRef<AbsPath>>(path: P, content: impl AsRef<[u8]>) -> Result<(), IoError> {
    let path = path.as_ref().to_owned();
    let content = content.as_ref().to_owned();
    spawn_blocking(move || fs_util::write(path, content))
        .await
        .map_err(IoError::from_join)?
}

pub async fn read_to_string<P: AsRef<AbsPath>>(path: P) -> Result<String, IoError> {
    let path = path.as_ref().to_owned();
    spawn_blocking(move || fs_util::read_to_string(path))
        .await
        .map_err(IoError::from_join)?
}

pub async fn read_to_string_if_exists<P: AsRef<AbsPath>>(
    path: P,
) -> buck2_error::Result<Option<String>> {
    let path = path.as_ref().to_owned();
    spawn_blocking(move || fs_util::read_to_string_if_exists(path)).await?
}

pub async fn create_dir_all<P: AsRef<AbsPath>>(dir: P) -> buck2_error::Result<()> {
    let dir = dir.as_ref().to_owned();
    spawn_blocking(move || fs_util::create_dir_all(dir)).await?
}

pub async fn metadata<P: AsRef<AbsPath>>(path: P) -> Result<Metadata, IoError> {
    let path = path.as_ref().to_owned();
    spawn_blocking(move || fs_util::metadata(path))
        .await
        .map_err(IoError::from_join)?
}

#[cfg(unix)]
pub async fn set_modified<P: AsRef<AbsPath>>(path: P, time: SystemTime) -> Result<(), IoError> {
    let path = path.as_ref().to_owned();
    spawn_blocking(move || fs_util::set_modified(path, time))
        .await
        .map_err(IoError::from_join)?
}
