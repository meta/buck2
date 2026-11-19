/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Reclassifies a failed action command as an infra error when its stderr shows the failure came
//! from shared infrastructure: the kernel, the filesystem, RE, dotslash. A match adds
//! `ActionCommandInfraFailure` and the specific tag instead of `ActionCommandFailure`.
//!
//! Only add patterns for shared infrastructure failures that are not practical to tag directly at
//! their source, and only match stable text that infrastructure produces, never text a user's own
//! tool or script could print. Anything a rule author can recognize belongs in the action's
//! Starlark `error_handler` instead.

use buck2_error::ErrorTag;

/// A fragment of infrastructure-produced stderr and the tag a failed command earns for it.
struct InfraErrorPattern {
    /// Matched case-insensitively as a substring of the command's stderr.
    pattern: &'static str,
    tag: ErrorTag,
    /// If the stderr also contains this, the entry does not match. Other entries still apply.
    exclusion: Option<&'static str>,
}

impl InfraErrorPattern {
    fn matches(&self, stderr_lower: &str) -> bool {
        stderr_lower.contains(self.pattern)
            && !self
                .exclusion
                .is_some_and(|exclusion| stderr_lower.contains(exclusion))
    }
}

const INFRA_PATTERNS: &[InfraErrorPattern] = &[
    InfraErrorPattern {
        pattern: "transport endpoint is not connected",
        tag: ErrorTag::IoNotConnected,
        exclusion: None,
    },
    InfraErrorPattern {
        pattern: "out of memory",
        tag: ErrorTag::ActionOom,
        exclusion: None,
    },
    // Windows reports commit-charge exhaustion (RAM plus pagefile) with this text when any
    // allocation or file mapping fails, so it is a machine-wide condition like OOM.
    InfraErrorPattern {
        pattern: "paging file is too small",
        tag: ErrorTag::ActionOom,
        exclusion: None,
    },
    InfraErrorPattern {
        pattern: "input/output error",
        tag: ErrorTag::IoInputOutputError,
        exclusion: None,
    },
    // dotslash prefixes its own failures with this before the wrapped tool ever runs, most often
    // because the tool's artifact expired from the artifact store. A dotslash file with no entry
    // for the current platform is reported the same way but is a bug in the rule or toolchain that
    // invokes it, so it stays attributed to the user.
    InfraErrorPattern {
        pattern: "dotslash error:",
        tag: ErrorTag::DotslashError,
        exclusion: Some("error when parsing dotslash file"),
    },
];

/// Tags for an action whose command exited non-zero. If the command's stderr matches an infra
/// pattern this is `ActionCommandInfraFailure` plus the specific tag, which sets the error's tier;
/// otherwise it is `ActionCommandFailure`, which marks the error as an input (user) error.
pub(crate) fn command_failure_tags(
    last_command: Option<&buck2_data::CommandExecution>,
) -> Vec<ErrorTag> {
    match check_infra_error_patterns(last_command) {
        Some(infra_tag) => vec![ErrorTag::ActionCommandInfraFailure, infra_tag],
        None => vec![ErrorTag::ActionCommandFailure],
    }
}

fn check_infra_error_patterns(
    last_command: Option<&buck2_data::CommandExecution>,
) -> Option<ErrorTag> {
    let stderr = last_command
        .and_then(|c| c.details.as_ref())
        .map_or("", |d| d.cmd_stderr.as_str());

    let stderr_lower = stderr.to_lowercase();
    INFRA_PATTERNS
        .iter()
        .find(|p| p.matches(&stderr_lower))
        .map(|p| p.tag)
}

#[cfg(test)]
mod tests {
    use buck2_error::ErrorTag;

    use crate::actions::errors::infra_error_handler::check_infra_error_patterns;
    use crate::actions::errors::infra_error_handler::command_failure_tags;

    #[test]
    fn test_command_failure_tags() {
        assert_eq!(
            command_failure_tags(Some(&command_with_stderr("fatal error: Out of memory"))),
            vec![ErrorTag::ActionCommandInfraFailure, ErrorTag::ActionOom],
        );
        assert_eq!(
            command_failure_tags(Some(&command_with_stderr(
                "main.cpp:1:1: error: expected `;`"
            ))),
            vec![ErrorTag::ActionCommandFailure],
        );
    }

