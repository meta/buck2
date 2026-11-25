/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;

use buck2_hash::BuckMutMap;
use dupe::Dupe;
use fancy_regex::Regex;
use fancy_regex::RegexBuilder;

/// The DFA size limit of the regex `filter()` matches target names with.
pub(crate) const FILTER_NAME_DFA_SIZE_LIMIT: usize = 100 << 20;

#[cfg(test)]
thread_local! {
    /// How many regexes this thread has compiled.
    pub(crate) static COMPILED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// `Regex::new(pattern)`, with `delegate_dfa_size_limit` if given.
pub(crate) fn compile_regex(
    pattern: &str,
    dfa_size_limit: Option<usize>,
) -> Result<Regex, fancy_regex::Error> {
    #[cfg(test)]
    COMPILED.set(COMPILED.get() + 1);
    match dfa_size_limit {
        Some(dfa_size_limit) => RegexBuilder::new(pattern)
            .delegate_dfa_size_limit(dfa_size_limit)
            .build(),
        None => Regex::new(pattern),
    }
}

/// The regexes one traversal's filter has compiled, by pattern and DFA size
/// limit. The filter is evaluated for every node the traversal expands, with
/// the same patterns each time.
#[derive(Default)]
pub(crate) struct FilterRegexes(Mutex<BuckMutMap<(String, Option<usize>), Arc<Regex>>>);

impl FilterRegexes {
    pub(crate) fn get_or_compile(
        &self,
        pattern: String,
        dfa_size_limit: Option<usize>,
    ) -> Result<Arc<Regex>, fancy_regex::Error> {
        let key = (pattern, dfa_size_limit);
        let mut regexes = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(regex) = regexes.get(&key) {
            return Ok(regex.dupe());
        }
        let regex = Arc::new(compile_regex(&key.0, dfa_size_limit)?);
        regexes.insert(key, regex.dupe());
        Ok(regex)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::FilterRegexes;

    #[test]
    fn test_a_pattern_is_compiled_once_per_dfa_size_limit() {
        let regexes = FilterRegexes::default();
        let first = regexes.get_or_compile("a.c".to_owned(), None).unwrap();
        let second = regexes.get_or_compile("a.c".to_owned(), None).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert!(first.is_match("xabcx").unwrap());

        let limited = regexes
            .get_or_compile("a.c".to_owned(), Some(1 << 20))
            .unwrap();
        assert!(!Arc::ptr_eq(&first, &limited));
        let other = regexes.get_or_compile("a.d".to_owned(), None).unwrap();
        assert!(!Arc::ptr_eq(&first, &other));
    }

    #[test]
    fn test_an_invalid_pattern_is_an_error_each_time() {
        let regexes = FilterRegexes::default();
        assert!(regexes.get_or_compile("(".to_owned(), None).is_err());
        assert!(regexes.get_or_compile("(".to_owned(), None).is_err());
    }
}
