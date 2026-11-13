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
use std::fmt::Debug;
use std::fmt::Display;
use std::mem;

use allocative::Allocative;
use display_container::fmt_keyed_container;
use serde::Serialize;
use starlark_derive::starlark_value;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::collections::Hashed;
use crate::collections::SmallMap;
use crate::collections::StarlarkHasher;
use crate::private::Private;
use crate::typing::Ty;
use crate::values::Heap;
use crate::values::StarlarkValue;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueTyped;
use crate::values::comparison::equals_slice;
use crate::values::layout::avalue::TrailingValues;
use crate::values::record::field::Field;
use crate::values::record::record_type::RecordTypeError;
use crate::values::record::record_type::RecordTypeGen;
use crate::values::record::record_type::RecordTypeView;
use crate::values::record::record_type::RecordVariant;
use crate::values::types::type_instance_id::TypeInstanceId;

#[derive(Debug, thiserror::Error)]
enum RecordError {
    #[error("Record type has {fields} fields, but {values} values were provided")]
    WrongNumberOfValues { fields: usize, values: usize },
}

/// An actual record.
///
/// The field values are stored inline after the fixed part, so a record is a
/// single heap allocation; it is created with [`Record::alloc`], never by value.
#[repr(C)]
#[derive(ProvidesStaticType, Allocative)]
pub struct Record<'v> {
    pub(crate) typ: Value<'v>, // Must be RecordType
    /// Number of field values, which must be derivable from the payload alone:
    /// the heap sizes allocations without following `typ`.
    len: usize,
    /// The field values, in field declaration order.
    content: [Value<'v>; 0],
}

impl<'v> Display for Record<'v> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = self.record_type_name().unwrap_or("anon");
        fmt_keyed_container(f, &format!("record[{name}]("), ")", "=", self.iter())
    }
}

impl<'v> Debug for Record<'v> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Record")
            .field("typ", &self.typ)
            .field("content", &self.content())
            .finish()
    }
}

impl<'v> Record<'v> {
    /// `type(x)` for records.
    pub const TYPE: &'static str = "record";

    /// The fixed part of a record whose `len` values follow it in memory.
    ///
    /// # Safety
    ///
    /// The caller must place `len` initialised values immediately after this
    /// struct before the record is used.
    pub(crate) const unsafe fn new_header(typ: Value<'v>, len: usize) -> Record<'v> {
        Record {
            typ,
            len,
            content: [],
        }
    }

    pub(crate) fn offset_of_content() -> usize {
        mem::offset_of!(Self, content)
    }

    /// Downcast the value.
    #[inline]
    pub fn from_value(x: Value<'v>) -> Option<&'v Self> {
        x.downcast_ref::<Record<'v>>()
    }

    /// Allocates a record of type `typ` on `heap`.
    ///
    /// `typ` must already have a name assigned, which happens when the record type
    /// is exported: assigned to a global variable in Starlark, or passed to
    /// [`Value::export_as`](crate::values::Value::export_as) from Rust.
    /// `values` must contain one value per field of `typ`, in field declaration
    /// order; defaults are not applied. Each value is checked against its
    /// field's type.
    pub fn alloc<V: RecordVariant>(
        heap: Heap<'v>,
        typ: ValueTyped<'v, RecordTypeGen<'v, V>>,
        values: &[Value<'v>],
    ) -> crate::Result<Value<'v>>
    where
        RecordTypeGen<'v, V>: StarlarkValue<'v>,
    {
        if typ.as_ref().ty_record_data.get_ty().is_none() {
            return Err(crate::Error::new_other(
                RecordTypeError::RecordTypeNotAssigned,
            ));
        }
        let fields = &typ.as_ref().fields;
        if fields.len() != values.len() {
            return Err(crate::Error::new_other(RecordError::WrongNumberOfValues {
                fields: fields.len(),
                values: values.len(),
            }));
        }
        for ((name, field), value) in fields.iter().zip(values) {
            field.typ.check_type(*value, Some(name))?;
        }
        Ok(heap.alloc_record(typ.to_value(), values))
    }

    fn get_record_type(&self) -> RecordTypeView<'v> {
        // Safe to unwrap because we always ensure typ is RecordType
        RecordTypeView::unpack_value_err(self.typ.to_value()).unwrap()
    }

    fn record_type_name(&self) -> Option<&'v str> {
        Some(&self.get_record_type().ty_record_data()?.name)
    }

    pub(crate) fn record_type_id(&self) -> TypeInstanceId {
        self.get_record_type().id()
    }

    fn get_record_fields(&self) -> &'v SmallMap<String, Field<'v>> {
        self.get_record_type().fields()
    }

    /// Iterate over the elements in the record.
    pub fn iter<'a>(&'a self) -> impl ExactSizeIterator<Item = (&'v str, Value<'v>)> + 'a
    where
        'v: 'a,
    {
        self.get_record_fields()
            .keys()
            .map(String::as_str)
            .zip(self.content().iter().copied())
    }
}

