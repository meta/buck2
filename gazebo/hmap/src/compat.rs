/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use crate::PathStyle;

pub(crate) fn split_legacy_path(value: &str) -> (&str, &str) {
    let mut separator = value.rfind(['\\', '/']).unwrap_or(0);
    if separator != 0 {
        separator += 1;
    }
    (
        value.get(..separator).expect("separator is a byte index"),
        value.get(separator..).expect("separator is a byte index"),
    )
}

pub(crate) fn split_hmaptool_path(value: &str, path_style: PathStyle) -> (&str, &str) {
    if matches!(path_style, PathStyle::Windows) {
        return split_windows_path(value);
    }

    let separator = value
        .char_indices()
        .rev()
        .find(|(_, character)| *character == '/')
        .map(|(index, character)| index + character.len_utf8());
    match separator {
        Some(index) => value.split_at(index),
        None => ("", value),
    }
}

fn split_windows_path(value: &str) -> (&str, &str) {
    let character = |index: usize| value.chars().nth(index);
    let remainder_start = if character(0).is_some_and(is_windows_separator) {
        if character(1).is_some_and(is_windows_separator) {
            let Some(remainder_start) = windows_unc_remainder_start(value) else {
                return (value, "");
            };
            remainder_start
        } else {
            1
        }
    } else if character(1) == Some(':') {
        match value.char_indices().nth(2) {
            Some((index, character)) if is_windows_separator(character) => index + 1,
            Some((index, _)) => index,
            None => value.len(),
        }
    } else {
        0
    };

    let suffix_start = value[remainder_start..]
        .char_indices()
        .rev()
        .find(|(_, character)| is_windows_separator(*character))
        .map_or(remainder_start, |(index, character)| {
            remainder_start + index + character.len_utf8()
        });
    value.split_at(suffix_start)
}

fn windows_unc_remainder_start(value: &str) -> Option<usize> {
    let extended_unc_prefix = ['\\', '\\', '?', '\\', 'U', 'N', 'C', '\\'];
    let is_extended_unc = value
        .chars()
        .take(extended_unc_prefix.len())
        .map(|character| {
            if character == '/' {
                '\\'
            } else {
                character.to_ascii_uppercase()
            }
        })
        .eq(extended_unc_prefix);
    let server_character_start = if is_extended_unc { 8 } else { 2 };
    let server_separator_byte_index = value
        .char_indices()
        .skip(server_character_start)
        .find(|(_, character)| is_windows_separator(*character))?
        .0;
    let share_byte_start = server_separator_byte_index + 1;
    let share_separator_relative_byte_index = value[share_byte_start..]
        .char_indices()
        .find(|(_, character)| is_windows_separator(*character))?
        .0;
    Some(share_byte_start + share_separator_relative_byte_index + 1)
}

fn is_windows_separator(character: char) -> bool {
    character == '/' || character == '\\'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_paths_match_python_ntpath() {
        let cases = [
            ("", ("", "")),
            ("header.h", ("", "header.h")),
            (
                "generated\\nested\\header.h",
                ("generated\\nested\\", "header.h"),
            ),
            (
                "generated/nested/header.h",
                ("generated/nested/", "header.h"),
            ),
            ("C:", ("C:", "")),
            ("C:header.h", ("C:", "header.h")),
            ("C:\\generated\\header.h", ("C:\\generated\\", "header.h")),
            (
                "C:\\generated\\\\header.h",
                ("C:\\generated\\\\", "header.h"),
            ),
            ("\\rooted\\header.h", ("\\rooted\\", "header.h")),
            ("\\\\server", ("\\\\server", "")),
            ("\\\\server\\share", ("\\\\server\\share", "")),
            ("\\\\server\\share\\", ("\\\\server\\share\\", "")),
            (
                "\\\\server\\share\\header.h",
                ("\\\\server\\share\\", "header.h"),
            ),
            ("//server/share//header.h", ("//server/share//", "header.h")),
            (
                "\\\\?\\C:\\generated\\header.h",
                ("\\\\?\\C:\\generated\\", "header.h"),
            ),
            (
                "//?/unc/server/share/header.h",
                ("//?/unc/server/share/", "header.h"),
            ),
            (
                "\\\\.\\C:\\generated\\header.h",
                ("\\\\.\\C:\\generated\\", "header.h"),
            ),
            (
                "C:\\generated\\雪\\naïve.hpp",
                ("C:\\generated\\雪\\", "naïve.hpp"),
            ),
        ];
        for (path, expected) in cases {
            assert_eq!(expected, split_windows_path(path), "{path}");
        }
    }
}
