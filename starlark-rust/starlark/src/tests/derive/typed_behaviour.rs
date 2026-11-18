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

//! `StarlarkType` methods written with typed signatures in a `#[starlark_value]` block.

use std::cmp::Ordering;
use std::fmt::Debug;
use std::hash::Hash;
use std::hash::Hasher;

use allocative::Allocative;
use derive_more::Display;
use either::Either;
use starlark_derive::NoSerialize;
use starlark_derive::StarlarkPagable;
use starlark_derive::StarlarkPagablePanic;
use starlark_derive::StarlarkRepresentations;
use starlark_derive::starlark_value;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::collections::StarlarkHasher;
use crate::const_frozen_string;
use crate::starlark_simple_value;
use crate::typing::HasTyVTable;
use crate::typing::OperatorSignature;
use crate::typing::Ty;
use crate::typing::TypingBinOp;
use crate::typing::TypingUnOp;
use crate::values::Heap;
use crate::values::StarlarkType;
use crate::values::StarlarkTypeExprHashable;
use crate::values::StarlarkValue;
use crate::values::StringValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::ValueTyped;
use crate::values::list::AllocList;
use crate::values::list::ListType;
use crate::values::string::StarlarkStr;

fn hash_of(value: Value) -> crate::Result<u64> {
    let mut hasher = StarlarkHasher::new();
    value.write_hash(&mut hasher)?;
    Ok(hasher.finish())
}

fn assert_always_hashes<'v, T: StarlarkTypeExprHashable<'v>>() {}

/// Every method typed, over the type itself, with the plain results.
#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("plain({})", _0)]
struct Plain(i32);
starlark_simple_value!(Plain);

#[starlark_value(type = "plain")]
impl<'v> StarlarkValue<'v> for Plain {
    fn equals(&self, other: &Self) -> bool {
        self.0 == other.0
    }

    fn compare(&self, other: &Self) -> Ordering {
        self.0.cmp(&other.0)
    }

    fn write_hash(&self, hasher: &mut StarlarkHasher) {
        self.0.hash(hasher)
    }
}

#[test]
fn test_operand_of_own_type() {
    Heap::temp(|heap| {
        let one = heap.alloc_simple(Plain(1));
        let uno = heap.alloc_simple(Plain(1));
        let two = heap.alloc_simple(Plain(2));

        assert!(one.equals(uno).unwrap());
        assert!(!one.equals(two).unwrap());
        assert_eq!(Ordering::Less, one.compare(two).unwrap());
        assert_eq!(Ordering::Equal, one.compare(uno).unwrap());
        assert_eq!(hash_of(one).unwrap(), hash_of(uno).unwrap());
        assert_ne!(hash_of(one).unwrap(), hash_of(two).unwrap());
    });
}

/// A value that is not of the operand's type gets what the trait's default gives: it is not
/// equal, and it does not compare, with the default's error.
#[test]
fn test_operand_of_another_type() {
    #[derive(
        Debug,
        Display,
        ProvidesStaticType,
        NoSerialize,
        Allocative,
        StarlarkPagable
    )]
    #[display("no_behaviour")]
    struct NoBehaviour;
    starlark_simple_value!(NoBehaviour);

    #[starlark_value(type = "plain")]
    impl<'v> StarlarkValue<'v> for NoBehaviour {}

    Heap::temp(|heap| {
        let plain = heap.alloc_simple(Plain(1));
        let int = heap.alloc(1i32).to_value();

        assert!(!plain.equals(int).unwrap());
        // The default's error names the type, and `NoBehaviour` is named as `Plain` is.
        let default = heap.alloc_simple(NoBehaviour).compare(int).unwrap_err();
        assert_eq!(
            default.to_string(),
            plain.compare(int).unwrap_err().to_string(),
        );
    });
}

#[test]
fn test_write_hash_without_result_always_hashes() {
    assert_always_hashes::<Plain>();
    assert_always_hashes::<Either<Plain, (Plain, Plain)>>();
}

/// Typed methods that return a `Result`, over a union and over a conversion that can fail.
#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("count({})", _0)]
struct Count(i32);
starlark_simple_value!(Count);

#[derive(Debug, thiserror::Error)]
#[error("a count is not compared with a negative number")]
struct Negative;

