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

//! A predicate type and its members: a type with one representation, a type with two, an
//! upstream member, and a `TyUser` that names the predicate type as a supertype. And a
//! recursive type over the predicate type.

use std::sync::OnceLock;

use allocative::Allocative;
use derive_more::Display;
use dupe::Dupe;
use either::Either;
use starlark_derive::NoSerialize;
use starlark_derive::StarlarkPagable;
use starlark_derive::StarlarkRepresentations;
use starlark_derive::starlark_module;
use starlark_derive::starlark_value;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::assert::Assert;
use crate::docs::DocItem;
use crate::docs::DocMember;
use crate::environment::GlobalsBuilder;
use crate::starlark_predicate_type;
use crate::starlark_recursive_type;
use crate::starlark_simple_value;
use crate::typing::Ty;
use crate::typing::TyStarlarkValue;
use crate::typing::TyUser;
use crate::typing::TyUserParams;
use crate::values::Heap;
use crate::values::StarlarkType;
use crate::values::StarlarkTypeExpr;
use crate::values::StarlarkValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::ValueTyped;
use crate::values::list::AllocList;
use crate::values::list::ListOf;
use crate::values::list::ListType;
use crate::values::predicate::StarlarkPredicate;
use crate::values::starlark_value_as_type::StarlarkValueAsType;
use crate::values::string::StarlarkStr;
use crate::values::tuple::TupleType;
use crate::values::types::list::value::ListData;
use crate::values::types::type_instance_id::TypeInstanceId;
use crate::values::typing::type_compiled::compiled::TypeCompiled;

/// What a shape is used through.
trait ShapeLike<'v> {
    fn sides(&self) -> usize;
}

starlark_predicate_type! {
    /// Something with sides.
    ///
    /// A square has four, and a string as many as it is long.
    StarlarkShape<'v>;
    name "Shape";
    get &'v dyn ShapeLike<'v>;
    upstream_member StarlarkStr;
}

starlark_recursive_type! {
    /// A shape, or a list of these.
    StarlarkShapes<'v>;
    name "Shapes";
    is Either<StarlarkShape, ListType<StarlarkShapes>>;
}

starlark_recursive_type! {
    /// A flag, or a tuple of these: no value is both this and `Shapes`.
    StarlarkFlags<'v>;
    name "Flags";
    is Either<bool, TupleType<StarlarkFlags>>;
}

/// A member that cannot say so itself.
impl<'v> ShapeLike<'v> for StarlarkStr {
    fn sides(&self) -> usize {
        self.len()
    }
}

/// A member with one representation.
#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("square")]
struct Square;
starlark_simple_value!(Square);

impl<'v> ShapeLike<'v> for Square {
    fn sides(&self) -> usize {
        4
    }
}

#[starlark_value(type = "square")]
impl<'v> StarlarkValue<'v> for Square {
    #[starlark(implements = StarlarkShape)]
    fn shape(this: ValueTyped<'v, Self>) -> &'v dyn ShapeLike<'v> {
        this.as_ref()
    }
}

/// A member with two representations: it is a member once, on the view.
#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("triangle")]
struct Triangle;

#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("polygon({})", _0)]
struct ManySided(usize);

impl<'v> ShapeLike<'v> for Triangle {
    fn sides(&self) -> usize {
        3
    }
}

impl<'v> ShapeLike<'v> for ManySided {
    fn sides(&self) -> usize {
        self.0
    }
}

#[derive(Debug, Clone, Copy, StarlarkRepresentations)]
enum PolygonView<'v> {
    Triangle(ValueTyped<'v, Triangle>),
    ManySided(ValueTyped<'v, ManySided>),
}

#[starlark_value(type = "polygon")]
impl<'v> StarlarkType<'v> for PolygonView<'v> {
    #[starlark(implements = StarlarkShape)]
    fn shape(this: Self) -> &'v dyn ShapeLike<'v> {
        match this {
            PolygonView::Triangle(x) => x.as_ref(),
            PolygonView::ManySided(x) => x.as_ref(),
        }
    }
}

