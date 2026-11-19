/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_cli_proto::client_context::HostArchOverride;
use buck2_cli_proto::client_context::HostPlatformOverride;
use buck2_core::soft_error;
use buck2_error::BuckErrorContext;
use buck2_interpreter::extra::InterpreterHostArchitecture;
use buck2_interpreter::extra::InterpreterHostPlatform;
use buck2_interpreter::extra::xcode::XcodeVersionInfo;

pub fn get_host_info(
    host_platform: HostPlatformOverride,
    host_arch: HostArchOverride,
    host_xcode_override: &Option<String>,
) -> buck2_error::Result<(
    InterpreterHostPlatform,
    InterpreterHostArchitecture,
    Option<XcodeVersionInfo>,
)> {
    let interpreter_platform = match host_platform {
        HostPlatformOverride::Linux => InterpreterHostPlatform::Linux,
        HostPlatformOverride::MacOs => InterpreterHostPlatform::MacOS,
        HostPlatformOverride::Windows => InterpreterHostPlatform::Windows,
        HostPlatformOverride::DefaultPlatform => match std::env::consts::OS {
            "linux" => InterpreterHostPlatform::Linux,
            "macos" => InterpreterHostPlatform::MacOS,
            "windows" => InterpreterHostPlatform::Windows,
            "freebsd" => InterpreterHostPlatform::FreeBsd,
            _ => InterpreterHostPlatform::Unknown,
        },
    };
    // TODO(nga): this checks binary architecture, not runtime architecture.
    //   When x86_64 binary is running on aarch64, this will return x86_64.
    let interpreter_architecture = match host_arch {
        HostArchOverride::AArch64 => InterpreterHostArchitecture::AArch64,
        HostArchOverride::X8664 => InterpreterHostArchitecture::X86_64,
        HostArchOverride::DefaultArch => match std::env::consts::ARCH {
            "aarch64" => InterpreterHostArchitecture::AArch64,
            "x86_64" => InterpreterHostArchitecture::X86_64,
            "riscv64" => InterpreterHostArchitecture::Riscv64,
            "arm" => InterpreterHostArchitecture::Arm,
            "x86" => InterpreterHostArchitecture::X86,
            "mips" => InterpreterHostArchitecture::Mips,
            "mips64" => InterpreterHostArchitecture::Mips64,
            "powerpc" => InterpreterHostArchitecture::PowerPc,
            "powerpc64" => InterpreterHostArchitecture::PowerPc64,
            _ => InterpreterHostArchitecture::Unknown,
        },
    };

    let interpreter_xcode_version = match host_xcode_override {
        Some(s) => Some(
            XcodeVersionInfo::from_version_and_build(s.as_str())
                .buck_error_context("Constructing `XcodeVersionInfo` from string.")?,
        ),
        None if interpreter_platform == InterpreterHostPlatform::MacOS => {
            match XcodeVersionInfo::new()
                .buck_error_context("Constructing `XcodeVersionInfo` using host platform MacOS.")
            {
                Ok(v) => v,
                Err(e) => {
                    soft_error!("invalid_xcode_version", e);
                    None
                }
            }
        }
        _ => None,
    };

    Ok((
        interpreter_platform,
        interpreter_architecture,
        interpreter_xcode_version,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_host_info_create_xcode_override() {
        let linux = InterpreterHostPlatform::Linux;
        let linux_override = HostPlatformOverride::Linux;
        let mac = InterpreterHostPlatform::MacOS;
        let mac_override = HostPlatformOverride::MacOs;
        let aarch64 = InterpreterHostArchitecture::AArch64;
        let aarch64_override = HostArchOverride::AArch64;
        let xcode_str = "14.3.0-14C18";
        let xcode_string = Some(xcode_str.to_owned());

        let xcode_override_on_linux =
            get_host_info(linux_override, aarch64_override, &xcode_string).unwrap();
        let want1 = (
            linux,
            aarch64,
            XcodeVersionInfo::from_version_and_build(xcode_str).ok(),
        );
        assert_eq!(xcode_override_on_linux, want1);

        let xcode_override_on_mac =
            get_host_info(mac_override, aarch64_override, &xcode_string).unwrap();
        let want2 = (
            mac,
            aarch64,
            XcodeVersionInfo::from_version_and_build(xcode_str).ok(),
        );
        assert_eq!(xcode_override_on_mac, want2);
    }
}
