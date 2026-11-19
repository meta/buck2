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

use std::fmt;
use std::fmt::Display;

use allocative::Allocative;
use dupe::Dupe;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::codemap::CodeMap;
use crate::codemap::FileSpan;
use crate::codemap::FileSpanRef;
use crate::codemap::NativeCodeMap;
use crate::codemap::Span;
use crate::values::ValueTyped;
use crate::values::any::StarlarkAny;

static EMPTY_NATIVE_CODEMAP: NativeCodeMap = NativeCodeMap::new("", 0, 0);
pagable::static_value!(
    EMPTY_NATIVE_CODEMAP_STATIC: NativeCodeMap = &EMPTY_NATIVE_CODEMAP,
    starlark_syntax::codemap::NativeCodeMapStaticEntry
);
crate::static_starlark_any!(VALUE_EMPTY_CODEMAP: CodeMap = NativeCodeMap::to_codemap(EMPTY_NATIVE_CODEMAP_STATIC));

/// A span in a file whose [`CodeMap`] is allocated in a frozen heap, at the brand of that heap.
#[derive(
    Debug,
    Copy,
    Clone,
    Dupe,
    Eq,
    Allocative,
    ProvidesStaticType,
    starlark_derive::StarlarkPagable
)]
pub(crate) struct HeapFileSpan<'f> {
    file: ValueTyped<'f, StarlarkAny<CodeMap>>,
    #[allocative(skip)]
    #[starlark_pagable(pagable)]
    span: Span,
}

impl<'f> PartialEq for HeapFileSpan<'f> {
    fn eq(&self, other: &Self) -> bool {
        // `CodeMap` compares by identity.
        self.file.as_ref().0 == other.file.as_ref().0 && self.span == other.span
    }
}

impl<'f> Display for HeapFileSpan<'f> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.to_file_span(), f)
    }
}

impl<'f> Default for HeapFileSpan<'f> {
    fn default() -> HeapFileSpan<'f> {
        HeapFileSpan::new_unchecked(VALUE_EMPTY_CODEMAP.at(), Span::default())
    }
}

impl<'f> HeapFileSpan<'f> {
    pub(crate) const fn new_unchecked(
        file: ValueTyped<'f, StarlarkAny<CodeMap>>,
        span: Span,
    ) -> HeapFileSpan<'f> {
        HeapFileSpan { file, span }
    }

    pub(crate) fn new(file: ValueTyped<'f, StarlarkAny<CodeMap>>, span: Span) -> HeapFileSpan<'f> {
        // Spans outside their file have been observed in production, and
        // resolving one degrades to a clamped snippet rather than panicking.
        // Debug builds fail fast here; release builds report through the
        // global soft-error handler so the corrupt pairing stays visible for
        // root-causing instead of degrading silently.
        if span.begin() > span.end() || span.end() > file.full_span().end() {
            debug_assert!(
                false,
                "span {:?} does not lie within `{}` ({} bytes)",
                span,
                file.filename(),
                file.full_span().end().get(),
            );
            crate::eval::soft_error::global_soft_error(
                "corrupt_file_span",
                crate::Error::new_other(std::io::Error::other(format!(
                    "span {:?} does not lie within `{}` ({} bytes); \
                     span resolution will degrade to clamped snippets",
                    span,
                    file.filename(),
                    file.full_span().end().get(),
                ))),
            );
        }
        Self::new_unchecked(file, span)
    }

    pub(crate) fn file(&self) -> ValueTyped<'f, StarlarkAny<CodeMap>> {
        self.file
    }

    pub(crate) fn span(&self) -> Span {
        self.span
    }

    pub(crate) fn end_span(&self) -> HeapFileSpan<'f> {
        HeapFileSpan {
            file: self.file,
            span: self.span.end_span(),
        }
    }

    pub(crate) fn file_span_ref(&self) -> FileSpanRef<'f> {
        FileSpanRef {
            file: &self.file.as_ref().0,
            span: self.span,
        }
    }

    pub(crate) fn to_file_span(&self) -> FileSpan {
        FileSpan {
            file: self.file.as_ref().0.dupe(),
            span: self.span,
        }
    }

    pub(crate) fn merge(&self, other: &HeapFileSpan<'f>) -> HeapFileSpan<'f> {
        if self.file.as_ref().0 == other.file.as_ref().0 {
            HeapFileSpan {
                file: self.file,
                span: self.span.merge(other.span),
            }
        } else {
            // We need to pick something if we merge two spans from different files.
            *self
        }
    }
}
