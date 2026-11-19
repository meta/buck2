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
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::mem;

use crate::allocative_trait::Allocative;
use crate::impls::common::CAPACITY_NAME;
use crate::impls::common::DATA_NAME;
use crate::impls::common::UNUSED_CAPACITY_NAME;
use crate::impls::hashbrown_util;
use crate::visitor::Visitor;

impl<K: Allocative, V: Allocative> Allocative for BTreeMap<K, V> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        visitor.visit_generic_map_fields(self);
        visitor.exit();
    }
}

impl<K: Allocative> Allocative for BTreeSet<K> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        visitor.visit_generic_set_fields(self);
        visitor.exit();
    }
}

impl<K: Allocative, V: Allocative, S> Allocative for HashMap<K, V, S> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        if self.capacity() != 0 && mem::size_of::<(K, V)>() != 0 {
            let mut visitor = visitor.enter_unique(DATA_NAME, mem::size_of::<*const ()>());
            visitor.visit_field_with(
                CAPACITY_NAME,
                hashbrown_util::raw_table_alloc_size_for_capacity::<(K, V)>(self.capacity()),
                |visitor| {
                    visitor.visit_map_entries(self);
                    visitor.visit_simple(
                        UNUSED_CAPACITY_NAME,
                        hashbrown_unused_capacity_size::<(K, V)>(self.capacity(), self.len()),
                    );
                },
            );
        }
        visitor.exit();
    }
}

impl<K: Allocative, S> Allocative for HashSet<K, S> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        if self.capacity() != 0 && mem::size_of::<K>() != 0 {
            let mut visitor = visitor.enter_unique(DATA_NAME, mem::size_of::<*const ()>());
            visitor.visit_field_with(
                CAPACITY_NAME,
                hashbrown_util::raw_table_alloc_size_for_capacity::<K>(self.capacity()),
                |visitor| {
                    visitor.visit_set_entries(self);
                    visitor.visit_simple(
                        UNUSED_CAPACITY_NAME,
                        hashbrown_unused_capacity_size::<K>(self.capacity(), self.len()),
                    );
                },
            );
        }
        visitor.exit();
    }
}

fn hashbrown_unused_capacity_size<T>(capacity: usize, len: usize) -> usize {
    hashbrown_util::raw_table_alloc_size_for_capacity::<T>(capacity)
        .saturating_sub(len * mem::size_of::<T>())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::collections::HashSet;

    use crate::FlameGraphBuilder;

    #[test]
    fn test_hash_map_capacity_has_no_negative_size_warnings() {
        let mut map = HashMap::with_capacity(10);
        map.insert("key".to_owned(), "value".to_owned());

        let mut graph = FlameGraphBuilder::default();
        graph.visit_root(&map);
        let output = graph.finish();
        let source = output.flamegraph().write();

        assert_eq!("", output.warnings());
        assert!(
            source.contains(";data;capacity;unused_capacity"),
            "flamegraph source should include hash map capacity accounting: {source}"
        );
    }

    #[test]
    fn test_hash_set_capacity_has_no_negative_size_warnings() {
        let mut set = HashSet::with_capacity(10);
        set.insert("key".to_owned());

        let mut graph = FlameGraphBuilder::default();
        graph.visit_root(&set);
        let output = graph.finish();
        let source = output.flamegraph().write();

        assert_eq!("", output.warnings());
        assert!(
            source.contains(";data;capacity;unused_capacity"),
            "flamegraph source should include hash set capacity accounting: {source}"
        );
    }
}
