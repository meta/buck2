/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::RefCell;
use std::fmt;
use std::sync::Arc;

use allocative::Allocative;
use buck2_common::legacy_configs::configs::LegacyBuckConfig;
use buck2_common::legacy_configs::dice::OpaqueLegacyBuckConfigOnDice;
use buck2_common::legacy_configs::key::BuckconfigKeyRef;
use dice::DiceComputations;
use hashbrown::HashTable;
use starlark::any::ProvidesStaticType;
use starlark::collections::Hashed;
use starlark::eval::Evaluator;
use starlark::values::StringValue;
use starlark::values::Trace;

use crate::interpreter::extra_value::InterpreterExtraValue;

#[derive(Debug, Trace, Allocative)]
struct BuckConfigEntry<'v> {
    section: Hashed<String>,
    key: Hashed<String>,
    value: Option<StringValue<'v>>,
}

/// The `read_config` results of one module by `(section, key)`: a repeated read costs one table
/// lookup and hands out the same string. Kept in the module's extra value because the strings
/// are values of the module.
#[derive(Default, Debug, ProvidesStaticType, Trace, Allocative)]
pub(crate) struct BuckConfigsCache<'v> {
    /// Hash map by `(section, key)` pair, so we do one table lookup per request.
    /// So we hash the `key` even if the section does not exist,
    /// but this is practically not an issue, because keys usually come with cached hash.
    current_cell: RefCell<HashTable<BuckConfigEntry<'v>>>,
    root_cell: RefCell<HashTable<BuckConfigEntry<'v>>>,
}

pub trait BuckConfigsViewForStarlark {
    fn read_current_cell_config(
        &mut self,
        key: BuckconfigKeyRef,
    ) -> buck2_error::Result<Option<Arc<str>>>;

    fn read_root_cell_config(
        &mut self,
        key: BuckconfigKeyRef,
    ) -> buck2_error::Result<Option<Arc<str>>>;
}

/// Version of cell buckconfig optimized for fast query from `read_config` Starlark function.
pub(crate) struct LegacyBuckConfigsForStarlark<'a> {
    configs_view: RefCell<&'a mut (dyn BuckConfigsViewForStarlark + 'a)>,
}

impl<'a> fmt::Debug for LegacyBuckConfigsForStarlark<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LegacyBuckConfigForStarlark")
            .finish_non_exhaustive()
    }
}

impl<'a> LegacyBuckConfigsForStarlark<'a> {
    // `section` or `key` 32 bit hashes are well swizzled,
    // but concatenation of them into 64 bit integer is not.
    // This function tries to fix that.
    fn mix_hashes(a: u32, b: u32) -> u64 {
        fn murmur3_mix64(mut x: u64) -> u64 {
            x ^= x >> 33;
            x = x.wrapping_mul(0xff51afd7ed558ccd);
            x ^= x >> 33;
            x = x.wrapping_mul(0xc4ceb9fe1a85ec53);
            x ^= x >> 33;
            x
        }

        murmur3_mix64(((a as u64) << 32) | (b as u64))
    }

