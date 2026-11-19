load("@fbcode//buck2/app:modifier.bzl", "buck2_modifiers")
load("@fbcode_macros//build_defs:clippy_configuration.bzl", "clippy_configuration")
load("@fbcode_macros//build_defs:native_rules.bzl", "alias")
load("@fbsource//tools/target_determinator/macros:ci.bzl", "ci")
load(":defs.bzl", "buck2_bundle", "pagable_transition_alias")

oncall("build_infra")

# Attached to all buck2 rust rules by the PACKAGE file. OSS cargo clippy picks clippy.toml
# up by directory discovery instead.
clippy_configuration(
    name = "clippy_config",
    clippy_toml_src = "clippy.toml",
)

# Need a custom transition here so that buck2 is always built with pagable enabled,
# even if its parent does not have pagable enabled.
pagable_transition_alias(
    name = "buck2",
    actual = "//buck2/app/buck2:buck2-bin",
    labels = ci.labels(
        ci.modifiers(
            [
                "ovr_config//cpu:arm64",
                "ovr_config//build_mode/constraints:dev",
            ],
            ci.skip_test(),
        ),
        ci.modifiers(
            [
                "ovr_config//cpu:x86_64",
                "ovr_config//build_mode/constraints:opt",
            ],
            ci.skip_test(),
        ),
        ci.modifiers(
            [
                "ovr_config//cpu:arm64",
                "ovr_config//build_mode/constraints:opt",
            ],
            ci.skip_test(),
        ),
    ),
    modifiers = buck2_modifiers(),
    # The e2e tests run it. Open source has no `PACKAGE` file that makes it public.
    visibility = ["PUBLIC"],
)

buck2_bundle(
    name = "buck2_bundle",
    buck2 = "//buck2:buck2",
    buck2_client = "//buck2/app/buck2:buck2_client-bin",
    buck2_health_check = "//buck2/buck2_health_check_cli:buck2_health_check_cli",
    modifiers = buck2_modifiers(),
    tpx = "//buck2/buck2_tpx_cli:buck2_tpx_cli",
    visibility = ["PUBLIC"],
)

# For backcompat with bash aliases and so forth
# You can use this target to test custom builds of buck2.
#
# Step 1: `buck2 build @fbcode//mode/opt fbcode//buck2:symlinked_buck2_and_tpx --out ~/buck2`
# Step 2: Use the buck2 binary from `~/buck2/buck2`
#
# If you're testing on macOS, use `@fbcode//mode/opt-mac-arm64`
alias(
    name = "symlinked_buck2_and_tpx",
    actual = ":buck2_bundle",
    modifiers = buck2_modifiers(),
)
