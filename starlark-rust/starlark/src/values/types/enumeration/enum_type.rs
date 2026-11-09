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

use std::cell::UnsafeCell;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::sync::Arc;

use allocative::Allocative;
use display_container::fmt_container;
use dupe::Dupe;
use mini_map::MiniFixedSortedMap;
use once_cell::unsync::OnceCell;
use starlark_derive::NoSerialize;
use starlark_derive::StarlarkRepresentations;
use starlark_derive::Trace;
use starlark_derive::starlark_module;
use starlark_derive::starlark_value;
use starlark_map::small_map::SmallMap;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::environment::Methods;
use crate::environment::MethodsBuilder;
use crate::eval::Arguments;
use crate::eval::Evaluator;
use crate::pagable::starlark_deserialize::StarlarkDeserialize;
use crate::pagable::starlark_deserialize::StarlarkDeserializeContext;
use crate::pagable::starlark_serialize::StarlarkSerialize;
use crate::pagable::starlark_serialize::StarlarkSerializeContext;
use crate::typing::ParamSpec;
use crate::typing::Ty;
use crate::typing::callable::TyCallable;
use crate::typing::starlark_value::TyStarlarkValue;
use crate::typing::user::TyUser;
use crate::typing::user::TyUserFields;
use crate::typing::user::TyUserIndex;
use crate::typing::user::TyUserParams;
use crate::values::AllocValue;
use crate::values::Demand;
use crate::values::Freeze;
use crate::values::FreezeResult;
use crate::values::Freezer;
use crate::values::Heap;
use crate::values::StarlarkType;
use crate::values::StarlarkValue;
use crate::values::StringValue;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueTyped;
use crate::values::dict::value::ValueStr;
use crate::values::enumeration::matcher::EnumTypeMatcher;
use crate::values::enumeration::ty_enum_type::TyEnumData;
use crate::values::enumeration::value::EnumValue;
use crate::values::function::FUNCTION_TYPE;
use crate::values::index::convert_index;
use crate::values::list::AllocList;
use crate::values::type_expr::StarlarkTypeExpr;
use crate::values::type_expr::StarlarkTypeExprStable;
use crate::values::type_repr::StarlarkTypeRepr;
use crate::values::types::type_instance_id::StarlarkTypeIdDomain;
use crate::values::types::type_instance_id::TypeInstanceId;
use crate::values::typing::type_compiled::type_matcher_factory::TypeMatcherFactory;

#[derive(thiserror::Error, Debug)]
enum EnumError {
    #[error("enum values must all be distinct, but repeated `{0}`")]
    DuplicateEnumValue(String),
    #[error("Unknown enum element `{0}`, given to `{1}`")]
    InvalidElement(String, String),
}

#[doc(hidden)]
pub(crate) trait EnumVariant: Allocative + Debug + 'static {
    fn get_or_init_ty(&self, f: &dyn Fn() -> crate::Result<Arc<TyEnumData>>) -> crate::Result<()>;

    fn get_ty(&self) -> Option<&Arc<TyEnumData>>;
}

#[derive(Debug, Allocative)]
pub(crate) struct EnumVariantUnfrozen {
    // FIXME(JakobDegen): allocative OSS release
    #[allocative(skip)]
    data: OnceCell<Arc<TyEnumData>>,
}

impl EnumVariant for EnumVariantUnfrozen {
    fn get_or_init_ty(&self, f: &dyn Fn() -> crate::Result<Arc<TyEnumData>>) -> crate::Result<()> {
        self.data.get_or_try_init(f)?;
        Ok(())
    }

    fn get_ty(&self) -> Option<&Arc<TyEnumData>> {
        self.data.get()
    }
}

#[derive(Debug, Allocative)]
pub(crate) struct EnumVariantFrozen {
    data: Option<Arc<TyEnumData>>,
}

impl EnumVariant for EnumVariantFrozen {
    fn get_or_init_ty(&self, _f: &dyn Fn() -> crate::Result<Arc<TyEnumData>>) -> crate::Result<()> {
        Ok(())
    }

    fn get_ty(&self) -> Option<&Arc<TyEnumData>> {
        self.data.as_ref()
    }
}

