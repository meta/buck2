/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::ops::Deref;
use std::str::CharIndices;
use std::str::Chars;

use dupe::Dupe;
use nom::Compare;
use nom::CompareResult;
use nom::Input;
use nom::Needed;
use nom::Offset;

/// Similar to `LocatedSpan` from `nom_locate`, but without line number.
#[derive(Clone, Debug, PartialEq, Copy, Dupe)]
pub struct Span<'a> {
    offset: usize,
    fragment: &'a str,
}

impl Deref for Span<'_> {
    type Target = str;

    fn deref(&self) -> &str {
        self.fragment
    }
}

impl<'a> Span<'a> {
    pub(crate) fn new(s: &'a str) -> Span<'a> {
        Span {
            offset: 0,
            fragment: s,
        }
    }

    pub fn fragment(&self) -> &'a str {
        self.fragment
    }

    pub fn location_offset(&self) -> usize {
        self.offset
    }
}

impl<'a> Input for Span<'a> {
    type Item = char;
    type Iter = Chars<'a>;
    type IterIndices = CharIndices<'a>;

    fn input_len(&self) -> usize {
        self.fragment.len()
    }

    fn take(&self, index: usize) -> Self {
        Span {
            offset: self.offset,
            fragment: &self.fragment[..index],
        }
    }

    fn take_from(&self, index: usize) -> Self {
        Span {
            offset: self.offset + index,
            fragment: &self.fragment[index..],
        }
    }

    fn take_split(&self, index: usize) -> (Self, Self) {
        let (front, back) = self.fragment.split_at(index);
        (
            Span {
                offset: self.offset + index,
                fragment: back,
            },
            Span {
                offset: self.offset,
                fragment: front,
            },
        )
    }

    fn position<P>(&self, predicate: P) -> Option<usize>
    where
        P: Fn(Self::Item) -> bool,
    {
        self.fragment.find(predicate)
    }

    fn iter_elements(&self) -> Self::Iter {
        self.fragment.chars()
    }

    fn iter_indices(&self) -> Self::IterIndices {
        self.fragment.char_indices()
    }

    fn slice_index(&self, count: usize) -> Result<usize, Needed> {
        Input::slice_index(&self.fragment, count)
    }
}

impl Offset for Span<'_> {
    fn offset(&self, second: &Self) -> usize {
        let fst = self.offset;
        let snd = second.offset;

        snd - fst
    }
}

impl<'a> Compare<&'a str> for Span<'a> {
    fn compare(&self, t: &'a str) -> CompareResult {
        self.fragment.compare(t)
    }

    fn compare_no_case(&self, t: &'a str) -> CompareResult {
        self.fragment.compare_no_case(t)
    }
}
