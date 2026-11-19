/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Mutex;

use crate::allocative_trait::Allocative;

static ROOTS: Mutex<Vec<&'static (dyn Allocative + Sync + 'static)>> = Mutex::new(Vec::new());

/// Register global root which can be later traversed by profiler.
///
/// [`root`](crate::root) macro can be used to register global root.
pub fn register_root(root: &'static (dyn Allocative + Sync + 'static)) {
    ROOTS.lock().unwrap().push(root);
}

pub(crate) fn roots() -> Vec<&'static (dyn Allocative + Sync + 'static)> {
    ROOTS.lock().unwrap().clone()
}

// `#[root]` registers from a global constructor, which does not run under Miri.
#[cfg(all(test, not(miri)))]
mod tests {
    use crate as allocative;
    use crate::Allocative;
    use crate::FlameGraphBuilder;

    #[derive(Allocative)]
    #[expect(dead_code)]
    struct TestGlobalRoot {
        x: u32,
    }

    #[allocative::root]
    static TEST_GLOBAL_ROOT: TestGlobalRoot = TestGlobalRoot { x: 17 };

    #[test]
    fn test_derive() {
        let mut fg = FlameGraphBuilder::default();
        fg.visit_global_roots();
        let fg = fg.finish_and_write_flame_graph();
        assert!(fg.contains("TestGlobalRoot;x;u32 4"), "{}", fg);
    }
}