/// The type of an enumeration, created by `enum()`.
#[derive(Debug, Trace, NoSerialize, ProvidesStaticType, Allocative)]
#[repr(C)]
// Deliberately store fully populated values
// for each entry, so we can produce enum values with zero allocation.
#[trace(bound = "")]
pub(crate) struct EnumTypeGen<'v, V: EnumVariant + 'static + ?Sized> {
    pub(super) id: TypeInstanceId,
    // The key is the value of the enumeration
    // The value is a value of type EnumValue
    #[allocative(skip)] // TODO(nga): do not skip.
    elements: UnsafeCell<SmallMap<Value<'v>, Value<'v>>>,
    #[trace(static)]
    pub(super) ty_enum_data: V,
}

pub(super) type EnumType<'v> = EnumTypeGen<'v, EnumVariantUnfrozen>;

pub(crate) type FrozenEnumType<'v> = EnumTypeGen<'v, EnumVariantFrozen>;

impl<'v> Freeze<'v> for EnumType<'v> {
    type Frozen<'fv> = FrozenEnumType<'fv>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        let EnumTypeGen {
            id,
            ty_enum_data,
            elements,
        } = self;
        let elements = elements.freeze(freezer)?;
        Ok(EnumTypeGen {
            id,
            ty_enum_data: EnumVariantFrozen {
                data: ty_enum_data.data.into_inner(),
            },
            elements,
        })
    }
}

impl<'v> AllocValue<'v> for EnumType<'v> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_complex(self)
    }
}

// SAFETY: `elements` is written once, in `EnumType::new`, before the value is reachable from
// anywhere else (see `elements`), so the `UnsafeCell` is shared like the `SmallMap` inside it,
// whose `Value<'static>`s are `Send + Sync` like the contents of every frozen value. At any
// other brand the value is unfrozen and belongs to one thread, like every `Value<'v>`.
unsafe impl<V: EnumVariant + Send> Send for EnumTypeGen<'static, V> {}
unsafe impl<V: EnumVariant + Sync> Sync for EnumTypeGen<'static, V> {}

impl<'v, V: EnumVariant + ?Sized> Display for EnumTypeGen<'v, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_container(f, "enum(", ")", self.elements().iter().map(|(k, _v)| k))
    }
}

impl<'v> StarlarkSerialize for FrozenEnumType<'v> {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        use pagable::PagableSerialize;
        self.id.starlark_serialize(ctx)?;
        self.ty_enum_data.data.pagable_serialize(ctx.pagable())?;
        self.elements().starlark_serialize(ctx)?;
        Ok(())
    }
}

impl<'v> StarlarkDeserialize<'v> for FrozenEnumType<'v> {
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'v>,
    ) -> crate::Result<Self> {
        use pagable::PagableDeserialize;
        let id = TypeInstanceId::starlark_deserialize(ctx)?;
        let data = <Option<Arc<TyEnumData>>>::pagable_deserialize(ctx.pagable())?;
        let elements = SmallMap::starlark_deserialize(ctx)?;
        Ok(EnumTypeGen {
            id,
            ty_enum_data: EnumVariantFrozen { data },
            elements: UnsafeCell::new(elements),
        })
    }
}

