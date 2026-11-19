/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The buckconfig keys settings fall back to while their values still live
//! in buckconfigs.

use std::fmt::Display;
use std::str::FromStr;

/// A buckconfig that a setting declared with `buckconfig_fallback` reads when
/// no settings layer set it. Each process implements it for the config it
/// has, which is how a fallback keeps being read where the buckconfig key was
/// read before: the client's root config, the daemon's, or a DICE-tracked
/// view.
pub trait BuckconfigSource {
    /// The raw value of `section.property`, if set.
    fn get(&mut self, section: &str, property: &str) -> buck2_error::Result<Option<String>>;
}

/// No buckconfig: every fallback reads as unset.
pub struct NoBuckconfig;

impl BuckconfigSource for NoBuckconfig {
    fn get(&mut self, _section: &str, _property: &str) -> buck2_error::Result<Option<String>> {
        Ok(None)
    }
}

/// Reads and parses one fallback key. The accessors `#[buck_settings_section]`
/// generates call this.
#[doc(hidden)]
pub fn read_fallback<T: FromStr>(
    source: &mut dyn BuckconfigSource,
    section: &str,
    property: &str,
) -> buck2_error::Result<Option<T>>
where
    T::Err: Display,
{
    let Some(raw) = source.get(section, property)? else {
        return Ok(None);
    };
    raw.parse::<T>().map(Some).map_err(|error| {
        buck2_error::buck2_error!(
            buck2_error::ErrorTag::Input,
            "Invalid value `{raw}` for buckconfig `{section}.{property}`: {error}"
        )
    })
}
