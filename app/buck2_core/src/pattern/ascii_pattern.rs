/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! This module implement faster (that stdlib) string operations when
//! arguments of these operations are ASCII strings.
//!
//! These should be actually fixed in Rust stdlib.

use std::str;

use dupe::Dupe;

const fn assert_ascii_char(c: char) -> u8 {
    assert!(c.is_ascii());
    c as u8
}

const fn assert_ascii_str(s: &str) -> &[u8] {
    // There's no `for` here because this is `const fn`.
    // We use `const fn` to make sure compiler is able to optimize this away.
    let mut i = 0;
    while i != s.len() {
        assert!(s.as_bytes()[i].is_ascii());
        i += 1;
    }
    s.as_bytes()
}

/// Fixed ASCII string as search pattern.
///
/// Caller of this trait skips boundary checks/UTF-8 checks, so this trait is `unsafe`.
///
/// # Safety
///
/// Implementors must ensure that all methods correctly handle ASCII strings and that
/// the pattern methods (`first_index_in`, `last_index_in`, `is_prefix_of`, `is_suffix_of`)
/// return valid byte indices that lie on UTF-8 character boundaries when used with
/// valid UTF-8 strings. The `len()` method must return the correct byte length of the pattern.
pub(crate) unsafe trait AsciiPattern {
    fn first_index_in(&self, s: &str) -> Option<usize>;
    #[allow(dead_code)]
    fn last_index_in(&self, s: &str) -> Option<usize>;
    fn is_prefix_of(&self, s: &str) -> bool;
    fn is_suffix_of(&self, s: &str) -> bool;
    fn len(&self) -> usize;
}

/// Single ASCII char.
#[derive(Copy, Clone, Dupe, Debug)]
pub(crate) struct AsciiChar(u8);

impl AsciiChar {
    pub(crate) const fn new(c: char) -> AsciiChar {
        assert_ascii_char(c);
        AsciiChar(c as u8)
    }
}

/// ASCII string.
#[derive(Copy, Clone, Dupe, Debug)]
pub(crate) struct AsciiStr<'s>(&'s str);

impl<'s> AsciiStr<'s> {
    pub(crate) const fn new(s: &'s str) -> AsciiStr<'s> {
        assert_ascii_str(s);
        AsciiStr(s)
    }
}

/// ASCII string of length 2.
#[derive(Copy, Clone, Debug)]
pub(crate) struct AsciiStr2([u8; 2]);

impl Dupe for AsciiStr2 {}

impl AsciiStr2 {
    pub(crate) const fn new(s: &str) -> AsciiStr2 {
        assert_ascii_str(s);
        assert!(s.len() == 2);
        AsciiStr2([s.as_bytes()[0], s.as_bytes()[1]])
    }
}

unsafe impl AsciiPattern for AsciiChar {
    fn first_index_in(&self, s: &str) -> Option<usize> {
        memchr::memchr(self.0, s.as_bytes())
    }

    fn last_index_in(&self, s: &str) -> Option<usize> {
        memchr::memrchr(self.0, s.as_bytes())
    }

    fn is_prefix_of(&self, s: &str) -> bool {
        s.as_bytes().first().copied() == Some(self.0)
    }

    fn is_suffix_of(&self, s: &str) -> bool {
        s.as_bytes().last().copied() == Some(self.0)
    }

    fn len(&self) -> usize {
        1
    }
}

unsafe impl AsciiPattern for AsciiStr2 {
    #[allow(clippy::int_plus_one)]
    fn first_index_in(&self, s: &str) -> Option<usize> {
        if s.len() < 2 {
            return None;
        }
        let mut start = 0;
        loop {
            unsafe {
                debug_assert!(start <= s.len() - 1);
                // Find the first char.
                start =
                    match memchr::memchr(self.0[0], s.as_bytes().get_unchecked(start..s.len() - 1))
                    {
                        Some(from_i) => {
                            // Check the second char.
                            if *s.as_bytes().get_unchecked(start + from_i + 1) == self.0[1] {
                                return Some(start + from_i);
                            }
                            start + from_i + 1
                        }
                        None => return None,
                    }
            }
        }
    }