#[starlark_value(type = "count")]
impl<'v> StarlarkValue<'v> for Count {
    fn equals(&self, other: Either<&Self, &str>) -> starlark::Result<bool> {
        match other {
            Either::Left(other) => Ok(self.0 == other.0),
            Either::Right(other) => {
                Ok(self.0 == other.parse::<i32>().map_err(starlark::Error::new_other)?)
            }
        }
    }

    fn compare(&self, other: i32) -> starlark::Result<Ordering> {
        if other < 0 {
            return Err(starlark::Error::new_other(Negative));
        }
        Ok(self.0.cmp(&other))
    }
}

#[test]
fn test_union_operand() {
    Heap::temp(|heap| {
        let one = heap.alloc_simple(Count(1));

        assert!(one.equals(heap.alloc_simple(Count(1))).unwrap());
        assert!(one.equals(heap.alloc("1").to_value()).unwrap());
        assert!(!one.equals(heap.alloc("2").to_value()).unwrap());
        assert!(!one.equals(heap.alloc(1i32).to_value()).unwrap());
    });
}

#[test]
fn test_body_error() {
    Heap::temp(|heap| {
        let one = heap.alloc_simple(Count(1));

        let e = one.equals(heap.alloc("one").to_value()).unwrap_err();
        assert!(e.to_string().contains("invalid digit"), "{e}");
        let e = one.compare(heap.alloc(-1i32).to_value()).unwrap_err();
        assert!(e.to_string().contains("negative number"), "{e}");
    });
}

/// A value of the operand's type that does not convert is the conversion's error, and a value
/// of another type is still the default's.
#[test]
fn test_operand_conversion_fails() {
    Heap::temp(|heap| {
        let one = heap.alloc_simple(Count(1));

        assert_eq!(
            Ordering::Less,
            one.compare(heap.alloc(2i32).to_value()).unwrap()
        );
        let e = one.compare(heap.alloc(1i64 << 40).to_value()).unwrap_err();
        assert!(e.to_string().contains("too big to fit in i32"), "{e}");
        let e = one.compare(heap.alloc("2").to_value()).unwrap_err();
        assert!(
            e.to_string()
                .contains("Operation `compare` not supported for types `count` and `string`"),
            "{e}",
        );
    });
}

/// An operand that is any value: the trait's operand with a plain result is a typed method
/// too, and one written with the trait's signature is left as it is.
#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("any_operand")]
struct AnyOperand;
starlark_simple_value!(AnyOperand);

#[starlark_value(type = "any_operand")]
impl<'v> StarlarkValue<'v> for AnyOperand {
    fn equals(&self, other: Value<'v>) -> bool {
        other.is_none()
    }

    fn compare(&self, other: Value<'v>) -> starlark::Result<Ordering> {
        if other.is_none() {
            Ok(Ordering::Greater)
        } else {
            Ok(Ordering::Less)
        }
    }

    fn write_hash(&self, hasher: &mut StarlarkHasher) -> starlark::Result<()> {
        hasher.write_u8(0);
        Ok(())
    }
}

#[test]
fn test_operand_any_value() {
    Heap::temp(|heap| {
        let x = heap.alloc_simple(AnyOperand);

        assert!(x.equals(Value::new_none()).unwrap());
        assert!(!x.equals(heap.alloc(1i32).to_value()).unwrap());
        assert_eq!(Ordering::Greater, x.compare(Value::new_none()).unwrap());
        assert_eq!(
            Ordering::Less,
            x.compare(heap.alloc(1i32).to_value()).unwrap()
        );
        hash_of(x).unwrap();
    });
}

/// A generic impl: the typed bodies see `Self` and the impl's parameters.
#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("tagged({:?})", _0)]
struct Tagged<T: Tag>(T::Key);

trait Tag: Debug + Allocative + Send + Sync + 'static {
    type Key: Debug + Ord + Hash + Allocative + Send + Sync + 'static;
    const SALT: u8;
}

#[derive(Debug, ProvidesStaticType, Allocative)]
struct ByNumber;

impl Tag for ByNumber {
    type Key = u32;
    const SALT: u8 = 7;
}

