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
use crate::eval::runtime::heap_file_span::HeapFileSpan;
use crate::eval::runtime::inlined_frame::InlinedFrames;
use crate::register_starlark_any_complex;
use crate::values::HeapEdge;

/// Span of the call frame (including inlined call frames), at the brand of the frozen heap that
/// holds the file and the inlined frames.
///
/// The bytecode's call instructions carry their span as a `StarlarkAnyComplex<FrameSpan>`
/// allocation, so that the call stack can hold it by reference.
#[derive(
    Debug,
    Clone,
    Copy,
    Dupe,
    PartialEq,
    Eq,
    Default,
    Allocative,
    ProvidesStaticType,
    starlark_derive::StarlarkPagable
)]
pub(crate) struct FrameSpan<'f> {
    pub(crate) span: HeapFileSpan<'f>,
    /// Parent frames.
    pub(crate) inlined_frames: InlinedFrames<'f>,
}

register_starlark_any_complex!(frozen FrameSpan<'_>);

impl<'f> FrameSpan<'f> {
    pub(crate) const fn new(span: HeapFileSpan<'f>) -> FrameSpan<'f> {
        FrameSpan {
            span,
            inlined_frames: InlinedFrames { frames: None },
        }
    }

    pub(crate) fn end_span(&self) -> FrameSpan<'f> {
        FrameSpan {
            span: self.span.end_span(),
            inlined_frames: self.inlined_frames,
        }
    }

    pub(crate) fn merge(&self, other: &FrameSpan<'f>) -> FrameSpan<'f> {
        // TODO(nga): merge inlined frames.
        FrameSpan {
            span: self.span.merge(&other.span),
            inlined_frames: self.inlined_frames,
        }
    }
}

impl FrameSpan<'static> {
    /// The span, for use with any heap: a static span (see `rust_loc!`) is immortal, see
    /// [`HeapEdge::immortal`].
    pub(crate) fn at<'v>(&'static self) -> &'v FrameSpan<'v> {
        HeapEdge::immortal().rebrand_ref(self)
    }
}

impl<'f> Display for FrameSpan<'f> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO(nga): either display inlined frames or remove this.
        Display::fmt(&self.span, f)
    }
}
