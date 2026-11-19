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

#![cfg(test)]

use crate::assert;
use crate::values::Value;
use crate::values::int::pointer_i32::PointerI32;
use crate::values::types::int::inline_int::InlineInt;

#[test]
fn test_arithmetic_operators() {
    assert::all_true(
        r#"
+1 == 1
-1 == 0 - 1
1 + 2 == 3
1 + 2.0 == 3.0
1 - 2 == -1
1 - 2.0 == -1.0
2 * 3 == 6
2 * 3.0 == 6.0
4 / 2 == 2.0
5 % 3 == 2
4 // 2 == 2
"#,
    );
}

#[test]
fn test_minus() {
    // `-i32::MIN` should overflow to `StarlarkBigInt`.
    assert::eq("2147483648", "-(-2147483647 - 1)")
}

#[test]
fn test_int_tag() {
    fn check(x: InlineInt) {
        assert_eq!(x, Value::new_int(x).unpack_inline_int().unwrap());
    }

    for x in -10..10 {
        check(InlineInt::try_from(x).ok().unwrap())
    }
    check(InlineInt::MAX);
    check(InlineInt::MIN);
}

#[test]
fn test_alignment_int_pointer() {
    assert_eq!(1, std::mem::align_of::<PointerI32>());
}

#[test]
fn test_as_avalue_dyn() {
    // `get_type` calls `as_avalue_dyn` internally.
    assert_eq!("int", Value::new_int(InlineInt::MINUS_ONE).get_type());
}

/// `int(s)` falls back to `BigInt::from_str_radix` when `s` does not fit an `i32`; that parser
/// skips `_`, which is not part of Starlark integer syntax, so such strings are rejected first.
#[test]
fn test_int_from_str_rejects_underscores() {
    assert::fail("int('1__0')", "Cannot parse");
    assert::fail("int('7_')", "Cannot parse");
    assert::fail("int('-1_0')", "Cannot parse");
    assert::fail("int('1_000', 10)", "Cannot parse");
    assert::fail("int('0x1_f', 0)", "Cannot parse");
    assert::fail("int('1_0000000000_0')", "Cannot parse");
    // The `i32` parser overflows before it reaches the underscore.
    assert::fail("int('10000000000_0')", "Cannot parse");
    assert::fail("int('_1')", "Cannot parse");
    assert::eq("int('100000000000')", "100000000000");
    assert::eq("int('-0x1f', 0)", "-31");
}