// SAFETY: every record allocation writes `len` elements after `content`.
unsafe impl<'v> TrailingValues<'v> for Record<'v> {
    fn len(&self) -> usize {
        self.len
    }

    fn content_ptr(&self) -> *const Value<'v> {
        self.content.as_ptr()
    }

    fn content_mut_ptr(&mut self) -> *mut Value<'v> {
        self.content.as_mut_ptr()
    }
}

#[starlark_value(type = Record::TYPE)]
impl<'v> StarlarkValue<'v> for Record<'v> {
    fn is_special(_: Private) -> bool
    where
        Self: Sized,
    {
        true
    }

    fn equals(&self, other: Value<'v>) -> crate::Result<bool> {
        match Record::from_value(other) {
            Some(other) if self.typ.equals(other.typ)? => {
                equals_slice(self.content(), other.content(), |x, y| x.equals(*y))
            }
            _ => Ok(false),
        }
    }

    fn get_attr(&self, attribute: &str, heap: Heap<'v>) -> Option<Value<'v>> {
        self.get_attr_hashed(Hashed::new(attribute), heap)
    }

    fn get_attr_hashed(&self, attribute: Hashed<&str>, _heap: Heap<'v>) -> Option<Value<'v>> {
        let i = self.get_record_fields().get_index_of_hashed(attribute)?;
        Some(self.content()[i].to_value())
    }

    fn write_hash(&self, hasher: &mut StarlarkHasher) -> crate::Result<()> {
        self.typ.write_hash(hasher)?;
        for v in self.content() {
            v.write_hash(hasher)?;
        }
        Ok(())
    }

    fn dir_attr(&self) -> Vec<String> {
        self.get_record_fields().keys().cloned().collect()
    }

    fn typechecker_ty(&self) -> Option<Ty> {
        Some(self.get_record_type().instance_ty())
    }
}

impl<'v> Serialize for Record<'v> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_map(self.iter())
    }
}

#[cfg(test)]
mod tests {
    use starlark_map::small_map::SmallMap;

    use crate::assert;
    use crate::environment::Globals;
    use crate::environment::Module;
    use crate::eval::Evaluator;
    use crate::syntax::AstModule;
    use crate::syntax::Dialect;
    use crate::typing::Ty;
    use crate::values::Heap;
    use crate::values::ValueTyped;
    use crate::values::layout::heap::name::StarlarkTestHeapName;
    use crate::values::record::field::Field;
    use crate::values::record::instance::Record;
    use crate::values::record::record_type::FrozenRecordType;
    use crate::values::record::record_type::RecordType;
    use crate::values::types::type_instance_id::TypeInstanceId;
    use crate::values::typing::type_compiled::compiled::TypeCompiled;