    /// Constructor.
    pub(crate) fn new(
        configs_view: &'a mut (dyn BuckConfigsViewForStarlark + 'a),
    ) -> LegacyBuckConfigsForStarlark<'a> {
        LegacyBuckConfigsForStarlark {
            configs_view: RefCell::new(configs_view),
        }
    }

    fn get_impl<'v>(
        &self,
        section: Hashed<&str>,
        key: Hashed<&str>,
        from_root_cell: bool,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> buck2_error::Result<Option<StringValue<'v>>> {
        let hash = Self::mix_hashes(section.hash().get(), key.hash().get());

        let cache = &InterpreterExtraValue::get(eval.module())?.buckconfigs;
        let mut cache = if from_root_cell {
            cache.root_cell.borrow_mut()
        } else {
            cache.current_cell.borrow_mut()
        };
        if let Some(e) = cache.find(hash, |e| {
            e.section.key() == section.key() && e.key.as_str() == *key.key()
        }) {
            return Ok(e.value);
        }

        let key_ref = BuckconfigKeyRef {
            section: section.key(),
            property: key.key(),
        };
        let value = {
            let mut configs_view = self.configs_view.borrow_mut();
            if from_root_cell {
                configs_view.read_root_cell_config(key_ref)?
            } else {
                configs_view.read_current_cell_config(key_ref)?
            }
        }
        // The frozen heap, so that freezing the module does not copy the strings; the module
        // keeps them either way.
        .map(|v| eval.frozen_heap(|fh, edge| edge.rebrand(fh.alloc_str(&v))));

        cache.insert_unique(
            hash,
            BuckConfigEntry {
                section: Hashed::new_unchecked(section.hash(), (*section.key()).to_owned()),
                key: Hashed::new_unchecked(key.hash(), (*key.key()).to_owned()),
                value,
            },
            |e| Self::mix_hashes(e.section.hash().get(), e.key.hash().get()),
        );

        Ok(value)
    }

    /// Find the buckconfig entry.
    pub(crate) fn current_cell_get<'v>(
        &self,
        section: StringValue,
        key: StringValue,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> buck2_error::Result<Option<StringValue<'v>>> {
        // Note here we reuse the hashes of `section` and `key`,
        // if `read_config` is called repeatedly with the same constant arguments:
        // `StringValue` caches the hashes.
        self.get_impl(section.get_hashed_str(), key.get_hashed_str(), false, eval)
    }

    pub(crate) fn root_cell_get<'v>(
        &self,
        section: StringValue,
        key: StringValue,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> buck2_error::Result<Option<StringValue<'v>>> {
        // Note here we reuse the hashes of `section` and `key`,
        // if `read_config` is called repeatedly with the same constant arguments:
        // `StringValue` caches the hashes.
        self.get_impl(section.get_hashed_str(), key.get_hashed_str(), true, eval)
    }
}

pub(crate) struct ConfigsOnDiceViewForStarlark<'a, 'd> {
    ctx: &'a mut DiceComputations<'d>,
    buckconfig: OpaqueLegacyBuckConfigOnDice<'d>,
    root_buckconfig: OpaqueLegacyBuckConfigOnDice<'d>,
}

impl<'a, 'd> ConfigsOnDiceViewForStarlark<'a, 'd> {
    pub(crate) fn new(
        ctx: &'a mut DiceComputations<'d>,
        buckconfig: OpaqueLegacyBuckConfigOnDice<'d>,
        root_buckconfig: OpaqueLegacyBuckConfigOnDice<'d>,
    ) -> Self {
        Self {
            ctx,
            buckconfig,
            root_buckconfig,
        }
    }
}

impl BuckConfigsViewForStarlark for ConfigsOnDiceViewForStarlark<'_, '_> {
    fn read_current_cell_config(
        &mut self,
        key: BuckconfigKeyRef,
    ) -> buck2_error::Result<Option<Arc<str>>> {
        self.buckconfig.lookup(self.ctx, key)
    }

    fn read_root_cell_config(
        &mut self,
        key: BuckconfigKeyRef,
    ) -> buck2_error::Result<Option<Arc<str>>> {
        self.root_buckconfig.lookup(self.ctx, key)
    }
}

pub struct LegacyConfigsViewForStarlark {
    current_cell_config: LegacyBuckConfig,
    root_cell_config: LegacyBuckConfig,
}

impl LegacyConfigsViewForStarlark {
    pub(crate) fn new(buckconfig: LegacyBuckConfig, root_buckconfig: LegacyBuckConfig) -> Self {
        Self {
            current_cell_config: buckconfig,
            root_cell_config: root_buckconfig,
        }
    }
}

impl BuckConfigsViewForStarlark for LegacyConfigsViewForStarlark {
    fn read_current_cell_config(
        &mut self,
        key: BuckconfigKeyRef,
    ) -> buck2_error::Result<Option<Arc<str>>> {
        Ok(self
            .current_cell_config
            .get(key)
            .map(|v| v.to_owned().into()))
    }

    fn read_root_cell_config(
        &mut self,
        key: BuckconfigKeyRef,
    ) -> buck2_error::Result<Option<Arc<str>>> {
        Ok(self.root_cell_config.get(key).map(|v| v.to_owned().into()))
    }
}
