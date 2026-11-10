/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::Once;

use buck2_core::deprecation;
use buck2_core::error::StructuredErrorOptions;
use buck2_core::error::initialize;
use buck2_core::soft_error;
use buck2_error::buck2_error;

static RESULT: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn mock_handler(
    category: &str,
    err: &buck2_error::Error,
    loc: (&str, u32, u32),
    _context: &Arc<buck2_core::error::SoftErrorContext>,
    options: StructuredErrorOptions,
) {
    RESULT.lock().unwrap().push(format!(
        "{:?}, : {} : {} : {} : {:?}",
        loc, err, category, options.user_facing, options.kind
    ));
}

fn test_init() -> MutexGuard<'static, ()> {
    // Tests in Rust can be executed concurrently, and these tests work with global state,
    // so use mutex to ensure we only run one test at a time.
    static TEST_MUTEX: Mutex<()> = Mutex::new(());
    let guard = TEST_MUTEX.lock().unwrap();

    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        initialize(Box::new(mock_handler), Box::new(|| None)).unwrap();
    });

    RESULT.lock().unwrap().clear();

    guard
}

#[test]
fn test_soft_error() -> buck2_error::Result<()> {
    let _guard = test_init();

    let before_error_line = line!();
    soft_error!(
        "test_logged_soft_error",
        buck2_error!(buck2_error::ErrorTag::Input, "Should be logged").into(),
    );
    assert_eq!(
        Some(&format!(
            "({:?}, {}, 5), : Should be logged : test_logged_soft_error : false : SoftError",
            file!(),
            before_error_line + 1,
        )),
        RESULT.lock().unwrap().first()
    );
    Ok(())
}

#[test]
fn test_deprecation() -> buck2_error::Result<()> {
    let _guard = test_init();

    let before_error_line = line!();
    deprecation!(
        "test_logged_deprecation",
        buck2_error!(buck2_error::ErrorTag::Input, "Still used").into(),
        owner: "jakobdegen",
        started: "2026-10-25",
    );
    assert_eq!(
        Some(&format!(
            "({:?}, {}, 5), : Still used : test_logged_deprecation : false : Deprecation {{ owner: \"jakobdegen\", started: \"2026-10-25\" }}",
            file!(),
            before_error_line + 1,
        )),
        RESULT.lock().unwrap().first()
    );
    Ok(())
}

#[test]
fn test_counter_limit() -> buck2_error::Result<()> {
    let _guard = test_init();

    assert_eq!(0, RESULT.lock().unwrap().len(), "Sanity check");

    for _ in 0..100 {
        soft_error!(
            "test_counter_limit",
            buck2_error!(buck2_error::ErrorTag::Input, "Message").into()
        );
    }

    assert_eq!(
        10,
        RESULT.lock().unwrap().len(),
        "Should be logged 10 times"
    );
    Ok(())
}
