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

use std::sync::LazyLock;

use dupe::Dupe;
use hashbrown::HashMap;

use crate::environment::Methods;
use crate::eval::Arguments;
use crate::eval::Evaluator;
use crate::pagable::StarlarkDeserialize;
use crate::pagable::StarlarkDeserializeContext;
use crate::pagable::StarlarkSerialize;
use crate::pagable::StarlarkSerializeContext;
use crate::values::HeapEdge;
use crate::values::StarlarkValue;
use crate::values::Value;
use crate::values::ValueTyped;
use crate::values::dict::value::FrozenDict;
use crate::values::function::NativeMeth;
use crate::values::function::NativeMethod;
use crate::values::list::value::FrozenList;
use crate::values::set::value::FrozenSet;
use crate::values::string::StarlarkStr;

/// Method and a `Methods` container which declares it.
///
/// The method comes from a `&'static Methods`, so it is immortal and is brought to the brand of
/// the heap in use through [`HeapEdge::immortal`].
#[derive(Clone, Copy, Dupe)]
pub(crate) struct KnownMethod {
    /// An object where the method is defined.
    pub(crate) type_methods: &'static Methods,
    /// The method.
    method: ValueTyped<'static, NativeMethod<'static>>,
    /// Copied here from `method` to faster invocation (one fewer deref).
    imp: &'static NativeMeth<'static>,
}

// `type_methods` and `imp` are `'static` pointers tied to the in-process
// known-methods table. Round-trip the method name only and re-resolve via
// `get_known_method` on deserialize.
impl StarlarkSerialize for KnownMethod {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        self.method.as_ref().name.starlark_serialize(ctx)
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
        let imp: &NativeMeth<'v> = HeapEdge::immortal().rebrand(self.imp);
        imp.invoke(eval, this, args)
    }
}

/// Some of stdlib methods.
struct KnownMethods {
    methods: HashMap<&'static str, KnownMethod>,
}

impl KnownMethods {
    fn build() -> KnownMethods {
        let mut methods = HashMap::new();

        fn add_methods(
            methods: &mut HashMap<&'static str, KnownMethod>,
            type_methods: Option<&'static Methods>,
        ) {
            let type_methods = type_methods.unwrap();
            let mut has_at_least_one_method = false;
            for (name, member) in type_methods.members::<'static>() {
                // Take methods, ignore attributes.
                if let Some(method) = ValueTyped::<NativeMethod>::new(member) {
                    // First wins, e. g. `list.clear` is hit, and `dict.clear` is miss.
                    methods.entry(name).or_insert(KnownMethod {
                        type_methods,
                        method,
                        imp: &method.as_ref().function,
                    });
                    has_at_least_one_method = true;
                }
            }
            // Sanity check.
            assert!(has_at_least_one_method);
        }

        // We don't need to add all the methods, only the most common ones. This is fine.
        add_methods(&mut methods, FrozenList::get_methods());
        add_methods(&mut methods, FrozenDict::get_methods());
        add_methods(&mut methods, FrozenSet::get_methods());
        add_methods(&mut methods, StarlarkStr::get_methods());

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
