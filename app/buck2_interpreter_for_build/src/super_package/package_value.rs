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
use std::io;
use std::sync::Arc;

use allocative::Allocative;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::conversion::from_any_with_tag;
use buck2_hash::BuckFixedInsertionMap;
use buck2_hash::BuckMutInsertionMap;
use buck2_interpreter::file_type::StarlarkFileType;
use buck2_node::metadata::key::MetadataKey;
use buck2_node::metadata::key::MetadataKeyRef;
use buck2_node::metadata::super_package_values::SuperPackageValues;
use dupe::Dupe;
use pagable::Pagable;
use pagable::pagable_typetag;
use starlark::environment::GlobalsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::Freeze;
use starlark::values::FreezeError;
use starlark::values::FreezeErrorContext;
use starlark::values::FreezeResult;
use starlark::values::Freezer;
use starlark::values::Heap;
use starlark::values::OwnedFrozen;
use starlark::values::StarlarkPagable;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::none::NoneType;

use crate::interpreter::build_context::BuildContext;
use crate::interpreter::package_file_extra::PackageFileExtra;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum PackageValueError {
    #[error("key already set in this file: `{0}`")]
    KeyAlreadySetInThisFile(MetadataKey),
    #[error("key set in parent `PACKAGE` file, and overwrite flag is not set: `{0}`")]
    KeySetInParentFile(MetadataKey),
}

#[derive(Debug, Default, Allocative, Pagable)]
pub struct SuperPackageValuesImpl {
    /// In the order the values were written, which is the order `audit package-values` prints
    /// them in.
    values: BuckFixedInsertionMap<MetadataKey, OwnedFrozenStarlarkPackageValue>,
}

impl SuperPackageValuesImpl {
    pub fn get(values: &dyn SuperPackageValues) -> buck2_error::Result<&SuperPackageValuesImpl> {
        values
            .as_any()
            .downcast_ref::<SuperPackageValuesImpl>()
            .internal_error("Expecting SuperPackageValuesImpl")
    }

    pub fn get_package_value(
        &self,
        key: &MetadataKeyRef,
    ) -> Option<&OwnedFrozenStarlarkPackageValue> {
        self.values.get(key)
    }

    pub(crate) fn merge(
        parent: &Arc<dyn SuperPackageValues>,
        this_package: BuckMutInsertionMap<MetadataKey, OwnedFrozenStarlarkPackageValue>,
    ) -> buck2_error::Result<Arc<dyn SuperPackageValues>> {
        if this_package.is_empty() {
            Ok(parent.dupe())
        } else {
            let parent = Self::get(&**parent)?;
            Ok(Arc::new(SuperPackageValuesImpl {
                values: parent
                    .values
                    .iter()
                    .map(|(key, value)| (key.dupe(), value.dupe()))
                    .chain(this_package)
                    .collect(),
            }))
        }
    }
}

#[pagable_typetag]
impl SuperPackageValues for SuperPackageValuesImpl {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    fn package_values_json(
        &self,
    ) -> buck2_error::Result<BuckMutInsertionMap<MetadataKey, serde_json::Value>> {
        let mut values = BuckMutInsertionMap::with_capacity(self.values.len());
        for (key, value) in &self.values {
            values.insert(key.clone(), value.to_json_value()?);
        }
        Ok(values)
    }

    fn contains_key(&self, key: &MetadataKeyRef) -> bool {
        self.values.contains_key(key)
    }

    fn get_package_value_json(
        &self,
        key: &MetadataKeyRef,
    ) -> buck2_error::Result<Option<serde_json::Value>> {
        match self.values.get(key) {
            Some(value) => Ok(Some(value.to_json_value()?)),
            None => Ok(None),
        }
    }
}

/// Value that is known to be serializable to JSON.
#[derive(Trace, Debug, Allocative, Clone, Dupe, Copy, StarlarkPagable)]
pub(crate) struct StarlarkPackageValue<'v>(Value<'v>);

#[derive(Debug, Allocative, Clone, Dupe, Pagable)]
pub struct OwnedFrozenStarlarkPackageValue(OwnedFrozen<Value<'static>>);

impl<'v> StarlarkPackageValue<'v> {
    pub(crate) fn new(value: Value<'v>) -> buck2_error::Result<StarlarkPackageValue<'v>> {
        serde_json::to_writer(io::sink(), &value).buck_error_context(
            "Value must be serializable to JSON to be stored as package value",
        )?;
        Ok(StarlarkPackageValue(value))
    }

    pub(crate) fn value(self) -> Value<'v> {
        self.0
    }
}

impl<'v> Freeze<'v> for StarlarkPackageValue<'v> {
    type Frozen<'fv> = StarlarkPackageValue<'fv>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<StarlarkPackageValue<'fv>> {
        let frozen = self.0.freeze(freezer)?;

        // Error is possible if either:
        // * package value is modified after `write_package_value`
        // * frozen value is not valid JSON even if original value was
        StarlarkPackageValue::new(frozen)
            .map_err(|e| FreezeError::new(e.to_string()))
            .freeze_error_context("Frozen value is not valid JSON")
    }
}