    fn last_index_in(&self, s: &str) -> Option<usize> {
        if s.len() < 2 {
            return None;
        }
        let mut end = s.len();
        loop {
            unsafe {
                debug_assert!(end >= 1);
                debug_assert!(end <= s.len());
                // Suppose the string is `abcde` and we are searching `xd`.
                // Here we are searching for the second char first, which is `d`.
                end = match memchr::memrchr(self.0[1], s.as_bytes().get_unchecked(1..end)) {
                    Some(from_1) => {
                        // `from_1` for `d` is 2
                        if *s.as_bytes().get_unchecked(from_1) == self.0[0] {
                            return Some(from_1);
                        }
                        // and then next `end` would be 3, pointing here: `abc<>de`.
                        from_1 + 1
                    }
                    None => return None,
                }
            }
        }
    }

    fn is_prefix_of(&self, s: &str) -> bool {
        if s.len() < 2 {
            return false;
        }
        s.as_bytes()[0] == self.0[0] && s.as_bytes()[1] == self.0[1]
    }

    fn is_suffix_of(&self, s: &str) -> bool {
        if s.len() < 2 {
            return false;
        }
        s.as_bytes()[s.len() - 2] == self.0[0] && s.as_bytes()[s.len() - 1] == self.0[1]
    }

    fn len(&self) -> usize {
        2
    }
}

unsafe impl AsciiPattern for AsciiStr<'_> {
    fn first_index_in(&self, s: &str) -> Option<usize> {
        memchr::memmem::find(s.as_bytes(), self.0.as_bytes())
    }

    fn last_index_in(&self, s: &str) -> Option<usize> {
        memchr::memmem::rfind(s.as_bytes(), self.0.as_bytes())
    }

    fn is_prefix_of(&self, s: &str) -> bool {
        s.as_bytes().starts_with(self.0.as_bytes())
    }

    fn is_suffix_of(&self, s: &str) -> bool {
        s.as_bytes().ends_with(self.0.as_bytes())
    }

    fn len(&self) -> usize {
        self.0.len()
    }
}

/// `s.strip_suffix(suffix)`.
#[inline]
pub(crate) fn strip_suffix_ascii(s: &str, suffix: impl AsciiPattern) -> Option<&str> {
    if suffix.is_suffix_of(s) {
        unsafe { Some(s.get_unchecked(..s.len() - suffix.len())) }
    } else {
        None
    }
}

/// `s.trim_prefix(prefix)`.
#[inline]
pub(crate) fn trim_prefix_ascii(s: &str, prefix: impl AsciiPattern) -> &str {
    if prefix.is_prefix_of(s) {
        unsafe { s.get_unchecked(prefix.len()..) }
    } else {
        s
    }
}

/// `s.split_once(needle)`.
#[inline]
pub(crate) fn split1_opt_ascii(s: &str, needle: impl AsciiPattern) -> Option<(&str, &str)> {
    needle
        .first_index_in(s)
        .map(|i| unsafe { (s.get_unchecked(..i), s.get_unchecked(i + needle.len()..)) })
}

#[cfg(test)]
mod tests {
    use std::str::pattern::Pattern;
    use std::str::pattern::ReverseSearcher;

    use crate::pattern::ascii_pattern::AsciiChar;
    use crate::pattern::ascii_pattern::AsciiPattern;
    use crate::pattern::ascii_pattern::AsciiStr;
    use crate::pattern::ascii_pattern::AsciiStr2;
    use crate::pattern::ascii_pattern::strip_suffix_ascii;

    const STRINGS: &[&str] = &[
        "", "x", "y", "xx", "xy", "yx", "yy", "xxx", "xxy", "xyx", "xyy", "yxx", "yxy", "yyx",
        "yyy",
    ];

    fn test_is_prefix_of_impl(ascii: impl AsciiPattern + Copy, str_pattern: impl Pattern + Copy) {
        for s in STRINGS {
            assert_eq!(ascii.is_prefix_of(s), str_pattern.is_prefix_of(s));
        }
    }

