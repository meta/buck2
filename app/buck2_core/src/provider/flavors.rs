/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Buck1 `//pkg:target#flavor` labels.
//!
//! Buck1 used flavors for two different things: selecting a platform (`#android-arm64`,
//! `#platform010-clang`) and naming a secondary output (`#shared`, `#compilation-database`).
//! In buck2 the first is a configuration and the second is a subtarget (`//pkg:target[shared]`).
//! Buck2 no longer accepts the syntax; this module turns a flavored label into an error that
//! says what to write instead. See T147637580.

use std::fmt;
use std::sync::OnceLock;

use regex::RegexSet;

static PLATFORM_REGEX_SET: OnceLock<RegexSet> = OnceLock::new();

/// Whether `flavor` named a buck1 platform.
fn is_platform_flavor(flavor: &str) -> bool {
    let regex_set = PLATFORM_REGEX_SET.get_or_init(|| {
        // copied from https://fburl.com/code/sgxwq68n and modified for our needs
        RegexSet::new([
            r"^android-.*$",
            r"^(linux-.*|platform[0-9]{3}-(clang|clang-12|gcc)(-nosan)?(-split-dwarf)?)$",
            r"^macosx-(x86_64|arm64)(_minimal_xcode)?$",
            // Too broad pattern, so ignoring it
            // "^(default|host)$"
            r"^windows-x86_64$",
            //  an original data has the following pattern: FBOBJC_REGEX = "^(macosx|iphone|watch|appletv|osmeta-).*$"
            // we use modified version w/o macosx prefix already checked in the pattern above
            r"^(iphone|watch|appletv|osmeta-).*$",
        ])
        .unwrap()
    });
    regex_set.is_match(flavor)
}

/// The buck2 spelling a flavored label should be replaced with.
#[derive(Debug, PartialEq, Eq)]
enum FlavorFix {
    /// Replace `#flavors` with `[subtarget]`.
    Subtarget(&'static str),
    /// Delete `#flavors`.
    Remove,
    /// No buck2 equivalent.
    Unsupported,
}

impl fmt::Display for FlavorFix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FlavorFix::Subtarget(name) => write!(
                f,
                "write the subtarget as `[{name}]` instead (platforms come from mode files, `--target-platforms` or `-m`, not from the label)"
            ),
            FlavorFix::Remove => write!(
                f,
                "it has no effect in buck2; remove it (platforms come from mode files, `--target-platforms` or `-m`)"
            ),
            FlavorFix::Unsupported => {
                write!(f, "buck2 has no equivalent for it")
            }
        }
    }
}

/// The buck2 spelling of the buck1 flavors that were in common use, keyed by the sorted flavor
/// list with any platform flavor removed.
fn fix_for_subtarget_flavors(flavors: &[&str]) -> FlavorFix {
    use FlavorFix::Remove;
    use FlavorFix::Subtarget;
    use FlavorFix::Unsupported;

    match flavors {
        [] => Remove,

        // android_binary intermediate outputs
        ["package_string_assets"] => Subtarget("package_string_assets"),
        ["aapt2_link"] => Subtarget("aapt2_link"),
        ["unstripped_native_libraries"] => Subtarget("unstripped_native_libraries"),
        ["proguard_text_output"] => Subtarget("proguard_text_output"),
        ["generate_string_resources"] => Subtarget("generate_string_resources"),
        ["generate_voltron_string_resources"] => Subtarget("generate_voltron_string_resources"),
        ["exo_symlink_tree"] => Subtarget("exo_symlink_tree"),

        // android_library intermediate outputs
        ["dummy_r_dot_java"] => Subtarget("dummy_r_dot_java"),

        // cxx_library header-only dep
        ["headers"] => Subtarget("headers"),

        // This is used by Rust quite a bit
        ["check"] => Subtarget("check"),

        // Buck2 never implemented strip-debug; the flavor was ignored.
        ["strip-debug"] => Remove,

        // Used in JEX builder script (https://fburl.com/code/2w2gjkey)
        ["shared"] => Subtarget("shared"),

        // Used by Nullsafe for (android|java)_libraries
        ["nullsafex-json"] => Subtarget("nullsafex-json"),

        // Java/Kotlin sub-targets
        ["class-abi"] => Subtarget("class-abi"),
        ["source-abi"] => Subtarget("source-abi"),
        ["source-only-abi"] => Subtarget("source-only-abi"),

        ["compilation-database"] => Subtarget("compilation-database"),

        // For js_bundle rules. The platform and optimization ("release") flavors were ignored.
        ["android"] | ["android", "release"] => Remove,
        ["android", "dependencies"] | ["android", "dependencies", "release"] => {
            Subtarget("dependencies")
        }
        ["android", "misc"] | ["android", "misc", "release"] => Subtarget("misc"),
        ["android", "release", "source_map"] | ["android", "source_map"] => Subtarget("source_map"),

        _ => Unsupported,
    }
}

