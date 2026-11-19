/*
 * Copyright 2019 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Generic interner for starlark strings.

use hashbrown::HashTable;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::collections::Hashed;
use crate::values::StringValue;
use crate::values::Trace;

/// Interner of the strings of one heap.
#[derive(Default, Trace, ProvidesStaticType)]
pub(crate) struct StringValueInterner<'v> {
    map: HashTable<StringValue<'v>>,
}

impl<'v> StringValueInterner<'v> {
    pub(crate) fn intern(
        &mut self,
        s: Hashed<&str>,
        alloc: impl FnOnce() -> StringValue<'v>,
    ) -> StringValue<'v> {
        match self
            .map
            .find(s.hash().promote(), |x| s == x.get_hashed_str())
        {
            Some(string_value) => *string_value,
            None => {
                let string_value = alloc();
                self.map
                    .insert_unique(s.hash().promote(), string_value, |x| x.get_hash().promote());
                string_value
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::collections::Hashed;
    use crate::values::FrozenHeap;
    use crate::values::Heap;
    use crate::values::string::intern::interner::StringValueInterner;

    #[test]
    fn test_intern() {
        FrozenHeap::temp(|heap| {
            let mut intern = StringValueInterner::default();

            let xx1 = intern.intern(Hashed::new("xx"), || heap.alloc("xx"));
            let xx2 = intern.intern(Hashed::new("xx"), || {
                panic!("alloc_str should be only called once")
            });
            assert!(xx1.to_value().ptr_eq(xx2.to_value()));
        });
    }

    #[test]
    fn test_string_value_intern() {
        Heap::temp(|heap1| {
            let mut intern = StringValueInterner::default();

            let xx1 = intern.intern(Hashed::new("xx"), || heap1.alloc("xx"));
            let xx2 = intern.intern(Hashed::new("xx"), || {
                panic!("alloc_str should be only called once")
            });
            assert!(xx1.to_value().ptr_eq(xx2.to_value()));
        });
    }
}
