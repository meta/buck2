/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The fbcode mode files inplace tests build with.

/// A build mode that has a mode file for every operating system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Dev,
    Opt,
}

/// The fbcode mode file for `mode` on the operating system the test runs on, as an argument:
/// `@fbcode//mode/dev`, `@fbcode//mode/mac`, `@fbcode//mode/opt-win`, ... As
/// `get_mode_from_platform(mode)` in the Python harness.
pub fn get_mode_from_platform(mode: Mode) -> &'static str {
    mode_file(std::env::consts::OS, mode)
}

fn mode_file(os: &str, mode: Mode) -> &'static str {
    match (os, mode) {
        ("macos", Mode::Dev) => "@fbcode//mode/mac",
        ("macos", Mode::Opt) => "@fbcode//mode/opt-mac",
        ("windows", Mode::Dev) => "@fbcode//mode/win",
        ("windows", Mode::Opt) => "@fbcode//mode/opt-win",
        (_, Mode::Dev) => "@fbcode//mode/dev",
        (_, Mode::Opt) => "@fbcode//mode/opt",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_linux_uses_the_plain_mode_files() {
        assert_eq!(mode_file("linux", Mode::Dev), "@fbcode//mode/dev");
        assert_eq!(mode_file("linux", Mode::Opt), "@fbcode//mode/opt");
    }

    #[test]
    fn test_macos_and_windows_have_mode_files_of_their_own() {
        assert_eq!(mode_file("macos", Mode::Dev), "@fbcode//mode/mac");
        assert_eq!(mode_file("macos", Mode::Opt), "@fbcode//mode/opt-mac");
        assert_eq!(mode_file("windows", Mode::Dev), "@fbcode//mode/win");
        assert_eq!(mode_file("windows", Mode::Opt), "@fbcode//mode/opt-win");
    }

    #[test]
    fn test_this_os_gets_its_mode_file() {
        let expected = if cfg!(target_os = "macos") {
            "@fbcode//mode/mac"
        } else if cfg!(windows) {
            "@fbcode//mode/win"
        } else {
            "@fbcode//mode/dev"
        };
        assert_eq!(get_mode_from_platform(Mode::Dev), expected);
    }
}