#[starlark_value(type = "tagged")]
impl<'v, T: Tag> StarlarkValue<'v> for Tagged<T>
where
    Self: HasTyVTable + ProvidesStaticType<'v>,
{
    fn equals(&self, other: &Self) -> bool {
        self.0 == other.0
    }

    fn compare(&self, other: &Tagged<T>) -> Ordering {
        self.0.cmp(&other.0)
    }

    fn write_hash(&self, hasher: &mut StarlarkHasher) {
        hasher.write_u8(T::SALT);
        self.0.hash(hasher)
    }
}

crate::register_avalue_simple_frozen!(Tagged<ByNumber>);

#[test]
fn test_generic_impl() {
    assert_always_hashes::<Tagged<ByNumber>>();
    Heap::temp(|heap| {
        let one = heap.alloc_simple(Tagged::<ByNumber>(1));
        let two = heap.alloc_simple(Tagged::<ByNumber>(2));

        assert!(
            one.equals(heap.alloc_simple(Tagged::<ByNumber>(1)))
                .unwrap()
        );
        assert!(!one.equals(two).unwrap());
        assert!(!one.equals(Value::new_none()).unwrap());
        assert_eq!(Ordering::Less, one.compare(two).unwrap());
        assert_ne!(hash_of(one).unwrap(), hash_of(two).unwrap());
    });
}

/// A type with two representations: the methods are on the view, and the operand is the view.
#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("small({})", _0)]
struct Small(u8);

#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("large({})", _0)]
struct Large(u64);

#[derive(Debug, Clone, Copy, StarlarkRepresentations)]
enum SizedView<'v> {
    Small(ValueTyped<'v, Small>),
    Large(ValueTyped<'v, Large>),
}

impl<'v> SizedView<'v> {
    fn get(self) -> u64 {
        match self {
            SizedView::Small(x) => x.0 as u64,
            SizedView::Large(x) => x.0,
        }
    }
}

#[starlark_value(type = "sized")]
impl<'v> StarlarkType<'v> for SizedView<'v> {
    fn equals(&self, other: SizedView<'v>) -> bool {
        self.get() == other.get()
    }

    fn compare(&self, other: Self) -> Ordering {
        self.get().cmp(&other.get())
    }

    fn write_hash(&self, hasher: &mut StarlarkHasher) {
        self.get().hash(hasher)
    }

    fn at(&self, index: i32, heap: Heap<'v>) -> Value<'v> {
        heap.alloc(self.get() as i32 + index).to_value()
    }

    fn is_in(&self, other: Self) -> starlark::Result<bool> {
        if other.get() == 0 {
            return Err(starlark::Error::new_other(Negative));
        }
        Ok(other.get() <= self.get())
    }

    fn length(&self) -> i32 {
        self.get() as i32
    }
}

#[starlark_value]
impl<'v> StarlarkValue<'v> for Small {
    type Type = SizedView<'v>;
}

#[starlark_value]
impl<'v> StarlarkValue<'v> for Large {
    type Type = SizedView<'v>;
}

#[test]
fn test_view() {
    assert_always_hashes::<SizedView>();
    Heap::temp(|heap| {
        let small = heap.alloc_simple(Small(1));
        let large = heap.alloc_simple(Large(1));
        let larger = heap.alloc_simple(Large(1 << 40));

        assert!(small.equals(large).unwrap());
        assert!(large.equals(small).unwrap());
        assert!(!small.equals(larger).unwrap());
        assert!(!small.equals(heap.alloc(1i32).to_value()).unwrap());
        assert_eq!(Ordering::Less, small.compare(larger).unwrap());
        assert_eq!(hash_of(small).unwrap(), hash_of(large).unwrap());

        let two = heap.alloc(2i32).to_value();
        assert_eq!("3", small.to_value().at(two, heap).unwrap().to_str());
        assert!(larger.to_value().is_in(small.to_value()).unwrap());
        assert!(!small.to_value().is_in(larger.to_value()).unwrap());
        let zero = heap.alloc_simple(Small(0)).to_value();
        let e = small.to_value().is_in(zero).unwrap_err();
        assert!(e.to_string().contains("negative number"), "{e}");
        assert_eq!(1, large.to_value().length().unwrap());
    });
}

/// A type that hashes and compares the values it holds through their handles: without a
/// `Result` where the part's type always hashes.
#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagablePanic
)]
#[display("holder")]
struct Holder {
    name: StringValue<'static>,
    flag: ValueOf<'static, Either<bool, StarlarkStr>>,
}
starlark_simple_value!(Holder);