fn fix_for_flavors(flavors: &str) -> FlavorFix {
    let mut rest: Vec<&str> = Vec::new();
    let mut seen_platform = false;
    for part in flavors.split(',') {
        // Buck1 allowed several platform flavors on one label (`#macosx-x86_64,macosx-arm64` for a
        // fat binary); only one platform can be replaced by the configuration.
        if is_platform_flavor(part) && !seen_platform {
            seen_platform = true;
        } else {
            rest.push(part);
        }
    }
    rest.sort_unstable();
    fix_for_subtarget_flavors(&rest)
}

/// The error for a label that carries a buck1 `#flavors` suffix.
///
/// The message names the buck2 spelling for the flavors buck2 used to map (`#shared` →
/// `[shared]`, platform flavors → nothing) so the fix is mechanical.
pub fn flavor_error(flavors: &str, full_target: &str) -> buck2_error::Error {
    let fix = fix_for_flavors(flavors);
    buck2_error::buck2_error!(
        buck2_error::ErrorTag::Buck1Flavor,
        "buck1 flavor `#{flavors}` on `{full_target}` is not supported: {fix}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_flavor_alone_is_removed() {
        for flavor in [
            "android-arm64",
            "platform010-clang",
            "platform010-clang-split-dwarf",
            "linux-x86_64",
            "macosx-arm64",
            "macosx-x86_64_minimal_xcode",
            "windows-x86_64",
            "iphonesimulator-arm64",
            // The prefix match swallows the whole flavor, `-tests` included.
            "iphoneos-arm64-tests",
            "watchos-arm64_32",
        ] {
            assert_eq!(fix_for_flavors(flavor), FlavorFix::Remove, "{flavor}");
        }
    }

    #[test]
    fn platform_flavor_with_subtarget() {
        assert_eq!(
            fix_for_flavors("shared,windows-x86_64"),
            FlavorFix::Subtarget("shared")
        );
        // Order on the label does not matter.
        assert_eq!(
            fix_for_flavors("android-armv7,shared"),
            FlavorFix::Subtarget("shared")
        );
    }

    #[test]
    fn second_platform_flavor_is_unsupported() {
        assert_eq!(
            fix_for_flavors("macosx-x86_64,macosx-arm64"),
            FlavorFix::Unsupported
        );
    }

    #[test]
    fn subtarget_flavors() {
        for (flavors, subtarget) in [
            ("shared", "shared"),
            ("headers", "headers"),
            ("check", "check"),
            ("compilation-database", "compilation-database"),
            ("unstripped_native_libraries", "unstripped_native_libraries"),
            ("proguard_text_output", "proguard_text_output"),
            ("class-abi", "class-abi"),
            ("android,dependencies", "dependencies"),
            ("android,release,dependencies", "dependencies"),
            ("release,android,misc", "misc"),
            ("android,release,source_map", "source_map"),
        ] {
            assert_eq!(
                fix_for_flavors(flavors),
                FlavorFix::Subtarget(subtarget),
                "{flavors}"
            );
        }
    }

    #[test]
    fn no_op_flavors() {
        for flavors in ["strip-debug", "android", "android,release"] {
            assert_eq!(fix_for_flavors(flavors), FlavorFix::Remove, "{flavors}");
        }
    }

    #[test]
    fn unrecognized_flavors() {
        for flavors in [
            "link-tree",
            "ios,release,dependencies",
            "default,py3,shared",
            "9.json",
        ] {
            assert_eq!(
                fix_for_flavors(flavors),
                FlavorFix::Unsupported,
                "{flavors}"
            );
        }
    }

    #[test]
    fn error_message_names_the_fix() {
        let err = flavor_error("android-arm64,shared", "//pkg:lib#android-arm64,shared");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("`#android-arm64,shared` on `//pkg:lib#android-arm64,shared`"),
            "{msg}"
        );
        assert!(msg.contains("`[shared]`"), "{msg}");
        assert!(err.has_tag(buck2_error::ErrorTag::Buck1Flavor));
    }
}
