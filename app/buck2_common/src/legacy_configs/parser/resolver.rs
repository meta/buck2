/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;
use std::sync::LazyLock;

use buck2_hash::BuckFixedSortedMap;
use regex::Regex;

use crate::legacy_configs::configs::LegacyBuckConfigSection;
use crate::legacy_configs::configs::ResolvedValue;
use crate::legacy_configs::configs::parse_config_section_and_key;
use crate::legacy_configs::parser::ConfigError;
use crate::legacy_configs::parser::SectionBuilder;

// Since we can't change other entries in values while we iterate over the configuration, we use
// ResolvedItems to store information about recursive resolutions and the current resolution stack.
struct ResolvedItems(
    // Maintains map of items that are resolved in the process of resolving requested items.
    BTreeMap<String, BTreeMap<String, ResolveState>>,
    // Maintains the resolution stack to provide error messages when a cycle is detected.
    Vec<(String, String)>,
);

enum ResolveState {
    Resolving,
    Done(String),
}

impl ResolvedItems {
    fn start_resolving(&mut self, section: &str, key: &str) -> buck2_error::Result<()> {
        let section_values = match self.0.get_mut(section) {
            Some(v) => v,
            None => {
                self.0.insert(section.to_owned(), BTreeMap::new());
                self.0.get_mut(section).unwrap()
            }
        };

        if section_values
            .insert(key.to_owned(), ResolveState::Resolving)
            .is_some()
        {
            return Err(self.cycle_error(section, key).into());
        }

        self.1.push((section.to_owned(), key.to_owned()));

        Ok(())
    }

    fn finish_resolving(&mut self, section: &str, key: &str, value: String) {
        let entry = self.0.get_mut(section).unwrap().get_mut(key).unwrap();
        *entry = ResolveState::Done(value);
        self.1.pop();
    }

    fn cycle_error(&self, section: &str, key: &str) -> ConfigError {
        let mut iter = self.1.iter();
        for v in &mut iter {
            if v.0 == section && v.1 == key {
                break;
            }
        }

        let mut cycle = vec![(section.to_owned(), key.to_owned())];
        cycle.extend(iter.cloned());
        cycle.push((section.to_owned(), key.to_owned()));

        ConfigError::ReferenceCycle(cycle)
    }

    fn get(&self, section: &str, key: &str) -> Option<&String> {
        self.0
            .get(section)
            .and_then(|e| e.get(key))
            .and_then(|e| match e {
                ResolveState::Resolving => None,
                ResolveState::Done(v) => Some(v),
            })
    }

    fn drain_to(self, value: &mut BTreeMap<String, SectionBuilder>) -> buck2_error::Result<()> {
        assert!(self.1.is_empty(), "All values should have been resolved.");
        for (section, items) in self.0.into_iter() {
            let result_section = value.get_mut(&section).unwrap_or_else(
                || panic!("Shouldn't have a resolved value for something that doesn't appear in the base config"));
            for (key, value) in items.into_iter() {
                match value {
                    ResolveState::Resolving => {
                        unreachable!("All values should have been resolved.");
                    }
                    ResolveState::Done(v) => {
                        result_section.values.get_mut(&key).unwrap().resolved_value =
                            ResolvedValue::Resolved(v);
                    }
                }
            }
        }

        Ok(())
    }
}

pub struct ConfigResolver {
    values: BTreeMap<String, SectionBuilder>,
}

impl ConfigResolver {
    pub fn resolve(
        values: BTreeMap<String, SectionBuilder>,
    ) -> buck2_error::Result<BuckFixedSortedMap<String, LegacyBuckConfigSection>> {
        let mut resolver = Self { values };
        resolver.resolve_all()?;
        Ok(BuckFixedSortedMap::from_iter(
            resolver.values.into_iter().map(|(k, v)| (k, v.finish())),
        ))
    }

    fn resolve_all(&mut self) -> buck2_error::Result<()> {
        // First, identify all the values that need to be resolved and mark all the others as literals.
        let mut to_resolve = Vec::new();
        for (section_name, section) in &mut self.values {
            for (key, value) in &mut section.values {
                // if it's been resolved already, move the resolved value into values.
                if Self::regex().is_match(value.raw_value()) {
                    to_resolve.push((section_name.to_owned(), key.to_owned()));
                } else {
                    value.resolved_value = ResolvedValue::Literal;
                }
            }
        }

        // Now, resolve all the items.
        for (section, key) in to_resolve {
            let mut resolved_items = ResolvedItems(BTreeMap::new(), Vec::new());
            self.resolve_item(&mut resolved_items, &section, &key)?;
            resolved_items.drain_to(&mut self.values)?;
        }
        Ok(())
    }

    fn regex() -> &'static Regex {
        static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$\(config ([^)]*)\)").unwrap());
        &RE
    }

    fn resolve_item<'a>(
        &'a self,
        resolved_items: &'a mut ResolvedItems,
        section: &str,
        key: &str,
    ) -> buck2_error::Result<&'a str> {
        let raw_value = match self.values.get(section).and_then(|e| e.values.get(key)) {
            None => return Ok(""),
            Some(v) => match &v.resolved_value {
                ResolvedValue::Unknown => v.raw_value(),
                ResolvedValue::Literal => {
                    return Ok(v.raw_value());
                }
                ResolvedValue::Resolved(v) => {
                    return Ok(v);
                }
            },
        };

        if resolved_items.get(section, key).is_none() {
            resolved_items.start_resolving(section, key)?;
            let v = self.do_resolve(resolved_items, raw_value)?;
            resolved_items.finish_resolving(section, key, v);
        }

        Ok(resolved_items.get(section, key).unwrap())
    }

    fn do_resolve(
        &self,
        resolved_items: &mut ResolvedItems,
        raw_value: &str,
    ) -> buck2_error::Result<String> {
        let mut resolved = String::new();
        let mut last = 0;

        let re = Self::regex();

        // TODO(cjhopman): Should add support for escaping the call, I guess.
        for capture in re.captures_iter(raw_value) {
            let m = capture.get_match();

            resolved.push_str(&raw_value[last..m.start()]);
            last = m.end();

            let captures = re.captures(m.as_str()).unwrap();

            let config_key = captures.get(1).unwrap().as_str();

            let config_section_and_key = parse_config_section_and_key(config_key, None)?;

            resolved.push_str(self.resolve_item(
                resolved_items,
                &config_section_and_key.section,
                &config_section_and_key.key,
            )?);
        }

        resolved.push_str(&raw_value[last..]);
        Ok(resolved)
    }
}