#[starlark_value]
impl<'v> StarlarkValue<'v> for Triangle {
    type Type = PolygonView<'v>;
}

#[starlark_value]
impl<'v> StarlarkValue<'v> for ManySided {
    type Type = PolygonView<'v>;
}

/// Not a member, with a static type that says it is one: a `TyUser` that names the predicate
/// type as a supertype, as the type of a provider made at run time names `Provider`.
#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("outline")]
struct Outline;
starlark_simple_value!(Outline);

#[starlark_value(type = "outline")]
impl<'v> StarlarkValue<'v> for Outline {
    fn get_type_starlark_repr() -> Ty {
        static TY: OnceLock<Ty> = OnceLock::new();
        TY.get_or_init(|| {
            Ty::custom(
                TyUser::new(
                    "Outline".to_owned(),
                    TyStarlarkValue::new::<Outline>(),
                    TypeInstanceId::for_test(&"outline"),
                    TyUserParams {
                        supertypes: StarlarkShape::starlark_type_repr().iter_union().to_vec(),
                        ..TyUserParams::default()
                    },
                )
                .unwrap(),
            )
        })
        .dupe()
    }
}

/// Not a member at all.
#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("blot")]
struct Blot;
starlark_simple_value!(Blot);

#[starlark_value(type = "blot")]
impl<'v> StarlarkValue<'v> for Blot {}

#[starlark_module]
fn shape_globals(globals: &mut GlobalsBuilder) {
    const Shape: StarlarkValueAsType<StarlarkShape> = StarlarkShape::global();

    fn sides<'v>(shape: ValueOf<'v, StarlarkShape>) -> starlark::Result<i32> {
        Ok(shape.sides() as i32)
    }

    const Shapes: StarlarkValueAsType<StarlarkShapes> = StarlarkShapes::global();

    const Flags: StarlarkValueAsType<StarlarkFlags> = StarlarkFlags::global();

    fn count_shapes<'v>(shapes: ValueOf<'v, StarlarkShapes>) -> starlark::Result<i32> {
        fn count<'v>(
            shapes: Either<&'v dyn ShapeLike<'v>, ListOf<'v, StarlarkShapes>>,
        ) -> starlark::Result<i32> {
            match shapes {
                Either::Left(_) => Ok(1),
                Either::Right(list) => {
                    let mut n = 0;
                    for item in list.iter() {
                        n += count(item?.try_get()?)?;
                    }
                    Ok(n)
                }
            }
        }
        count(shapes.try_get()?)
    }

    /// The handle is a parameter as well.
    fn sides_of_handle<'v>(shape: &'v dyn ShapeLike<'v>) -> starlark::Result<i32> {
        Ok(shape.sides() as i32)
    }

    fn square() -> starlark::Result<Square> {
        Ok(Square)
    }

    fn triangle() -> starlark::Result<Triangle> {
        Ok(Triangle)
    }

    fn polygon(sides: i32) -> starlark::Result<ManySided> {
        Ok(ManySided(sides as usize))
    }

    fn outline() -> starlark::Result<Outline> {
        Ok(Outline)
    }

    fn blot() -> starlark::Result<Blot> {
        Ok(Blot)
    }
}

fn sides_of<'v>(value: Value<'v>) -> Option<usize> {
    <StarlarkShape as StarlarkTypeExpr>::get(value).map(|shape| shape.sides())
}

