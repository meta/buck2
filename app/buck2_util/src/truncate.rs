/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

const TRUNCATION_MSG: &str = "<<omitted>>";
const TRUNCATION_DELIM: &str = ", ";

/// Quick and dirty function to either print the full debug message, or
/// the debug message with the middle elided if it's too long.
/// `max_length` is maximum length of truncated message.
pub fn truncate(msg: &str, max_length: usize) -> String {
    if msg.len() <= max_length {
        return msg.to_owned();
    }

    if let Some(max_length_without_truncation_msg) = max_length.checked_sub(TRUNCATION_MSG.len()) {
        // Note that for Unicode strings we might end up with less than max_length bytes,
        // because these functions are all in terms of bytes and a character that straddles a cut
        // point is dropped rather than kept.
        // Not worth the hassle to do better, given how rare that is.
        format!(
            "{}{}{}",
            &msg[0..msg.floor_char_boundary(max_length_without_truncation_msg / 2)],
            TRUNCATION_MSG,
            &msg[msg.ceil_char_boundary(msg.len() - (max_length_without_truncation_msg / 2))
                ..msg.len()]
        )
    } else {
        TRUNCATION_MSG.to_owned()
    }
}

/// Truncate a container of strings and return the comma-separated string representation. The end of
/// the collection is elided if the length is too long.
/// `max_length` is maximum length of truncated message excluding the truncation placeholder message.
pub fn truncate_container<T: AsRef<str>, Iter: IntoIterator<Item = T>>(
    iter: Iter,
    max_length: usize,
) -> String {
    let mut items = iter.into_iter();
    let mut result = String::new();

    match items.next() {
        None => (),
        Some(first) => {
            let first = first.as_ref();
            if first.len() > max_length {
                result.push_str(&first[0..first.floor_char_boundary(max_length)]);
                result.push_str(TRUNCATION_MSG);
                return result;
            } else {
                result.push_str(first);

                for v in items {
                    let v = v.as_ref();
                    if result.len() + TRUNCATION_DELIM.len() + v.len() > max_length {
                        result.push_str(
                            &TRUNCATION_DELIM[0..std::cmp::min(
                                max_length.saturating_sub(result.len()),
                                TRUNCATION_DELIM.len(),
                            )],
                        );
                        result.push_str(&v[0..v.floor_char_boundary(max_length - result.len())]);
                        result.push_str(TRUNCATION_MSG);
                        return result;
                    }

                    result.push_str(TRUNCATION_DELIM);
                    result.push_str(v);
                }
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const MSG: &str = "rdeps(set(fbcode//buck2/... fbsource//xplat/buck2/..., fbsource//fbobjc/buck2/...), fbcode//buck2/cli:buck2)";

    #[test]
    fn test_truncate() {
        assert_eq!(&truncate(MSG, 0), "<<omitted>>");
        assert_eq!(&truncate(MSG, TRUNCATION_MSG.len()), "<<omitted>>");
        assert_eq!(&truncate(MSG, 30), "rdeps(set<<omitted>>li:buck2)");
        assert_eq!(
            &truncate(MSG, 50),
            "rdeps(set(fbcode//b<<omitted>>e//buck2/cli:buck2)"
        );
    }

    #[test]
    fn test_truncate_container() {
        let mut vec = Vec::new();
        for n in 1..50 {
            vec.push(n.to_string())
        }
        assert_eq!(
            &truncate_container(vec.iter().map(|e| &**e), 0),
            "<<omitted>>"
        );
        assert_eq!(
            &truncate_container(vec.iter().map(|e| &**e), 60),
            "1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 1<<omitted>>"
        );

        let vec = ["123", "456"];

        assert_eq!(
            &truncate_container(vec.iter().copied(), 6),
            "123, 4<<omitted>>"
        );

        let vec = ["1234567890", "456"];

        assert_eq!(
            &truncate_container(vec.iter().copied(), 6),
            "123456<<omitted>>"
        );

        let vec = ["❤️🧡💛💚💙💜", "東京都"];

        assert_eq!(
            &truncate_container(vec.iter().copied(), 35),
            "❤\u{fe0f}🧡💛💚💙💜, 東京<<omitted>>"
        );
    }

    #[test]
    fn test_truncate_multibyte_result_length() {
        let max_length = TRUNCATION_MSG.len() + 2;

        let truncated = truncate(&"\u{e9}".repeat(20), max_length);
        assert_eq!(truncated, "<<omitted>>");
        assert!(truncated.len() <= max_length);

        let truncated = truncate(&"\u{1f600}".repeat(4), max_length);
        assert_eq!(truncated, "<<omitted>>");
        assert!(truncated.len() <= max_length);

        // Whole characters that fit in each half are kept.
        let max_length = TRUNCATION_MSG.len() + 6;
        let truncated = truncate(&"\u{6771}".repeat(10), max_length);
        assert_eq!(truncated, "\u{6771}<<omitted>>\u{6771}");
        assert_eq!(truncated.len(), max_length);
    }

    #[test]
    fn test_truncate_unicode() {
        // This will segfault if we try and split within a char boundary
        truncate(&"❤️🧡💛💚💙💜".repeat(500), 100);
        truncate(&"東京都".repeat(500), 100);
    }
}
