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

// https://github.com/rust-lang/rust-clippy/issues/11142
#![allow(clippy::needless_borrow)]

use std::collections::BTreeMap;
use std::str::FromStr;

use either::Either;
use num_bigint::BigInt;
use serde_json::value::RawValue;
use starlark_derive::starlark_module;

use crate as starlark;
use crate::environment::GlobalsBuilder;
use crate::values::AllocFrozenValue;
use crate::values::AllocValue;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::dict::AllocDict;
use crate::values::dict::DictType;
use crate::values::float::StarlarkFloat;
use crate::values::list::AllocList;
use crate::values::string::str_type::StarlarkStr;
use crate::values::types::int::int_or_big::StarlarkInt;
use crate::values::types::int::view::IntView;

impl<'v, 'a> AllocValue<'v> for &'a serde_json::Number {
    type Type = Either<IntView<'v>, StarlarkFloat>;

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        if let Some(x) = self.as_u64() {
            ValueOf::left(x.alloc_value(heap))
        } else if let Some(x) = self.as_i64() {
            ValueOf::left(x.alloc_value(heap))
        } else if let Some(x) = self.as_f64() {
            ValueOf::right(x.alloc_value(heap))
        } else if let Ok(x) = BigInt::from_str(&self.to_string()) {
            ValueOf::left(StarlarkInt::from(x).alloc_value(heap))
        } else {
            panic!("Unrepresentable number: {self:?}")
        }
    }
}

impl<'v> AllocValue<'v> for serde_json::Number {
    type Type = Either<IntView<'v>, StarlarkFloat>;

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        (&self).alloc_value(heap)
    }
}

impl<'fv, 'a> AllocFrozenValue<'fv> for &'a serde_json::Number {
    type Type = Either<IntView<'fv>, StarlarkFloat>;

    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        if let Some(x) = self.as_u64() {
            ValueOf::left(x.alloc_frozen_value(heap))
        } else if let Some(x) = self.as_i64() {
            ValueOf::left(x.alloc_frozen_value(heap))
        } else if let Some(x) = self.as_f64() {
            ValueOf::right(x.alloc_frozen_value(heap))
        } else if let Ok(x) = BigInt::from_str(&self.to_string()) {
            ValueOf::left(StarlarkInt::from(x).alloc_frozen_value(heap))
        } else {
            panic!("Unrepresentable number: {self:?}")
        }
    }
}

impl<'fv> AllocFrozenValue<'fv> for serde_json::Number {
    type Type = Either<IntView<'fv>, StarlarkFloat>;

    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        (&self).alloc_frozen_value(heap)
    }
}

impl<'a, 'v> AllocValue<'v> for &'a serde_json::Map<String, serde_json::Value> {
    type Type = DictType<StarlarkStr, Value<'v>>;

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        // For some reason `IntoIter` is implemented only for `String, Value` parameters,
        // so this implementation is limited.
        AllocDict(self).alloc_value(heap)
    }
}

impl<'v> AllocValue<'v> for serde_json::Map<String, serde_json::Value> {
    type Type = DictType<StarlarkStr, Value<'v>>;

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        (&self).alloc_value(heap)
    }
}

impl<'fv, 'a> AllocFrozenValue<'fv> for &'a serde_json::Map<String, serde_json::Value> {
    type Type = DictType<StarlarkStr, Value<'fv>>;

    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        AllocDict(self.iter().map(|(k, v)| (k.as_str(), v))).alloc_frozen_value(heap)
    }
}

impl<'fv> AllocFrozenValue<'fv> for serde_json::Map<String, serde_json::Value> {
    type Type = DictType<StarlarkStr, Value<'fv>>;

    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        (&self).alloc_frozen_value(heap)
    }
}