impl OwnedFrozenStarlarkPackageValue {
    pub(crate) fn new(value: OwnedFrozen<Value<'static>>) -> OwnedFrozenStarlarkPackageValue {
        OwnedFrozenStarlarkPackageValue(value)
    }

    pub(crate) fn to_json_value(&self) -> buck2_error::Result<serde_json::Value> {
        self.0
            .by_ref(|v| v.to_json_value())
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0))
            .internal_error("Not valid JSON, should have been validated at construction")
    }

    /// Reads the value without copying it to a heap.
    pub(crate) fn with_value<R>(&self, f: impl for<'v> FnOnce(Value<'v>) -> R) -> R {
        self.0.by_ref(|v| f(*v))
    }

    pub fn add_to_heap<'v>(&self, heap: Heap<'v>) -> Value<'v> {
        self.0.as_ref().add_to_heap(heap)
    }
}

pub(crate) fn write_package_value_impl<'v>(
    key: &str,
    value: Value<'v>,
    overwrite: bool,
    eval: &mut Evaluator<'v, '_, '_>,
    fn_name: &str,
) -> starlark::Result<NoneType> {
    let key = MetadataKeyRef::new(key).map_err(buck2_error::Error::from)?;

    let package_ctx = BuildContext::from_context(eval)?
        .additional
        .require_package_file(fn_name)?;

    let package_file_extra = PackageFileExtra::get_or_init(eval)?;

    if package_file_extra.package_values.borrow().contains_key(key) {
        return Err(
            buck2_error::Error::from(PackageValueError::KeyAlreadySetInThisFile(key.to_owned()))
                .into(),
        );
    }

    if !overwrite {
        if package_ctx.parent.package_values().contains_key(key) {
            return Err(
                buck2_error::Error::from(PackageValueError::KeySetInParentFile(key.to_owned()))
                    .into(),
            );
        }
    }

    let value = StarlarkPackageValue::new(value)?;

    package_file_extra
        .package_values
        .borrow_mut()
        .insert(key.to_owned(), value);

    Ok(NoneType)
}

#[starlark_module]
pub(crate) fn register_write_package_value(globals: &mut GlobalsBuilder) {
    /// Set the value to be accessible in the nested `PACKAGE` files.
    ///
    /// If any parent `PACKAGE` value has already set the same `key`,
    /// it will raise an error unless you pass `overwrite = True`,
    /// in which case it will replace the parent value.
    fn write_package_value<'v>(
        #[starlark(require = pos)] key: &str,
        #[starlark(require = pos)] value: Value<'v>,
        #[starlark(require = named, default = false)] overwrite: bool,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<NoneType> {
        write_package_value_impl(key, value, overwrite, eval, "write_package_value")
    }
}

pub(crate) fn read_parent_package_value_impl<'v>(
    key: &str,
    eval: &mut Evaluator<'v, '_, '_>,
    fn_name: &str,
) -> starlark::Result<Value<'v>> {
    let key = MetadataKeyRef::new(key).map_err(buck2_error::Error::from)?;

    let package_ctx = BuildContext::from_context(eval)?
        .additional
        .require_package_file(fn_name)?;

    match SuperPackageValuesImpl::get(&**package_ctx.parent.package_values())?
        .values
        .get(key)
    {
        Some(value) => Ok(value.add_to_heap(eval.heap())),
        None => Ok(Value::new_none()),
    }
}

#[starlark_module]
pub(crate) fn register_read_package_value(globals: &mut GlobalsBuilder) {
    /// Read value specified in the `PACKAGE` file.
    ///
    /// Returns `None` if value is not set.
    fn read_package_value<'v>(
        #[starlark(require = pos)] key: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<Value<'v>> {
        let key = MetadataKeyRef::new(key).map_err(buck2_error::Error::from)?;

        let build_ctx = BuildContext::from_context(eval)?;
        let build_ctx = build_ctx
            .additional
            .require_build("read_package_value")
            .map_err(|err| {
                let file_type = build_ctx.additional.file_type();

                if file_type == StarlarkFileType::Package {
                    err.context(format!(
                        "In a {file_type:?} context, consider using `read_parent_package_value`"
                    ))
                } else {
                    err
                }
            })?;

        match SuperPackageValuesImpl::get(&**build_ctx.super_package.package_values())?
            .values
            .get(key)
        {
            Some(value) => Ok(value.add_to_heap(eval.heap())),
            None => Ok(Value::new_none()),
        }
    }

    /// Read a package value defined in a parent `PACKAGE` file.
    ///
    /// This function can only be called in a Package context.
    ///
    /// Returns `None` if value is not set.
    fn read_parent_package_value<'v>(
        #[starlark(require = pos)] key: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<Value<'v>> {
        read_parent_package_value_impl(key, eval, "read_parent_package_value")
    }
}
