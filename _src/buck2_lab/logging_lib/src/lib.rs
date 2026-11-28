/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

pub fn info(message: &str) {
    println!("[INFO] {}", message);
}

pub fn warning(message: &str) {
    println!("\x1b[33m[WARNING] {}\x1b[0m", message);
}

pub fn error(message: &str) {
    println!("\x1b[31m[ERROR] {}\x1b[0m", message);
}