#[test]
fn test_handle() {
    Heap::temp(|heap| {
        assert_eq!(Some(4), sides_of(heap.alloc_simple(Square).to_value()));
        assert_eq!(Some(3), sides_of(heap.alloc_simple(Triangle).to_value()));
        assert_eq!(
            Some(7),
            sides_of(heap.alloc_simple(ManySided(7)).to_value())
        );
        assert_eq!(Some(5), sides_of(heap.alloc_str("abcde").to_value()));
        assert_eq!(None, sides_of(heap.alloc_simple(Blot).to_value()));
        assert_eq!(None, sides_of(heap.alloc_simple(Outline).to_value()));
        assert_eq!(None, sides_of(heap.alloc(1i32).to_value()));
        assert_eq!(None, sides_of(Value::new_none()));

        // The handle to a value is checked once, and read without a `Result`. It derefs to
        // the trait object.
        let square = ValueOf::<StarlarkShape>::new(heap.alloc_simple(Square).to_value()).unwrap();
        assert_eq!(4, square.get().sides());
        assert_eq!(4, square.sides());
        assert!(ValueOf::<StarlarkShape>::new(heap.alloc(1i32).to_value()).is_none());
    });
}

/// The matcher of the type is the check that gives the handle.
#[test]
fn test_matcher() {
    Heap::temp(|heap| {
        let matcher = TypeCompiled::from_ty(&StarlarkShape::starlark_type_repr(), heap);
        for (value, expected) in [
            (heap.alloc_simple(Square).to_value(), true),
            (heap.alloc_simple(Triangle).to_value(), true),
            (heap.alloc_simple(ManySided(5)).to_value(), true),
            (heap.alloc_str("abc").to_value(), true),
            (heap.alloc_simple(Blot).to_value(), false),
            (heap.alloc_simple(Outline).to_value(), false),
            (heap.alloc(1i32).to_value(), false),
        ] {
            assert_eq!(expected, matcher.matches(value), "{value}");
            assert_eq!(expected, sides_of(value).is_some(), "{value}");
        }
    });
}

#[test]
fn test_ty() {
    let ty = StarlarkShape::starlark_type_repr();
    assert_eq!("Shape", ty.to_string());
    assert_eq!(1, ty.iter_union().len());
    assert_eq!(ty, StarlarkShape::starlark_type_repr());
}

/// A type says once which predicate types it is a member of, whatever its representations.
#[test]
fn test_implemented_predicates() {
    assert_eq!(
        vec![StarlarkShape::PREDICATE],
        <Square as StarlarkType>::implemented_predicates()
    );
    assert_eq!(
        vec![StarlarkShape::PREDICATE],
        <PolygonView as StarlarkType>::implemented_predicates()
    );
    assert_eq!(
        vec![StarlarkShape::PREDICATE],
        TyStarlarkValue::new::<ManySided>().implemented_predicates()
    );
    assert!(<Blot as StarlarkType>::implemented_predicates().is_empty());
    assert!(<StarlarkStr as StarlarkType>::implemented_predicates().is_empty());
}

#[test]
fn test_in_starlark() {
    let mut a = Assert::new();
    a.globals_add(shape_globals);
    a.pass(
        r#"
def by_annotation(x: Shape) -> int:
    return sides(x)

assert_eq(4, sides(square()))
assert_eq(3, sides(triangle()))
assert_eq(6, sides(polygon(6)))
assert_eq(2, sides("ab"))
assert_eq(6, sides_of_handle(polygon(6)))
assert_eq(4, by_annotation(square()))
assert_eq(6, by_annotation(polygon(6)))
assert_eq(2, by_annotation("ab"))
assert_true(isinstance(square(), Shape))
assert_true(isinstance(polygon(5), Shape))
assert_true(isinstance("", Shape))
assert_false(isinstance(blot(), Shape))
assert_false(isinstance(1, Shape))
"#,
    );
    a.fail(
        "noop(sides)(blot())",
        "Type of parameter `shape` doesn't match, expected `Shape`, actual `blot",
    );
    a.fail(
        r#"
def by_annotation(x: Shape):
    pass

noop(by_annotation)(1)
"#,
        "Value `1` of type `int` does not match the type annotation `Shape`",
    );
}

