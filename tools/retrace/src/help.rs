/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Command-line help for the `retrace` binary, laid out like buck2's (clap's): `--help` gives
//! each option a paragraph, `-h` a one-line summary. The option table is also the list of flags
//! the binary accepts, so the help cannot drift from the parser.

use std::fmt::Write;

use crate::export::TOPICS;
use crate::sim::MAX_CASCADE_STEPS;

/// One command-line option.
pub struct Opt {
    /// The flag as typed, without its value: `--json`.
    pub flag: &'static str,
    /// How the help shows its value, if it takes one: `=<N>`.
    pub value: Option<&'static str>,
    /// Whether the flag is also accepted without its value, as `--json` is.
    pub value_optional: bool,
    pub summary: &'static str,
    /// The `--help` paragraph; the summary when `None`.
    pub details: Option<&'static str>,
}

pub struct Section {
    pub heading: &'static str,
    /// A line under the heading in `--help`.
    pub note: Option<&'static str>,
    pub opts: &'static [Opt],
}

const fn switch(flag: &'static str, summary: &'static str) -> Opt {
    Opt {
        flag,
        value: None,
        value_optional: false,
        summary,
        details: None,
    }
}

const fn valued(flag: &'static str, value: &'static str, summary: &'static str) -> Opt {
    Opt {
        flag,
        value: Some(value),
        value_optional: false,
        summary,
        details: None,
    }
}

const fn with_details(mut opt: Opt, details: &'static str) -> Opt {
    opt.details = Some(details);
    opt
}

const ABOUT: &str = "Explore a recorded buck2 build: its timeline, critical path, cascades, what-if replays \
     and per-action timing";

const USAGE: &[&str] = &[
    "retrace [OPTIONS] <INPUT>",
    "retrace [OPTIONS] --last",
    "retrace ingest [--out=<DIR>] [--phases] <INPUT|--last>",
];

const INPUT_DETAILS: &str = "A buck2 event log (.pb.zst, .pb.gz, .pb, .proto, .json-lines[.zst|.gz]), a \
     Command ID (the UUID buck2 prints), or a trace JSON retrace wrote earlier. Event logs and \
     Command IDs are converted first, and an unchanged conversion is reused. Record the action \
     graph with `buck2 build --log-action-graph`.";

pub const SECTIONS: &[Section] = &[
    Section {
        heading: "Output Options",
        note: Some("With none of these, retrace opens the timeline window. Choose at most one."),
        opts: &[
            Opt {
                flag: "--json",
                value: Some("[=<TOPIC,...>]"),
                value_optional: true,
                summary: "Print analyses as JSON: the named topics, or all of them",
                details: None,
            },
            switch(
                "--summary",
                "Print what was loaded and the critical path length",
            ),
            switch(
                "--stats",
                "Print a build report: when, where, what, and the machine",
            ),
            switch("--compact", "With --json: print compact JSON"),
            switch("--bench", "With --summary: add load and analysis timings"),
        ],
    },
    Section {
        heading: "Input Options",
        note: None,
        opts: &[
            switch(
                "--last",
                "Open the newest local buck2 log instead of <INPUT>",
            ),
            with_details(
                valued("--out", "=<DIR>", "Where converted files go"),
                "Where converted files go [default: ~/.cache/retrace; for `retrace ingest`: the \
                 current directory]",
            ),
            switch("--phases", "Print how long each conversion phase took"),
            with_details(
                valued(
                    "--graph",
                    "=<PATH>",
                    "The dependency graph for a trace JSON",
                ),
                "The dependency graph for a trace JSON [default: the <id>-graph.json beside it]",
            ),
            with_details(
                valued(
                    "--run",
                    "=<ID|N>",
                    "Which run to analyze, by ID or position",
                ),
                "Which run to analyze when the trace holds several: its ID, or its position",
            ),
            switch(
                "--repair-cycles",
                "Break dependency cycles instead of refusing the graph",
            ),
            with_details(
                valued(
                    "--max-input-bytes",
                    "=<N>",
                    "Refuse inputs larger than N bytes",
                ),
                "Refuse inputs larger than N bytes [default: 34359738368, 32 GiB]",
            ),
            with_details(
                valued("--threads", "=<N>", "Trace-JSON parser workers"),
                "Trace-JSON parser workers: 0 picks up to 16 within the available cores, 1 parses \
                 inline. Converting an event log sizes its own pools. [default: 0]",
            ),
        ],
    },
    Section {
        heading: "Analysis Options",
        note: Some("Each needs --json with the topic it names."),
        opts: &[
            with_details(
                valued("--topk", "=<N>", "Rows per table"),
                "Rows per table [default: 20]",
            ),
            valued(
                "--target",
                "=<LABEL>",
                "With --json=blast: the target whose downstream reach to show",
            ),
            with_details(
                valued(
                    "--action",
                    "=<NAME|action:N>",
                    "With --json=action-detail: the action by name or index",
                ),
                "With --json=action-detail: the action to describe, by exact name or by index",
            ),
            with_details(
                valued(
                    "--cascade-steps",
                    "=<N>",
                    "With --json=cp-cascade: how many steps to take",
                ),
                "With --json=cp-cascade: how many steps to take, 1 to 20. Each step removes the \
                 longest action on the current critical path and recomputes it. [default: 20]",
            ),
            with_details(
                valued(
                    "--simulate",
                    "=<NAME|node:N>",
                    "With --json=cp-rank: replay with one action changed",
                ),
                "With --json=cp-rank: replay the build with one action changed. An exact name \
                 wins; otherwise a unique substring is accepted; node:N picks a graph node.",
            ),
            with_details(
                valued(
                    "--scale",
                    "=<F>",
                    "With --simulate: multiply that action's duration by F",
                ),
                "With --simulate: multiply that action's duration by F, from 0 to 1000 [default: \
                 0, which removes it]",
            ),
            with_details(
                valued(
                    "--simulate-config",
                    "=free-admission",
                    "With --json=cp-rank: replay without admission waits",
                ),
                "With --json=cp-rank: replay the build with every execution-admission wait removed",
            ),
        ],
    },
];

