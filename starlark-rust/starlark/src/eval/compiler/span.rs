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

use std::ops::Deref;

use starlark_derive::StarlarkPagable;

use crate as starlark;
use crate::eval::runtime::frame_span::FrameSpan;

/// Similar to `Spanned<T>` but with file span.
///
/// For intermediate representation.
#[derive(Clone, Copy, Debug, StarlarkPagable)]
pub(crate) struct IrSpanned<'f, T> {
    pub(crate) span: FrameSpan<'f>,
    pub(crate) node: T,
}

impl<'f, T> IrSpanned<'f, T> {
    pub fn map<U>(&self, f: impl FnOnce(&T) -> U) -> IrSpanned<'f, U> {
        IrSpanned {
            node: f(&self.node),
            span: self.span,
        }
    }
}

impl<'f, T> Deref for IrSpanned<'f, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.node
    }
}
