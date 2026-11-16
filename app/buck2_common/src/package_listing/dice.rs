/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use async_trait::async_trait;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::package::PackageLabel;
use buck2_events::dispatch::async_record_root_spans;
use buck2_events::span::SpanId;
use buck2_util::time_span::TimeSpan;
use dice::DiceComputations;
use dice::EqualityBehavior;
use dice::Key;
use dice::OkPagableValueSerialize;
use dice::ValueSerialize;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use pagable::Pagable;
use pagable::pagable_typetag;
use smallvec::SmallVec;

use crate::file_ops::dice::DiceFileComputations;
use crate::package_listing::interpreter::InterpreterPackageListingResolver;
use crate::package_listing::listing::PackageListing;
use crate::package_listing::resolver::PackageListingResolver;

#[derive(
    Clone,
    Dupe,
    derive_more::Display,
    Debug,
    Eq,
    Hash,
    PartialEq,
    Allocative,
    Pagable
)]
#[pagable_typetag(dice::DiceKeyDyn)]
pub struct PackageListingKey(pub PackageLabel);

pub struct PackageListingKeyActivationData {
    pub time_span: TimeSpan,
    pub spans: SmallVec<[SpanId; 1]>,
}

#[async_trait]
impl Key for PackageListingKey {
    type Value = buck2_error::Result<PackageListing>;
    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        let now = TimeSpan::start_now();

        let (result, spans) = async_record_root_spans(
            InterpreterPackageListingResolver::new(ctx).resolve(self.0.dupe()),
        )
        .await;

        ctx.store_evaluation_data(PackageListingKeyActivationData {
            time_span: now.end_now(),
            spans,
        })?;

        result
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| match (x, y) {
            (Ok(x), Ok(y)) => x == y,
            _ => false,
        })
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        OkPagableValueSerialize::<Self::Value>::new()
    }
}

/// Whether a directory contains a buildfile. Depends only on the existence of
/// each buildfile candidate, not on the directory listing, so adding or removing
/// other files in the directory does not invalidate it.
#[derive(
    Clone,
    Copy,
    Dupe,
    derive_more::Display,
    Debug,
    Eq,
    Hash,
    PartialEq,
    Allocative,
    Pagable
)]
#[pagable_typetag(dice::DiceKeyDyn)]
struct HasBuildfileKey(CellPath);

#[async_trait]
impl Key for HasBuildfileKey {
    type Value = buck2_error::Result<bool>;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        for candidate in DiceFileComputations::buildfiles(ctx, self.0.cell())
            .await?
            .iter()
        {
            if DiceFileComputations::exists(ctx, self.0.join(candidate)).await? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| match (x, y) {
            (Ok(x), Ok(y)) => x == y,
            _ => false,
        })
    }

    fn validity(x: &Self::Value) -> bool {
        x.is_ok()
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        OkPagableValueSerialize::<Self::Value>::new()
    }
}

/// The package enclosing a directory: the nearest directory at or above it
/// that contains a buildfile, or `None` if there is none up to the cell root.
/// Only changes when a buildfile is added or removed at or above the directory.
#[derive(
    Clone,
    Copy,
    Dupe,
    derive_more::Display,
    Debug,
    Eq,
    Hash,
    PartialEq,
    Allocative,
    Pagable
)]
#[pagable_typetag(dice::DiceKeyDyn)]
struct EnclosingPackageKey(CellPath);

#[async_trait]
impl Key for EnclosingPackageKey {
    type Value = buck2_error::Result<Option<PackageLabel>>;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        if ctx.compute(&HasBuildfileKey(self.0)).await?.dupe()? {
            return Ok(Some(PackageLabel::from_cell_path(self.0)?));
        }
        match self.0.parent() {
            Some(parent) => enclosing_package_of_dir(ctx, parent).await,
            None => Ok(None),
        }
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| match (x, y) {
            (Ok(x), Ok(y)) => x == y,
            _ => false,
        })
    }

    fn validity(x: &Self::Value) -> bool {
        x.is_ok()
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        OkPagableValueSerialize::<Self::Value>::new()
    }
}

pub struct DicePackageListingResolver<'compute, 'dice>(pub &'compute mut DiceComputations<'dice>);

impl DicePackageListingResolver<'_, '_> {
    /// The package of the file at `path`: the nearest directory at or above it
    /// that contains a buildfile. Unlike `get_enclosing_package`, this returns
    /// `None` rather than an error when there is none.
    pub async fn find_enclosing_package(
        &mut self,
        path: CellPath,
    ) -> buck2_error::Result<Option<PackageLabel>> {
        match path.parent() {
            Some(dir) => enclosing_package_of_dir(self.0, dir).await,
            None => Ok(None),
        }
    }
}

/// The package enclosing the directory `dir`, or `None` if there is none.
pub(crate) async fn enclosing_package_of_dir(
    ctx: &mut DiceComputations<'_>,
    dir: CellPath,
) -> buck2_error::Result<Option<PackageLabel>> {
    ctx.compute(&EnclosingPackageKey(dir)).await?.dupe()
}

#[async_trait]
impl PackageListingResolver for DicePackageListingResolver<'_, '_> {
    async fn resolve(&mut self, package: PackageLabel) -> buck2_error::Result<PackageListing> {
        self.0.compute(&PackageListingKey(package)).await?.dupe()
    }

    async fn get_enclosing_package(&mut self, path: CellPath) -> buck2_error::Result<PackageLabel> {
        InterpreterPackageListingResolver::new(self.0)
            .get_enclosing_package(path)
            .await
    }

    async fn get_enclosing_packages(
        &mut self,
        path: CellPath,
        enclosing_violation_path: CellPath,
    ) -> buck2_error::Result<Vec<PackageLabel>> {
        InterpreterPackageListingResolver::new(self.0)
            .get_enclosing_packages(path, enclosing_violation_path)
            .await
    }
}

impl DicePackageListingResolver<'_, '_> {
    pub async fn resolve_package_listing(
        &mut self,
        package: PackageLabel,
    ) -> buck2_error::Result<PackageListing> {
        self.resolve(package).await
    }

    pub async fn resolve_package_listing_untracked(
        &mut self,
        package: PackageLabel,
    ) -> buck2_error::Result<PackageListing> {
        Ok(InterpreterPackageListingResolver::new(self.0)
            .gather_package_listing(package)
            .await?)
    }
}
