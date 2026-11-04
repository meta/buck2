/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Write;

use buck2_build_api::interpreter::rule_defs::cmd_args::HASH_VERSION;
use indoc::indoc;

use crate::interpreter::rule_defs::cmd_args::testing::tester;

/// What a command line's hash (`CommandLineBuilder::new_hashing`) promises: the same hash twice
/// for the same command line, and never the same hash for two command lines that render
/// differently. Equal renders may still hash differently; that only costs a cache miss.
#[test]
fn distinct_renders_have_distinct_hashes() -> buck2_error::Result<()> {
    let mut tester = tester()?;
    tester.run_starlark_bzl_test(indoc!(
        r#"
        def test():
            a1 = source_artifact("foo", "bar/baz.h")
            a2 = bound_artifact("//:dep1", "dir/baz.h")
            variants = [
                cmd_args("x"),
                cmd_args("x", "y"),
                cmd_args("xy"),
                cmd_args("x y"),
                cmd_args(["x", "y"], delimiter = " "),
                cmd_args(["x", "y"], delimiter = ""),
                cmd_args("x", format = "--{}"),
                cmd_args("--x"),
                cmd_args("x", prepend = "-I"),
                cmd_args("-I", "x"),
                cmd_args("x y", quote = "shell"),
                cmd_args("'x y'"),
                cmd_args(a1),
                cmd_args("foo/bar/baz.h"),
                cmd_args(a1, format = "-I{}"),
                cmd_args(a1, parent = 1),
                cmd_args(a1, relative_to = a2),
                cmd_args(a1, relative_to = (a2, 1)),
                cmd_args(a2),
                cmd_args(a1, absolute_prefix = "/root/"),
                cmd_args(a1, absolute_suffix = "!"),
                cmd_args(a1, a2),
                cmd_args(a2, a1),
                cmd_args(cmd_args(a1, format = "-I{}"), "z"),
                cmd_args(a1, "z", format = "-I{}"),
                cmd_args(a1, hidden = a2),
                cmd_args(a1, ignore_artifacts = True),
                cmd_args(a1, replace_regex = ("baz", "qux")),
                cmd_args(a1, replace_regex = ("bar", "qux")),
            ]
            renders = [get_args(v) for v in variants]
            hashes = [hash_cli_args(v) for v in variants]
            for i in range(len(variants)):
                if hashes[i] != hash_cli_args(variants[i]):
                    fail("hashing variant {} twice gave different hashes".format(i))
                for j in range(len(variants)):
                    if renders[i] != renders[j] and hashes[i] == hashes[j]:
                        fail("variants {} and {} render as {} and {} but hash the same".format(i, j, renders[i], renders[j]))
        "#
    ))?;
    Ok(())
}

/// Golden hashes over `corpus()`, keyed by `HASH_VERSION`: the hash of everything the regular
/// builder renders for it, and the hash the hashing builder gives it. A change to how anything
/// renders moves the first; a change to the hashing builder moves the second. Either means a
/// command line's hash no longer stands for what it renders, so entries keyed by hashes from
/// before the change must not be found after it: bump `HASH_VERSION` and add a pair under the
/// new version. Never edit a pair in place.
const GOLDEN: &[(u32, &str, &str)] = &[
    (
        0,
        "cfbedc058843ca652b8a2ca71b485284ca2593379cd8ab49444f14ecb1bb9cb7",
        "aefdd4564d5d2cf0844775524ae338f8f3d59b464e8b10d9c09192ec77ab25af",
    ),
    (
        1,
        "121136ff5f19572a7bda1cfa79e40367eb214c847bb039e234242f02c22c0de9",
        "8ec6a718b8680b09d3202af390783daca46de71d594a4ed8d24b79ddaaca9943",
    ),
];

