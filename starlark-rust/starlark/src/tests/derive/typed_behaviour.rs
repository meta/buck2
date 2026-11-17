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
