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
use std::convert::Infallible;
use std::ops::Add;
use std::ops::Mul;
use std::ops::Sub;

use dupe::Dupe;
use either::Either;
use num_bigint::BigInt;
use num_traits::Signed;

use crate::collections::StarlarkHashValue;
use crate::values::AllocFrozenValue;
use crate::values::AllocValue;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::UnpackValue;
use crate::values::ValueOf;
use crate::values::ValueTyped;
use crate::values::types::float::StarlarkFloat;
use crate::values::types::int::int_or_big::StarlarkInt;
use crate::values::types::int::int_or_big::StarlarkIntRef;
use crate::values::types::int::view::IntView;

#[derive(Debug, thiserror::Error)]
enum NumError {
    #[error("float division by zero: {0} / {1}")]
    DivisionByZero(Num, Num),
    #[error("int too large to convert to float")]
    IntTooLargeForFloat,
}

/// [`NumRef`] represents a numerical value that can be unpacked from a [`Value`].
///
/// It's an intermediate representation that facilitates conversions between
/// numerical types and helps in implementation of arithmetical operations
/// between them.
#[derive(Clone, Debug, Dupe, Copy)]
#[doc(hidden)]
pub enum NumRef<'v> {
    Int(StarlarkIntRef<'v>),
    // `StarlarkFloat` not `f64` here because `f64` unpacks from `int` too.
    Float(StarlarkFloat),
}

/// A conversion from `int | float`, not a type expression: both variants are by value.
impl<'v> UnpackValue<'v> for NumRef<'v> {
    type Type = Either<IntView<'v>, StarlarkFloat>;
    type Error = Infallible;

    #[inline]
    fn unpack(
        _value: ValueOf<'v, Self::Type>,
        get: Either<IntView<'v>, ValueTyped<'v, StarlarkFloat>>,
    ) -> Result<Self, Infallible> {
        Ok(match get {
            Either::Left(int) => NumRef::Int(int.as_int_ref()),
            Either::Right(float) => NumRef::Float(*float.as_ref()),
        })
    }
}

#[derive(Debug, derive_more::Display)]
#[doc(hidden)]
pub enum Num {
    Int(StarlarkInt),
    Float(f64),
}

impl<'v> AllocValue<'v> for Num {
    type Type = Either<IntView<'v>, StarlarkFloat>;

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        match self {
            Num::Int(i) => ValueOf::left(i.alloc_value(heap)),
            Num::Float(f) => ValueOf::right(f.alloc_value(heap)),
        }
    }
}

impl<'fv> AllocFrozenValue<'fv> for Num {
    type Type = Either<IntView<'fv>, StarlarkFloat>;

    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        match self {
            Num::Int(i) => ValueOf::left(i.alloc_frozen_value(heap)),
            Num::Float(f) => ValueOf::right(f.alloc_frozen_value(heap)),
        }
    }
}

impl<'v> NumRef<'v> {
    /// Get underlying value as float
    pub(crate) fn as_float(&self) -> f64 {
        match self {
            Self::Int(i) => i.to_f64(),
            Self::Float(f) => f.0,
        }
    }

    /// Get underlying value as float, failing if it is an int too large to be
    /// represented as a finite float.
    pub(crate) fn as_finite_float(&self) -> anyhow::Result<f64> {
        let f = self.as_float();
        if f.is_infinite() && matches!(self, Self::Int(_)) {
            return Err(NumError::IntTooLargeForFloat.into());
        }
        Ok(f)
    }

    pub(crate) fn f64_to_i32_exact(f: f64) -> Option<i32> {
        let i = f as i32;
        if i as f64 == f { Some(i) } else { None }
    }

    /// Get underlying value as int (if it can be precisely expressed as int)
    pub(crate) fn as_int(&self) -> Option<i32> {
        match self {
            Self::Int(i) => i.to_i32(),
            Self::Float(f) => Self::f64_to_i32_exact(f.0),
        }
    }

