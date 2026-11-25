/*
 * Copyright 2018 The Starlark in Rust Authors.
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

//! The methods and attributes of a type as static data.

use starlark_map::StarlarkHashValue;

use crate::__derive_refs::components::NativeCallableComponents;
use crate::pagable::StaticValueRegistered;
use crate::typing::Ty;
use crate::values::AllocStaticSimple;
use crate::values::FrozenValueTyped;
use crate::values::Heap;
use crate::values::Value;
use crate::values::function::NativeAttribute;
use crate::values::function::NativeMethFn;
use crate::values::function::NativeMethod;
use crate::values::types::unbound::UnboundValue;

// SAFETY: A `MethodStatic` or an `AttributeStatic` is declared by `#[starlark_module]`, which
// registers it next to its declaration.
unsafe impl StaticValueRegistered for NativeMethod {}
// SAFETY: As above.
unsafe impl StaticValueRegistered for NativeAttribute {}

/// The function behind a `#[starlark(attribute)]`. The first argument is always `None`.
#[doc(hidden)]
pub type NativeAttributeFn =
    for<'v> fn(Option<Value<'v>>, Value<'v>, Heap<'v>) -> crate::Result<Value<'v>>;

/// A native method as a `static`. `#[starlark_module]` declares one for each method.
#[doc(hidden)]
pub struct MethodStatic(AllocStaticSimple<NativeMethod>);

impl MethodStatic {
    /// `components` says what the method's parameters and result are; its type and its
    /// documentation are computed from it when they are asked for.
    pub const fn new(
        name: &'static str,
        speculative_exec_safe: bool,
        components: fn() -> NativeCallableComponents,
        function: NativeMethFn,
    ) -> MethodStatic {
        MethodStatic(AllocStaticSimple::alloc(NativeMethod::new(
            name,
            speculative_exec_safe,
            components,
            function,
        )))
    }

    /// The method as a value, for the registry of static values.
    pub fn value(&'static self) -> Value<'static> {
        self.0.unpack().to_value()
    }
}

/// A native attribute as a `static`. `#[starlark_module]` declares one for each attribute.
#[doc(hidden)]
pub struct AttributeStatic(AllocStaticSimple<NativeAttribute>);

impl AttributeStatic {
    pub const fn new(
        speculative_exec_safe: bool,
        docstring: Option<&'static str>,
        typ: fn() -> Ty,
        get: NativeAttributeFn,
    ) -> AttributeStatic {
        AttributeStatic(AllocStaticSimple::alloc(NativeAttribute::new(
            speculative_exec_safe,
            docstring,
            typ,
            None,
            get,
        )))
    }

    /// The attribute as a value, for the registry of static values.
    pub fn value(&'static self) -> Value<'static> {
        self.0.unpack().to_value()
    }
}

#[derive(Clone, Copy)]
enum MemberKind {
    /// A slot of a table that holds no member.
    Empty,
    Method(&'static MethodStatic),
    Attribute(&'static AttributeStatic),
}

/// A method or an attribute of a type: one slot of a [`MemberTable`].
#[derive(Clone, Copy)]
pub struct Member {
    name: &'static str,
    kind: MemberKind,
    /// The position among the members as they were declared.
    index: u32,
}

impl Member {
    const EMPTY: Member = Member {
        name: "",
        kind: MemberKind::Empty,
        index: 0,
    };

    /// A method.
    #[doc(hidden)]
    pub const fn method(name: &'static str, value: &'static MethodStatic) -> Member {
        Member {
            name,
            kind: MemberKind::Method(value),
            index: 0,
        }
    }

    /// An attribute.
    #[doc(hidden)]
    pub const fn attribute(name: &'static str, value: &'static AttributeStatic) -> Member {
        Member {
            name,
            kind: MemberKind::Attribute(value),
            index: 0,
        }
    }

    /// The name of the member.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The member as the old representation of a table holds it.
    pub(crate) fn unbound(&self) -> UnboundValue<'static> {
        match self.kind {
            MemberKind::Empty => unreachable!("an empty slot is not handed out"),
            MemberKind::Method(value) => UnboundValue::Method(
                FrozenValueTyped::from_typed(value.0.unpack()).expect("a static is frozen"),
            ),
            MemberKind::Attribute(value) => UnboundValue::Attr(
                FrozenValueTyped::from_typed(value.0.unpack()).expect("a static is frozen"),
            ),
        }
    }
}

/// The methods and attributes of a type, as a `static`.
///
/// It is a perfect hash table: the multiplier is chosen, when the table is built at compile
/// time, so that no two of its names share a slot.
#[repr(C)]
pub struct MemberTable<S: ?Sized = [Member]> {
    multiplier: u32,
    /// `32 - log2(number of slots)`.
    shift: u32,
    docstring: Option<&'static str>,
    slots: S,
}

impl<const S: usize> MemberTable<[Member; S]> {
    /// The table of `members`, in `S` slots.
    ///
    /// `S` is a power of two that leaves the table at most half full; `#[starlark_module]`
    /// works it out. More slots make the search for a multiplier shorter, which matters for
    /// a table of dozens of members, since the search runs in the compiler.
    #[doc(hidden)]
    pub const fn new<const N: usize>(
        docstring: Option<&'static str>,
        members: [Member; N],
    ) -> MemberTable<[Member; S]> {
        assert!(S.is_power_of_two() && S >= 2 && S >= 2 * N);
        let shift = 32 - S.trailing_zeros();

        let mut hashes = [0u32; N];
        let mut i = 0;
        while i < N {
            hashes[i] = StarlarkHashValue::new_str_const(members[i].name).get();
            let mut j = 0;
            while j < i {
                assert!(
                    hashes[i] != hashes[j],
                    "two members have the same name, or names with the same hash"
                );
                j += 1;
            }
            i += 1;
        }

        let mut multiplier: u32 = 0x9E37_79B1;
        loop {
            let mut used = [false; S];
            let mut i = 0;
            while i < N {
                let slot = (hashes[i].wrapping_mul(multiplier) >> shift) as usize;
                if used[slot] {
                    break;
                }
                used[slot] = true;
                i += 1;
            }
            if i == N {
                break;
            }
            multiplier = multiplier
                .wrapping_mul(0x85EB_CA6B)
                .wrapping_add(0xC2B2_AE35)
                | 1;
        }

        let mut slots = [Member::EMPTY; S];
        let mut i = 0;
        while i < N {
            let slot = (hashes[i].wrapping_mul(multiplier) >> shift) as usize;
            slots[slot] = members[i];
            slots[slot].index = i as u32;
            i += 1;
        }

        MemberTable {
            multiplier,
            shift,
            docstring,
            slots,
        }
    }
}

impl MemberTable {
    /// The members, in the order they were declared.
    pub fn members(&'static self) -> Vec<&'static Member> {
        let mut members: Vec<&'static Member> = self
            .slots
            .iter()
            .filter(|member| !matches!(member.kind, MemberKind::Empty))
            .collect();
        members.sort_by_key(|member| member.index);
        members
    }

    /// The documentation written on the module the table was declared by.
    pub(crate) fn docstring(&self) -> Option<&'static str> {
        self.docstring
    }
}
