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

/// [`read_fallback`] for a list setting. Buckconfig has no lists, so the
/// value is one string, split on commas the way buckconfig list reads split
/// it: elements are taken verbatim, surrounding whitespace included.
#[doc(hidden)]
pub fn read_fallback_list<T: FromStr>(
    source: &mut dyn BuckconfigSource,
    section: &str,
    property: &str,
) -> buck2_error::Result<Option<Vec<T>>>
where
    T::Err: Display,
{
    let Some(raw) = source.get(section, property)? else {
        return Ok(None);
    };
    raw.split(',')
        .map(|element| {
            element.parse::<T>().map_err(|error| {
                buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Input,
                    "Invalid element `{element}` in buckconfig `{section}.{property}`: {error}"
                )
            })
        })
        .collect::<buck2_error::Result<Vec<T>>>()
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct One(&'static str);

    impl BuckconfigSource for One {
        fn get(&mut self, _section: &str, _property: &str) -> buck2_error::Result<Option<String>> {
            Ok(Some(self.0.to_owned()))
        }
    }

    #[test]
    fn list_fallback_splits_like_buckconfig() -> buck2_error::Result<()> {
        assert_eq!(
            read_fallback_list::<String>(&mut One("a, b,c"), "s", "p")?,
            Some(vec!["a".to_owned(), " b".to_owned(), "c".to_owned()])
        );
        assert_eq!(
            read_fallback_list::<String>(&mut One(""), "s", "p")?,
            Some(vec![String::new()])
        );
        assert_eq!(
            read_fallback_list::<u32>(&mut NoBuckconfig, "s", "p")?,
            None
        );
        assert!(read_fallback_list::<u32>(&mut One("1,x"), "s", "p").is_err());
        Ok(())
    }
}