    fn unnamed_ip_address_record_type<'v>(heap: Heap<'v>) -> ValueTyped<'v, RecordType<'v>> {
        let mut fields = SmallMap::new();
        fields.insert(
            "host".to_owned(),
            Field::new(TypeCompiled::from_ty(&Ty::string(), heap), None),
        );
        fields.insert(
            "port".to_owned(),
            Field::new(TypeCompiled::from_ty(&Ty::int(), heap), None),
        );
        heap.alloc_representation(RecordType::new(
            fields,
            TypeInstanceId::for_test(&"unnamed_ip_address_record_type"),
        ))
    }

    fn ip_address_record_type<'v>(module: &Module<'v>) -> ValueTyped<'v, RecordType<'v>> {
        let typ = unnamed_ip_address_record_type(module.heap());
        let mut eval = Evaluator::new(module);
        typ.to_value()
            .export_as("IpAddress", &mut eval)
            .expect("assigning a name to a record type does not fail");
        typ
    }

    #[test]
    fn test_alloc_creates_record() {
        Module::with_temp_heap(|module| {
            let heap = module.heap();
            let typ = ip_address_record_type(&module);
            assert_eq!(2, typ.as_ref().len(), "`IpAddress` has two fields");
            assert!(!typ.as_ref().is_empty(), "`IpAddress` has fields");

            let record = Record::alloc(
                heap,
                typ,
                &[
                    heap.alloc("localhost").to_value(),
                    heap.alloc(80i32).to_value(),
                ],
            )
            .expect("values match the field types");
            assert_eq!(
                Some("localhost"),
                record
                    .get_attr("host", heap)
                    .expect("record attribute access does not fail")
                    .expect("`host` is a field of the record")
                    .unpack_str(),
                "constructed record should expose its fields"
            );

            let annotation = TypeCompiled::new(typ.to_value(), heap)
                .expect("an exported record type is a valid type annotation");
            assert!(
                annotation.matches(record),
                "constructed record should match its own type annotation"
            );
        });
    }

    #[test]
    fn test_alloc_rejects_wrong_number_of_values() {
        Module::with_temp_heap(|module| {
            let heap = module.heap();
            let typ = ip_address_record_type(&module);

            assert!(
                Record::alloc(heap, typ, &[heap.alloc("localhost").to_value()]).is_err(),
                "one value for two fields should be rejected"
            );
        });
    }

    #[test]
    fn test_alloc_rejects_ill_typed_values() {
        Module::with_temp_heap(|module| {
            let heap = module.heap();
            let typ = ip_address_record_type(&module);

            assert!(
                Record::alloc(
                    heap,
                    typ,
                    &[heap.alloc(1i32).to_value(), heap.alloc(80i32).to_value()]
                )
                .is_err(),
                "an int is not a valid `host`"
            );
        });
    }

    #[test]
    fn test_alloc_rejects_unnamed_record_type() {
        Module::with_temp_heap(|module| {
            let heap = module.heap();
            let typ = unnamed_ip_address_record_type(heap);

            assert!(
                Record::alloc(
                    heap,
                    typ,
                    &[
                        heap.alloc("localhost").to_value(),
                        heap.alloc(80i32).to_value()
                    ]
                )
                .is_err(),
                "records of a type that was never assigned a name cannot be created"
            );
        });
    }

    #[test]
    fn test_alloc_with_frozen_record_type() {
        let frozen_module = Module::with_temp_heap(|module| {
            let typ = ip_address_record_type(&module);
            module.set("IpAddress", typ.to_value());
            module
                .freeze_named(StarlarkTestHeapName::frozen_heap_name())
                .expect("freezing a module holding a record type succeeds")
        });

        Module::with_temp_heap(|module| {
            let heap = module.heap();
            let typ = frozen_module
                .get_option("IpAddress")
                .expect("symbol lookup does not fail")
                .expect("`IpAddress` was exported")
                .add_to_heap(heap);
            let typ = ValueTyped::<FrozenRecordType>::new_err(typ)
                .expect("the frozen module symbol is a frozen record type");

            let record = Record::alloc(
                heap,
                typ,
                &[
                    heap.alloc("localhost").to_value(),
                    heap.alloc(80i32).to_value(),
                ],
            )
            .expect("values match the field types");
            assert_eq!(
                Some(80),
                record
                    .get_attr("port", heap)
                    .expect("record attribute access does not fail")
                    .expect("`port` is a field of the record")
                    .unpack_i32(),
                "record built against a frozen record type should expose its fields"
            );
        });
    }

    #[test]
    fn test_alloc_requires_values_for_defaulted_fields() {
        Module::with_temp_heap(|module| {
            let heap = module.heap();
            let mut fields = SmallMap::new();
            fields.insert(
                "host".to_owned(),
                Field::new(TypeCompiled::from_ty(&Ty::string(), heap), None),
            );
            fields.insert(
                "port".to_owned(),
                Field::new(
                    TypeCompiled::from_ty(&Ty::int(), heap),
                    Some(heap.alloc(80i32).to_value()),
                ),
            );
            let typ = heap.alloc_representation(RecordType::new(
                fields,
                TypeInstanceId::for_test(&"ip_address_record_type_with_defaulted_port"),
            ));
            let mut eval = Evaluator::new(&module);
            typ.to_value()
                .export_as("IpAddress", &mut eval)
                .expect("assigning a name to a record type does not fail");

            assert!(
                Record::alloc(heap, typ, &[heap.alloc("localhost").to_value()]).is_err(),
                "defaults are not applied, so a value for `port` is still required"
            );
            assert!(
                Record::alloc(
                    heap,
                    typ,
                    &[
                        heap.alloc("localhost").to_value(),
                        heap.alloc(8080i32).to_value()
                    ]
                )
                .is_ok(),
                "explicitly passing a value for the defaulted field works"
            );
        });
    }

    #[test]
    fn test_empty_record() {
        assert::pass(
            r#"
Empty = record()
e = Empty()
assert_eq(dir(e), [])
assert_eq(e, Empty())
garbage_collect()
assert_eq(str(e), "record[Empty]()")
"#,
        );
    }

    /// A record type can reach an instance of itself through a mutable field
    /// default, so copying the type and copying the instance recurse into each
    /// other. Both the collector and the freezer must terminate on that cycle.
    const SELF_REFERENTIAL_DEFAULT: &str = r#"
