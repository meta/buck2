/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_hash::BuckMutSet;
use starlark::environment::GlobalsBuilder;
use starlark::starlark_module;
use starlark::values::Heap;
use starlark::values::Value;
use starlark::values::list::AllocList;

#[starlark_module]
pub(crate) fn register_dedupe(builder: &mut GlobalsBuilder) {
    /// Remove duplicates in a list. Uses identity of value (pointer),
    /// rather than by equality. In many cases you should use a transitive set instead.
    fn dedupe<'v>(
        #[starlark(require = pos)] val: Value<'v>,
        heap: Heap<'v>,
    ) -> starlark::Result<Value<'v>> {
        let mut seen = BuckMutSet::default();
        let mut res = Vec::new();
        for v in val.iterate(heap)? {
            if seen.insert(v.identity()) {
                res.push(v);
            }
        }
        Ok(heap.alloc(AllocList(res)).to_value())
    }
}

#[cfg(test)]
mod tests {

    use starlark::assert::Assert;

    use crate::interpreter::functions::dedupe::register_dedupe;

    #[test]
    fn test_dedupe() {
        let mut a = Assert::new();
        a.globals_add(register_dedupe);
        a.pass(
            r#"
assert_eq(dedupe([1,2,3]), [1,2,3])
assert_eq(dedupe([1,2,3,2,1]), [1,2,3])
a = [1]
b = [1]
assert_eq(dedupe([a,b,a]), [a,b])
            "#,
        );
    }
}
