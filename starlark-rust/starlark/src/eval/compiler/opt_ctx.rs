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

use crate::environment::FrozenModuleData;
use crate::eval::Evaluator;
use crate::eval::compiler::Compiler;
use crate::eval::compiler::def_inline::local_as_value::LocalAsValue;
use crate::eval::compiler::stmt::OptimizeOnFreezeContext;
use crate::eval::runtime::slots::LocalSlotId;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::HeapEdge;
use crate::values::SealEdge;
use crate::values::Value;
use crate::values::ValueTyped;

/// The context the optimizer runs in: a module's value heap, which it speculates on, and the
/// module's frozen heap, where the IR it produces is allocated, with the edges between the two.
pub(crate) trait OptCtxEval<'v, 'a, 'e, 'fm> {
    fn heap(&self) -> Heap<'v>;
    fn frozen_heap(&self) -> FrozenHeap<'fm>;
    /// The edge from the value heap to the frozen heap, see `ModuleHeaps`.
    fn edge(&self) -> HeapEdge<'v, 'fm>;
    /// The edge back, for frozen values, see [`OptCtx::demote`].
    fn seal_edge(&self) -> SealEdge<'fm, 'v>;
    fn eval(&mut self) -> Option<&mut Evaluator<'v, 'a, 'e>>;
    fn frozen_module(&self) -> Option<&FrozenModuleData<'fm>>;
    /// Storage for [`OptCtx::local_as_values`].
    fn local_as_values(&self) -> &[ValueTyped<'fm, LocalAsValue>];
    fn local_as_values_mut(&mut self) -> &mut Vec<ValueTyped<'fm, LocalAsValue>>;
}

impl<'v, 'a, 'e, 'fv> OptCtxEval<'v, 'a, 'e, 'fv> for OptimizeOnFreezeContext<'v, 'a, 'fv> {
    fn heap(&self) -> Heap<'v> {
        self.heap
    }

    fn frozen_heap(&self) -> FrozenHeap<'fv> {
        self.frozen_heap
    }

    fn edge(&self) -> HeapEdge<'v, 'fv> {
        self.edge
    }

    fn seal_edge(&self) -> SealEdge<'fv, 'v> {
        self.seal_edge
    }

    fn eval(&mut self) -> Option<&mut Evaluator<'v, 'a, 'e>> {
        None
    }

    fn frozen_module(&self) -> Option<&FrozenModuleData<'fv>> {
        Some(self.module)
    }

    fn local_as_values(&self) -> &[ValueTyped<'fv, LocalAsValue>] {
        &self.local_as_values
    }

    fn local_as_values_mut(&mut self) -> &mut Vec<ValueTyped<'fv, LocalAsValue>> {
        &mut self.local_as_values
    }
}

impl<'v, 'a, 'e, 'x, 'fm> OptCtxEval<'v, 'a, 'e, 'fm> for Compiler<'v, 'a, 'e, 'x, 'fm> {
    fn heap(&self) -> Heap<'v> {
        self.eval.heap()
    }

    fn frozen_heap(&self) -> FrozenHeap<'fm> {
        self.fh
    }

    fn edge(&self) -> HeapEdge<'v, 'fm> {
        self.edge
    }

    fn seal_edge(&self) -> SealEdge<'fm, 'v> {
        self.seal_edge
    }

    fn eval(&mut self) -> Option<&mut Evaluator<'v, 'a, 'e>> {
        Some(self.eval)
    }

    fn frozen_module(&self) -> Option<&FrozenModuleData<'fm>> {
        None
    }

    fn local_as_values(&self) -> &[ValueTyped<'fm, LocalAsValue>] {
        &self.local_as_values
    }

    fn local_as_values_mut(&mut self) -> &mut Vec<ValueTyped<'fm, LocalAsValue>> {
        &mut self.local_as_values
    }
}

/// Optimization context.
///
/// We perform optimization
/// * during compilation of AST to IR, and
/// * when freezing the heap.
pub(crate) struct OptCtx<'v: 'a, 'a, 'e: 'a, 'x, 'fm> {
    pub(crate) eval: &'x mut dyn OptCtxEval<'v, 'a, 'e, 'fm>,
    /// Current function parameter slot count. Zero when compiling module.
    pub(crate) param_count: u32,
}

impl<'v, 'a, 'e: 'a, 'x, 'fm> OptCtx<'v, 'a, 'e, 'x, 'fm> {
    pub(crate) fn new(
        eval: &'x mut dyn OptCtxEval<'v, 'a, 'e, 'fm>,
        param_count: u32,
    ) -> OptCtx<'v, 'a, 'e, 'x, 'fm> {
        OptCtx { eval, param_count }
    }

    pub(crate) fn heap(&self) -> Heap<'v> {
        self.eval.heap()
    }

    pub(crate) fn frozen_heap(&self) -> FrozenHeap<'fm> {
        self.eval.frozen_heap()
    }

    /// The edge from the value heap to the frozen heap: how the optimizer brings IR constants to
    /// `'v` to evaluate on them.
    pub(crate) fn edge(&self) -> HeapEdge<'v, 'fm> {
        self.eval.edge()
    }

    pub(crate) fn eval(&mut self) -> Option<&mut Evaluator<'v, 'a, 'e>> {
        self.eval.eval()
    }

    pub(crate) fn frozen_module(&self) -> Option<&FrozenModuleData<'fm>> {
        self.eval.frozen_module()
    }

    /// Bring a frozen value the optimizer observed at `'v` to the brand the IR is allocated at,
    /// or `None` if it is not frozen.
    ///
    /// The optimizer evaluates speculatively at `'v` (calls, attribute reads, operators, module
    /// slots) and folds frozen results into IR stored at `'fm`; the [`SealEdge`] is what makes
    /// that sound.
    pub(crate) fn demote(&self, v: Value<'v>) -> Option<Value<'fm>> {
        self.eval.seal_edge().rebrand(v)
    }

    /// The placeholders for the first `count` local slots, see [`LocalAsValue`]: one allocation
    /// per slot for the whole compilation, grown on demand.
    pub(crate) fn local_as_values(&mut self, count: u32) -> &[ValueTyped<'fm, LocalAsValue>] {
        let frozen_heap = self.frozen_heap();
        let cache = self.eval.local_as_values_mut();
        while cache.len() < count as usize {
            cache.push(frozen_heap.alloc_simple_typed(LocalAsValue {
                local: LocalSlotId(cache.len() as u32),
            }));
        }
        &self.eval.local_as_values()[..count as usize]
    }
}