crate::register_simple_vtable_entry!(FrozenEnumType<'static>);
// SAFETY: The vtable entry is registered above. The deser type id is
// lifetime-erased, so the `'static` instantiation covers all heap lifetimes.
unsafe impl<'v> crate::__derive_refs::VtableRegistered for FrozenEnumType<'v> {}

impl<'v> EnumType<'v> {
    pub(super) fn new(
        elements: Vec<StringValue<'v>>,
        heap: Heap<'v>,
        id: TypeInstanceId,
    ) -> crate::Result<ValueTyped<'v, EnumType<'v>>> {
        // We are constructing the enum and all elements in one go.
        // They both point at each other, which adds to the complexity.
        let typ = heap.alloc_typed(EnumType {
            id,
            ty_enum_data: EnumVariantUnfrozen {
                data: OnceCell::new(),
            },
            elements: UnsafeCell::new(SmallMap::new()),
        });

        let mut res = SmallMap::with_capacity(elements.len());
        for (i, x) in elements.iter().enumerate() {
            let v = heap.alloc(EnumValue {
                id,
                typ: typ.to_value(),
                index: i as i32,
                value: x.to_value(),
            });
            if res.insert_hashed(x.to_value().get_hashed()?, v).is_some() {
                return Err(crate::Error::new_other(EnumError::DuplicateEnumValue(
                    x.to_string(),
                )));
            }
        }

        // Here we tie the cycle
        unsafe {
            // SAFETY: we own unique reference to `t`.
            *typ.elements.get() = res;
        }
        Ok(typ)
    }
}

impl<'v, V: EnumVariant + ?Sized> EnumTypeGen<'v, V> {
    pub(super) fn elements(&self) -> &SmallMap<Value<'v>, Value<'v>> {
        // Safe because we never mutate the elements after construction.
        unsafe { &*self.elements.get() }
    }
}

impl<'v, V: EnumVariant + ?Sized> EnumTypeGen<'v, V> {
    pub(super) fn ty_enum_data(&self) -> Option<&Arc<TyEnumData>> {
        self.ty_enum_data.get_ty()
    }

    /// What `export_as` does: give the type its name, once.
    fn export_as(&self, variable_name: &str) -> crate::Result<()> {
        self.ty_enum_data.get_or_init_ty(&|| {
            let ty_enum_value = Ty::custom(TyUser::new(
                variable_name.to_owned(),
                TyStarlarkValue::new::<EnumValue>(),
                self.id,
                TyUserParams {
                    matcher: Some(TypeMatcherFactory::new(EnumTypeMatcher { id: self.id })),
                    ..TyUserParams::default()
                },
            )?);

            // The unwrap here is safe because the new() method requires the elements be
            // of type StringValue<'v>
            let fields_map: MiniFixedSortedMap<String, Ty> = self
                .elements()
                .keys()
                .map(|key| {
                    (
                        key.to_value().unpack_str().unwrap().to_owned(),
                        ty_enum_value.dupe(),
                    )
                })
                .collect();

            let ty_enum_type = Ty::custom(TyUser::new(
                format!("enum[{variable_name}]"),
                TyStarlarkValue::new::<EnumType>(),
                TypeInstanceId::from_identity(StarlarkTypeIdDomain::EnumTypeOfType, &self.id),
                TyUserParams {
                    fields: TyUserFields {
                        known: fields_map,
                        unknown: false,
                    },
                    index: Some(TyUserIndex {
                        index: Ty::int(),
                        result: ty_enum_value.dupe(),
                    }),
                    iter_item: Some(ty_enum_value.dupe()),
                    callable: Some(TyCallable::new(
                        ParamSpec::pos_only(
                            [
                                // TODO(nga): we can do better parameter type.
                                Ty::any(),
                            ],
                            [],
                        ),
                        ty_enum_value.dupe(),
                    )),
                    ..TyUserParams::default()
                },
            )?);
            Ok(Arc::new(TyEnumData {
                name: variable_name.to_owned(),
                id: self.id,
                ty_enum_value,
                ty_enum_type,
            }))
        })
    }

    pub(crate) fn construct(&self, val: Value<'v>) -> crate::Result<Value<'v>> {
        match self.elements().get_hashed_by_value(val.get_hashed()?) {
            Some(v) => Ok(*v),
            None => Err(crate::Error::new_other(EnumError::InvalidElement(
                val.to_str(),
                self.to_string(),
            ))),
        }
    }
}

starlark::methods_static!(ENUM_TYPE_METHODS = enum_type_methods);

pub(crate) type AnyEnumType<'v> = &'v EnumTypeGen<'v, dyn EnumVariant>;

impl<'v> StarlarkTypeRepr for AnyEnumType<'v> {
    type Canonical = <EnumTypeView<'v> as StarlarkTypeRepr>::Canonical;

    #[inline]
    fn starlark_type_repr() -> crate::typing::Ty {
        <FrozenEnumType<'v> as StarlarkValue<'v>>::get_type_starlark_repr()
    }
}

impl<'v> StarlarkTypeExpr<'v> for AnyEnumType<'v> {
    type Get = Self;

    fn get(value: Value<'v>) -> Option<Self> {
        AnyEnumType::unpack_value_opt(value)
    }
}

impl<'v> StarlarkTypeExprStable<'v> for AnyEnumType<'v> {}

impl<'v> UnpackValue<'v> for AnyEnumType<'v> {
    type Error = std::convert::Infallible;

    fn unpack_value_impl(value: Value<'v>) -> Result<Option<Self>, Self::Error> {
        Ok(value.request_value())
    }
}

