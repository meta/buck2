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

use std::sync::LazyLock;

use dupe::Dupe;

use crate::environment::Globals;
use crate::values::HeapEdge;
use crate::values::OwnedFrozen;
use crate::values::Value;
use crate::values::ValueTyped;
use crate::values::namespace::Namespace;

#[derive(Clone, Dupe, Debug)]
pub(crate) struct BuiltinFn(OwnedFrozen<Value<'static>>);

impl BuiltinFn {
    /// The function, for use with any heap.
    ///
    /// [`Constants`] is a process-wide static, so the heap it holds is immortal and the value can
    /// be brought to any brand, see [`HeapEdge::immortal`]; `&'static self` is the proof.
    pub(crate) fn at<'v>(&'static self) -> Value<'v> {
        HeapEdge::immortal().rebrand(self.0.as_ref().value())
    }

    /// Whether `v` is this function.
    pub(crate) fn is(&self, v: Value) -> bool {
        // Pointer equality works because `#[starlark_module]` allocates each native once, in a
        // static heap that every `GlobalsBuilder` populated from that module references
        // (`GlobalsStatic::populate`), so the `len` of any `Globals` built on the standard
        // library, buck2's included, is this value. A globals set that defines its own `len`
        // has a different function, and the optimizations keyed on these do not apply to it.
        self.0.by_ref(|f| f.ptr_eq(v))
    }
}

/// The builtins the optimizer recognizes and synthesizes calls to.
///
/// Each is a registered static (see the test): the six functions are `#[starlark_module]`
/// natives, allocated once in the static heaps that `globals_static!` registers, and
/// `typing.Callable` is a `static_starlark_value!`; none lives in the heap this type builds. They
/// reach the IR through [`HeapEdge::immortal`] rather than through a heap the module references,
/// so the pagable serializer can write them out only because it finds them in the static
/// registry.
pub(crate) struct Constants {
    pub(crate) fn_len: BuiltinFn,
    pub(crate) fn_type: BuiltinFn,
    pub(crate) fn_list: BuiltinFn,
    pub(crate) fn_dict: BuiltinFn,
    pub(crate) fn_tuple: BuiltinFn,
    pub(crate) fn_isinstance: BuiltinFn,
    // Technically, this is not a function.
    pub(crate) typing_callable: BuiltinFn,
}

impl Constants {
    pub fn get() -> &'static Constants {
        static RES: LazyLock<Constants> = LazyLock::new(|| {
            let g = Globals::extended_internal();
            let builtin = |name| BuiltinFn(g.get_owned(name).unwrap());
            Constants {
                fn_len: builtin("len"),
                fn_type: builtin("type"),
                fn_list: builtin("list"),
                fn_dict: builtin("dict"),
                fn_tuple: builtin("tuple"),
                fn_isinstance: builtin("isinstance"),
                typing_callable: BuiltinFn(
                    g.get_owned("typing")
                        .unwrap()
                        .by_ref_with_reconstructor(|typing, r| {
                            let typing = ValueTyped::<Namespace>::new(*typing).unwrap();
                            r.reconstruct(typing.as_ref().get("Callable").unwrap())
                        }),
                ),
            }
        });
        LazyLock::force(&RES)
    }
}

#[cfg(test)]
mod tests {
    use crate::environment::Globals;
    use crate::eval::compiler::constants::Constants;
    use crate::pagable::get_static_value_id;

    #[test]
    fn test_constants() {
        for globals in [Globals::standard(), Globals::extended_internal()] {
            let len = globals.get_owned("len").unwrap();
            assert!(len.by_ref(|len| Constants::get().fn_len.is(*len)));
        }
    }

    /// See the type doc: the pagable serializer relies on this.
    #[test]
    fn test_constants_are_registered_statics() {
        let c = Constants::get();
        for (name, f) in [
            ("len", &c.fn_len),
            ("type", &c.fn_type),
            ("list", &c.fn_list),
            ("dict", &c.fn_dict),
            ("tuple", &c.fn_tuple),
            ("isinstance", &c.fn_isinstance),
            ("typing.Callable", &c.typing_callable),
        ] {
            assert!(
                f.0.by_ref(|v| get_static_value_id(*v).is_some()),
                "{name} is not a registered static"
            );
        }
    }
}