#[starlark_value(type = "holder")]
impl<'v> StarlarkValue<'v> for Holder {
    fn equals(&self, other: &Self) -> starlark::Result<bool> {
        Ok(self.name.try_equals(other.name)? && self.flag.try_equals(other.flag)?)
    }

    fn write_hash(&self, hasher: &mut StarlarkHasher) {
        self.name.write_hash(hasher);
        self.flag.write_hash(hasher);
    }
}

#[test]
fn test_parts_through_their_handles() {
    assert_always_hashes::<Holder>();
    let holder = |name: StringValue<'static>, flag: Value<'static>| Holder {
        name,
        flag: ValueOf::new(flag).unwrap(),
    };
    Heap::temp(|heap| {
        let a = heap.alloc_simple(holder(const_frozen_string!("a"), Value::new_bool(true)));
        let same = heap.alloc_simple(holder(const_frozen_string!("a"), Value::new_bool(true)));
        let other_name =
            heap.alloc_simple(holder(const_frozen_string!("b"), Value::new_bool(true)));
        let other_flag = heap.alloc_simple(holder(
            const_frozen_string!("a"),
            const_frozen_string!("true").to_value(),
        ));

        assert!(a.equals(same).unwrap());
        assert!(!a.equals(other_name).unwrap());
        assert!(!a.equals(other_flag).unwrap());
        assert_eq!(hash_of(a).unwrap(), hash_of(same).unwrap());
        assert_ne!(hash_of(a).unwrap(), hash_of(other_name).unwrap());
        assert_ne!(hash_of(a).unwrap(), hash_of(other_flag).unwrap());
    });
}

/// A handle hashes as the value does, with a `Result` for a type that may not hash.
#[test]
fn test_handle_hash() {
    Heap::temp(|heap| {
        let s: StringValue = heap.alloc("s");
        let mut by_handle = StarlarkHasher::new();
        s.write_hash(&mut by_handle);
        let mut by_try = StarlarkHasher::new();
        s.try_write_hash(&mut by_try).unwrap();
        assert_eq!(hash_of(s.to_value()).unwrap(), by_handle.finish());
        assert_eq!(hash_of(s.to_value()).unwrap(), by_try.finish());

        let list = |items| -> ValueOf<ListType<Value>> {
            ValueOf::new(heap.alloc(AllocList::<Vec<i32>>(items)).to_value()).unwrap()
        };
        let e = list(vec![1])
            .try_write_hash(&mut StarlarkHasher::new())
            .unwrap_err();
        assert!(e.to_string().contains("not hashable"), "{e}");
        assert!(list(vec![1]).try_equals(list(vec![1])).unwrap());
        assert!(!list(vec![1]).try_equals(list(vec![2])).unwrap());
    });
}

/// The operators typed: an operand of the type itself, a converted one, a union, any value;
/// results that are made, and results that may fail; with the heap and without it. `floor_div`
/// and `bit_not` are written as the trait declares them.
#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("{}s", _0)]
struct Seconds(i32);
starlark_simple_value!(Seconds);

#[derive(Debug, thiserror::Error)]
#[error("a time span is not negative")]
struct NegativeSpan;

#[starlark_value(type = "seconds")]
impl<'v> StarlarkValue<'v> for Seconds {
    fn add(&self, other: &Self) -> Seconds {
        Seconds(self.0 + other.0)
    }

    fn radd(&self, minutes: &str) -> starlark::Result<Seconds> {
        let minutes: i32 = minutes.parse().map_err(starlark::Error::new_other)?;
        Ok(Seconds(minutes * 60 + self.0))
    }

    fn mul(&self, times: i32, _heap: Heap<'v>) -> Seconds {
        Seconds(self.0 * times)
    }

    fn rmul(&self, times: i32) -> Seconds {
        Seconds(times * self.0)
    }

    fn sub(&self, other: &Self) -> starlark::Result<Seconds> {
        if other.0 > self.0 {
            return Err(starlark::Error::new_other(NegativeSpan));
        }
        Ok(Seconds(self.0 - other.0))
    }

