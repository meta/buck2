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

/// Create a [`StringValue<'static>`](crate::values::StringValue) from a string literal.
///
/// The string is a static, which is what the `'static` brand means; use
/// [`at`](crate::values::StringValue::at) to bring it to the brand of the heap in use.
///
/// Multi-char strings created by this macro are automatically registered
/// for pagable serialization using the inventory crate.
#[macro_export]
macro_rules! const_frozen_string {
    ($s:expr) => {{
        $crate::values::constant_string($s).unwrap_or_else(|| {
            // `$s.len() <= 1`, `StarlarkStrNRepr::new` should not be called
            // because it fails and it should be handled by `constant_string`.
            // But we still have to put something in `static`.
            // so for `$s.len() <= 1` we put dummy string of length 2 there,
            // and `N == 1` in that case.
            const UNREACHABLE: bool = $s.len() <= 1;
            const N: usize = if UNREACHABLE {
                1
            } else {
                $crate::values::string::StarlarkStr::payload_len_for_len($s.len())
            };
            static X: $crate::values::StarlarkStrNRepr<N> =
                $crate::values::StarlarkStrNRepr::new(if UNREACHABLE { "xx" } else { $s });

            // Register for pagable serialization (only for multi-char strings).
            $crate::__derive_refs::inventory::submit! {
                $crate::__derive_refs::StaticValueEntry::new(
                    file!(),
                    line!(),
                    || X.erase().to_value()
                )
            }

            if UNREACHABLE {
                unreachable!()
            } else {
                X.erase()
            }
        })
    }};
}

#[cfg(test)]
mod tests {
    use crate::values::FrozenHeap;
    use crate::values::Heap;

    #[test]
    fn test_const_frozen_string_for_short_strings() {
        assert!(
            const_frozen_string!("a")
                .to_value()
                .ptr_eq(const_frozen_string!("a").to_value())
        );

        Heap::temp(|heap| {
            assert!(
                const_frozen_string!("a")
                    .to_value()
                    .ptr_eq(heap.alloc("a").to_value())
            );
        });

        FrozenHeap::temp(|frozen_heap| {
            assert!(
                const_frozen_string!("a")
                    .to_value()
                    .ptr_eq(frozen_heap.alloc("a").to_value())
            );
        });
    }

    #[test]
    fn test_const_frozen_string() {
        assert_eq!("", const_frozen_string!("").as_str());
        assert_eq!("a", const_frozen_string!("a").as_str());
        assert_eq!("ab", const_frozen_string!("ab").as_str());
        assert_eq!("abc", const_frozen_string!("abc").as_str());
        assert_eq!("abcd", const_frozen_string!("abcd").as_str());
        assert_eq!("abcde", const_frozen_string!("abcde").as_str());
        assert_eq!("abcdef", const_frozen_string!("abcdef").as_str());
        assert_eq!("abcdefg", const_frozen_string!("abcdefg").as_str());
        assert_eq!("abcdefgh", const_frozen_string!("abcdefgh").as_str());
        assert_eq!("abcdefghi", const_frozen_string!("abcdefghi").as_str());
        assert_eq!("abcdefghij", const_frozen_string!("abcdefghij").as_str());
        assert_eq!("abcdefghijk", const_frozen_string!("abcdefghijk").as_str());
    }
}
