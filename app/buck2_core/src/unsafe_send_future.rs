/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::future::Future;
use std::pin::Pin;

use pin_project::pin_project;

/// A wrapper type to make a future which is not normally [`Send`] declare itself as [`Send`].
/// See the specific constructors for the arguments why this might be safe, even though
/// Rust can't infer it as such.
#[pin_project]
pub struct UnsafeSendFuture<F>(#[pin] F);

unsafe impl<F> Send for UnsafeSendFuture<F> {}

impl<F> Future for UnsafeSendFuture<F>
where
    F: Future,
{
    type Output = <F as Future>::Output;

    fn poll(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        self.project().0.poll(cx)
    }
}

impl<F: Future> UnsafeSendFuture<F> {
    /// Create a future that is [`Send`] from a future that isn't [`Send`].
    /// Safe to use when the only reason the future isn't [`Send`] is due to Starlark types
    /// (e.g. `Module`, `Value`) and that the future owns _all_ the Starlark types on the heap
    /// (e.g. all those with the same lifetime). As a first approximation, if the `'v` lifetime
    /// isn't in the futures type, then the future probably owns all the Starlark.
    ///
    /// The reason this pattern is safe is that while a `Value` isn't [`Send`], if you send _all_ the
    /// `Value`s on a given heap, along with the heap, to a different thread, together, that is safe.
    /// It's only not safe when some `Value` end up on one heap, and some on another.
    /// Rust can't express that natively, so we use this function to explain it to Rust.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the future encapsulates all Starlark values it references,
    /// meaning it owns the entire Starlark heap that contains those values. If the future
    /// references Starlark values from multiple heaps, or references values it doesn't own,
    /// this is undefined behavior.
    pub unsafe fn new_encapsulates_starlark(fut: F) -> Self {
        Self(fut)
    }
}
