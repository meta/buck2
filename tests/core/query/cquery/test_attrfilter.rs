/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeSet;

use buck2_e2e::prelude::*;

#[buck_test]
async fn test_configured_attribute_filters(buck: &Buck) -> anyhow::Result<()> {
    let cases: &[(&str, &[&str])] = &[
        ("attrfilter(text, hello, root//:plain)", &["root//:plain"]),
        (
            "attrregexfilter(text, '^he.lo$', root//:plain)",
            &["root//:plain"],
        ),
        ("attrfilter(strings, last, root//:plain)", &["root//:plain"]),
        ("attrfilter(strings, missing, root//:plain)", &[]),
        (
            "attrfilter(text, fallback, root//:defaults)",
            &["root//:defaults"],
        ),
        ("attrfilter(strings, anything, root//:defaults)", &[]),
        ("attrfilter(missing_attribute, anything, root//:plain)", &[]),
        (
            "attrfilter(strings, prefix, root//:selected)",
            &["root//:selected"],
        ),
        (
            "attrfilter(strings, selected, root//:selected)",
            &["root//:selected"],
        ),
        ("attrfilter(strings, unselected, root//:selected)", &[]),
        (
            "attrfilter(text, helloworld, root//:selected)",
            &["root//:selected"],
        ),
        ("attrfilter(text, hello, root//:selected)", &[]),
        ("attrfilter(mixed, nested, root//:mixed)", &["root//:mixed"]),
        ("attrfilter(mixed, '17', root//:mixed)", &["root//:mixed"]),
        (
            "attrregexfilter(buck.package, 'TARGETS.fixture$', root//:plain)",
            &["root//:plain"],
        ),
    ];
    for (query, expected) in cases {
        let result = buck
            .cquery(&[query, "--target-platforms=root//:chosen"])
            .await?;
        let actual: BTreeSet<&str> = result
            .stdout
            .lines()
            .map(|line| line.split(" (").next().unwrap_or_default())
            .collect();
        let expected: BTreeSet<&str> = expected.iter().copied().collect();
        assert_eq!(actual, expected, "{query}: {}", result.stdout);
    }

    let result = buck
        .cquery(&[
            "attrfilter(strings, selected, root//:selected)",
            "--target-platforms=root//:other",
        ])
        .await?;
    assert_eq!(result.stdout, "");
    let result = buck
        .cquery(&[
            "attrfilter(strings, unselected, root//:selected)",
            "--target-platforms=root//:other",
        ])
        .await?;
    assert!(result.stdout.starts_with("root//:selected ("));
    Ok(())
}
