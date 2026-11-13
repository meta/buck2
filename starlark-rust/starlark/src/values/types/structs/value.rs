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

use std::cmp::Ordering;
use std::fmt;
use std::fmt::Display;
use std::hash::Hash;
use std::hash::Hasher;

use allocative::Allocative;
use display_container::fmt_keyed_container;
use serde::Serialize;
use starlark_derive::Freeze;
use starlark_derive::Trace;
use starlark_derive::starlark_value;
use starlark_map::Hashed;
use starlark_map::StarlarkHasher;
use starlark_map::small_map::SmallMap;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::docs::DocItem;
use crate::docs::DocMember;
use crate::docs::DocProperty;
use crate::starlark_complex_value;
use crate::typing::Ty;
use crate::typing::TyStruct;
use crate::util::arc_str::ArcStr;
use crate::values::Heap;
use crate::values::StarlarkPagable;
use crate::values::StarlarkValue;
use crate::values::StringValue;
use crate::values::Value;
use crate::values::ValueError;
use crate::values::comparison::compare_small_map;
use crate::values::comparison::equals_small_map;
use crate::values::starlark_type_id::StarlarkTypeId;
use crate::values::structs::unordered_hasher::UnorderedHasher;

impl<'v> Struct<'v> {
    /// The result of calling `type()` on a struct.
    pub(crate) const TYPE: &'static str = "struct";

    /// Create a new [`Struct`].
    pub(crate) fn new(fields: SmallMap<StringValue<'v>, Value<'v>>) -> Self {
        Self { fields }
    }

    #[inline]
    pub(crate) fn is_instance(value: Value<'v>) -> bool {
        value.starlark_type_id() == StarlarkTypeId::of::<Struct>()
    }

    /// The fields, in the order they were given to `struct()`.
    pub fn iter<'a>(&'a self) -> impl ExactSizeIterator<Item = (StringValue<'v>, Value<'v>)> + 'a
    where
        'v: 'a,
    {
        self.fields.iter().map(|(name, value)| (*name, *value))
    }

    /// The field named `name`, if there is one.
    pub fn get(&self, name: &str) -> Option<Value<'v>> {
        self.fields.get_hashed(Hashed::new(name)).copied()
    }

    /// The number of fields.
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// Whether the struct has no fields.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    fn self_ty(&self) -> Ty {
        Ty::custom(TyStruct {
            fields: self
                .fields
                .iter()
                .map(|(name, value)| (ArcStr::from(name.as_str()), Ty::of_value(value.to_value())))
                .collect(),
            extra: false,
        })
    }
}

/// The `struct` type: the result of calling `struct()`.
///
/// A struct is built with [`AllocStruct`](crate::values::structs::AllocStruct), as
/// `heap.alloc(AllocStruct([("a", 1), ("b", 2)]))`. A value known to be a struct is a
/// [`ValueOf<Struct>`](crate::values::ValueOf), which derefs to the `Struct`; its fields are read
/// with [`get`](Self::get) and [`iter`](Self::iter).
#[derive(
    Clone,
    Default,
    Debug,
    Trace,
    Freeze,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[repr(C)]
pub struct Struct<'v> {
    fields: SmallMap<StringValue<'v>, Value<'v>>,
}

starlark_complex_value!(pub Struct);

impl<'v> Display for Struct<'v> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_keyed_container(
            f,
            "struct(",
            ")",
            "=",
            self.iter().map(|(k, v)| (k.as_str(), v)),
        )
    }
}