    /// Get hash of the underlying number
    pub(crate) fn get_hash_64(self) -> u64 {
        fn float_hash(f: f64) -> u64 {
            if f.is_nan() {
                // all possible NaNs should hash to the same value
                0
            } else if f.is_infinite() {
                u64::MAX
            } else if f == 0.0 {
                // Both 0.0 and -0.0 need the same hash, but are both equal to 0.0
                0.0f64.to_bits()
            } else {
                f.to_bits()
            }
        }

        match (self.as_int(), self) {
            // equal ints and floats should have the same hash
            (Some(i), _) => i as u64,
            (None, Self::Float(f)) => float_hash(f.0),
            (None, Self::Int(StarlarkIntRef::Small(i))) => {
                // shouldn't happen - as_int() should have resulted in an int
                i.to_i32() as u64
            }
            (None, Self::Int(StarlarkIntRef::Big(b))) => {
                // Not perfect, but OK: `1000000000000000000000003` and `1000000000000000000000005`
                // flush to the same float, and neither is exact float,
                // so we could use better hash for such numbers.
                float_hash(b.to_f64())
            }
        }
    }

    pub(crate) fn get_hash(self) -> StarlarkHashValue {
        StarlarkHashValue::hash_64(self.get_hash_64())
    }

    fn to_owned(self) -> Num {
        match self {
            NumRef::Int(i) => Num::Int(i.to_owned()),
            NumRef::Float(f) => Num::Float(f.0),
        }
    }

    pub(crate) fn div(self, other: NumRef) -> anyhow::Result<f64> {
        let a = self.as_finite_float()?;
        let b = other.as_finite_float()?;
        if b == 0.0 {
            Err(NumError::DivisionByZero(self.to_owned(), other.to_owned()).into())
        } else {
            Ok(a / b)
        }
    }

    pub(crate) fn floor_div(self, other: NumRef) -> anyhow::Result<Num> {
        if let (NumRef::Int(a), NumRef::Int(b)) = (self, other) {
            a.floor_div(b).map(Num::Int)
        } else {
            StarlarkFloat::floor_div_impl(self.as_finite_float()?, other.as_finite_float()?)
                .map(Num::Float)
        }
    }

    pub(crate) fn percent(self, other: NumRef) -> anyhow::Result<Num> {
        if let (NumRef::Int(a), NumRef::Int(b)) = (self, other) {
            a.percent(b).map(Num::Int)
        } else {
            StarlarkFloat::percent_impl(self.as_finite_float()?, other.as_finite_float()?)
                .map(Num::Float)
        }
    }
}

fn cmp_int_float(a: StarlarkIntRef, b: f64) -> Ordering {
    match a {
        StarlarkIntRef::Small(a) => StarlarkFloat::compare_impl(a.to_f64(), b),
        StarlarkIntRef::Big(a) => cmp_big_float(a.get(), b),
    }
}

