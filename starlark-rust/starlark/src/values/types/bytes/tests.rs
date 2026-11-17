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

#![cfg(test)]

use std::cell::RefCell;
use std::rc::Rc;

use dupe::Dupe;

use crate::assert;
use crate::assert::Assert;
use crate::stdlib::PrintHandler;

// Note: assert::all_true disables static typechecking, which is needed for
// some bytes operations that the type checker may not yet fully model.

#[test]
fn test_bytes_type() {
    assert::all_true(
        r#"
type(b"") == "bytes"
type(b"hello") == "bytes"
type(bytes("hi")) == "bytes"
"#,
    );
}

#[test]
fn test_bytes_literals() {
    assert::all_true(
        r#"
b"hello" == b"hello"
b'hello' == b"hello"
b"" == b""
b"\x00" == bytes([0])
b"\xff" == bytes([255])
b"\n\r\t" == bytes([10, 13, 9])
b"\012" == bytes([10])
b"\101" == bytes([65])
"#,
    );
}

#[test]
fn test_bytes_literal_triple_quoted() {
    assert::all_true(
        r#"
b"""hello""" == b"hello"
b'''world''' == b"world"
"#,
    );
    // Triple-quoted spanning multiple lines must be tested with pass() since
    // all_true() splits on newlines.
    assert::pass(
        r#"assert_eq(b"""line1
line2""", bytes([108, 105, 110, 101, 49, 10, 108, 105, 110, 101, 50]))"#,
    );
}

#[test]
fn test_bytes_literal_raw() {
    assert::all_true(
        r#"
rb"\r\n\t" == b"\\r\\n\\t"
br"\r\n\t" == b"\\r\\n\\t"
rb"\x41" == b"\\x41"
"#,
    );
}

#[test]
fn test_bytes_constructor_from_bytes() {
    assert::all_true(
        r#"
bytes(b"hello") == b"hello"
bytes(b"") == b""
"#,
    );
}

#[test]
fn test_bytes_constructor_from_str() {
    assert::all_true(
        r#"
bytes("hello") == b"hello"
bytes("") == b""
bytes("ABC") == bytes([65, 66, 67])
"#,
    );
}

#[test]
fn test_bytes_constructor_from_iterable() {
    assert::all_true(
        r#"
bytes([65, 66, 67]) == b"ABC"
bytes([0]) == b"\x00"
bytes([255]) == b"\xff"
bytes([]) == b""
bytes((65, 66, 67)) == b"ABC"
"#,
    );
}

#[test]
fn test_bytes_constructor_errors() {
    assert::fail("bytes([256])", "out of range");
    assert::fail("bytes([-1])", "out of range");
    assert::fail(r#"bytes(1)"#, "bytes()");
    assert::fail("bytes([b\"a\"])", "integers");
}

#[test]
fn test_bytes_length() {
    assert::all_true(
        r#"
len(b"") == 0
len(b"a") == 1
len(b"hello") == 5
len(b"\x00\xff") == 2
"#,
    );
}

// Multi-byte UTF-8 characters: each occupies multiple bytes.
#[test]
fn test_bytes_length_multibyte() {
    assert::all_true(
        r#"
len(bytes("Ѐ")) == 2
len(bytes("世")) == 3
len(bytes("😿")) == 4
"#,
    );
}

#[test]
fn test_bytes_truth() {
    assert::all_true(
        r#"
not b""
bool(b"") == False
bool(b"x") == True
bool(b"\x00") == True
"#,
    );
}

// repr formats bytes as b"..." with escapes for control/non-ASCII bytes.
// Our implementation uses \xHH for bytes outside 0x20–0x7e.
#[test]
fn test_bytes_repr() {
    assert::eq(r#"repr(b"hello")"#, r#""b\"hello\"" "#);
    assert::eq(r#"repr(b"")"#, r#""b\"\"" "#);
    assert::eq(r#"repr(b"\t\n\r")"#, r#""b\"\\t\\n\\r\"" "#);
    assert::eq(r#"repr(b"\x7f")"#, r#""b\"\\x7f\"" "#);
    assert::eq(r#"repr(b"\x00")"#, r#""b\"\\x00\"" "#);
    assert::eq(r#"repr(b"\xff")"#, r#""b\"\\xff\"" "#);
    assert::eq(r#"repr(b"\\")"#, r#""b\"\\\\\"" "#);
    assert::eq(r#"repr(b"\"")"#, r#""b\"\\\"\"" "#);
}

// str(bytes) decodes UTF-8, replacing invalid sequences with U+FFFD.
#[test]
fn test_bytes_str() {
    assert::eq(r#"str(b"hello")"#, r#""hello""#);
    assert::eq(r#"str(b"")"#, r#""""#);
    assert::eq(r#"str(bytes("hello, 世界"))"#, r#""hello, 世界""#);
    // Single invalid byte → one U+FFFD replacement character.
    assert::eq(r#"str(b"\xff")"#, r#""\ufffd""#);
    // UTF-8 encoding of an unpaired surrogate → replacement characters.
    assert::eq(r#"str(b"\xed\xb0\x80")"#, r#""\ufffd\ufffd\ufffd""#);
}

#[test]
fn test_bytes_equality() {
    assert::all_true(
        r#"
b"abc" == b"abc"
b"abc" != b"abd"
b"" == b""
b"abc" != b"abcd"
b"abc" != "abc"
"#,
    );
}

#[test]
fn test_bytes_comparison() {
    assert::all_true(
        r#"
b"abc" < b"abd"
b"abc" < b"abcd"
b"abcd" > b"abc"
b"abd" > b"abc"
b"abc" <= b"abc"
b"abc" >= b"abc"
b"\x7f" < b"\x80"
"#,
    );
}

// Per the spec, bytes[i] returns the integer value of byte i (not a 1-byte bytes object).
#[test]
fn test_bytes_indexing() {
    assert::all_true(
        r#"
b"abc"[0] == 97
b"abc"[1] == 98
b"abc"[2] == 99
b"abc"[-1] == 99
b"abc"[-3] == 97
b"\x00\xff"[0] == 0
b"\x00\xff"[1] == 255
"#,
    );
}

#[test]
fn test_bytes_index_out_of_range() {
    assert::fail("b\"abc\"[100]", "out of bound");
    assert::fail("b\"abc\"[-100]", "out of bound");
    assert::fail("b\"\"[0]", "out of bound");
}

#[test]
fn test_bytes_slicing() {
    assert::all_true(
        r#"
b"hello"[1:3] == b"el"
b"hello"[:4] == b"hell"
b"hello"[4:] == b"o"
b"hello"[::2] == b"hlo"
b"hello"[::-1] == b"olleh"
b"hello"[1:4:2] == b"el"
b"hello"[0:0] == b""
b"hello"[5:5] == b""
"#,
    );
}

// int in bytes: checks if a byte value is present.
#[test]
fn test_bytes_contains_int() {
    assert::all_true(
        r#"
97 in b"abc"
98 in b"abc"
100 not in b"abc"
0 in b"\x00\xff"
255 in b"\x00\xff"
"#,
    );
}

#[test]
fn test_bytes_contains_int_out_of_range() {
    assert::fail("256 in b\"abc\"", "range");
    assert::fail("-1 in b\"abc\"", "range");
}

// bytes in bytes: subsequence check.
#[test]
fn test_bytes_contains_bytes() {
    assert::all_true(
        r#"
b"bc" in b"abcd"
b"ab" in b"abcd"
b"cd" in b"abcd"
b"" in b"abcd"
b"" in b""
b"abcd" not in b"abc"
b"xyz" not in b"abc"
"#,
    );
}

#[test]
fn test_bytes_contains_wrong_type() {
    assert::fail(r#""bc" in b"dcab""#, "bytes");
}

#[test]
fn test_bytes_concatenation() {
    assert::all_true(
        r#"
b"ab" + b"cd" == b"abcd"
b"" + b"abc" == b"abc"
b"abc" + b"" == b"abc"
b"" + b"" == b""
"#,
    );
}

#[test]
fn test_bytes_repetition() {
    assert::all_true(
        r#"
b"ab" * 3 == b"ababab"
3 * b"ab" == b"ababab"
b"ab" * 1 == b"ab"
b"ab" * 0 == b""
b"ab" * -1 == b""
"#,
    );
}

#[test]
fn test_bytes_not_iterable() {
    assert::fail("for x in b\"abc\": pass", "not supported");
}

#[test]
fn test_bytes_not_assignable() {
    assert::fail(r#"b"abc"[1] = 66"#, "Immutable");
}

#[test]
fn test_bytes_as_dict_key() {
    // all_true() evaluates each line independently, so assignments (→ None)
    // can't be mixed with boolean expressions. Use pass() instead.
    assert::pass(
        r#"
d = {b"hello": 1, b"goodbye": 2}
assert_eq(d[b"hello"], 1)
assert_eq(d[b"goodbye"], 2)
assert_eq(len(d), 2)
d[b"goodbye"] = 3
assert_eq(d[b"goodbye"], 3)
assert_eq(len(d), 2)
"#,
    );
}

// Per the spec, elems() returns an iterable of 1-byte bytes objects.
// Note: bytes(iterable) requires integers, so bytes(x.elems()) doesn't work
// since elems() yields bytes objects — use list comprehension with indexing instead.
#[test]
fn test_bytes_elems() {
    assert::all_true(
        r#"
list(b"abc".elems()) == [b"a", b"b", b"c"]
list(b"".elems()) == []
list(b"\x00\xff".elems()) == [b"\x00", b"\xff"]
"#,
    );
}

// Bytes are usable as frozenset elements (hashable).
#[test]
fn test_bytes_in_set() {
    assert::all_true(
        r#"
b"a" in {b"a": True}
b"b" not in {b"a": True}
"#,
    );
}

// Hashing: equal bytes must have equal hashes.
#[test]
fn test_bytes_hash_consistency() {
    assert::pass(
        r#"
d = {}
d[b"hello"] = 1
d[b"hello"] = 2
assert_eq(len(d), 1)
assert_eq(d[b"hello"], 2)
"#,
    );
}

// hash() built-in only accepts strings in this implementation; bytes are
// hashable via dict keys (tested in test_bytes_as_dict_key / test_bytes_hash_consistency)
// but hash() itself does not accept bytes.
#[test]
fn test_bytes_hash_builtin_unsupported() {
    assert::fail(r#"hash(b"")"#, "str");
}

// repr of multi-byte UTF-8 content: printable non-ASCII chars are shown as-is;
// non-printable Unicode gets \uXXXX; invalid UTF-8 bytes get \xHH.
#[test]
fn test_bytes_repr_multibyte() {
    // "é" is printable non-ASCII → shown as-is
    assert::eq(r#"repr(bytes("é"))"#, r#""b\"é\"" "#);
    // "世" is printable non-ASCII → shown as-is
    assert::eq(r#"repr(bytes("世"))"#, r#""b\"世\"" "#);
}

#[test]
fn test_bytes_repr_non_printable_unicode() {
    // U+200D ZERO WIDTH JOINER (format/invisible char) → \u200d
    assert::eq(r#"repr(b"\xe2\x80\x8d")"#, r#""b\"\\u200d\"" "#);
}

#[test]
fn test_bytes_repr_invalid_utf8() {
    // Unpaired surrogate encoded as modified UTF-8 is invalid UTF-8 → \xHH for each byte
    assert::eq(r#"repr(b"\xed\xb0\x80")"#, r#""b\"\\xed\\xb0\\x80\"" "#);
}

// repr decodes one character at a time, so characters that follow each other,
// a 4-byte character and a sequence cut off at the end must all come out the same.
#[test]
fn test_bytes_repr_adjacent_multibyte() {
    assert::eq(
        r#"repr(b"\xc3\xa9\xe4\xb8\x96\xc3")"#,
        r#""b\"é世\\xc3\"" "#,
    );
    assert::eq(
        r#"repr(b"\xf0\x9f\x98\x80\xc3\xa9")"#,
        r#""b\"\\U0001f600é\"" "#,
    );
    assert::eq(
        r#"repr(b"\xe4\xb8\x96\xe4\xb8")"#,
        r#""b\"世\\xe4\\xb8\"" "#,
    );
    assert::eq(r#"len(repr(b"\xc3\xa9" * 100000))"#, "100003");
}

// str() on an incomplete UTF-8 sequence yields U+FFFD replacements.
// from_utf8_lossy replaces each maximal invalid subsequence with one U+FFFD.
#[test]
fn test_bytes_str_incomplete_utf8() {
    // 0xc3 alone is the start of a 2-byte sequence — one replacement.
    assert::eq(r#"str(b"\xc3")"#, r#""\ufffd""#);
    // 0xe4 0xb8 is an incomplete 3-byte sequence — treated as one invalid
    // subsequence, yielding one U+FFFD (not two).
    assert::eq(r#"str(b"\xe4\xb8")"#, r#""\ufffd""#);
}

// str() on control bytes decodes to the same control characters.
#[test]
fn test_bytes_str_control_chars() {
    assert::eq(r#"str(b"\t\n")"#, r#""\t\n""#);
}

// bytes() constructor accepts a range() iterable.
#[test]
fn test_bytes_constructor_from_range() {
    assert::all_true(
        r#"
bytes(range(65, 68)) == b"ABC"
bytes(range(0)) == b""
bytes(range(0, 256, 51)) == bytes([0, 51, 102, 153, 204, 255])
"#,
    );
}

// bytes() with no arguments should fail (x is required positional).
#[test]
fn test_bytes_constructor_no_args() {
    assert::fail("bytes()", "positional");
}

// bytes + non-bytes produces a type error.
#[test]
fn test_bytes_add_type_error() {
    assert::fail(r#"b"abc" + "xyz""#, "bytes");
}

// bytes < non-bytes produces a type error.
#[test]
fn test_bytes_compare_type_error() {
    assert::fail(r#"b"abc" < 1"#, "Operation `compare` not supported");
}

// elems() on multi-byte UTF-8 content iterates individual bytes, not codepoints.
#[test]
fn test_bytes_elems_multibyte() {
    // "é" = 0xc3 0xa9 → two 1-byte bytes objects
    assert::all_true(r#"list(bytes("é").elems()) == [b"\xc3", b"\xa9"]"#);
}

// bytes(iterable_of_bytes_objects) fails — elems() yields bytes, not ints.
// This documents our deviation from starlark-go (where elems() yields ints).
#[test]
fn test_bytes_constructor_rejects_bytes_elements() {
    assert::fail(r#"bytes(b"abc".elems())"#, "integers");
}

// Slices with stride > 1 (beyond the stride=2 case already tested).
#[test]
fn test_bytes_slice_stride() {
    assert::all_true(
        r#"
b"abcdef"[::3] == b"ad"
b"abcdef"[1::3] == b"be"
b"abcdef"[::6] == b"a"
"#,
    );
}

// print(b"...") should output the UTF-8 decoded string, not the repr.
#[test]
fn test_bytes_print() {
    let s = Rc::new(RefCell::new(String::new()));
    struct PrintHandlerImpl {
        s: Rc<RefCell<String>>,
    }
    impl PrintHandler for PrintHandlerImpl {
        fn println(&self, msg: &str) -> crate::Result<()> {
            *self.s.borrow_mut() = msg.to_owned();
            Ok(())
        }
    }
    let print_handler = PrintHandlerImpl { s: s.dupe() };
    let mut a = Assert::new();
    a.set_print_handler(&print_handler);
    a.pass(r#"print(b"hello")"#);
    assert_eq!("hello", s.borrow().as_str());
}

// ord() accepts a 1-byte bytes argument.
#[test]
fn test_ord_bytes() {
    assert::all_true(
        r#"
ord(b"A") == 65
ord(b"\xff") == 255
ord(b"\x00") == 0
"#,
    );
    assert::fail(r#"ord(b"")"#, "length 1");
    assert::fail(r#"ord(b"AB")"#, "length 1");
}