/// The typechecker takes a nominal type that says it is a member, an upstream member, a
/// `TyUser` that names the type as a supertype, and anything it does not know the type of; it
/// does not take a nominal type that is none of these.
#[test]
fn test_typechecker() {
    let mut a = Assert::new();
    a.globals_add(shape_globals);
    a.pass(
        r#"
def takes(x: Shape):
    pass

def test(x, s: str, shape: Shape):
    sides(square())
    sides(triangle())
    sides(polygon(5))
    sides(s)
    sides(outline())
    sides(x)
    sides(shape)
    takes(square())
    takes(polygon(5))
    takes(s)
    takes(outline())
    takes(shape)
"#,
    );
    a.fail(
        r#"
def test():
    sides(blot())
"#,
        "Expected type `Shape` but got `blot`",
    );
    a.fail(
        r#"
def test():
    sides(1)
"#,
        "Expected type `Shape` but got `int`",
    );
    a.fail(
        r#"
def takes(x: Shape):
    pass

def test():
    takes([square()])
"#,
        "Expected type `Shape` but got `list[square]`",
    );
}

/// Nothing is known of what a value of the type can do, so what is done with one is not
/// checked.
#[test]
fn test_operations_are_untyped() {
    let mut a = Assert::new();
    a.globals_add(shape_globals);
    a.pass(
        r#"
def test(shape: Shape):
    shape.area
    shape + 1
    shape[0]
    shape()
    for _ in shape:
        pass
"#,
    );
}

/// The value that stands for the type is documented by the declaration.
#[test]
fn test_docs() {
    let DocItem::Type(doc) = StarlarkShape::global().to_value().documentation() else {
        panic!("a predicate type is documented as a type");
    };
    let docs = doc.docs.unwrap();
    assert_eq!("Something with sides.", docs.summary);
    assert_eq!(
        Some("A square has four, and a string as many as it is long."),
        docs.details.as_deref()
    );
    assert_eq!(StarlarkShape::starlark_type_repr(), doc.ty);
}

/// A signature prints the name of the type.
#[test]
fn test_signature() {
    let globals = GlobalsBuilder::new().with(shape_globals).build();
    let DocItem::Member(DocMember::Function(sides)) = globals.get("sides").unwrap().documentation()
    else {
        panic!("a function is documented as one");
    };
    assert_eq!("Shape", sides.params.pos_or_named[0].typ.to_string());
}

/// The type and its matcher are written out by the name of the type and read back as the
/// same.
#[cfg(feature = "pagable")]
#[test]
fn test_pagable_round_trip() {
    use pagable::PagableDeserialize;
    use pagable::PagableSerialize;
    use pagable::testing::TestingDeserializer;
    use pagable::testing::TestingSerializer;

    use crate::values::typing::type_compiled::alloc::TypeMatcherAlloc;
    use crate::values::typing::type_compiled::matcher::TypeMatcher;
    use crate::values::typing::type_compiled::matcher::TypeMatcherBox;
    use crate::values::typing::type_compiled::matcher::TypeMatcherBoxAlloc;

    let ty = StarlarkShape::starlark_type_repr();
    let mut ser = TestingSerializer::new();
    ty.pagable_serialize(&mut ser).unwrap();
    let bytes = ser.finish();
    let mut de = TestingDeserializer::new(&bytes);
    assert_eq!(ty, Ty::pagable_deserialize(&mut de).unwrap());

    let matcher = TypeMatcherBoxAlloc.ty(&ty);
    let mut ser = TestingSerializer::new();
    matcher.pagable_serialize(&mut ser).unwrap();
    let bytes = ser.finish();
    let mut de = TestingDeserializer::new(&bytes);
    let matcher = TypeMatcherBox::pagable_deserialize(&mut de).unwrap();
    Heap::temp(|heap| {
        assert!(matcher.matches(heap.alloc_simple(Square).to_value()));
        assert!(!matcher.matches(heap.alloc_simple(Blot).to_value()));
    });
}

