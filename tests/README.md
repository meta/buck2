# buck2 e2e tests

This directory contains tests for buck2. Primary constituents:

 - `core/` - the primary and fully endorsed set of integration tests for buck2 core code. If you're
   working on buck2 itself, this is probably what you want.
 - `e2e_util/` - the test framework for the integration tests.
 - `e2e/` and `meta_only/e2e` - a hodgepodge of tests covering a combination of buck2 itself, the
   prelude, some macros, and various integrations. Avoid if possible. Strongly avoid in favor of
   `core/` if testing buck2 core.
 - `targets/` - target definitions accessed by `e2e` tests.
 - `prelude/` - there is currently no fully endorsed testing strategy for the prelude. This
   directory is an attempt at creating one, however its still immature and there are gaps.
   Trendsetters are welcome to try it.
 - An assortment of other things that mostly shouldn't be here.

`core` and `prelude` tests are visible in open source but not executed there.

## Writing a test

A test module is a `BuckTestCase` subclass (`e2e_util/buck_workspace.py`); each test is an
`async def test_*(self, buck: Buck)` method decorated with `@buck_test(...)`, which runs it with a
`Buck` on a fresh daemon (and a `tmp_path: Path` if the method takes one). `@parametrize(...)`
runs a method once per set of arguments, as pytest's did. The test targets are `python_unittest`s:
`buck2 test <target> -- --regex <name>` runs one test.

A test module can be written in Rust instead, on the harness in `e2e_util/rust` (see its crate
documentation): `test_foo.rs` takes the place of `test_foo.py`, next to the same `test_foo_data`.
Each test is an `async fn test_*(buck: &Buck)` marked `#[buck_test(...)]`, whose options are those
of `@buck_test(...)`, written `name = value`. The test targets are `rust_unittest`s.

A module still written for pytest is converted with
`buck2 run fbcode//buck2/tests/tools/pytest_to_unittest:pytest_to_unittest -- <files>`.

Some of these directories have their own `README.md` files.
