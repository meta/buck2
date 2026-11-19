# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Basic test-level provider for passing both the raw artifacts
# and an artifact with its own associated artifact
Artifacts = provider(fields = ["associated", "artifacts"])

def _artifacts(ctx: AnalysisContext) -> list[Provider]:
    out1 = ctx.actions.write("out1", "", has_content_based_path = False)
    out2 = ctx.actions.write("out2", "", has_content_based_path = False)
    return [
        DefaultInfo(),
        Artifacts(
            associated = out1.with_associated_artifacts([out2]),
            artifacts = [out1, out2],
        ),
    ]

# Builds two artifacts which have a dependency relationship
# Such that out1 -> out2
artifacts = rule(impl = _artifacts, attrs = {})

def _check_all_exists(ctx: AnalysisContext) -> list[Provider]:
    dep1 = ctx.attrs.dep1[Artifacts]
    dep2 = ctx.attrs.dep2[Artifacts]

    # Those are the paths we'll check.
    all_artifacts = []
    all_artifacts.extend(dep1.artifacts)
    all_artifacts.extend(dep2.artifacts)

    inputs = []

    # Use associated artifacts on declared artifact from dep1
    inputs.append(dep1.associated)

    # Use associated artifacts on frozen artifact from dep2
    (out1, out2) = dep2.artifacts
    inputs.append(out1.with_associated_artifacts([out2]))

    check = ctx.actions.declare_output("check", has_content_based_path = False)
    ctx.actions.run(
        [
            "fbpython",
            "-c",
            ";".join([
                "import sys, os",
                "assert all(os.path.exists(f) for f in sys.argv[1:])",
                "open(os.environ['OUT'], 'w')",
            ]),
            # Include all artifacts for the purpose of testing but don't add a dependency here.
            cmd_args(all_artifacts, ignore_artifacts = True),
            # Include the command args similar to how a user would
            cmd_args(hidden = inputs),
        ],
        env = {"OUT": check.as_output()},
        category = "check",
    )

    return [DefaultInfo(check)]

check_all_exists = rule(impl = _check_all_exists, attrs = {"dep1": attrs.dep(), "dep2": attrs.dep()})

def _check_dropped_artifacts(ctx: AnalysisContext) -> list[Provider]:
    # Describe a transitive chain of artifacts such that
    # third -> second -> first
    first = ctx.actions.write("first", "", has_content_based_path = False)
    second = ctx.actions.write("second", "", has_content_based_path = False).with_associated_artifacts([first])
    third = ctx.actions.write("third", "", has_content_based_path = False).with_associated_artifacts([second])

    check = ctx.actions.declare_output("check", has_content_based_path = False)
    ctx.actions.run(
        [
            "fbpython",
            "-c",
            ";".join([
                # Verify the existing behavior where the named file exists,
                # and the declared artifact of the named file exists, but
                # the artifact of the artifact does NOT exist
                # TODO(T227006457) - revisit this behavior to be more intuitive
                "import os, sys",
                "third, second, first = sys.argv[1:]",
                "assert os.path.exists(third)",
                "assert os.path.exists(second)",
                "assert not os.path.exists(first)",
                "open(os.environ['OUT'], 'w')",
            ]),
            # Include all artifacts for the purpose of testing but don't add a dependency here.
            cmd_args([third, second, first], ignore_artifacts = True),
            # Only specify 'third' as a dependency so as to see what artifacts get pulled in
            cmd_args(hidden = [third]),
        ],
        env = {"OUT": check.as_output()},
        category = "check",
    )

    return [DefaultInfo(check)]

check_dropped_artifacts = rule(impl = _check_dropped_artifacts, attrs = {})

def defs():
    # Generates :dep1 with output out1 and associated artifact out2
    artifacts(name = "dep1")

    # Generates :dep2 with output out1 and associated artifact out2
    artifacts(name = "dep2")

    # Checks that all of [dep1/out1, dep1/out2, dep2/out1, dep2,out2]
    # are actually present in the rule's cmd_args as requested
    check_all_exists(name = "check_artifacts", dep1 = ":dep1", dep2 = ":dep2")

    check_dropped_artifacts(name = "check_dropped_artifacts")