/// A recursive type is what its expression is: here a shape, or a list of what the name stands
/// for, to any depth.
#[test]
fn test_recursive_type() {
    let ty = StarlarkShapes::starlark_type_repr();
    assert_eq!("Shapes", ty.to_string());
    assert_eq!(1, ty.iter_union().len());

    let mut a = Assert::new();
    a.globals_add(shape_globals);
    a.pass(
        r#"
assert_eq(1, count_shapes(square()))
assert_eq(1, count_shapes("ab"))
assert_eq(0, count_shapes([]))
assert_eq(2, count_shapes([square(), "ab"]))
assert_eq(4, count_shapes([square(), [triangle(), [polygon(5)]], "ab"]))
assert_true(isinstance(square(), Shapes))
assert_true(isinstance([[square()], "ab"], Shapes))
assert_false(isinstance(1, Shapes))
assert_false(isinstance([square(), 1], Shapes))
assert_false(isinstance([[blot()]], Shapes))
"#,
    );
    a.fail(
        "noop(count_shapes)([square(), [blot()]])",
        "Type of parameter `shapes` doesn't match, expected `Shapes`, actual `list",
    );
}

/// The typechecker takes for the name what it takes for the expression: a list is taken if
/// its items are, and a list of unknown items is.
#[test]
fn test_recursive_type_typechecker() {
    let mut a = Assert::new();
    a.globals_add(shape_globals);
    a.pass(
        r#"
def takes(x: Shapes):
    pass

def test(x, xs: list, shape: Shape, shapes: Shapes):
    count_shapes(square())
    count_shapes("ab")
    count_shapes([square()])
    count_shapes([[triangle()], "ab"])
    count_shapes(x)
    count_shapes([x])
    count_shapes(xs)
    count_shapes(shape)
    count_shapes([shape])
    count_shapes(shapes)
    count_shapes([shapes])
    takes([outline()])
    sides(shapes)
"#,
    );
    a.fail(
        r#"
def test():
    count_shapes(1)
"#,
        "Expected type `Shapes` but got `int`",
    );
    a.fail(
        r#"
def test():
    count_shapes([1])
"#,
        "Expected type `Shapes` but got `list[int]`",
    );
    a.fail(
        r#"
def test():
    count_shapes([[blot()]])
"#,
        "Expected type `Shapes` but got `list[list[blot]]`",
    );
    a.fail(
        r#"
def test():
    count_shapes({"a": square()})
"#,
        "Expected type `Shapes` but got `dict[str, square]`",
    );
}

/// What the typechecker does not know of a recursive type: what is done with a value of it is
/// not checked, and two recursive types are taken for each other, even these two, which share
/// no value.
#[test]
fn test_recursive_type_typechecker_approximations() {
    let mut a = Assert::new();
    a.globals_add(shape_globals);
    a.pass(
        r#"
def takes(x: Shapes):
    pass

def test(shapes: Shapes, flags: Flags):
    shapes.anything
    shapes()
    for _ in shapes:
        pass
    takes(flags)

assert_false(isinstance(True, Shapes))
assert_false(isinstance((True, (False, True)), Shapes))
assert_true(isinstance((True, (False, True)), Flags))
assert_false(isinstance([], Flags))
"#,
    );
}

/// The value that stands for a recursive type is documented by the declaration.
#[test]
fn test_recursive_type_docs() {
    let DocItem::Type(doc) = StarlarkShapes::global().to_value().documentation() else {
        panic!("a recursive type is documented as a type");
    };
    assert_eq!("A shape, or a list of these.", doc.docs.unwrap().summary);
    assert_eq!(StarlarkShapes::starlark_type_repr(), doc.ty);
}

/// A recursive type whose expression looks into a list is checked when it is made and again
/// when it is read: the list may have changed.
#[test]
fn test_recursive_type_is_not_stable() {
    Heap::temp(|heap| {
        let list = heap.alloc(AllocList([heap.alloc_simple(Square).to_value()]));
        let shapes = ValueOf::<StarlarkShapes>::new(list.to_value()).unwrap();
        assert!(shapes.try_get().is_ok());
        ListData::from_value_mut(list.to_value())
            .unwrap()
            .push(heap.alloc_simple(Blot).to_value(), heap);
        assert!(shapes.try_get().is_err());
    });
}