    fn div(&self, by: Either<&Self, i32>, _heap: Heap<'v>) -> Either<f64, Seconds> {
        match by {
            Either::Left(other) => Either::Left(self.0 as f64 / other.0 as f64),
            Either::Right(parts) => Either::Right(Seconds(self.0 / parts)),
        }
    }

    fn percent(&self, unit: Value<'v>, heap: Heap<'v>) -> StringValue<'v> {
        heap.alloc_str(&format!("{}{}", self.0, unit.to_str()))
    }

    /// Without the heap this is not the trait's signature, whatever its types are.
    fn bit_xor(&self, other: Value<'v>) -> starlark::Result<Value<'v>> {
        Ok(other)
    }

    fn floor_div(&self, other: Value<'v>, heap: Heap<'v>) -> starlark::Result<Value<'v>> {
        match other.unpack_i32() {
            Some(parts) => Ok(heap.alloc(Seconds(self.0 / parts)).to_value()),
            None => Ok(Value::new_none()),
        }
    }

    fn minus(&self) -> starlark::Result<Seconds> {
        if self.0 > 0 {
            return Err(starlark::Error::new_other(NegativeSpan));
        }
        Ok(Seconds(0))
    }

    fn plus(&self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc(self.0).to_value()
    }

    fn bit_not(&self, heap: Heap<'v>) -> starlark::Result<Value<'v>> {
        Ok(heap.alloc(Seconds(!self.0)).to_value())
    }
}

/// A type with no operators, named as `Seconds` is: its errors are the defaults'.
#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("no_operators")]
struct NoOperators;
starlark_simple_value!(NoOperators);

#[starlark_value(type = "seconds")]
impl<'v> StarlarkValue<'v> for NoOperators {}

#[test]
fn test_operators() {
    Heap::temp(|heap| {
        let seconds = |n: i32| heap.alloc_simple(Seconds(n)).to_value();
        let int = |n: i32| heap.alloc(n).to_value();
        let str = |s: &str| heap.alloc(s).to_value();
        let shown = |v: starlark::Result<Value>| v.unwrap().to_str();

        assert_eq!("5s", shown(seconds(2).add(seconds(3), heap)));
        assert_eq!("62s", shown(str("1").add(seconds(2), heap)));
        assert_eq!("6s", shown(seconds(2).mul(int(3), heap)));
        assert_eq!("6s", shown(int(3).mul(seconds(2), heap)));
        assert_eq!("1s", shown(seconds(3).sub(seconds(2), heap)));
        assert_eq!("1.5", shown(seconds(3).div(seconds(2), heap)));
        assert_eq!("2s", shown(seconds(6).div(int(3), heap)));
        assert_eq!("6min", shown(seconds(6).percent(str("min"), heap)));
        assert_eq!("2s", shown(seconds(6).floor_div(int(3), heap)));
        assert_eq!("3", shown(seconds(6).bit_xor(int(3), heap)));
        assert_eq!("0s", shown(seconds(0).minus(heap)));
        assert_eq!("6", shown(seconds(6).plus(heap)));
        assert_eq!("-1s", shown(seconds(0).bit_not(heap)));
    });
}

/// A value that is not of the operand's type gets what the trait's default gives: for `add`
/// and `mul` the other operand's turn and then the error of `+` and `*`, for the others the
/// default's error.
#[test]
fn test_operator_operand_of_another_type() {
    Heap::temp(|heap| {
        let seconds = heap.alloc_simple(Seconds(1)).to_value();
        let default = heap.alloc_simple(NoOperators).to_value();
        let none = Value::new_none();
        let error = |v: starlark::Result<Value>| v.unwrap_err().to_string();

        assert_eq!(
            error(default.add(none, heap)),
            error(seconds.add(none, heap))
        );
        assert_eq!(
            error(none.add(default, heap)),
            error(none.add(seconds, heap))
        );
        assert_eq!(
            error(default.mul(none, heap)),
            error(seconds.mul(none, heap))
        );
        assert_eq!(
            error(none.mul(default, heap)),
            error(none.mul(seconds, heap))
        );
        assert_eq!(
            error(default.sub(none, heap)),
            error(seconds.sub(none, heap))
        );
        assert_eq!(
            error(default.div(none, heap)),
            error(seconds.div(none, heap))
        );
        assert!(
            error(seconds.sub(none, heap))
                .contains("Operation `-` not supported for types `seconds` and `NoneType`"),
        );
    });
}

