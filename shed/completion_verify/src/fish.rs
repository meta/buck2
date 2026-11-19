/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io;
use std::path::Path;

use crate::runtime::FishRuntime;

fn parse_completions(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| {
            let completion = line
                .split_once('\t')
                .map_or(line, |(completion, _)| completion);
            (!completion.is_empty()).then(|| completion.to_owned())
        })
        .collect()
}

fn replace_last_word(input: &str, completion: &str) -> String {
    let last_word = input
        .rsplit_once(|c: char| c.is_ascii_whitespace())
        .map_or(input, |(_, last_word)| last_word);
    format!("{}{completion}", &input[..input.len() - last_word.len()])
}

pub(crate) fn run_fish(
    completion_name: &str,
    script: &str,
    input: &str,
    tempdir: &Path,
) -> io::Result<Vec<String>> {
    let home = tempdir;

    let mut r = FishRuntime::new(home.to_owned())?;
    r.register(completion_name, script)?;

    let completions = parse_completions(&r.complete(input)?);
    let [completion] = completions.as_slice() else {
        return Ok(completions);
    };

    let last_word = input
        .rsplit_once(|c: char| c.is_ascii_whitespace())
        .map_or(input, |(_, last_word)| last_word);
    if !completion.ends_with('/') || completion == last_word {
        return Ok(completions);
    }

    // Fish leaves a trailing-slash completion active instead of terminating it with a space.
    // Query the completed command line once more to match the existing two-Tab behavior.
    Ok(parse_completions(
        &r.complete(&replace_last_word(input, completion))?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_completions_strips_descriptions() {
        assert_eq!(
            parse_completions("car1\tdescription\ncar2\n\n"),
            vec!["car1".to_owned(), "car2".to_owned()],
        );
    }

    #[test]
    fn test_replace_last_word() {
        assert_eq!(
            replace_last_word("buck2 build other/", "root//other/"),
            "buck2 build root//other/",
        );
    }
}
