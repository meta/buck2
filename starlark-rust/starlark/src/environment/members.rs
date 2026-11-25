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

use starlark_map::Hashed;
use starlark_map::StarlarkHashValue;

use crate::__derive_refs::components::NativeCallableComponents;
use crate::collections::symbol::symbol::Symbol;
use crate::collections::symbol::symbol::name_head;
use crate::eval::Arguments;
use crate::eval::Evaluator;
use crate::eval::runtime::frame_span::FrameSpan;
use crate::pagable::StaticValueRegistered;
use crate::typing::Ty;
use crate::values::AllocStaticSimple;
use crate::values::FrozenValueTyped;
use crate::values::Heap;
use crate::values::HeapEdge;
use crate::values::Value;
use crate::values::function::BoundMethod;
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

    /// The method as a value at the brand of the heap in use.
    #[inline(always)]
    fn at<'v>(&'static self) -> Value<'v> {
        HeapEdge::immortal().rebrand(self.value())
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
    Method {
        value: &'static MethodStatic,
        /// The function is here as well as in the value: a call does not read the value.
        function: NativeMethFn,
    },
    Attribute {
        value: &'static AttributeStatic,
    },
}

/// A method or an attribute of a type: one slot of a [`MemberTable`].
///
/// A slot is one cache line, and it holds what a lookup compares and what a call needs, so
/// that finding a member and calling it reads nothing else of the table.
#[derive(Clone, Copy)]
#[repr(C, align(64))]
pub struct Member {
    name: &'static str,
    /// The first sixteen bytes of `name`: with the length, the whole of a name that short.
    head: [u64; 2],
    kind: MemberKind,
    /// The position among the members as they were declared.
    index: u32,
}

impl Member {
    const EMPTY: Member = Member {
        name: "",
        head: [0; 2],
        kind: MemberKind::Empty,
        index: 0,
    };

    /// A method. `function` is the function `value` was declared with.
    #[doc(hidden)]
    pub const fn method(
        name: &'static str,
        value: &'static MethodStatic,
        function: NativeMethFn,
    ) -> Member {
        Member {
            name,
            head: name_head(name),
            kind: MemberKind::Method { value, function },
            index: 0,
        }
    }

    /// An attribute.
    #[doc(hidden)]
    pub const fn attribute(name: &'static str, value: &'static AttributeStatic) -> Member {
        Member {
            name,
            head: name_head(name),
            kind: MemberKind::Attribute { value },
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
            MemberKind::Method { value, .. } => UnboundValue::Method(
                FrozenValueTyped::from_typed(value.0.unpack()).expect("a static is frozen"),
            ),
            MemberKind::Attribute { value } => UnboundValue::Attr(
                FrozenValueTyped::from_typed(value.0.unpack()).expect("a static is frozen"),
            ),
        }
    }

    /// Calls the member as a method of `this`. An attribute is read and its value called.
    #[inline(always)]
    pub(crate) fn invoke_method<'v>(
        &'static self,
        this: Value<'v>,
        span: &'v FrameSpan<'v>,
        args: &Arguments<'v, '_>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> crate::Result<Value<'v>> {
        match self.kind {
            MemberKind::Method { value, function } => {
                eval.with_call_stack(value.at(), Some(span), |eval| function(eval, this, args))
            }
            MemberKind::Attribute { value } => {
                let value = value.0.as_payload().invoke(this, eval.heap())?;
                value.invoke_with_loc(Some(span), args, eval)
            }
            MemberKind::Empty => unreachable!("an empty slot is not handed out"),
        }
    }

    /// What `this.name` is: the method bound to `this`, or the attribute's value.
    #[inline(always)]
    pub(crate) fn bind<'v>(
        &'static self,
        this: Value<'v>,
        heap: Heap<'v>,
    ) -> crate::Result<Value<'v>> {
        match self.kind {
            MemberKind::Method { value, .. } => {
                let method = FrozenValueTyped::new(value.at()).expect("a static method");
                Ok(heap.alloc_complex(BoundMethod::new(this, method)))
            }
            MemberKind::Attribute { value } => value.0.as_payload().invoke(this, heap),
            MemberKind::Empty => unreachable!("an empty slot is not handed out"),
        }
    }
}