/// A value of the operand's type that does not convert is the conversion's error, and a body's
/// error is the body's.
#[test]
fn test_operator_errors() {
    Heap::temp(|heap| {
        let seconds = |n: i32| heap.alloc_simple(Seconds(n)).to_value();
        let big = heap.alloc(1i64 << 40).to_value();
        let error = |v: starlark::Result<Value>| v.unwrap_err().to_string();

        let e = error(seconds(1).mul(big, heap));
        assert!(e.contains("too big to fit in i32"), "{e}");
        let e = error(big.mul(seconds(1), heap));
        assert!(e.contains("too big to fit in i32"), "{e}");
        let e = error(seconds(1).div(big, heap));
        assert!(e.contains("too big to fit in i32"), "{e}");
        let e = error(seconds(1).sub(seconds(2), heap));
        assert!(e.contains("not negative"), "{e}");
        let e = error(heap.alloc("one").to_value().add(seconds(1), heap));
        assert!(e.contains("invalid digit"), "{e}");
        let e = error(seconds(1).minus(heap));
        assert!(e.contains("not negative"), "{e}");
    });
}

/// The typechecker is told what the signatures say: the operand's type and the result's for a
/// typed operator, any operand and an unknown result for one written as the trait declares it,
/// and nothing for an operator the type does not have.
#[test]
fn test_operator_signatures() {
    fn signature(operand: Ty, result: Ty) -> Vec<OperatorSignature> {
        vec![OperatorSignature { operand, result }]
    }
    let seconds = Ty::starlark_value::<Seconds>;
    let binary = <Seconds as StarlarkType>::bin_op_signatures;
    let reflected = <Seconds as StarlarkType>::rbin_op_signatures;
    let unary = <Seconds as StarlarkType>::un_op_result;

    assert_eq!(signature(seconds(), seconds()), binary(TypingBinOp::Add));
    assert_eq!(signature(Ty::int(), seconds()), binary(TypingBinOp::Mul));
    assert_eq!(signature(seconds(), seconds()), binary(TypingBinOp::Sub));
    assert_eq!(
        signature(
            Ty::union2(seconds(), Ty::int()),
            Ty::union2(Ty::float(), seconds()),
        ),
        binary(TypingBinOp::Div),
    );
    assert_eq!(
        signature(Ty::any(), Ty::string()),
        binary(TypingBinOp::Percent)
    );
    assert_eq!(
        vec![OperatorSignature::any()],
        binary(TypingBinOp::FloorDiv)
    );
    assert_eq!(Vec::<OperatorSignature>::new(), binary(TypingBinOp::BitOr));
    assert_eq!(Vec::<OperatorSignature>::new(), binary(TypingBinOp::Less));

    assert_eq!(
        signature(Ty::string(), seconds()),
        reflected(TypingBinOp::Add)
    );
    assert_eq!(signature(Ty::int(), seconds()), reflected(TypingBinOp::Mul));
    assert_eq!(Vec::<OperatorSignature>::new(), reflected(TypingBinOp::Sub));

    assert_eq!(Some(seconds()), unary(TypingUnOp::Minus));
    assert_eq!(Some(Ty::any()), unary(TypingUnOp::Plus));
    assert_eq!(Some(seconds()), unary(TypingUnOp::BitNot));
}

/// A typed `compare` says what `<` takes; one written as the trait declares it takes anything.
#[test]
fn test_compare_signature() {
    assert_eq!(
        vec![OperatorSignature {
            operand: Ty::starlark_value::<Plain>(),
            result: Ty::bool(),
        }],
        <Plain as StarlarkType>::bin_op_signatures(TypingBinOp::Less),
    );
    assert_eq!(
        vec![OperatorSignature {
            operand: Ty::int(),
            result: Ty::bool(),
        }],
        <Count as StarlarkType>::bin_op_signatures(TypingBinOp::Less),
    );
    assert_eq!(
        vec![OperatorSignature::any()],
        <AnyOperand as StarlarkType>::bin_op_signatures(TypingBinOp::Less),
    );
}

