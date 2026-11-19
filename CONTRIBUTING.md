# Contributing to Buck2

We want to make contributing to this project as easy and transparent as
possible.

## Our Development Process

Buck2 is currently developed in Meta's internal repositories. Code that is
developed internally gets reviewed, sent through CI, committed, and then
automatically mirrored out to GitHub every 15 minutes. Code that arrives through
a PR is reviewed by a Meta developer on GitHub, then once accepted, moved into
our internal workflow where it is reviewed, sent through CI, committed and added
to the repo. We maintain both external CI (the results of which are visible on
GitHub) and a more thorough internal CI (building internal projects etc). Alas,
our full test suite is not yet mirrored to the open source repo, but we hope to
fix that in due course.

## Pull Requests

We actively welcome your pull requests.

1. Fork the repo and create your branch from `main`.
2. If you've added code that should be tested, add tests.
3. If you've changed APIs, update the documentation.
4. Ensure the test suite passes.
5. Make sure your code passes any lints.
6. Format your code with `python3 fmt.py` (see below).
7. If you haven't already, complete the Contributor License Agreement ("CLA").

## Formatting

Run `python3 fmt.py` before you push. It formats Rust and Starlark exactly as
Meta's internal formatter does, so a pull request that passes the "Format"
check needs no Rust or Starlark reformatting when it is imported;
`python3 fmt.py --check` shows what would change without touching anything.

Rust is formatted by the nightly `rustfmt` pinned in `.rustfmt-version`, not
by the toolchain in `rust-toolchain.toml`, so plain `cargo fmt` can disagree
with the check; `fmt.py` installs the pinned `rustfmt` through `rustup` on
first use. Starlark (`.bzl`, `.bxl`, `BUCK`, `PACKAGE`, ...) is formatted by
[`tools/starlark_fmt`](./tools/starlark_fmt/README.md), built from this tree.

## Contributor License Agreement ("CLA")

In order to accept your pull request, we need you to submit a CLA. You only need
to do this once to work on any of Meta's open source projects.

Complete your CLA here: <https://code.facebook.com/cla>

## Issues

We use GitHub issues to track public bugs. Please ensure your description is
clear and has sufficient instructions to be able to reproduce the issue.

Meta has a [bounty program](https://www.facebook.com/whitehat/) for the safe
disclosure of security bugs. In those cases, please go through the process
outlined on that page and do not file a public issue.

## Coding Style, Compiling, and Hacking Pro-tips

See [here](./docs/developers/basics.md) for the nitty gritty on how to contribute.

## License

By contributing to Buck2, you agree that your contributions will be licensed
under both the [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE)
files in the root directory of this source tree.
