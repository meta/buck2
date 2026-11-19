/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_util::golden_test_helper::trim_rust_backtrace;

#[derive(Debug, buck2_error::Error)]
#[error("test error")]
#[buck2(tag = Input)]
struct TestError;

fn assert_eq_no_backtrace<T: AsRef<str>, U: AsRef<str>>(a: T, b: U) {
    assert_eq!(
        trim_rust_backtrace(a.as_ref()),
        trim_rust_backtrace(b.as_ref())
    );
}

#[test]
fn test_shows_context() {
    let e = buck2_error::Error::from(TestError)
        .context("context 1")
        .context("context 2");
    assert_eq_no_backtrace(
        format!("{e:?}"),
        r#"context 2

Caused by:
    0: context 1
    1: test error"#,
    );
    assert_eq_no_backtrace(format!("{e:#}"), r#"context 2: context 1: test error"#);
}

#[test]
fn test_with_context_from_source() {
    #[derive(buck2_error::Error, Debug)]
    #[error("with source")]
    #[buck2(tag = Environment)]
    struct E(#[source] TestError);

    let e = buck2_error::Error::from(E(TestError)).context("context");

    assert_eq_no_backtrace(
        format!("{e:?}"),
        r#"context

Caused by:
    0: with source
    1: test error"#,
    );
    assert_eq_no_backtrace(format!("{e:#}"), r#"context: with source: test error"#);
    assert_eq_no_backtrace(format!("{e}"), r#"context"#);
}
