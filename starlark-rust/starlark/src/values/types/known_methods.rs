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

use std::ptr;
use std::sync::LazyLock;

use dupe::Dupe;
use hashbrown::HashMap;

use crate::eval::Arguments;
use crate::eval::Evaluator;
use crate::pagable::StarlarkDeserialize;
use crate::pagable::StarlarkDeserializeContext;
use crate::pagable::StarlarkSerialize;
use crate::pagable::StarlarkSerializeContext;
use crate::values::HeapEdge;
use crate::values::StarlarkType;
use crate::values::StarlarkValue;
use crate::values::Value;
use crate::values::ValueTyped;
use crate::values::dict::value::FrozenDict;
use crate::values::function::NativeMethFn;
use crate::values::function::NativeMethod;
use crate::values::layout::vtable::AValueVTable;
use crate::values::list::value::FrozenList;
use crate::values::set::value::FrozenSet;
use crate::values::string::StarlarkStr;

/// A method and the table that declares it.
///
/// The method is a `static`, so it is immortal and is brought to the brand of the heap in use
/// through [`HeapEdge::immortal`].
#[derive(Clone, Copy)]
pub(crate) struct KnownMethod {
    /// The address of the table of the type the method is defined by. A receiver whose vtable
    /// names the same table has this method under this name.
    type_members: usize,
    /// The method.
    method: ValueTyped<'static, NativeMethod>,
    /// Copied here from `method` to faster invocation (one fewer deref).
    imp: NativeMethFn,
}

// Not derived: the derive does not see through the function pointer's lifetimes.
impl Dupe for KnownMethod {}

// `type_methods` and `imp` are `'static` pointers tied to the in-process
// known-methods table. Round-trip the method name only and re-resolve via
// `get_known_method` on deserialize.
impl StarlarkSerialize for KnownMethod {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        self.method.as_ref().name.to_owned().starlark_serialize(ctx)
    }
}

impl<'fv> StarlarkDeserialize<'fv> for KnownMethod {
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        let name = String::starlark_deserialize(ctx)?;
        get_known_method(&name).ok_or_else(|| {
            crate::Error::new_other(anyhow::anyhow!(
                "KnownMethod `{name}` not in known-methods table on deserialize"
            ))
        })
    }
}

impl KnownMethod {
    /// Whether a value with this vtable has this method under its name: whether the vtable
    /// names the table the method was taken from. That is a comparison of pointers, where a
    /// lookup by name would probe the table.
    #[inline(always)]
    pub(crate) fn is_method_of(&self, vtable: &'static AValueVTable) -> bool {
        match vtable.members() {
            Some(members) => ptr::from_ref(members).addr() == self.type_members,
            None => false,
        }
    }

    #[inline]
    pub(crate) fn to_value<'v>(&self) -> Value<'v> {
        HeapEdge::immortal().rebrand(self.method).to_value()
    }

    #[inline]
    pub(crate) fn invoke_method<'v>(
        &self,
        this: Value<'v>,
        args: &Arguments<'v, '_>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> crate::Result<Value<'v>> {
        (self.imp)(eval, this, args)
    }
}

/// Some of stdlib methods.
struct KnownMethods {
    methods: HashMap<&'static str, KnownMethod>,
}

impl KnownMethods {
    fn build() -> KnownMethods {
        let mut methods = HashMap::new();

        fn add<T: StarlarkValue<'static>>(methods: &mut HashMap<&'static str, KnownMethod>) {
            let table = <T::Type as StarlarkType<'static>>::MEMBERS
                .expect("a type with known methods has a table")
                .table();
            let mut has_at_least_one_method = false;
            for member in table.members() {
                // Take methods, ignore attributes.
                if let Some(method) = ValueTyped::<NativeMethod>::new(member.value()) {
                    // First wins, e. g. `list.clear` is hit, and `dict.clear` is miss.
                    methods.entry(member.name()).or_insert(KnownMethod {
                        type_members: ptr::from_ref(table).addr(),
                        method,
                        imp: method.as_ref().function,
                    });
                    has_at_least_one_method = true;
                }
            }
            // Sanity check.
            assert!(has_at_least_one_method);
        }

        // We don't need to add all the methods, only the most common ones. This is fine.
        add::<FrozenList>(&mut methods);
        add::<FrozenDict>(&mut methods);
        add::<FrozenSet>(&mut methods);
        add::<StarlarkStr>(&mut methods);

        KnownMethods { methods }
    }
}

/// Get stdlib method by name, or `None` if method is not found
/// or method is not very common. Return arbitrary method if more than one
/// method is found (e. g. `list.clear` and `dict.clear`).
pub(crate) fn get_known_method(name: &str) -> Option<KnownMethod> {
    static ANY_METHODS: LazyLock<KnownMethods> = LazyLock::new(KnownMethods::build);
    ANY_METHODS.methods.get(name).copied()
}