#[test]
fn golden_corpus() -> buck2_error::Result<()> {
    let (version, render, hash) = *GOLDEN.last().unwrap();
    assert_eq!(
        version, HASH_VERSION,
        "a new `HASH_VERSION` needs its golden pair appended to `GOLDEN`"
    );
    let mut tester = tester()?;
    tester.run_starlark_bzl_test(&format!(
        indoc!(
            r#"
            def test():
                a1 = source_artifact("foo", "bar/baz.h")
                a2 = bound_artifact("//:dep1", "dir/baz.h")
                a3 = source_artifact("foo", "qux/x y.txt")
                a4 = bound_artifact("//:dep2", "out.o")
                variants = [
            {corpus}
                ]
                renders = []
                hashes = []
                for v in variants:
                    renders.extend(get_args(v))
                    renders.append("<end of variant>")
                    hashes.append(hash_cli_args(v))
                if blake3_hex(renders) != "{render}":
                    fail("rendering changed: the corpus now renders to {{}}; bump HASH_VERSION".format(blake3_hex(renders)))
                if blake3_hex(hashes) != "{hash}":
                    fail("hashing changed: the corpus now hashes to {{}}; bump HASH_VERSION".format(blake3_hex(hashes)))
            "#
        ),
        corpus = corpus(),
        render = render,
        hash = hash,
    ))?;
    Ok(())
}

/// A deterministic corpus of `cmd_args` expressions: strings and artifacts of several shapes,
/// every option, nested up to three deep. The same text on every host and every run, so that the
/// goldens above mean something; broadened only together with a new golden pair.
fn corpus() -> String {
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            // xorshift64*, seeded once; `rand` would do too but this cannot change under us.
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545F4914F6CDD1D)
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
        fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
            items[self.below(items.len())]
        }
        fn chance(&mut self, one_in: usize) -> bool {
            self.below(one_in) == 0
        }
    }
    const STRINGS: &[&str] = &[
        r#""x""#,
        r#""y""#,
        r#""a b""#,
        r#""-I""#,
        r#""--flag=1""#,
        r#""{}""#,
        r#""'q'""#,
        r#""\"dq\"""#,
        r#""$HOME""#,
        r#""path/to/file""#,
        r#""""#,
        r#""baz""#,
    ];
    const ARTIFACTS: &[&str] = &["a1", "a2", "a3", "a4"];

    fn expr(rng: &mut Rng, depth: usize, out: &mut String) {
        out.push_str("cmd_args(");
        let items = 1 + rng.below(4);
        let mut artifacts_only = true;
        for i in 0..items {
            if i > 0 {
                out.push_str(", ");
            }
            match rng.below(if depth < 3 { 3 } else { 2 }) {
                0 => {
                    out.push_str(rng.pick(STRINGS));
                    artifacts_only = false;
                }
                1 => out.push_str(rng.pick(ARTIFACTS)),
                _ => {
                    expr(rng, depth + 1, out);
                    artifacts_only = false;
                }
            }
        }
        if rng.chance(3) {
            write!(
                out,
                ", delimiter = {}",
                rng.pick(&[r#""""#, r#"" ""#, r#"",""#, r#""\n""#])
            )
            .unwrap();
        }
        if rng.chance(3) {
            write!(
                out,
                ", format = {}",
                rng.pick(&[r#""--{}""#, r#""-I{}""#, r#""{}.o""#, r#""pre{}post""#])
            )
            .unwrap();
        }
        if rng.chance(4) {
            write!(out, ", prepend = {}", rng.pick(&[r#""-I""#, r#""-D""#])).unwrap();
        }
        if rng.chance(4) {
            out.push_str(r#", quote = "shell""#);
        }
        if rng.chance(5) {
            out.push_str(r#", absolute_prefix = "/abs/""#);
        }
        if rng.chance(5) {
            out.push_str(r#", absolute_suffix = "/end""#);
        }
        if rng.chance(4) {
            write!(
                out,
                ", replace_regex = ({}, {})",
                rng.pick(&[r#""ba""#, r#""x""#, r#""/""#]),
                rng.pick(&[r#""zz""#, r#""""#, r#""_""#])
            )
            .unwrap();
        }
        if rng.chance(5) {
            write!(out, ", hidden = [{}]", rng.pick(ARTIFACTS)).unwrap();
        }
        if artifacts_only {
            if rng.chance(3) {
                write!(out, ", parent = {}", 1 + rng.below(2)).unwrap();
            }
            if rng.chance(3) {
                write!(
                    out,
                    ", relative_to = {}",
                    rng.pick(&["a2", "(a4, 1)", "a1"])
                )
                .unwrap();
            }
            if rng.chance(4) {
                out.push_str(", ignore_artifacts = True");
            }
        }
        out.push(')');
    }

    let mut rng = Rng(0x9E3779B97F4A7C15);
    let mut out = String::new();
    for _ in 0..400 {
        out.push_str("        ");
        expr(&mut rng, 1, &mut out);
        out.push_str(",\n");
    }
    out
}