/// A number of slots that is enough for [`MemberTable::concat`] of two tables with `first` and
/// `second` slots.
#[doc(hidden)]
pub const fn concat_slots(first: usize, second: usize) -> usize {
    (first + second).next_power_of_two()
}

/// The methods and attributes of a type, as a `static`.
///
/// It is a perfect hash table: the multiplier is chosen, when the table is built at compile
/// time, so that no two of its names share a slot. A lookup therefore reads one slot. A hit
/// is confirmed by the whole name, because a name that is not in the table can have the hash
/// of one that is; a name of up to sixteen bytes is compared as two words held in the slot
/// and in the [`Symbol`], with no pointer followed.
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
        assert!(S >= N);
        let mut all = [Member::EMPTY; S];
        let mut i = 0;
        while i < N {
            all[i] = members[i];
            i += 1;
        }
        Self::of(docstring, all, N)
    }

    /// The table of the members of two tables, the first's before the second's: for a type
    /// whose methods are declared by two modules. As for [`new`](Self::new), `S` is a power of
    /// two that leaves the table at most half full.
    pub const fn concat<const A: usize, const B: usize>(
        first: &MemberTable<[Member; A]>,
        second: &MemberTable<[Member; B]>,
    ) -> MemberTable<[Member; S]> {
        // The documentation is the second module's, or else the first's.
        let docstring = match second.docstring {
            Some(docstring) => Some(docstring),
            None => first.docstring,
        };
        let mut all = [Member::EMPTY; S];
        let mut len = 0;
        // Each table's members in the order they were declared: a slot's `index` says where.
        let mut index = 0;
        while index < A {
            let mut slot = 0;
            while slot < A {
                let member = first.slots[slot];
                if !matches!(member.kind, MemberKind::Empty) && member.index as usize == index {
                    assert!(len < S);
                    all[len] = member;
                    len += 1;
                }
                slot += 1;
            }
            index += 1;
        }
        let mut index = 0;
        while index < B {
            let mut slot = 0;
            while slot < B {
                let member = second.slots[slot];
                if !matches!(member.kind, MemberKind::Empty) && member.index as usize == index {
                    assert!(len < S);
                    all[len] = member;
                    len += 1;
                }
                slot += 1;
            }
            index += 1;
        }
        Self::of(docstring, all, len)
    }

    /// The table of the first `len` of `members`.
    const fn of(
        docstring: Option<&'static str>,
        members: [Member; S],
        len: usize,
    ) -> MemberTable<[Member; S]> {
        assert!(S.is_power_of_two() && S >= 2 && S >= 2 * len);
        let shift = 32 - S.trailing_zeros();

        let mut hashes = [0u32; S];
        let mut i = 0;
        while i < len {
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
            while i < len {
                let slot = (hashes[i].wrapping_mul(multiplier) >> shift) as usize;
                if used[slot] {
                    break;
                }
                used[slot] = true;
                i += 1;
            }
            if i == len {
                break;
            }
            multiplier = multiplier
                .wrapping_mul(0x85EB_CA6B)
                .wrapping_add(0xC2B2_AE35)
                | 1;
        }

        let mut slots = [Member::EMPTY; S];
        let mut i = 0;
        while i < len {
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
    #[inline(always)]
    fn slot(&'static self, hash: StarlarkHashValue) -> &'static Member {
        let slot = (hash.get().wrapping_mul(self.multiplier) >> self.shift) as usize;
        // SAFETY: `shift` leaves `log2(slots.len())` bits.
        unsafe { self.slots.get_unchecked(slot) }
    }

    /// The member called `name`.
    #[inline(always)]
    pub(crate) fn get(&'static self, name: &Symbol) -> Option<&'static Member> {
        let member = self.slot(name.small_hash());
        if member.name.len() == name.len()
            && member.head == name.head()
            && (name.len() <= 16 || member.name == name.as_str())
            && !matches!(member.kind, MemberKind::Empty)
        {
            Some(member)
        } else {
            None
        }
    }

    /// The member called `name`, for a name that is not a [`Symbol`].
    #[inline]
    pub(crate) fn get_str(&'static self, name: Hashed<&str>) -> Option<&'static Member> {
        let member = self.slot(name.hash());
        if member.name == *name.key() && !matches!(member.kind, MemberKind::Empty) {
            Some(member)
        } else {
            None
        }
    }

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