impl<'v, 'a> AllocValue<'v> for &'a serde_json::Value {
    type Type = Value<'v>;

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        ValueOf::from(match self {
            serde_json::Value::Null => Value::new_none(),
            serde_json::Value::Bool(x) => Value::new_bool(*x),
            serde_json::Value::Number(x) => heap.alloc(x).to_value(),
            serde_json::Value::String(x) => heap.alloc(x.as_str()).to_value(),
            serde_json::Value::Array(x) => heap.alloc(AllocList(x)).to_value(),
            serde_json::Value::Object(x) => heap.alloc(x).to_value(),
        })
    }
}

impl<'v> AllocValue<'v> for serde_json::Value {
    type Type = Value<'v>;

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        (&self).alloc_value(heap)
    }
}

impl<'fv, 'a> AllocFrozenValue<'fv> for &'a serde_json::Value {
    type Type = Value<'fv>;

    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        ValueOf::from(match self {
            serde_json::Value::Null => Value::new_none(),
            serde_json::Value::Bool(x) => Value::new_bool(*x),
            serde_json::Value::Number(x) => heap.alloc(x).to_value(),
            serde_json::Value::String(x) => heap.alloc(x.as_str()).to_value(),
            serde_json::Value::Array(x) => heap.alloc(AllocList(x)).to_value(),
            serde_json::Value::Object(x) => heap.alloc(x).to_value(),
        })
    }
}

impl<'fv> AllocFrozenValue<'fv> for serde_json::Value {
    type Type = Value<'fv>;

    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        (&self).alloc_frozen_value(heap)
    }
}

/// Nesting `serde_json` allows when it decodes to `serde_json::Value`. Splitting a value into
/// `RawValue`s does not count nesting, so `decode_raw` counts it itself.
const DECODE_RECURSION_LIMIT: usize = 128;

/// Decode a JSON value keeping every integer exact.
///
/// `serde_json::Number` has no integer form wider than 64 bits, so decoding through
/// `serde_json::Value` turns a longer integer into the nearest float. Instead `serde_json`
/// validates the value and splits off its elements as `RawValue`s, and each number is parsed
/// from its own text here.
fn decode_raw<'v>(
    raw: &RawValue,
    heap: Heap<'v>,
    remaining_depth: usize,
) -> anyhow::Result<Value<'v>> {
    let text = raw.get();
    let nested = || {
        if remaining_depth > 1 {
            Ok(remaining_depth - 1)
        } else {
            Err(anyhow::anyhow!("recursion limit exceeded"))
        }
    };
    match text.as_bytes().first() {
        Some(b'[') => {
            let remaining_depth = nested()?;
            let items = serde_json::from_str::<Vec<&RawValue>>(text)?
                .into_iter()
                .map(|item| decode_raw(item, heap, remaining_depth))
                .collect::<anyhow::Result<Vec<_>>>()?;
            Ok(heap.alloc(AllocList(items)).to_value())
        }
        Some(b'{') => {
            let remaining_depth = nested()?;
            let members = serde_json::from_str::<BTreeMap<String, &RawValue>>(text)?
                .into_iter()
                .map(|(key, member)| Ok((key, decode_raw(member, heap, remaining_depth)?)))
                .collect::<anyhow::Result<Vec<_>>>()?;
            Ok(heap.alloc(AllocDict(members)).to_value())
        }
        Some(b'"') => Ok(heap.alloc(serde_json::from_str::<String>(text)?).to_value()),
        // A number is an integer unless it has a fraction or an exponent.
        Some(b'-' | b'0'..=b'9') => {
            if text.bytes().any(|c| matches!(c, b'.' | b'e' | b'E')) {
                Ok(heap.alloc(serde_json::from_str::<f64>(text)?).to_value())
            } else if let Ok(i) = i32::from_str(text) {
                Ok(heap.alloc(i).to_value())
            } else {
                // Not `BigInt::from_str`, which is quadratic in the number of digits.
                let x = StarlarkInt::from_str_radix(text, 10).map_err(|e| e.into_anyhow())?;
                Ok(heap.alloc(x).to_value())
            }
        }
        _ => Ok(heap
            .alloc(serde_json::from_str::<serde_json::Value>(text)?)
            .to_value()),
    }
}