const HELP_OPT: (&str, &str, &str) = (
    "-h, --help",
    "Print help (see more with '--help')",
    "Print help (see a summary with '-h')",
);

const EXAMPLES: &[(&str, &str)] = &[
    (
        "retrace <command-id>",
        "Open a build in the timeline window",
    ),
    ("retrace --last", "Open the newest local build"),
    (
        "retrace <log> --json=critical-path --compact",
        "Print the critical path as JSON",
    ),
    (
        "retrace <log> --json=cp-cascade --cascade-steps=10",
        "Take the top 10 items off the critical path",
    ),
    (
        "retrace ingest --last --out=/tmp/rt",
        "Convert without opening the window",
    ),
];

/// Every flag the binary accepts, as its parser matches them: entries ending in `=` match by
/// prefix, the rest by equality.
pub fn known_flags() -> Vec<String> {
    SECTIONS
        .iter()
        .flat_map(|s| s.opts)
        .flat_map(|opt| match opt.value {
            None => vec![opt.flag.to_owned()],
            Some(_) if opt.value_optional => vec![opt.flag.to_owned(), format!("{}=", opt.flag)],
            Some(_) => vec![format!("{}=", opt.flag)],
        })
        .collect()
}

/// The `retrace ingest` flags: it reaches none of the analysis or window options.
pub const INGEST_FLAGS: &[&str] = &["--last", "--out=", "--phases"];

const WIDTH: usize = 100;
const DETAIL_INDENT: usize = 10;

struct Style {
    on: bool,
}