#[starlark_value(type = Struct::TYPE)]
impl<'v> StarlarkValue<'v> for Struct<'v> {
    fn collect_repr_cycle(&self, collector: &mut String) {
        collector.push_str("struct(...)");
    }

    fn equals(&self, other: Value<'v>) -> crate::Result<bool> {
        match Struct::from_value(other) {
            None => Ok(false),
            Some(other) => equals_small_map(&self.fields, &other.fields, |x, y| x.equals(*y)),
        }
    }

    fn compare(&self, other: Value<'v>) -> crate::Result<Ordering> {
        match Struct::from_value(other) {
            None => ValueError::unsupported_with(self, "cmp()", other),
            Some(other) => compare_small_map(
                &self.fields,
                &other.fields,
                |k| k.as_str(),
                |x, y| x.compare(*y),
            ),
        }
    }

    fn get_attr(&self, attribute: &str, heap: Heap<'v>) -> Option<Value<'v>> {
        self.get_attr_hashed(Hashed::new(attribute), heap)
    }

    fn get_attr_hashed(&self, attribute: Hashed<&str>, _heap: Heap<'v>) -> Option<Value<'v>> {
        self.fields.get_hashed(attribute).copied()
    }

    fn write_hash(&self, hasher: &mut StarlarkHasher) -> crate::Result<()> {
        // Must use unordered hash because equality is unordered,
        // and `a = b  =>  hash(a) = hash(b)`.
        let mut unordered_hasher = UnorderedHasher::new();

        for (k, v) in self.fields.iter_hashed() {
            // Should hash key and value together, so two structs
            // `a=1 b=2` and `a=2 b=1` would produce different hashes.
            let mut entry_hasher = StarlarkHasher::new();
            k.hash().hash(&mut entry_hasher);
            v.write_hash(&mut entry_hasher)?;
            unordered_hasher.write_hash(entry_hasher.finish());
        }

        hasher.write_u64(unordered_hasher.finish());

        Ok(())
    }

    fn dir_attr(&self) -> Vec<String> {
        self.fields.keys().map(|x| x.as_str().to_owned()).collect()
    }

    fn documentation(&self) -> DocItem {
        // This treats structs as being value-like, and intentionally generates bad docs in the case
        // of namespace-like usage. See
        // <https://fb.workplace.com/groups/starlark/permalink/1463680027654154/> for some
        // additional discussion
        let typ = self.self_ty();
        DocItem::Member(DocMember::Property(DocProperty { docs: None, typ }))
    }

    fn get_type_starlark_repr() -> Ty {
        Ty::any_struct()
    }

    fn typechecker_ty(&self) -> Option<Ty> {
        Some(self.self_ty())
    }
}

impl<'v> Serialize for Struct<'v> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_map(self.iter())
    }
}

#[cfg(test)]
mod tests {

    use crate::assert;

    #[test]
    fn test_repr() {
        assert::eq("repr(struct(a=1, b=[]))", "'struct(a=1, b=[])'");
        assert::eq("str(struct(a=1, b=[]))", "'struct(a=1, b=[])'");
    }

    #[test]
    fn test_repr_cycle() {
        assert::eq(
            "l = []; s = struct(f=l); l.append(s); repr(s)",
            "'struct(f=[struct(...)])'",
        );
        assert::eq(
            "l = []; s = struct(f=l); l.append(s); str(s)",
            "'struct(f=[struct(...)])'",
        );
    }

    #[test]
    fn test_to_json_cycle() {
        assert::fail(
            "l = []; s = struct(f=l); l.append(s); json.encode(s)",
            "Cycle detected when serializing value of type `struct` to JSON",
        );
    }

    #[test]
    fn test_to_json() {
        assert::all_true(
            r#"
json.encode(struct(key = None)) == '{"key":null}'
json.encode(struct(key = True)) == '{"key":true}'
json.encode(struct(key = False)) == '{"key":false}'
json.encode(struct(key = 42)) == '{"key":42}'
json.encode(struct(key = 'value')) == '{"key":"value"}'
json.encode(struct(key = 'value"')) == '{"key":"value\\\""}'
json.encode(struct(key = 'value\\')) == '{"key":"value\\\\"}'
json.encode(struct(key = 'value/')) == '{"key":"value/"}'
json.encode(struct(key = 'value\u0008')) == '{"key":"value\\b"}'
json.encode(struct(key = 'value\u000C')) == '{"key":"value\\f"}'
json.encode(struct(key = 'value\n')) == '{"key":"value\\n"}'
json.encode(struct(key = 'value\r')) == '{"key":"value\\r"}'
json.encode(struct(key = 'value\t')) == '{"key":"value\\t"}'
json.encode(struct(foo = 42, bar = "some")) == '{"foo":42,"bar":"some"}'
json.encode(struct(foo = struct(bar = "some"))) == '{"foo":{"bar":"some"}}'
json.encode(struct(foo = ["bar/", "some"])) == '{"foo":["bar/","some"]}'
json.encode(struct(foo = [struct(bar = "some")])) == '{"foo":[{"bar":"some"}]}'
"#,
        );
    }

    #[test]
    fn test_comparison_bug() {
        // TODO(nga): this should be false, because `a < b`,
        //   and comparisons are usually lexicographic.
        // TODO(nga): Also, since structs are ordered, but not sorted,
        //   comparisons on structs should be forbidden
        //   (because it is too expensive to sort keys on each comparison).
        assert::is_true("struct(b=1) < struct(a=1, x=1)")
    }
}
