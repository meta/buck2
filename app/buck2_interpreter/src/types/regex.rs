/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

use allocative::Allocative;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::starlark_module;
use starlark::starlark_simple_value;
use starlark::typing::Ty;
use starlark::values::Freeze;
use starlark::values::NoSerialize;
use starlark::values::StarlarkValue;
use starlark::values::StringValue;
use starlark::values::ValueOf;
use starlark::values::list::ListType;
use starlark::values::starlark_value;

/// Wrapper for `regex::Regex`.
#[derive(
    ProvidesStaticType,
    Debug,
    Freeze,
    NoSerialize,
    Allocative,
    starlark::StarlarkPagable
)]
pub enum StarlarkBuckRegex {
    // TODO(nga): do not skip.
    //   And this is important because regex can have a lot of cache.
    Regular(
        #[allocative(skip)]
        #[freeze(identity)]
        #[starlark_pagable(pagable)]
        regex::Regex,
    ),
    Fancy(
        #[allocative(skip)]
        #[freeze(identity)]
        #[starlark_pagable(pagable)]
        fancy_regex::Regex,
    ),
}

impl StarlarkBuckRegex {
    pub fn as_str(&self) -> &str {
        match self {
            StarlarkBuckRegex::Regular(r) => r.as_str(),
            StarlarkBuckRegex::Fancy(r) => r.as_str(),
        }
    }

    fn is_match(&self, s: &str) -> buck2_error::Result<bool> {
        match self {
            StarlarkBuckRegex::Regular(r) => Ok(r.is_match(s)),
            StarlarkBuckRegex::Fancy(r) => Ok(r.is_match(s)?),
        }
    }

    /// Matches the regex against each element of `strings`, combining results with OR
    /// (`any = true`) or AND (`any = false`). Returns as soon as the result is decided,
    /// without matching the remaining elements.
    fn multi_match<'v>(
        &self,
        strings: ValueOf<'v, ListType<StringValue<'v>>>,
        any: bool,
    ) -> starlark::Result<bool> {
        for string in strings.iter() {
            if self.is_match(string?.unpack().as_str())? == any {
                return Ok(any);
            }
        }
        Ok(!any)
    }

    pub fn replace_all<'a>(&self, haystack: &'a str, rep: &str) -> Cow<'a, str> {
        match self {
            StarlarkBuckRegex::Regular(r) => r.replace_all(haystack, rep),
            StarlarkBuckRegex::Fancy(r) => r.replace_all(haystack, rep),
        }
    }
}

starlark::methods_static!(REGEX_METHODS = regex_methods);

#[starlark_value(type = "BuckRegex")] // "regex" is used for "experimental_regex" in starlark-rust.
impl<'v> StarlarkValue<'v> for StarlarkBuckRegex {
    fn get_methods() -> Option<&'static Methods> {
        Some(REGEX_METHODS.methods())
    }

    fn typechecker_ty(&self) -> Option<Ty> {
        Some(Ty::starlark_value::<Self>())
    }
}

impl Display for StarlarkBuckRegex {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        // TODO(nga): should use starlark string repr.
        write!(f, "regex({:?})", self.as_str())
    }
}

starlark_simple_value!(StarlarkBuckRegex);

/// Type created by the [`regex`](../regex) function.
#[starlark_module]
fn regex_methods(builder: &mut MethodsBuilder) {
    /// Determine if the regex matches any substring of the given string.
    fn r#match(
        this: &StarlarkBuckRegex,
        #[starlark(require = pos)] str: &str,
    ) -> starlark::Result<bool> {
        Ok(this.is_match(str)?)
    }

    /// Determine if the regex matches a substring of any of the given strings.
    /// Returns `False` for an empty list. Returns as soon as a match is found,
    /// without matching the remaining elements.
    ///
    /// Behaves like calling `match` on each element, but is significantly faster on large
    /// lists because it crosses from Starlark into native code only once.
    fn any_match<'v>(
        this: &StarlarkBuckRegex,
        #[starlark(require = pos)] strings: ValueOf<'v, ListType<StringValue<'v>>>,
    ) -> starlark::Result<bool> {
        this.multi_match(strings, true)
    }

    /// Determine if the regex matches a substring of every one of the given strings.
    /// Returns `True` for an empty list. Returns as soon as a non-matching element is
    /// found, without matching the remaining elements.
    ///
    /// Behaves like calling `match` on each element, but is significantly faster on large
    /// lists because it crosses from Starlark into native code only once.
    fn all_match<'v>(
        this: &StarlarkBuckRegex,
        #[starlark(require = pos)] strings: ValueOf<'v, ListType<StringValue<'v>>>,
    ) -> starlark::Result<bool> {
        this.multi_match(strings, false)
    }

    /// Replace all matches of the regex in the given string with the replacement string.
    /// Takes the following parameters:
    /// * haystack - The string you will be regex matching against
    /// * replacement - The replacement string to replace the regex matches with
    ///
    /// Returns a new string with all regex matches replaced.
    ///
    /// Sample usage:
    /// ```pytyon
    /// regex_pattern = regex(r"foo")
    /// result = regex_pattern.replace_all("foo bar foo", "baz")
    /// ```
    /// `result` equals `"baz bar baz"` in this example
    fn r#replace_all(
        this: &StarlarkBuckRegex,
        #[starlark(require = pos)] haystack: &str,
        #[starlark(require = pos)] replacement: &str,
    ) -> starlark::Result<String> {
        Ok(this.replace_all(haystack, replacement).into_owned())
    }
}