pub(crate) fn json(globals: &mut GlobalsBuilder) {
    #[starlark_module]
    fn json_members(globals: &mut GlobalsBuilder) {
        fn encode(#[starlark(require = pos)] x: Value) -> anyhow::Result<String> {
            x.to_json()
        }

        fn decode<'v>(
            #[starlark(require = pos)] x: &str,
            heap: Heap<'v>,
        ) -> anyhow::Result<Value<'v>> {
            decode_raw(
                serde_json::from_str::<&RawValue>(x)?,
                heap,
                DECODE_RECURSION_LIMIT,
            )
        }
    }

    // Copying Bazel's json module: https://bazel.build/rules/lib/json
    // or starlark-go json module:
    // https://github.com/google/starlark-go/blob/d1966c6b9fcd6631f48f5155f47afcd7adcc78c2/lib/json/json.go#L28
    globals.namespace("json", json_members);
}

#[cfg(test)]
mod tests {
    use crate::assert::Assert;

    #[test]
    fn test_json_encode() {
        let a = Assert::new();
        a.eq("'[10]'", "json.encode([10])");

        // Test small integer
        a.eq("'1'", "json.encode(1)");

        // Test 63-bit integer (largest 63-bit signed integer)
        a.eq("'9223372036854775807'", "json.encode(9223372036854775807)");
    }

    #[test]
    fn test_json_encode_deeply_nested() {
        let a = Assert::new();

        // Deeply nested values are rejected rather than overflowing the native stack. How deep we
        // get before that depends on how much stack is left, so this uses a value deep enough to
        // be rejected on any stack we might run on.
        a.fail(
            "def f():\n    x = []\n    for _ in range(10000):\n        x = [x]\n    return json.encode(x)\nf()",
            "nested too deeply",
        );
        a.fail(
            "def f():\n    x = {}\n    for _ in range(10000):\n        x = {'a': x}\n    return json.encode(x)\nf()",
            "nested too deeply",
        );

        // Ordinary nesting is unaffected.
        a.eq(
            "'%s0%s' % ('[' * 50, ']' * 50)",
            "def f():\n    x = 0\n    for _ in range(50):\n        x = [x]\n    return json.encode(x)\nf()",
        );

        // A cycle is still reported as a cycle, not as excessive nesting.
        a.fail(
            "def f():\n    x = []\n    x.append(x)\n    return json.encode(x)\nf()",
            "Cycle detected",
        );
    }

    #[test]
    fn test_json_decode() {
        let a = Assert::new();
        a.eq(
            "[10, None, False, {'k': 'v'}]",
            "json.decode('[10, null, false, {\"k\": \"v\"}]')",
        );

        a.eq("3.142", "json.decode('3.142')");
        a.eq(
            "123456789123456789123456789",
            "json.decode('123456789123456789123456789')",
        );
    }

