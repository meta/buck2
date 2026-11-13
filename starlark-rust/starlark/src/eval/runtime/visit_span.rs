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

use starlark_syntax::syntax::def::DefParamIndices;
use starlark_syntax::syntax::def::DefRegularParamMode;

use crate::collections::symbol::symbol::Symbol;
use crate::environment::slots::ModuleSlotId;
use crate::eval::compiler::expr::CompareOp;
use crate::eval::compiler::span::IrSpanned;
use crate::eval::runtime::frame_span::FrameSpan;
use crate::eval::runtime::params::spec::ParametersSpecPrototype;
use crate::util::arc_str::ArcStr;
use crate::values::FrozenValueTyped;
use crate::values::StarlarkTypeExpr;
use crate::values::StarlarkValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::ValueTyped;
use crate::values::typing::type_compiled::compiled::TypeCompiled;

/// Visitor for code spans in the IR.
pub(crate) trait VisitSpanMut<'f> {
    fn visit_spans(&mut self, visitor: &mut impl FnMut(&mut FrameSpan<'f>));
}

impl<'f, V: VisitSpanMut<'f>> VisitSpanMut<'f> for IrSpanned<'f, V> {
    fn visit_spans(&mut self, visitor: &mut impl FnMut(&mut FrameSpan<'f>)) {
        visitor(&mut self.span);
        self.node.visit_spans(visitor);
    }
}

impl<'f> VisitSpanMut<'f> for Value<'f> {
    fn visit_spans(&mut self, _visitor: &mut impl FnMut(&mut FrameSpan<'f>)) {}
}

impl<'f> VisitSpanMut<'f> for TypeCompiled<'f> {
    fn visit_spans(&mut self, _visitor: &mut impl FnMut(&mut FrameSpan<'f>)) {}
}

impl<'f> VisitSpanMut<'f> for String {
    fn visit_spans(&mut self, _visitor: &mut impl FnMut(&mut FrameSpan<'f>)) {}
}

impl<'f> VisitSpanMut<'f> for bool {
    fn visit_spans(&mut self, _visitor: &mut impl FnMut(&mut FrameSpan<'f>)) {}
}

impl<'f> VisitSpanMut<'f> for u32 {
    fn visit_spans(&mut self, _visitor: &mut impl FnMut(&mut FrameSpan<'f>)) {}
}

impl<'f> VisitSpanMut<'f> for ModuleSlotId {
    fn visit_spans(&mut self, _visitor: &mut impl FnMut(&mut FrameSpan<'f>)) {}
}

impl<'f> VisitSpanMut<'f> for CompareOp {
    fn visit_spans(&mut self, _visitor: &mut impl FnMut(&mut FrameSpan<'f>)) {}
}

impl<'f, V: VisitSpanMut<'f>> VisitSpanMut<'f> for Box<V> {
    fn visit_spans(&mut self, visitor: &mut impl FnMut(&mut FrameSpan<'f>)) {
        (**self).visit_spans(visitor);
    }
}

impl<'f, T: StarlarkValue<'f>> VisitSpanMut<'f> for ValueTyped<'f, T> {
    fn visit_spans(&mut self, _visitor: &mut impl FnMut(&mut FrameSpan<'f>)) {}
}

impl<'f, T: StarlarkValue<'f>> VisitSpanMut<'f> for FrozenValueTyped<'f, T> {
    fn visit_spans(&mut self, _visitor: &mut impl FnMut(&mut FrameSpan<'f>)) {}
}

impl<'f, T: StarlarkTypeExpr<'f>> VisitSpanMut<'f> for ValueOf<'f, T> {
    fn visit_spans(&mut self, _visitor: &mut impl FnMut(&mut FrameSpan<'f>)) {}
}

impl<'f> VisitSpanMut<'f> for Symbol {
    fn visit_spans(&mut self, _visitor: &mut impl FnMut(&mut FrameSpan<'f>)) {}
}

impl<'f, A: VisitSpanMut<'f>, B: VisitSpanMut<'f>> VisitSpanMut<'f> for (A, B) {
    fn visit_spans(&mut self, visitor: &mut impl FnMut(&mut FrameSpan<'f>)) {
        self.0.visit_spans(visitor);
        self.1.visit_spans(visitor);
    }
}

impl<'f, A: VisitSpanMut<'f>, B: VisitSpanMut<'f>, C: VisitSpanMut<'f>> VisitSpanMut<'f>
    for (A, B, C)
{
    fn visit_spans(&mut self, visitor: &mut impl FnMut(&mut FrameSpan<'f>)) {
        self.0.visit_spans(visitor);
        self.1.visit_spans(visitor);
        self.2.visit_spans(visitor);
    }
}

impl<'f, A: VisitSpanMut<'f>, B: VisitSpanMut<'f>, C: VisitSpanMut<'f>, D: VisitSpanMut<'f>>
    VisitSpanMut<'f> for (A, B, C, D)
{
    fn visit_spans(&mut self, visitor: &mut impl FnMut(&mut FrameSpan<'f>)) {
        self.0.visit_spans(visitor);
        self.1.visit_spans(visitor);
        self.2.visit_spans(visitor);
        self.3.visit_spans(visitor);
    }
}

impl<'f, V: VisitSpanMut<'f>> VisitSpanMut<'f> for Vec<V> {
    fn visit_spans(&mut self, visitor: &mut impl FnMut(&mut FrameSpan<'f>)) {
        for v in self {
            v.visit_spans(visitor);
        }
    }
}

impl<'f, V: VisitSpanMut<'f>> VisitSpanMut<'f> for Option<V> {
    fn visit_spans(&mut self, visitor: &mut impl FnMut(&mut FrameSpan<'f>)) {
        if let Some(v) = self {
            v.visit_spans(visitor);
        }
    }
}

impl<'f> VisitSpanMut<'f> for DefRegularParamMode {
    fn visit_spans(&mut self, _visitor: &mut impl FnMut(&mut FrameSpan<'f>)) {}
}

impl<'f> VisitSpanMut<'f> for DefParamIndices {
    fn visit_spans(&mut self, _visitor: &mut impl FnMut(&mut FrameSpan<'f>)) {}
}

impl<'f> VisitSpanMut<'f> for triomphe::Arc<ParametersSpecPrototype> {
    fn visit_spans(&mut self, _visitor: &mut impl FnMut(&mut FrameSpan<'f>)) {}
}

impl<'f> VisitSpanMut<'f> for ArcStr {
    fn visit_spans(&mut self, _visitor: &mut impl FnMut(&mut FrameSpan<'f>)) {}
}
