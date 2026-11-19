/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::any::Any;
use std::sync::Arc;

use allocative::Allocative;
use buck2_util::late_binding::LateBinding;
use dice::UserComputationData;

/// Live dep-file state owned by one repository.
pub trait DepFileCache: Allocative + Send + Sync + 'static {
    /// Exposes the concrete cache implementation to the action layer.
    fn as_any(&self) -> &dyn Any;

    /// Removes all cached dep-file state.
    fn clear(&self);

    /// Removes remotely produced state while retaining locally produced state.
    fn clear_non_local(&self);
}

/// Factory supplied by the action implementation to preserve crate layering.
pub static CREATE_DEP_FILE_CACHE: LateBinding<fn() -> Arc<dyn DepFileCache>> =
    LateBinding::new("CREATE_DEP_FILE_CACHE");

/// Creates an empty live dep-file cache for one repository.
pub fn create_dep_file_cache() -> Arc<dyn DepFileCache> {
    (CREATE_DEP_FILE_CACHE
        .get()
        .expect("DepFileCache should be set"))()
}

struct DepFileCacheHolder(Arc<dyn DepFileCache>);

/// Installs the current repository's live dep-file cache in command data.
pub trait SetDepFileCache {
    /// Sets the cache used by this command.
    fn set_dep_file_cache(&mut self, cache: Arc<dyn DepFileCache>);
}

/// Reads the current repository's live dep-file cache from command data.
pub trait HasDepFileCache {
    /// Returns the cache used by this command.
    fn get_dep_file_cache(&self) -> &dyn DepFileCache;
}

impl SetDepFileCache for UserComputationData {
    fn set_dep_file_cache(&mut self, cache: Arc<dyn DepFileCache>) {
        self.data.set(DepFileCacheHolder(cache));
    }
}

impl HasDepFileCache for UserComputationData {
    fn get_dep_file_cache(&self) -> &dyn DepFileCache {
        self.data
            .get::<DepFileCacheHolder>()
            .expect("DepFileCache should be set")
            .0
            .as_ref()
    }
}