    fn command_with_stderr(stderr: &str) -> buck2_data::CommandExecution {
        buck2_data::CommandExecution {
            details: Some(buck2_data::CommandExecutionDetails {
                cmd_stderr: stderr.to_owned(),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn test_host_read_fault_is_infra() {
        let stderr = "Exception in thread \"main\" java.lang.RuntimeException: zipalign failed to process apk file:\n\
             W zip     : fread 32768 bytes failed, Input/output error\n\
             W zip     : copy of 'resources.arsc' failed\n";

        assert_eq!(
            check_infra_error_patterns(Some(&command_with_stderr(stderr))),
            Some(ErrorTag::IoInputOutputError),
            "EIO raised inside the action should be tagged as an environment error"
        );
    }

    #[test]
    fn test_malformed_input_is_not_infra() {
        let stderr = "Exception in thread \"main\" java.lang.RuntimeException: zipalign failed to process apk file:\n\
             Unable to open 'intermediate.apk' as zip archive\n";

        assert_eq!(
            check_infra_error_patterns(Some(&command_with_stderr(stderr))),
            None,
            "A malformed input is a user error, not an infra failure"
        );
    }

    #[test]
    fn test_existing_patterns_still_match() {
        assert_eq!(
            check_infra_error_patterns(Some(&command_with_stderr(
                "error: Transport endpoint is not connected"
            ))),
            Some(ErrorTag::IoNotConnected),
        );
        assert_eq!(
            check_infra_error_patterns(Some(&command_with_stderr("fatal error: Out of memory"))),
            Some(ErrorTag::ActionOom),
        );
    }

    #[test]
    fn test_dotslash_failure_is_environment() {
        let stderr = "dotslash error: problem with `fbcode/tools/build/buck/wrappers/bin/fbcc_rust`\n\
             caused by: failed to download artifact into cache `/home/user/.cache/dotslash` artifact location `/home/user/.cache/dotslash/obj/cas/d0/318634b943`\n\
             \n\
             This failed because the artifact likely no longer exists.\n\
             caused by: backend fetch failed\n\
             caused by: failed to fetch CAS digest from `https://re-cas-prod-l7.internal.tfbnw.net:8443/cas/use_cases/dotslash_msdk_prod/digests/d0318634b943/440154`\n\
             caused by: 404 Not Found\n";

        assert_eq!(
            check_infra_error_patterns(Some(&command_with_stderr(stderr))),
            Some(ErrorTag::DotslashError),
            "dotslash failing to run the tool is an environment error, not a user error"
        );
    }

    #[test]
    fn test_dotslash_unsupported_platform_is_not_infra() {
        let stderr = "dotslash error: problem with `xplat/arfx/skylight/tools/skyscript/bin/skyscript`\n\
             caused by: error when parsing DotSlash file\n\
             caused by: platform not supported\n\
             caused by: expected `windows`, `windows/x86_64` - but found `linux`, `macos`\n";

        assert_eq!(
            check_infra_error_patterns(Some(&command_with_stderr(stderr))),
            None,
            "A dotslash file that lacks the current platform is a rule bug, not an environment error"
        );
    }

    #[test]
    fn test_dotslash_exclusion_does_not_mask_other_infra_patterns() {
        let stderr = "dotslash error: problem with `xplat/arfx/skylight/tools/skyscript/bin/skyscript`\n\
             caused by: error when parsing DotSlash file\n\
             caused by: Input/output error\n";

        assert_eq!(
            check_infra_error_patterns(Some(&command_with_stderr(stderr))),
            Some(ErrorTag::IoInputOutputError),
            "The dotslash exclusion only skips the dotslash entry; other infra patterns still apply"
        );
    }

    #[test]
    fn test_windows_commit_limit_is_infra() {
        let stderr = "error[E0786]: found invalid metadata files for crate `std`\n\
             = note: failed to mmap file 'D:\\buck-out\\v2\\art\\libstd-348d8c37.rlib': The paging file is too small for this operation to complete. (os error 1455)\n";

        assert_eq!(
            check_infra_error_patterns(Some(&command_with_stderr(stderr))),
            Some(ErrorTag::ActionOom),
            "Exhausted Windows commit charge is an environment error, not a user error"
        );
    }

    #[test]
    fn test_compile_failure_is_not_infra() {
        assert_eq!(
            check_infra_error_patterns(Some(&command_with_stderr(
                "main.cpp:12:5: error: use of undeclared identifier `foo`"
            ))),
            None,
            "A compile error should stay attributed to the user"
        );
    }

    #[test]
    fn test_no_command_is_not_infra() {
        assert_eq!(check_infra_error_patterns(None), None);
    }
}