    #[test]
    fn test_json_decode_int_exact() {
        let a = Assert::new();

        // Checked through `str` and `type`: an int compares equal to the float it rounds to,
        // so `==` would accept a rounded decode.
        a.eq("'int'", "type(json.decode('18446744073709551616'))");
        a.eq(
            "'18446744073709551616'",
            "str(json.decode('18446744073709551616'))",
        );
        a.eq(
            "'-9223372036854775809'",
            "str(json.decode('-9223372036854775809'))",
        );
        a.eq(
            "'123456789123456789123456789'",
            "str(json.decode('123456789123456789123456789'))",
        );
        a.eq(
            "'{\"a\": [123456789123456789123456789]}'",
            "str(json.decode('{\"a\": [123456789123456789123456789]}'))",
        );
        a.eq(
            "'{\"a\": [123456789123456789123456789]}'",
            "str(json.decode(' { \"a\" :\\n\\t[ 123456789123456789123456789 ] } '))",
        );

        // Numbers with a fraction or an exponent are still floats, and ints of every size
        // are still ints.
        a.eq("'float'", "type(json.decode('1e3'))");
        a.eq("'float'", "type(json.decode('1.0'))");
        a.eq(
            "'1.8446744073709552e+19'",
            "str(json.decode('1.8446744073709552e19'))",
        );
        a.eq("'int'", "type(json.decode('0'))");
        a.eq("'0'", "str(json.decode('-0'))");
        a.eq("'int'", "type(json.decode('9223372036854775807'))");

        // Invalid or out-of-range numbers are still rejected.
        a.fail("json.decode('1e999')", "number out of range");
        a.fail("json.decode('01')", "invalid number");
        a.fail("json.decode('[1,]')", "expected value");
        a.fail("json.decode('[' * 200 + ']' * 200)", "recursion limit");
        a.fail(
            "json.decode('{\"a\":' * 200 + '0' + '}' * 200)",
            "recursion limit",
        );
        a.eq(
            "'[' * 50 + '0' + ']' * 50",
            "str(json.decode('[' * 50 + '0' + ']' * 50))",
        );
    }

    #[test]
    fn test_json_decode_long_int() {
        // Long enough for the digits to be parsed in parts.
        let a = Assert::new();
        a.pass(
            r#"
digits = "1234567890" * 3000 + "7"
assert_eq(digits, str(json.decode(digits)))
assert_eq("-" + digits, str(json.decode("-" + digits)))
assert_eq(int(digits), json.decode("[" + digits + "]")[0])
"#,
        );
    }

    #[test]
    fn test_json_very_large_int() {
        let a = Assert::new();

        // Test edge cases: numbers at the boundary of different ranges

        // u64::MAX - should be encoded as unquoted number
        a.eq(
            "'18446744073709551615'",
            "json.encode(18446744073709551615)",
        );
        a.eq(
            "18446744073709551615",
            "json.decode(json.encode(18446744073709551615))",
        );

        // i64::MIN - should be encoded as unquoted number
        a.eq(
            "'-9223372036854775808'",
            "json.encode(-9223372036854775808)",
        );
        a.eq(
            "-9223372036854775808",
            "json.decode(json.encode(-9223372036854775808))",
        );

        // Test decoding of very large numbers from JSON strings
        a.eq(
            "18446744073709551616",
            "json.decode('18446744073709551616')",
        );
        a.eq(
            "-9223372036854775809",
            "json.decode('-9223372036854775809')",
        );
    }

    #[test]
    fn test_json_encode_int_beyond_64bit_exact() {
        let a = Assert::new();
        a.eq(
            "'18446744073709551616'",
            "json.encode(18446744073709551616)",
        );
        a.eq(
            "'-9223372036854775809'",
            "json.encode(-9223372036854775809)",
        );
        a.eq(
            "'340282366920938463463374607431768211455'",
            "json.encode(340282366920938463463374607431768211455)",
        );
        a.eq(
            "'[115792089237316195423570985008687907853269984665640564039457584007913129639937]'",
            "json.encode([(1 << 256) + 1])",
        );
        a.eq(
            "'{\"k\":-1267650600228229401496703205377}'",
            "json.encode({'k': -(1 << 100) - 1})",
        );
    }