/// Typed operators in a generic impl and on a view: the signatures are read where `Self` and
/// the impl's parameters mean what they do in the body.
#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("scaled({:?})", _0)]
struct Scaled<T: Tag>(T::Key);

#[starlark_value(type = "scaled")]
impl<'v, T: Tag> StarlarkValue<'v> for Scaled<T>
where
    Self: HasTyVTable + ProvidesStaticType<'v>,
{
    fn bit_and(&self, other: &Scaled<T>) -> bool {
        self.0 == other.0
    }

    fn mul(&self, times: i32, _heap: Heap<'v>) -> i32 {
        times * T::SALT as i32
    }

    fn minus(&self, _heap: Heap<'v>) -> i32 {
        -(T::SALT as i32)
    }
}

crate::register_avalue_simple_frozen!(Scaled<ByNumber>);

#[starlark_value(type = "length")]
impl<'v> StarlarkType<'v> for LengthView<'v> {
    fn add(&self, other: LengthView<'v>) -> Metres {
        Metres(self.get() + other.get())
    }

    fn sub(&self, other: Self, _heap: Heap<'v>) -> starlark::Result<Metres> {
        match self.get().checked_sub(other.get()) {
            Some(metres) => Ok(Metres(metres)),
            None => Err(starlark::Error::new_other(NegativeSpan)),
        }
    }
}

#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("{}m", _0)]
struct Metres(u64);

#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("{}km", _0)]
struct Kilometres(u64);