/// An enum type in either of its representations.
#[derive(Debug, Clone, Copy, StarlarkRepresentations)]
pub(crate) enum EnumTypeView<'v> {
    Frozen(ValueTyped<'v, FrozenEnumType<'v>>),
    Unfrozen(ValueTyped<'v, EnumType<'v>>),
}

impl<'v> EnumTypeView<'v> {
    fn as_dyn(self) -> AnyEnumType<'v> {
        match self {
            EnumTypeView::Frozen(x) => x.as_ref(),
            EnumTypeView::Unfrozen(x) => x.as_ref(),
        }
    }
}

#[starlark_value(type = FUNCTION_TYPE)]
impl<'v> StarlarkType<'v> for EnumTypeView<'v> {
    // TODO(nga): replace `Color("RED")` with `Color.RED`.
    //   https://www.internalfb.com/tasks/?t=183515013
    fn invoke(
        &self,
        _me: Value<'v>,
        args: &Arguments<'v, '_>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> crate::Result<Value<'v>> {
        args.no_named_args()?;
        let val = args.positional1(eval.heap())?;
        Ok(self.as_dyn().construct(val)?.to_value())
    }

    fn get_attr(&self, attribute: &str, _heap: Heap<'v>) -> Option<Value<'v>> {
        self.as_dyn()
            .elements()
            .get(&ValueStr(attribute))
            .map(|v| v.to_value())
    }

    fn dir_attr(&self) -> Vec<String> {
        // The unwrap here is safe because the new() method requires the elements be
        // of type StringValue<'v>
        self.as_dyn()
            .elements()
            .keys()
            .map(|key| key.to_value().unpack_str().unwrap().to_owned())
            .collect()
    }

    fn length(&self) -> crate::Result<i32> {
        Ok(self.as_dyn().elements().len() as i32)
    }

    fn at(&self, index: Value, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        let elements = self.as_dyn().elements();
        let i = convert_index(index, elements.len() as i32)? as usize;
        // Must be in the valid range since convert_index checks that, so just unwrap
        Ok(elements.get_index(i).map(|x| *x.1).unwrap().to_value())
    }

    unsafe fn iterate(&self, me: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(me)
    }

    unsafe fn iter_size_hint(&self, index: usize) -> (usize, Option<usize>) {
        let len = self.as_dyn().elements().len();
        debug_assert!(index <= len);
        let rem = len - index;
        (rem, Some(rem))
    }

    unsafe fn iter_next(&self, index: usize, _heap: Heap<'v>) -> Option<Value<'v>> {
        self.as_dyn()
            .elements()
            .values()
            .nth(index)
            .map(|v| v.to_value())
    }

    unsafe fn iter_stop(&self) {}

    fn get_methods() -> Option<&'static Methods> {
        Some(ENUM_TYPE_METHODS.methods())
    }

    fn eval_type(&self) -> Option<Ty> {
        self.as_dyn().ty_enum_data().map(|t| t.ty_enum_value.dupe())
    }

    fn typechecker_ty(&self) -> Option<Ty> {
        self.as_dyn().ty_enum_data().map(|t| t.ty_enum_type.dupe())
    }

    fn export_as(
        &self,
        variable_name: &str,
        _eval: &mut Evaluator<'v, '_, '_>,
    ) -> crate::Result<()> {
        self.as_dyn().export_as(variable_name)
    }
}

#[starlark_value]
impl<'v> StarlarkValue<'v> for EnumType<'v> {
    type Type = EnumTypeView<'v>;

    fn provide(&'v self, demand: &mut Demand<'_, 'v>) {
        demand.provide_value::<AnyEnumType<'v>>(self);
    }
}

#[starlark_value]
impl<'v> StarlarkValue<'v> for FrozenEnumType<'v> {
    type Type = EnumTypeView<'v>;

    fn provide(&'v self, demand: &mut Demand<'_, 'v>) {
        demand.provide_value::<AnyEnumType<'v>>(self);
    }
}

