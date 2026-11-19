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
use std::fmt::Debug;

use allocative::Allocative;
use buck2_hash::BuckMutInsertionMap;
use pagable::PagableTagged;
use pagable::pagable_typetag;

use crate::metadata::key::MetadataKey;
use crate::metadata::key::MetadataKeyRef;

#[pagable_typetag]
pub trait SuperPackageValues:
    PagableTagged + Debug + Allocative + Any + Send + Sync + 'static
{
    fn as_any(&self) -> &dyn Any;
    fn is_empty(&self) -> bool;
    /// Only error is internal error: all values must be serializable to JSON,
    /// validated at construction time.
    fn package_values_json(
        &self,
    ) -> buck2_error::Result<BuckMutInsertionMap<MetadataKey, serde_json::Value>>;
    fn contains_key(&self, key: &MetadataKeyRef) -> bool;
    /// Gets package value as a JSON for the given key.
    /// Only error is internal error: all values must be serializable to JSON,
    /// validated at construction time.
    fn get_package_value_json(
        &self,
        key: &MetadataKeyRef,
    ) -> buck2_error::Result<Option<serde_json::Value>>;
}
