/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::OnceLock;

use buck2_error::BuckErrorOptionContext;

/// Value (typically a function pointer or a trait pointer) that is initialized at program start.
///
/// # Motivation
///
/// Late binding pattern is used to call code defined in the downstream crates.
///
/// buck2 compilation speed is very important for us, so we try to make smaller crates
/// with as few dependencies between them as possible. So when we change one line of code,
/// we recompile as little as possible.
///
/// # Example
///
/// We have a crate `buck2_node` that defines target graph data types
/// and a function to compute a target node.
///
/// To compute a target node, we need dependency on heavy `starlark` crate.
/// So we use late binding here. Actual implementation lives in downstream crate
/// `buck2_interpreter_for_build`, but code that needs to compute a target node,
/// calls the function defined in `buck2_node` crate without compile time dependency
/// on `buck2_interpreter_for_build` or `starlark` crates.
///
/// The best example of this pattern is probably `buck2_bxl` crate:
/// the only target depending on `buck2_bxl` is the final binary with `buck2.rs` file.
/// So when `buck2_bxl` changes, we recompile only `buck2_bxl` and `buck2.rs`.
/// At the moment of writing this comment, `buck2_bxl` had only one public function,
/// that is `init_late_bindings()`, that initializes all late bindings.
///
/// # Usage
///
/// `LateBinding` is stored in a static variable, and must be initialized explicitly.
/// We tried to use `#[ctor]` crate for initialization, but unfortunately it does not work
/// reliably on macOS. Some details are in [this post](https://fburl.com/ctor).
///
/// Suppose you have a function `fn foo()` initialized with `LateBinding`.
/// The convention is this:
/// * in the interface crate define a static variable
///   `static FOO: LateBinding<fn()> = LateBinding::new("FOO");`
///   ([example](https://fburl.com/code/rvxqbf4f)).
/// * in the implementation crate define an implementation like `fn foo() { ... }`,
///   and next to the implementation, define a function like `fn init_foo() { FOO.init(foo); }`
///   ([example](https://fburl.com/code/0wd4xoql)).
/// * in the root of the implementation crate, define a function `fn init_late_bindings() { ... }`
///   that calls `init_foo()` and other `init_*` functions
///   ([example](https://fburl.com/code/wbj4tt25)).
/// * in the file `app/buck2/bin/buck2.rs` call `init_late_bindings()` of the corresponding crate
///   ([example](https://fburl.com/code/maorfzdy)).
/// * In the test crates that need to call this function, also call `init_late_bindings()`
///   of the corresponding crates
///   ([example](https://fburl.com/code/ynd8ylo1)).
///   Note, to use `#[ctor]` in test crate, it should be placed in `#[test]`
///   to avoid rust compiler and linker erase the initialization code.
///
/// # Safety
///
/// `LateBinding` is safe. All late bindings are initialized in the main function
/// before any threads start. If late binding is not initialized,
/// it will panic with the name of the late binding.
///
/// # Performance
///
/// `LateBinding` is relatively fast: it is extra indirection. However,
/// combined with boxed futures, it results in extra allocations of the boxed futures:
/// we cannot put `fn() -> impl Future` in the `LateBinding` directly, so we have to box it.
///
/// # Alternatives
///
/// There are two alternatives to late binding, that can be used in similar situations.
/// These are similar to trait downcasting, which is not supported natively in the Rust language.
///
/// * `Provider` from `shed/provider` crate
/// * Starlark's `StarlarkValue`/`Value` support similar API to `provider::Provider`
///
/// # Drawbacks
///
/// Late binding (or `std::any::Provider`) is not ergonomic. Code navigation is harder
/// because of extra steps: instead of jumping to the definition of the function,
/// one has to jump to the definition of the `LateBinding` and then "find usages"
/// of that late binding to find where the implementation lives.
pub struct LateBinding<T> {
    /// Name for diagnostic.
    name: &'static str,
    symbol: OnceLock<T>,
}

impl<T> LateBinding<T> {
    pub const fn new(name: &'static str) -> LateBinding<T> {
        LateBinding {
            name,
            symbol: OnceLock::new(),
        }
    }

    pub fn init(&self, symbol: T) {
        if self.symbol.set(symbol).is_err() {
            panic!("{} already set", self.name);
        }
    }

    #[inline]
    pub fn get(&self) -> buck2_error::Result<&T> {
        self.symbol
            .get()
            .with_internal_error(|| format!("{} not set", self.name))
    }
}
