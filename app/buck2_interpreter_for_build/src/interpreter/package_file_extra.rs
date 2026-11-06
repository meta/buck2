/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::OnceCell;
use std::cell::RefCell;
use std::sync::Arc;

use allocative::Allocative;
use buck2_hash::BuckMutInsertionMap;
use buck2_node::cfg_constructor::CfgConstructorImpl;
use buck2_node::metadata::key::MetadataKey;
use buck2_util::late_binding::LateBinding;
use starlark::any::ProvidesStaticType;
use starlark::environment::FrozenModule;
use starlark::eval::Evaluator;
use starlark::values::Freeze;
use starlark::values::FreezeErrorContext;
use starlark::values::FreezeResult;
use starlark::values::Freezer;
use starlark::values::NoSerialize;
use starlark::values::OwnedFrozen;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkRepresentations;
use starlark::values::StarlarkType;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::Tracer;
use starlark::values::Value;
use starlark::values::ValueTyped;
use starlark::values::starlark_value;

use crate::interpreter::extra_value::FrozenInterpreterExtraValue;
use crate::interpreter::extra_value::InterpreterExtraValue;
use crate::interpreter::extra_value::OwnedFrozenInterpreterExtraValue;
use crate::super_package::package_value::OwnedFrozenStarlarkPackageValue;
use crate::super_package::package_value::StarlarkPackageValue;

/// `Module.extra_value` when evaluating `PACKAGE` file.
#[derive(
    Default,
    Debug,
    NoSerialize,
    derive_more::Display,
    ProvidesStaticType,
    Allocative
)]
#[display("{:?}", self)]
pub struct PackageFileExtra<'v> {
    pub cfg_constructor: OnceCell<Value<'v>>,
    pub(crate) package_values: RefCell<BuckMutInsertionMap<MetadataKey, StarlarkPackageValue<'v>>>,
}

unsafe impl<'v> Trace<'v> for PackageFileExtra<'v> {
    fn trace(&mut self, tracer: &Tracer<'v>) {
        let PackageFileExtra {
            cfg_constructor,
            package_values,
        } = self;
        cfg_constructor.trace(tracer);
        for (k, v) in package_values.get_mut().iter_mut() {
            fn assert_static<T: 'static>(_t: &T) {}
            assert_static(k);
            v.trace(tracer);
        }
    }
}

#[derive(
    Debug,
    NoSerialize,
    derive_more::Display,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[display("{:?}", self)]
pub struct FrozenPackageFileExtra<'v> {
    pub(crate) cfg_constructor: Option<Value<'v>>,
    pub(crate) package_values: BuckMutInsertionMap<MetadataKey, StarlarkPackageValue<'v>>,
}

/// Resolve `FrozenPackageFileExtra.cfg_constructor` to a `CfgConstructorImpl`.
pub static MAKE_CFG_CONSTRUCTOR: LateBinding<
    fn(OwnedFrozen<Value<'static>>) -> buck2_error::Result<Arc<dyn CfgConstructorImpl>>,
> = LateBinding::new("MAKE_CFG_CONSTRUCTOR");

// TODO(nga): this does not need to be fully starlark_value,
// but we don't have lighter machinery for that.
/// A `PackageFileExtra` in either of its representations.
#[derive(Debug, Clone, Copy, StarlarkRepresentations)]
pub enum PackageFileExtraView<'v> {
    Unfrozen(ValueTyped<'v, PackageFileExtra<'v>>),
    Frozen(ValueTyped<'v, FrozenPackageFileExtra<'v>>),
}

#[starlark_value(type = "PackageFileExtra")]
impl<'v> StarlarkType<'v> for PackageFileExtraView<'v> {}

#[starlark_value]
impl<'v> StarlarkValue<'v> for PackageFileExtra<'v> {
    type Type = PackageFileExtraView<'v>;
}

#[starlark_value(frozen_vtable)]
impl<'v> StarlarkValue<'v> for FrozenPackageFileExtra<'v> {
    type Type = PackageFileExtraView<'v>;
}

impl<'v> Freeze<'v> for PackageFileExtra<'v> {
    type Frozen<'fv> = FrozenPackageFileExtra<'fv>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        let PackageFileExtra {
            cfg_constructor,
            package_values,
        } = self;
        let cfg_constructor = Freeze::freeze(cfg_constructor, freezer)?;
        let package_values = package_values.into_inner();
        // N.B. collect::<Result<_>> sets the lower bound to zero,
        // which can cause over-allocations in frozen containers.
        let mut frozen_package_values = BuckMutInsertionMap::with_capacity(package_values.len());
        for (k, v) in package_values.into_iter_hashed() {
            let v = Freeze::freeze(v, freezer).freeze_error_context(&format!("freezing `{k}`"))?;
            frozen_package_values.insert_hashed(k, v);
        }
        Ok(FrozenPackageFileExtra {
            cfg_constructor,
            package_values: frozen_package_values,
        })
    }
}

impl<'v> PackageFileExtra<'v> {
    pub fn get_or_init(
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> buck2_error::Result<&'v PackageFileExtra<'v>> {
        Ok(InterpreterExtraValue::get(eval.module())?
            .package_extra
            .get_or_init(Default::default))
    }
}

impl FrozenPackageFileExtra<'_> {
    pub(crate) fn get(
        module: &FrozenModule,
    ) -> buck2_error::Result<Option<OwnedFrozenPackageFileExtra>> {
        Ok(OwnedFrozenPackageFileExtra::new(
            FrozenInterpreterExtraValue::get(module)?,
        ))
    }
}

/// A frozen module's interpreter extra value, known to have a `package_extra`.
pub(crate) struct OwnedFrozenPackageFileExtra(OwnedFrozenInterpreterExtraValue);

impl OwnedFrozenPackageFileExtra {
    fn new(extra: OwnedFrozenInterpreterExtraValue) -> Option<Self> {
        extra
            .by_ref(|v| v.as_ref().value.package_extra.is_some())
            .then(|| OwnedFrozenPackageFileExtra(extra))
    }

    /// The value stored by `set_cfg_constructor()`, if any.
    pub(crate) fn cfg_constructor(&self) -> Option<OwnedFrozen<Value<'static>>> {
        self.0
            .as_ref()
            .maybe_map::<Value<'static>, _>(|v| package_extra(&v.as_ref().value).cfg_constructor)
            .map(|v| v.to_owned())
    }

    /// The package values written by this `PACKAGE` file.
    pub(crate) fn package_values(
        &self,
    ) -> BuckMutInsertionMap<MetadataKey, OwnedFrozenStarlarkPackageValue> {
        self.0.by_ref_with_reconstructor(|extra, reconstructor| {
            let package_values = &package_extra(&extra.as_ref().value).package_values;
            let mut values = BuckMutInsertionMap::with_capacity(package_values.len());
            for (name, value) in package_values {
                values.insert(
                    name.clone(),
                    OwnedFrozenStarlarkPackageValue::new(reconstructor.reconstruct(value.value())),
                );
            }
            values
        })
    }
}

/// Unwraps the invariant that [`OwnedFrozenPackageFileExtra`]'s constructor establishes.
fn package_extra<'v>(extra: &'v FrozenInterpreterExtraValue<'v>) -> &'v FrozenPackageFileExtra<'v> {
    extra
        .package_extra
        .as_ref()
        .expect("checked by `OwnedFrozenPackageFileExtra::new`")
}