#[derive(Debug, Clone, Copy, StarlarkRepresentations)]
enum LengthView<'v> {
    Metres(ValueTyped<'v, Metres>),
    Kilometres(ValueTyped<'v, Kilometres>),
}

impl<'v> LengthView<'v> {
    fn get(self) -> u64 {
        match self {
            LengthView::Metres(x) => x.0,
            LengthView::Kilometres(x) => x.0 * 1000,
        }
    }
}

#[starlark_value]
impl<'v> StarlarkValue<'v> for Metres {
    type Type = LengthView<'v>;
}

#[starlark_value]
impl<'v> StarlarkValue<'v> for Kilometres {
    type Type = LengthView<'v>;
}

#[test]
fn test_operators_generic_impl() {
    Heap::temp(|heap| {
        let one = heap.alloc_simple(Scaled::<ByNumber>(1)).to_value();
        let two = heap.alloc_simple(Scaled::<ByNumber>(2)).to_value();
        let shown = |v: starlark::Result<Value>| v.unwrap().to_str();

        assert_eq!("False", shown(one.bit_and(two, heap)));
        assert_eq!("True", shown(one.bit_and(one, heap)));
        assert_eq!("14", shown(one.mul(heap.alloc(2i32).to_value(), heap)));
        assert_eq!("-7", shown(one.minus(heap)));
    });
    assert_eq!(
        vec![OperatorSignature {
            operand: Ty::starlark_value::<Scaled<ByNumber>>(),
            result: Ty::bool(),
        }],
        <Scaled<ByNumber> as StarlarkType>::bin_op_signatures(TypingBinOp::BitAnd),
    );
    assert_eq!(
        Some(Ty::int()),
        <Scaled<ByNumber> as StarlarkType>::un_op_result(TypingUnOp::Minus),
    );
}

#[test]
fn test_operators_view() {
    Heap::temp(|heap| {
        let metres = heap.alloc_simple(Metres(1)).to_value();
        let kilometres = heap.alloc_simple(Kilometres(1)).to_value();
        let shown = |v: starlark::Result<Value>| v.unwrap().to_str();

        assert_eq!("1001m", shown(metres.add(kilometres, heap)));
        assert_eq!("1001m", shown(kilometres.add(metres, heap)));
        assert_eq!("999m", shown(kilometres.sub(metres, heap)));
        let e = metres.sub(kilometres, heap).unwrap_err().to_string();
        assert!(e.contains("not negative"), "{e}");
        let e = metres.sub(Value::new_none(), heap).unwrap_err().to_string();
        assert!(
            e.contains("Operation `-` not supported for types `length` and `NoneType`"),
            "{e}"
        );
    });
    let length = Ty::starlark_value::<Metres>();
    assert_eq!(
        vec![OperatorSignature {
            operand: length.clone(),
            result: length,
        }],
        <LengthView as StarlarkType>::bin_op_signatures(TypingBinOp::Add),
    );
}

/// `at`, `is_in` and `length` typed, without a heap and without a `Result` where there is
/// nothing to fail.
#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("word({})", _0)]
struct Word(String);
starlark_simple_value!(Word);

#[derive(Debug, thiserror::Error)]
#[error("no letter {0} in the word")]
struct NoLetter(i32);

#[starlark_value(type = "word")]
impl<'v> StarlarkValue<'v> for Word {
    fn at(&self, index: i32) -> starlark::Result<char> {
        usize::try_from(index)
            .ok()
            .and_then(|i| self.0.chars().nth(i))
            .ok_or_else(|| starlark::Error::new_other(NoLetter(index)))
    }

    fn is_in(&self, other: &str) -> bool {
        self.0.contains(other)
    }

    fn length(&self) -> i32 {
        self.0.len() as i32
    }
}

#[test]
fn test_at_is_in_length() {
    Heap::temp(|heap| {
        let word = heap.alloc_simple(Word("word".to_owned())).to_value();
        let int = |n: i32| heap.alloc(n).to_value();
        let str = |s: &str| heap.alloc(s).to_value();

        assert_eq!("o", word.at(int(1), heap).unwrap().to_str());
        assert!(word.is_in(str("or")).unwrap());
        assert!(!word.is_in(str("ro")).unwrap());
        assert_eq!(4, word.length().unwrap());

        // The body's error.
        let e = word.at(int(9), heap).unwrap_err();
        assert!(e.to_string().contains("no letter 9 in the word"), "{e}");
        // A value of the index's type that does not convert.
        let e = word
            .at(heap.alloc(1i64 << 40).to_value(), heap)
            .unwrap_err();
        assert!(e.to_string().contains("too big to fit in i32"), "{e}");
    });
}

/// A value that is not of the index's or the operand's type is the error of a conversion that
/// expected one, not what the trait's default answers for a type without `[]` or `in`.
#[test]
fn test_at_is_in_operand_of_another_type() {
    Heap::temp(|heap| {
        let word = heap.alloc_simple(Word("word".to_owned())).to_value();
        let default = heap.alloc_simple(NoOperators).to_value();
        let int = heap.alloc(1i32).to_value();
        let str = heap.alloc("x").to_value();

        let e = word.at(str, heap).unwrap_err().to_string();
        assert!(
            e.contains("Expected `int`, but got `string (repr: \"x\")`"),
            "{e}"
        );
        let e = word.is_in(int).unwrap_err().to_string();
        assert!(e.contains("Expected `str`, but got `int (repr: 1)`"), "{e}");

        let e = default.at(str, heap).unwrap_err().to_string();
        assert!(e.contains("Operation `[]` not supported"), "{e}");
        let e = default.is_in(int).unwrap_err().to_string();
        assert!(e.contains("Operation `in` not supported"), "{e}");
    });
}

/// The typechecker is told the index's type and the result's for a typed `at`, and the
/// operand's for a typed `is_in`.
#[test]
fn test_at_is_in_signatures() {
    assert_eq!(
        Some(OperatorSignature {
            operand: Ty::int(),
            result: Ty::string(),
        }),
        <Word as StarlarkType>::at_signature(),
    );
    assert_eq!(
        vec![OperatorSignature {
            operand: Ty::string(),
            result: Ty::bool(),
        }],
        <Word as StarlarkType>::bin_op_signatures(TypingBinOp::In),
    );
    let sized = Ty::starlark_value::<Small>();
    assert_eq!(
        Some(OperatorSignature {
            operand: Ty::int(),
            result: Ty::any(),
        }),
        <SizedView as StarlarkType>::at_signature(),
    );
    assert_eq!(
        vec![OperatorSignature {
            operand: sized,
            result: Ty::bool(),
        }],
        <SizedView as StarlarkType>::bin_op_signatures(TypingBinOp::In),
    );
    assert_eq!(None, <Seconds as StarlarkType>::at_signature());
}