    fn test_is_suffix_of_impl<'p>(
        ascii: impl AsciiPattern + Copy,
        str_pattern: impl Pattern<Searcher<'p> = impl ReverseSearcher<'p>> + Copy,
    ) {
        for s in STRINGS {
            assert_eq!(ascii.is_suffix_of(s), str_pattern.is_suffix_of(s));
        }
    }

    fn test_first_index_in_impl(ascii: impl AsciiPattern + Copy, str_pattern: impl Pattern + Copy) {
        for s in STRINGS {
            assert_eq!(ascii.first_index_in(s), s.find(str_pattern));
        }
    }

    fn test_last_index_in_impl<P>(ascii: impl AsciiPattern + Copy, str_pattern: P)
    where
        P: Pattern + Copy,
        for<'p> <P as Pattern>::Searcher<'p>: ReverseSearcher<'p>,
    {
        for s in STRINGS {
            assert_eq!(ascii.last_index_in(s), s.rfind(str_pattern));
        }
    }

    #[test]
    fn test_is_prefix_of() {
        test_is_prefix_of_impl(AsciiChar::new('x'), 'x');
        test_is_prefix_of_impl(AsciiStr2::new("xx"), "xx");
        test_is_prefix_of_impl(AsciiStr2::new("xy"), "xy");
        test_is_prefix_of_impl(AsciiStr::new("xx"), "xx");
        test_is_prefix_of_impl(AsciiStr::new("xy"), "xy");
    }

    #[test]
    fn test_is_suffix_of() {
        test_is_suffix_of_impl(AsciiChar::new('x'), 'x');
        test_is_suffix_of_impl(AsciiStr2::new("xx"), "xx");
        test_is_suffix_of_impl(AsciiStr2::new("xy"), "xy");
        test_is_suffix_of_impl(AsciiStr::new("xx"), "xx");
        test_is_suffix_of_impl(AsciiStr::new("xy"), "xy");
    }

    #[test]
    fn test_first_index_in() {
        test_first_index_in_impl(AsciiChar::new('x'), 'x');
        test_first_index_in_impl(AsciiStr2::new("xx"), "xx");
        test_first_index_in_impl(AsciiStr2::new("xy"), "xy");
        test_first_index_in_impl(AsciiStr::new("xx"), "xx");
        test_first_index_in_impl(AsciiStr::new("xy"), "xy");
    }

    // With panic=abort the panic kills the in-process Miri harness.
    #[cfg(not(all(miri, panic = "abort")))]
    #[test]
    #[should_panic]
    fn test_non_ascii_char_is_rejected() {
        let _ = AsciiChar::new(std::hint::black_box('\u{a9}'));
    }

    // With panic=abort the panic kills the in-process Miri harness.
    #[cfg(not(all(miri, panic = "abort")))]
    #[test]
    #[should_panic]
    fn test_non_ascii_str_is_rejected() {
        let _ = AsciiStr::new(std::hint::black_box("\u{e9}"));
    }

    // With panic=abort the panic kills the in-process Miri harness.
    #[cfg(not(all(miri, panic = "abort")))]
    #[test]
    #[should_panic]
    fn test_non_ascii_str2_is_rejected() {
        let _ = AsciiStr2::new(std::hint::black_box("\u{e9}"));
    }

    #[test]
    fn test_is_suffix_of_non_ascii_haystack() {
        // The needle's length measured back from the end lands inside `é`, so the check must not
        // slice the haystack there.
        let needle = AsciiStr::new("/...");
        let haystack = "\u{e9}...";
        assert!(!haystack.is_char_boundary(haystack.len() - needle.len()));
        assert!(!needle.is_suffix_of(haystack));
        assert_eq!(None, strip_suffix_ascii(haystack, needle));
        assert_eq!(Some("\u{e9}"), strip_suffix_ascii("\u{e9}/...", needle));
    }

    #[test]
    fn test_last_index_in() {
        test_last_index_in_impl(AsciiChar::new('x'), 'x');
        test_last_index_in_impl(AsciiStr2::new("xx"), "xx");
        test_last_index_in_impl(AsciiStr2::new("xy"), "xy");
        test_last_index_in_impl(AsciiStr::new("xx"), "xx");
        test_last_index_in_impl(AsciiStr::new("xy"), "xy");
    }
}