impl Style {
    fn heading(&self, text: &str) -> String {
        if self.on {
            format!("\x1b[1;4m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }

    fn literal(&self, text: &str) -> String {
        if self.on {
            format!("\x1b[1m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }
}

/// Word-wraps `text` to `WIDTH`, indenting every line by `indent`; the first line starts at
/// column `first_col` (after text already written on it).
fn wrap(text: &str, indent: usize, first_col: usize) -> String {
    let mut out = String::new();
    let mut col = first_col;
    let mut line_empty = true;
    for word in text.split_whitespace() {
        if !line_empty && col + 1 + word.len() > WIDTH {
            out.push('\n');
            out.push_str(&" ".repeat(indent));
            col = indent;
            line_empty = true;
        }
        if !line_empty {
            out.push(' ');
            col += 1;
        }
        out.push_str(word);
        col += word.len();
        line_empty = false;
    }
    out
}

fn opt_display(opt: &Opt) -> String {
    format!("{}{}", opt.flag, opt.value.unwrap_or(""))
}

/// The help text: the `--help` form when `long`, the `-h` summary otherwise. `styled` adds
/// bold and underline for a terminal.
pub fn render(long: bool, styled: bool) -> String {
    let style = Style { on: styled };
    let mut out = String::new();
    let _ = writeln!(out, "{}\n", wrap(ABOUT, 0, 0));
    for (i, usage) in USAGE.iter().enumerate() {
        let lead = if i == 0 {
            format!("{} ", style.heading("Usage:"))
        } else {
            "       ".to_owned()
        };
        let _ = writeln!(out, "{lead}{}", style.literal(usage));
    }

    let _ = writeln!(out, "\n{}", style.heading("Commands:"));
    let _ = writeln!(
        out,
        "  {}  Convert an event log into retrace's files without opening the window",
        style.literal("ingest")
    );

    let _ = writeln!(out, "\n{}", style.heading("Arguments:"));
    if long {
        let _ = writeln!(
            out,
            "  {}\n{}{}",
            style.literal("<INPUT>"),
            " ".repeat(DETAIL_INDENT),
            wrap(INPUT_DETAILS, DETAIL_INDENT, DETAIL_INDENT)
        );
    } else {
        let _ = writeln!(
            out,
            "  {}  An event log, a Command ID or a trace JSON (see more with '--help')",
            style.literal("<INPUT>")
        );
    }

    // `-h` aligns every summary on one column, as clap does.
    let column = SECTIONS
        .iter()
        .flat_map(|s| s.opts)
        .map(|o| 6 + opt_display(o).len())
        .chain([2 + HELP_OPT.0.len()])
        .max()
        .unwrap_or(0)
        + 2;
    let topics = format!("Topics: {}", TOPICS.join(", "));

    for section in SECTIONS {
        let _ = writeln!(out, "\n{}", style.heading(&format!("{}:", section.heading)));
        if long && let Some(note) = section.note {
            let _ = writeln!(out, "  {}\n", wrap(note, 2, 2));
        }
        for (i, opt) in section.opts.iter().enumerate() {
            let shown = opt_display(opt);
            if long && i > 0 {
                out.push('\n');
            }
            if long {
                let mut details = opt.details.unwrap_or(opt.summary).to_owned();
                if opt.flag == "--json" {
                    details = format!("{details}. {topics}");
                }
                let _ = writeln!(
                    out,
                    "      {}\n{}{}",
                    style.literal(&shown),
                    " ".repeat(DETAIL_INDENT),
                    wrap(&details, DETAIL_INDENT, DETAIL_INDENT)
                );
            } else {
                let pad = column.saturating_sub(6 + shown.len());
                let _ = writeln!(
                    out,
                    "      {}{}{}",
                    style.literal(&shown),
                    " ".repeat(pad),
                    wrap(opt.summary, column, column)
                );
            }
        }
    }

    let _ = writeln!(out, "\n{}", style.heading("Options:"));
    if long {
        let _ = writeln!(
            out,
            "  {}\n{}{}",
            style.literal(HELP_OPT.0),
            " ".repeat(DETAIL_INDENT),
            HELP_OPT.2
        );
    } else {
        let pad = column.saturating_sub(2 + HELP_OPT.0.len());
        let _ = writeln!(
            out,
            "  {}{}{}",
            style.literal(HELP_OPT.0),
            " ".repeat(pad),
            HELP_OPT.1
        );
    }

    if long {
        let _ = writeln!(out, "\n{}", style.heading("Examples:"));
        let width = EXAMPLES.iter().map(|(e, _)| e.len()).max().unwrap_or(0) + 2;
        for (example, what) in EXAMPLES {
            let _ = writeln!(
                out,
                "  {}{}{}",
                style.literal(example),
                " ".repeat(width - example.len()),
                what
            );
        }
        let note = format!(
            "Cascade steps are capped at {MAX_CASCADE_STEPS}. Commands without completed actions \
             open a command profile; the action graph and simulations don't apply."
        );
        let _ = writeln!(out, "\n{}", wrap(&note, 0, 0));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_flag_is_in_both_forms() {
        let long = render(true, false);
        let short = render(false, false);
        for opt in SECTIONS.iter().flat_map(|s| s.opts) {
            assert!(long.contains(opt.flag), "{} missing from --help", opt.flag);
            assert!(short.contains(opt.flag), "{} missing from -h", opt.flag);
        }
        for topic in TOPICS {
            assert!(long.contains(topic), "topic {topic} missing from --help");
        }
    }

    #[test]
    fn lines_fit_the_width_and_carry_no_styling_when_plain() {
        for text in [render(true, false), render(false, false)] {
            for line in text.lines() {
                assert!(line.len() <= WIDTH, "too wide: {line}");
            }
            assert!(!text.contains('\x1b'), "plain output has no escape codes");
        }
        assert!(render(true, true).contains("\x1b[1;4mUsage:"));
    }

    #[test]
    fn known_flags_match_the_parser_conventions() {
        let flags = known_flags();
        let has = |f: &str| flags.iter().any(|k| k == f);
        assert!(
            has("--json") && has("--json="),
            "--json takes an optional value"
        );
        assert!(has("--out=") && !has("--out"), "--out needs a value");
        assert!(
            has("--compact") && !has("--compact="),
            "--compact is a switch"
        );
        for flag in INGEST_FLAGS {
            assert!(has(flag), "{flag} is also a main flag");
        }
    }
}
