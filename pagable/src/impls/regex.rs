/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use regex::Regex;

use crate::PagableDeserialize;
use crate::PagableDeserializer;
use crate::PagableSerialize;
use crate::PagableSerializer;

impl PagableSerialize for Regex {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        // Regex::as_str() returns the original pattern string.
        self.as_str().pagable_serialize(serializer)
    }
}

impl<'de> PagableDeserialize<'de> for Regex {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        let pattern = String::pagable_deserialize(deserializer)?;
        Regex::new(&pattern).map_err(|e| crate::Error::msg(format!("invalid regex: {e}")))
    }
}

impl PagableSerialize for fancy_regex::Regex {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        // Regex::as_str() returns the original pattern string.
        self.as_str().pagable_serialize(serializer)
    }
}

impl<'de> PagableDeserialize<'de> for fancy_regex::Regex {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        let pattern = String::pagable_deserialize(deserializer)?;
        fancy_regex::Regex::new(&pattern)
            .map_err(|e| crate::Error::msg(format!("invalid fancy regex: {e}")))
    }
}