#[starlark_module]
#[starlark_types(StarlarkBuckRegex as BuckRegex)]
pub fn register_buck_regex(builder: &mut GlobalsBuilder) {
    /// Compile a regular expression from a string.
    ///
    /// ## Fanciness
    ///
    /// Buck2 regexes support two backing implementations:
    ///
    /// - With `fancy = False` (the default), [the `regex`
    ///   crate](https://docs.rs/regex/latest/regex/#syntax) is used. These regular expressions
    ///   compile down to a DFA and match very quickly and efficiently (in linear time), but do not
    ///   support look-around or backreferences.
    ///
    /// - With `fancy = True`, [the `fancy_regex`
    ///   crate](https://docs.rs/fancy-regex/latest/fancy_regex/#syntax) is used, which does support
    ///   look-around and backreferences, but is slower to match (exponential time in the worst
    ///   case).
    #[starlark(as_type = StarlarkBuckRegex)]
    fn regex<'v>(
        #[starlark(require = pos)] regex: &str,
        #[starlark(require = named, default = false)] fancy: bool,
    ) -> starlark::Result<StarlarkBuckRegex> {
        match fancy {
            false => Ok(StarlarkBuckRegex::Regular(
                regex::Regex::new(regex).map_err(buck2_error::Error::from)?,
            )),
            true => Ok(StarlarkBuckRegex::Fancy(
                fancy_regex::Regex::new(regex).map_err(buck2_error::Error::from)?,
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use starlark::assert::Assert;

    use crate::types::regex::register_buck_regex;

    #[test]
    fn test_match() {
        let mut a = Assert::new();
        a.globals_add(register_buck_regex);

        a.is_true("regex('abc|def|ghi').match('abc')");
        a.is_true("regex('x').match('aaaxbbb')");
    }

    #[test]
    fn test_any_match() {
        let mut a = Assert::new();
        a.globals_add(register_buck_regex);

        a.is_true("regex('abc|def|ghi').any_match(['none', 'def'])");
        a.is_false("regex('abc|def|ghi').any_match(['none', 'other'])");
        a.is_false("regex('x').any_match([])");
        a.fail("regex('x').any_match('x')", "expected `list[str]`");
        a.fail("regex('x').any_match([1])", "expected `list[str]`");
        // The list is checked as a whole before any matching, so a non-string anywhere is an
        // error even when an earlier element decides the result.
        a.fail("regex('x').any_match(['x', 1])", "expected `list[str]`");
    }

    #[test]
    fn test_all_match() {
        let mut a = Assert::new();
        a.globals_add(register_buck_regex);

        a.is_true("regex('abc|def|ghi').all_match(['xdefy', 'abc'])");
        a.is_false("regex('abc|def|ghi').all_match(['def', 'other'])");
        a.is_true("regex('x').all_match([])");
        a.fail("regex('x').all_match('x')", "expected `list[str]`");
        a.fail("regex('x').all_match([1])", "expected `list[str]`");
        // The list is checked as a whole before any matching, so a non-string anywhere is an
        // error even when an earlier element decides the result.
        a.fail(
            "regex('x').all_match(['nomatch', 1])",
            "expected `list[str]`",
        );
    }

    #[test]
    fn test_str() {
        let mut a = Assert::new();
        a.globals_add(register_buck_regex);
        a.is_true(
            r#"
str(regex("foo")) == 'regex("foo")'
"#,
        );
    }

    #[test]
    fn test_fancy() {
        let mut a = Assert::new();
        a.globals_add(register_buck_regex);

        a.fail(r"regex('(?=x)')", "not supported");
        a.pass(r"regex('(?=x)', fancy=True)");

        a.is_true(r"regex('^(?=x)x$', fancy=True).match('x')");
        a.is_true(r"regex('(?=x)', fancy=True).any_match(['y', 'x'])");
        a.is_false(r"regex('(?=x)', fancy=True).all_match(['y', 'x'])");
    }

    #[test]
    fn test_as_type() {
        let mut a = Assert::new();
        a.globals_add(register_buck_regex);
        a.is_true("isinstance(regex('foo'), regex)");
        a.is_false("isinstance(1, regex)");
    }

    #[test]
    fn test_replace_all() {
        let mut a = Assert::new();
        a.globals_add(register_buck_regex);

        // Simple replacement
        a.eq(
            r#"regex("foo").replace_all("foo bar foo", "baz")"#,
            r#""baz bar baz""#,
        );

        // No match, should return original string
        a.eq(
            r#"regex("qux").replace_all("foo bar foo", "baz")"#,
            r#""foo bar foo""#,
        );

        // Replacement with capture group
        a.eq(
            r#"regex("f(o+)").replace_all("foo foo", "b$1")"#,
            r#""boo boo""#,
        );

        // Fancy regex replacement
        a.eq(
            r#"regex("(?<=a)b", fancy=True).replace_all("ab ac ab", "X")"#,
            r#""aX ac aX""#,
        );
    }
}