fn cmp_big_float(a: &BigInt, b: f64) -> Ordering {
    if !b.is_finite() {
        return StarlarkFloat::compare_impl(0.0, b);
    }
    let a_negative = a.is_negative();
    if a_negative != (b < 0.0) {
        return if a_negative {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    let ord = cmp_magnitude_float(a, b.abs());
    if a_negative { ord.reverse() } else { ord }
}

fn cmp_magnitude_float(a: &BigInt, b: f64) -> Ordering {
    let raw = b.to_bits();
    let a_bits = a.bits();
    let whole_bits = (raw >> 52).saturating_sub(1022);
    if a_bits != whole_bits {
        return a_bits.cmp(&whole_bits);
    }
    let mut digits = a.iter_u64_digits();
    let Some(top) = digits.next_back() else {
        return StarlarkFloat::compare_impl(0.0, b);
    };
    let next = digits.next_back().unwrap_or(0);
    let shift = top.leading_zeros();
    let (a_top, a_rest) = if shift == 0 {
        (top, next)
    } else {
        ((top << shift) | (next >> (64 - shift)), next << shift)
    };
    let b_top = (raw << 11) | (1 << 63);
    a_top.cmp(&b_top).then_with(|| {
        if a_rest != 0 || digits.any(|digit| digit != 0) {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    })
}

impl<'v> From<f64> for NumRef<'v> {
    fn from(f: f64) -> Self {
        Self::Float(StarlarkFloat(f))
    }
}

/// This is total eq per starlark spec, not Rust's partial eq.
impl<'v> PartialEq for NumRef<'v> {
    fn eq(&self, other: &Self) -> bool {
        if let (NumRef::Int(a), NumRef::Int(b)) = (self, other) {
            a == b
        } else {
            self.cmp(other) == Ordering::Equal
        }
    }
}

impl<'v> Eq for NumRef<'v> {}

impl<'v> PartialOrd for NumRef<'v> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<'v> Ord for NumRef<'v> {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (NumRef::Int(a), NumRef::Int(b)) => a.cmp(b),
            (NumRef::Float(a), NumRef::Float(b)) => StarlarkFloat::compare_impl(a.0, b.0),
            (NumRef::Int(a), NumRef::Float(b)) => cmp_int_float(*a, b.0),
            (NumRef::Float(a), NumRef::Int(b)) => cmp_int_float(*b, a.0).reverse(),
        }
    }
}

impl<'v> Add for NumRef<'v> {
    type Output = anyhow::Result<Num>;

    fn add(self, rhs: Self) -> Self::Output {
        if let (NumRef::Int(a), NumRef::Int(b)) = (self, rhs) {
            return Ok(Num::Int(a + b));
        }
        Ok(Num::Float(self.as_finite_float()? + rhs.as_finite_float()?))
    }
}

impl<'v> Sub for NumRef<'v> {
    type Output = anyhow::Result<Num>;

    fn sub(self, rhs: Self) -> Self::Output {
        if let (NumRef::Int(a), NumRef::Int(b)) = (self, rhs) {
            return Ok(Num::Int(a - b));
        }
        Ok(Num::Float(self.as_finite_float()? - rhs.as_finite_float()?))
    }
}

impl<'v> Mul for NumRef<'v> {
    type Output = anyhow::Result<Num>;

    fn mul(self, rhs: Self) -> Self::Output {
        if let (NumRef::Int(a), NumRef::Int(b)) = (self, rhs) {
            return Ok(Num::Int(a * b));
        }
        Ok(Num::Float(self.as_finite_float()? * rhs.as_finite_float()?))
    }
}

#[cfg(test)]
mod tests {
    use num_traits::FromPrimitive;

    use super::*;
    use crate::assert;
    use crate::values::StringValue;
    use crate::values::Value;
    use crate::values::types::int::inline_int::InlineInt;

    #[test]
    fn test_from_value() {
        assert!(
            NumRef::unpack_value(Value::new_bool(true))
                .unwrap()
                .is_none()
        );
        assert!(
            NumRef::unpack_value(Value::new_bool(false))
                .unwrap()
                .is_none()
        );
        assert!(
            NumRef::unpack_value(StringValue::default().to_value())
                .unwrap()
                .is_none()
        );
        assert!(NumRef::unpack_value(Value::new_none()).unwrap().is_none());

        assert_eq!(
            NumRef::unpack_value(Value::testing_new_int(0))
                .unwrap()
                .unwrap()
                .as_int(),
            Some(0)
        );
        assert_eq!(
            NumRef::unpack_value(Value::testing_new_int(42))
                .unwrap()
                .unwrap()
                .as_int(),
            Some(42)
        );
        assert_eq!(
            NumRef::unpack_value(Value::testing_new_int(-42))
                .unwrap()
                .unwrap()
                .as_int(),
            Some(-42)
        );
    }

    #[test]
    fn test_conversion_to_float() {
        assert_eq!(
            NumRef::Int(StarlarkIntRef::Small(InlineInt::ZERO)).as_float(),
            0.0
        );
        assert_eq!(
            NumRef::Int(StarlarkIntRef::Small(InlineInt::MAX)).as_float(),
            InlineInt::MAX.to_f64()
        );
        assert_eq!(
            NumRef::Int(StarlarkIntRef::Small(InlineInt::MIN)).as_float(),
            InlineInt::MIN.to_f64()
        );

        assert_eq!(NumRef::Float(StarlarkFloat(0.0)).as_float(), 0.0);
        assert!(NumRef::Float(StarlarkFloat(f64::NAN)).as_float().is_nan());
    }

    #[test]
    fn test_conversion_to_int() {
        assert_eq!(
            NumRef::Int(StarlarkIntRef::Small(InlineInt::testing_new(0))).as_int(),
            Some(0)
        );
        assert_eq!(
            NumRef::Int(StarlarkIntRef::Small(InlineInt::testing_new(42))).as_int(),
            Some(42)
        );
        assert_eq!(
            NumRef::Int(StarlarkIntRef::Small(InlineInt::testing_new(-42))).as_int(),
            Some(-42)
        );

        assert_eq!(NumRef::Float(StarlarkFloat(0_f64)).as_int(), Some(0));
        assert_eq!(NumRef::Float(StarlarkFloat(42_f64)).as_int(), Some(42));
        assert_eq!(NumRef::Float(StarlarkFloat(-42_f64)).as_int(), Some(-42));

        assert_eq!(
            NumRef::Float(StarlarkFloat(i32::MIN as f64)).as_int(),
            Some(i32::MIN)
        );
        assert_eq!(
            NumRef::Float(StarlarkFloat(i32::MAX as f64)).as_int(),
            Some(i32::MAX)
        );

        assert_eq!(NumRef::Float(StarlarkFloat(42.75)).as_int(), None);
        assert_eq!(NumRef::Float(StarlarkFloat(-42.75)).as_int(), None);
        assert_eq!(NumRef::Float(StarlarkFloat(f64::NAN)).as_int(), None);
        assert_eq!(NumRef::Float(StarlarkFloat(f64::INFINITY)).as_int(), None);
        assert_eq!(
            NumRef::Float(StarlarkFloat(f64::NEG_INFINITY)).as_int(),
            None
        );
    }

    #[test]
    fn test_hashing() {
        assert_eq!(
            NumRef::Int(StarlarkIntRef::Small(InlineInt::testing_new(0))).get_hash_64(),
            NumRef::Float(StarlarkFloat(0.0)).get_hash_64()
        );
        assert_eq!(
            NumRef::Int(StarlarkIntRef::Small(InlineInt::testing_new(42))).get_hash_64(),
            NumRef::Float(StarlarkFloat(42.0)).get_hash_64()
        );

        assert_eq!(
            NumRef::Float(StarlarkFloat(f64::INFINITY + f64::NEG_INFINITY)).get_hash_64(),
            NumRef::Float(StarlarkFloat(f64::NAN)).get_hash_64()
        );
        assert_eq!(
            NumRef::Float(StarlarkFloat("0.25".parse().unwrap())).get_hash_64(),
            NumRef::Float(StarlarkFloat("25e-2".parse().unwrap())).get_hash_64()
        );

        let x = 1u64 << 55;
        assert_eq!(x as f64 as u64, x, "Self-check");
        assert_eq!(
            NumRef::Float(StarlarkFloat(x as f64)).get_hash_64(),
            NumRef::Int(StarlarkInt::from(BigInt::from(x)).as_ref()).get_hash_64(),
        )
    }

    #[test]
    fn test_eq() {
        assert_eq!(
            NumRef::Float(StarlarkFloat(f64::NAN)),
            NumRef::Float(StarlarkFloat(f64::NAN))
        );
        assert_eq!(
            NumRef::Float(StarlarkFloat(f64::INFINITY)),
            NumRef::Float(StarlarkFloat(f64::INFINITY))
        );
        assert_eq!(
            NumRef::Int(StarlarkIntRef::Small(InlineInt::testing_new(10))),
            NumRef::Float(StarlarkFloat(10.0))
        );
    }

    #[test]
    fn test_int_float_comparison_is_exact() {
        assert::all_true(
            r#"
float((1 << 53) + 1) != (1 << 53) + 1
float((1 << 53) + 1) == 1 << 53
float((1 << 53) + 1) < (1 << 53) + 1
(1 << 53) + 1 > float((1 << 53) + 1)
float(1 << 100) == 1 << 100
1 << 100 == float(1 << 100)
float(1 << 100) != (1 << 100) + 1
(1 << 100) + 1 > float(1 << 100)
(1 << 100) - 1 < float(1 << 100)
float(1 << 100) < (1 << 100) + 1
-(1 << 100) - 1 < -float(1 << 100)
-(1 << 100) + 1 > -float(1 << 100)
1 << 40 < (1 << 40) + 0.5
(1 << 40) + 1 > (1 << 40) + 0.5
-(1 << 40) > -(1 << 40) - 0.5
-(1 << 40) - 1 < -(1 << 40) - 0.5
1 << 40 == (1 << 40) + 0.0
"#,
        );
    }

    #[test]
    fn test_int_compared_to_non_finite_float() {
        assert::all_true(
            r#"
1 << 2000 < float("inf")
1 << 2000 != float("inf")
-(1 << 2000) > float("-inf")
-(1 << 2000) != float("-inf")
1 << 2000 < float("nan")
1 << 100 < float("inf")
1 << 100 > float("-inf")
1 < float("inf")
1 > float("-inf")
1 < float("nan")
"#,
        );
    }

    #[test]
    fn test_int_float_equality_is_transitive() {
        assert::all_true(
            r#"
len(dict([(1 << 100, 1), ((1 << 100) + 1, 2), (float(1 << 100), 3)])) == 2
len(dict([(float(1 << 100), 1), (1 << 100, 2), ((1 << 100) + 1, 3)])) == 2
len(set([float(1 << 100), 1 << 100, (1 << 100) + 1])) == 2
[type(x) for x in sorted([(1 << 100) + 1, float(1 << 100), (1 << 100) - 1])] == ["int", "float", "int"]
"#,
        );
    }

    #[test]
    fn test_cmp_big_float_matches_conversion_to_big_int() {
        fn reference(a: &BigInt, b: f64) -> Ordering {
            let whole = BigInt::from_f64(b.trunc()).unwrap();
            a.cmp(&whole)
                .then(StarlarkFloat::compare_impl(b.trunc(), b))
        }

        let mantissas: [u64; 6] = [
            1,
            3,
            0x0001_2345_6789_abcd,
            1 << 52,
            (1 << 52) + 1,
            (1 << 53) - 1,
        ];
        for exponent in -1074..=971 {
            for mantissa in mantissas {
                for sign in [1.0, -1.0] {
                    let b = sign * mantissa as f64 * 2f64.powi(exponent);
                    assert!(b.is_finite());
                    let whole = BigInt::from_f64(b.trunc()).unwrap();
                    let neighbours = (-2..=2).map(|delta| &whole + delta);
                    let others = [
                        -&whole,
                        &whole << 1,
                        &whole >> 1,
                        &whole << 64,
                        &whole >> 64,
                    ];
                    for a in neighbours.chain(others) {
                        assert_eq!(
                            cmp_big_float(&a, b),
                            reference(&a, b),
                            "comparing {a} with {b:e}"
                        );
                    }
                }
            }
        }
    }
}
