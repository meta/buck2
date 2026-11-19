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

use std::fmt;
use std::fmt::Display;

use allocative::Allocative;
use display_container::fmt_keyed_container;
use serde::Serialize;
use starlark_derive::Freeze;
use starlark_derive::Trace;
use starlark_derive::starlark_value;
use starlark_map::Hashed;
use starlark_map::small_map::SmallMap;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::docs::DocItem;
use crate::docs::DocModule;
use crate::starlark_complex_value;
use crate::typing::Ty;
use crate::util::arc_str::ArcStr;
use crate::values::Heap;
use crate::values::StarlarkPagable;
use crate::values::StarlarkValue;
use crate::values::StringValue;
use crate::values::Value;
use crate::values::types::namespace::typing::TyNamespace;

#[derive(Clone, Copy, Debug, Trace, Freeze, Allocative, StarlarkPagable)]
#[repr(C)]
pub(crate) struct MaybeDocHiddenValue<'v> {
    pub(crate) value: Value<'v>,
    pub(crate) doc_hidden: bool,
}

/// The return value of `namespace()`
#[derive(
    Clone,
    Debug,
    Trace,
    Freeze,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[repr(C)]
pub struct Namespace<'v> {
    fields: SmallMap<StringValue<'v>, MaybeDocHiddenValue<'v>>,
}

impl<'v> Namespace<'v> {
    pub(crate) fn new(fields: SmallMap<StringValue<'v>, MaybeDocHiddenValue<'v>>) -> Self {
        Self { fields }
    }

    /// Get a member of this namespace.
    pub fn get(&self, key: &str) -> Option<Value<'v>> {
        self.fields.get_hashed(Hashed::new(key)).map(|v| v.value)
    }
}

starlark_complex_value!(pub Namespace);

impl<'v> Display for Namespace<'v> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_keyed_container(
            f,
            "namespace(",
            ")",
            "=",
            self.fields.iter().map(|(k, v)| (k.as_str(), v.value)),
        )
    }
}

#[starlark_value(type = "namespace")]
impl<'v> StarlarkValue<'v> for Namespace<'v> {
    fn collect_repr_cycle(&self, collector: &mut String) {
        collector.push_str("namespace(...)");
    }

    fn get_attr(&self, attribute: &str, heap: Heap<'v>) -> Option<Value<'v>> {
        self.get_attr_hashed(Hashed::new(attribute), heap)
    }

    fn get_attr_hashed(&self, attribute: Hashed<&str>, _heap: Heap<'v>) -> Option<Value<'v>> {
        self.fields.get_hashed(attribute).map(|v| v.value)
    }

    fn dir_attr(&self) -> Vec<String> {
        self.fields.keys().map(|x| x.as_str().to_owned()).collect()
    }

    fn documentation(&self) -> DocItem {
        DocItem::Module(DocModule {
            docs: None,
            members: self
                .fields
                .iter()
                .filter(|(_, v)| !v.doc_hidden)
                .map(|(k, v)| (k.as_str().to_owned(), v.value.documentation()))
                .collect(),
        })
    }

    fn get_type_starlark_repr() -> Ty {
        Ty::custom(TyNamespace {
            fields: Default::default(),
            extra: true,
        })
    }

    fn typechecker_ty(&self) -> Option<Ty> {
        Some(Ty::custom(TyNamespace {
            fields: self
                .fields
                .iter()
                .map(|(name, value)| (ArcStr::from(name.as_str()), Ty::of_value(value.value)))
                .collect(),
            extra: false,
        }))
    }
}

impl<'v> Serialize for Namespace<'v> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_map(self.fields.iter().map(|(k, v)| (k, v.value)))
    }
}

#[cfg(test)]
mod tests {

    use crate::assert;

    #[test]
    fn test_repr() {
        assert::eq("repr(namespace(a=1, b=[]))", "'namespace(a=1, b=[])'");
        assert::eq("str(namespace(a=1, b=[]))", "'namespace(a=1, b=[])'");
    }

    #[test]
    fn test_repr_cycle() {
        assert::eq(
            "l = []; s = namespace(f=l); l.append(s); repr(s)",
            "'namespace(f=[namespace(...)])'",
        );
        assert::eq(
            "l = []; s = namespace(f=l); l.append(s); str(s)",
            "'namespace(f=[namespace(...)])'",
        );
    }

    #[test]
    fn test_to_json_cycle() {
        assert::fail(
            "l = []; s = namespace(f=l); l.append(s); json.encode(s)",
            "Cycle detected when serializing value of type `namespace` to JSON",
        );
    }

    #[test]
    fn test_kwargs() {
        assert::eq(
            "d = {'b': 2}; s = namespace(a=1, **d); str(s)",
            "'namespace(a=1, b=2)'",
        );
    }
}
