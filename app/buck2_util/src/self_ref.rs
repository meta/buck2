/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::convert::Infallible;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::sync::Arc;

use allocative::Allocative;

/// Describing data that can be stored in `SelfRef`.
pub trait RefData: 'static {
    type Data<'a>: 'a;
}

/// Self-referential struct.
#[derive(Allocative)]
#[allocative(bound = "D: RefData")]
pub struct SelfRef<D: RefData> {
    #[allocative(skip)] // TODO(nga): do not skip.
    data: D::Data<'static>,
    // Owner must be placed after `data` to ensure that `data` is dropped before `owner`.
    // Owner must be in `Arc` (or `Rc`) because
    // - pointers stay valid when `SelfRef` is moved.
    // - it cannot be `Box` because it would violate aliasing rules
    #[expect(dead_code)]
    owner: Arc<dyn Allocative + Send + Sync + 'static>,
}

impl<D> Debug for SelfRef<D>
where
    D: RefData,
    for<'a> D::Data<'a>: Debug,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelfRef")
            .field("data", self.data())
            .finish_non_exhaustive()
    }
}

impl<D: RefData> SelfRef<D> {
    pub fn try_new<O: Allocative + Send + Sync + 'static, E>(
        owner: O,
        data: impl for<'a> FnOnce(&'a O) -> Result<D::Data<'a>, E>,
    ) -> Result<Self, E> {
        let owner: Arc<O> = Arc::new(owner);
        let data = data(&owner)?;
        let data = unsafe { std::mem::transmute::<D::Data<'_>, D::Data<'static>>(data) };
        Ok(SelfRef { data, owner })
    }

    pub fn new<O: Allocative + Send + Sync + 'static>(
        owner: O,
        data: impl for<'a> FnOnce(&'a O) -> D::Data<'a>,
    ) -> Self {
        match Self::try_new(owner, |f| Ok::<_, Infallible>(data(f))) {
            Ok(x) => x,
        }
    }

    #[inline]
    pub fn data(&self) -> &D::Data<'_> {
        unsafe { std::mem::transmute::<&D::Data<'static>, &D::Data<'_>>(&self.data) }
    }
}
