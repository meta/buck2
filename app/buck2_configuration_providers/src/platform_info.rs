/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Debug;

use allocative::Allocative;
use buck2_build_api_derive::internal_provider;
use buck2_core::configuration::data::ConfigurationData;
use buck2_core::configuration::data::ExecPlatformMarking;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::starlark_module;
use starlark::values::Freeze;
use starlark::values::Heap;
use starlark::values::StarlarkPagable;
use starlark::values::StringValue;
use starlark::values::Trace;
use starlark::values::ValueTyped;

use crate::configuration_info::ConfigurationInfo;

#[internal_provider(platform_info_creator)]
#[derive(
    Clone,
    Debug,
    Trace,
    Freeze,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[repr(C)]
pub struct PlatformInfo<'v> {
    label: StringValue<'v>,
    configuration: ValueTyped<'v, ConfigurationInfo<'v>>,
}

impl<'v> PlatformInfo<'v> {
    /// Convert to an internal configuration with the inherited execution marker.
    pub fn to_configuration(
        &self,
        exec_marking: ExecPlatformMarking,
    ) -> buck2_error::Result<ConfigurationData> {
        let label = self.label.as_str().to_owned();
        let data = self.configuration.to_configuration_data()?;
        ConfigurationData::from_platform(label, data, exec_marking)
    }

    pub fn from_configuration(
        cfg: &ConfigurationData,
        heap: Heap<'v>,
    ) -> buck2_error::Result<PlatformInfo<'v>> {
        Ok(PlatformInfo {
            label: heap.alloc_str(cfg.label()?),
            configuration: heap.alloc_typed(ConfigurationInfo::from_configuration_data(
                cfg.data()?,
                heap,
            )),
        })
    }
}

#[starlark_module]
fn platform_info_creator(globals: &mut GlobalsBuilder) {
    #[starlark(as_type = PlatformInfo<'static>)]
    fn PlatformInfo<'v>(
        #[starlark(require = named)] label: StringValue<'v>,
        #[starlark(require = named)] configuration: ValueTyped<'v, ConfigurationInfo<'v>>,
    ) -> starlark::Result<PlatformInfo<'v>> {
        Ok(PlatformInfo {
            label,
            configuration,
        })
    }
}