#[starlark_module]
fn enum_type_methods(builder: &mut MethodsBuilder) {
    #[starlark(attribute)]
    fn r#type<'v>(this: AnyEnumType<'v>, heap: Heap<'_>) -> starlark::Result<Value<'v>> {
        match this.ty_enum_data() {
            Some(ty_enum_type) => Ok(heap.alloc(ty_enum_type.name.as_str())),
            None => Ok(heap.alloc(EnumValue::TYPE)),
        }
    }

    fn values<'v>(
        this: AnyEnumType<'v>,
    ) -> starlark::Result<AllocList<impl Iterator<Item = Value<'v>>>> {
        Ok(AllocList(this.elements().keys().copied()))
    }
}

#[cfg(test)]
mod tests {
    use crate::assert;

    #[test]
    fn test_enum_type_as_type_pass() {
        assert::pass(
            r#"
Color = enum("RED", "GREEN", "BLUE")

def f_pass(x: Color):
    pass

def g_pass(x: Color):
    f_pass(x)
"#,
        );
    }

    #[test]
    fn test_enum_type_fail_runtime() {
        assert::fail(
            r#"
Color = enum("RED", "GREEN", "BLUE")
Season = enum("SPRING", "SUMMER", "AUTUMN", "WINTER")

def f(x: Color):
    pass

def g(x):
    f(x)

g(Season[0])
"#,
            r#"Value `Season("SPRING")` of type `enum` does not match the type annotation `Color` for argument `x`"#,
        );
    }

    #[test]
    fn test_enum_type_fail_compile_time() {
        assert::fail(
            r#"
Color = enum("RED", "GREEN", "BLUE")
Season = enum("SPRING", "SUMMER", "AUTUMN", "WINTER")

def f(x: Color):
    pass

def g(x: Season):
    f(x)
"#,
            r#"Expected type `Color` but got `Season`"#,
        );
    }

    #[test]
    fn test_enum_is_callable() {
        assert::pass(
            r#"
Color = enum("RED", "GREEN", "BLUE")

def foo(x: typing.Callable):
    pass

def bar():
    foo(Color)
"#,
        );
    }

    #[test]
    fn test_enum_value_index() {
        // Test `.index` is available at both compile and runtime.
        assert::pass(
            r#"
Color = enum("RED", "GREEN", "BLUE")

def test():
    for c in Color:
        if c.index == 1:
            pass

test()
"#,
        );
    }

    #[test]
    fn test_enum_value_index_correct_type() {
        assert::fail(
            r#"
Fruit = enum("APPLE", "BANANA", "ORANGE")

def expect_str(s: str):
    pass

def test():
    for f in Fruit:
        expect_str(f.index)
"#,
            "Expected type `str` but got `int`",
        );
    }

    #[test]
    fn test_enum_index() {
        assert::pass(
            r#"
Mood = enum("HAPPY", "SAD")

def test() -> Mood:
    return Mood[0]

test()
"#,
        );
    }

    #[test]
    fn test_enum_index_fail() {
        assert::fail(
            r#"
Shape = enum("SQUARE", "CIRCLE")

def accept_str(s: str):
    pass

def test():
    accept_str(Shape[0])
"#,
            "Expected type `str` but got `Shape`",
        );
    }

    #[test]
    fn test_enum_call() {
        assert::fail(
            r#"
Currency = enum("GBP", "USD", "EUR")

def accept_str(s: str):
    pass

def test():
    accept_str(Currency("GBP"))
"#,
            "Expected type `str` but got `Currency`",
        );
    }

    #[test]
    fn test_enum_attribute_access() {
        assert::pass(
            r#"
Color = enum("RED", "GREEN", "BLUE")

def test():
    red = Color.RED
    green = Color.GREEN
    blue = Color.BLUE

    assert_eq(red, Color("RED"))
    assert_eq(green, Color("GREEN"))
    assert_eq(blue, Color("BLUE"))

    assert_eq(red.value, "RED")
    assert_eq(green.value, "GREEN")
    assert_eq(blue.value, "BLUE")

test()
"#,
        );
    }

    #[test]
    fn test_enum_attribute_access_invalid() {
        assert::fail(
            r#"
Color = enum("RED", "GREEN", "BLUE")

def test():
    purple = Color.PURPLE

test()
"#,
            "Object of type `function` has no attribute `PURPLE`",
        );
    }

    #[test]
    fn test_enum_attribute_access_type() {
        assert::fail(
            r#"
Color = enum("RED", "GREEN", "BLUE")

def foo() -> str:
    return Color.RED
"#,
            "Expected type `str` but got `Color`",
        );
    }
}