    #[test]
    fn test_json_128bit_and_beyond() {
        let a = Assert::new();

        // Test 128-bit boundary cases

        // 2^64 (u64::MAX + 1) - first number requiring more than 64 bits
        a.eq(
            "18446744073709551616",
            "json.decode('18446744073709551616')",
        );
        a.eq(
            "18446744073709551616",
            "json.decode(json.encode(18446744073709551616))",
        );

        // 2^100 - large 128-bit number
        a.eq(
            "1267650600228229401496703205376",
            "json.decode('1267650600228229401496703205376')",
        );
        a.eq(
            "1267650600228229401496703205376",
            "json.decode(json.encode(1267650600228229401496703205376))",
        );

        // 2^128 - 1 (largest 128-bit unsigned integer)
        a.eq(
            "340282366920938463463374607431768211455",
            "json.decode('340282366920938463463374607431768211455')",
        );
        a.eq(
            "340282366920938463463374607431768211455",
            "json.decode(json.encode(340282366920938463463374607431768211455))",
        );

        // 2^128 (first number requiring more than 128 bits)
        a.eq(
            "340282366920938463463374607431768211456",
            "json.decode('340282366920938463463374607431768211456')",
        );
        a.eq(
            "340282366920938463463374607431768211456",
            "json.decode(json.encode(340282366920938463463374607431768211456))",
        );

        // Beyond 128-bit: 2^200
        a.eq(
            "1606938044258990275541962092341162602522202993782792835301376",
            "json.decode('1606938044258990275541962092341162602522202993782792835301376')",
        );
        a.eq(
            "1606938044258990275541962092341162602522202993782792835301376",
            "json.decode(json.encode(1606938044258990275541962092341162602522202993782792835301376))"
        );

        // 2^256 - extremely large number
        a.eq(
            "115792089237316195423570985008687907853269984665640564039457584007913129639936",
            "json.decode('115792089237316195423570985008687907853269984665640564039457584007913129639936')"
        );
        a.eq(
            "115792089237316195423570985008687907853269984665640564039457584007913129639936",
            "json.decode(json.encode(115792089237316195423570985008687907853269984665640564039457584007913129639936))"
        );

        // Test negative 128-bit and beyond numbers

        // -(2^100)
        a.eq(
            "-1267650600228229401496703205376",
            "json.decode('-1267650600228229401496703205376')",
        );
        a.eq(
            "-1267650600228229401496703205376",
            "json.decode(json.encode(-1267650600228229401496703205376))",
        );

        // -(2^128)
        a.eq(
            "-340282366920938463463374607431768211456",
            "json.decode('-340282366920938463463374607431768211456')",
        );
        a.eq(
            "-340282366920938463463374607431768211456",
            "json.decode(json.encode(-340282366920938463463374607431768211456))",
        );

        // -(2^200)
        a.eq(
            "-1606938044258990275541962092341162602522202993782792835301376",
            "json.decode('-1606938044258990275541962092341162602522202993782792835301376')",
        );
        a.eq(
            "-1606938044258990275541962092341162602522202993782792835301376",
            "json.decode(json.encode(-1606938044258990275541962092341162602522202993782792835301376))"
        );

        // -(2^256)
        a.eq(
            "-115792089237316195423570985008687907853269984665640564039457584007913129639936",
            "json.decode('-115792089237316195423570985008687907853269984665640564039457584007913129639936')"
        );
        a.eq(
            "-115792089237316195423570985008687907853269984665640564039457584007913129639936",
            "json.decode(json.encode(-115792089237316195423570985008687907853269984665640564039457584007913129639936))"
        );

        // Test extremely large numbers beyond typical use cases

        // 2^512 - cryptocurrency/cryptographic scale number
        let large_512bit = "13407807929942597099574024998205846127479365820592393377723561443721764030073546976801874298166903427690031858186486050853753882811946569946433649006084096";
        a.eq(large_512bit, &format!("json.decode('{large_512bit}')"));
        a.eq(
            large_512bit,
            &format!("json.decode(json.encode({large_512bit}))"),
        );

        // Test large decimal with many digits (308 digits - close to f64 limit but still manageable)
        let large_number = "10000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";
        a.eq(large_number, &format!("json.decode('{large_number}')"));
        a.eq(
            large_number,
            &format!("json.decode(json.encode({large_number}))"),
        );
    }
}