_shared = []
Node = record(children = field(list, _shared), label = field(str, "root"))
_shared.append(Node(children = [], label = "leaf"))
root = Node()
other = Node(children = [root], label = "other")
"#;

    const SELF_REFERENTIAL_DEFAULT_CHECKS: &str = r#"
def check(cond, what):
    if not cond:
        fail("unexpected value for " + what)

check(root.label == "root", "root.label")
check(len(root.children) == 1, "len(root.children)")
check(root.children[0].label == "leaf", "root.children[0].label")
check(root.children[0].children == [], "root.children[0].children")
check(other.children[0] == root, "other.children[0]")
check(other.children[0].children[0] == root.children[0], "other.children[0].children[0]")
"#;

    #[test]
    fn test_gc_with_self_referential_default() {
        assert::pass(&format!(
            "{SELF_REFERENTIAL_DEFAULT}\ngarbage_collect()\n{SELF_REFERENTIAL_DEFAULT_CHECKS}"
        ));
    }

    #[test]
    fn test_freeze_with_self_referential_default() {
        let frozen = Module::with_temp_heap(|module| {
            {
                let mut eval = Evaluator::new(&module);
                eval.eval_module(
                    AstModule::parse(
                        "records.star",
                        SELF_REFERENTIAL_DEFAULT.to_owned(),
                        &Dialect::AllOptionsInternal,
                    )
                    .expect("the module parses"),
                    &Globals::extended_internal(),
                )
                .expect("the module evaluates");
            }
            module
                .freeze_named(StarlarkTestHeapName::frozen_heap_name())
                .expect("freezing a record type cycle through a default terminates")
        });

        Module::with_temp_heap(|module| {
            for name in ["Node", "root", "other"] {
                let value = frozen
                    .get_option(name)
                    .expect("symbol lookup does not fail")
                    .expect("the symbol was exported")
                    .add_to_heap(module.heap());
                module.set(name, value);
            }
            let mut eval = Evaluator::new(&module);
            eval.eval_module(
                AstModule::parse(
                    "checks.star",
                    SELF_REFERENTIAL_DEFAULT_CHECKS.to_owned(),
                    &Dialect::AllOptionsInternal,
                )
                .expect("the checks parse"),
                &Globals::extended_internal(),
            )
            .expect("the frozen records read back correctly");
        });
    }
}
