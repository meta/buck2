/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::fs::project::ProjectRootTemp;
use buck2_core::target::label::label::TargetLabel;
use buck2_node::nodes::frontend::TargetGraphCalculation;
use buck2_node::nodes::unconfigured::TargetNode;
use buck2_node::visibility::VisibilitySpecification;
use buck2_settings::PackageVisibilityDefaultIntersection;

use crate::tests::calculation;
use crate::tests::calculation_with_package_visibility_mode;

const RULES_BZL: &str = r#"
simple = rule(
    impl = lambda ctx: fail(),
    attrs = {},
)
"#;

async fn target_a_with_package(
    package_body: &str,
    mode: PackageVisibilityDefaultIntersection,
) -> TargetNode {
    let fs = ProjectRootTemp::new().unwrap();

    fs.write_file("rules.bzl", RULES_BZL);
    fs.write_file("juxtaposition/PACKAGE", package_body);
    fs.write_file(
        "juxtaposition/BUCK",
        "load(\"//:rules.bzl\", \"simple\")\nsimple(name = \"a\")\n",
    );

    let ctx = calculation_with_package_visibility_mode(&fs, mode).await;

    ctx.ctx()
        .get_target_node(&TargetLabel::testing_parse("root//juxtaposition:a"))
        .await
        .unwrap()
}

#[tokio::test]
async fn test_package() {
    let fs = ProjectRootTemp::new().unwrap();

    fs.write_file("rules.bzl", RULES_BZL);
    fs.write_file(
        "juxtaposition/PACKAGE",
        r#"
package(
    visibility = ["//aaa/..."],
    within_view = ["//bbb/..."],
    inherit = True,
)
"#,
    );
    fs.write_file(
        "juxtaposition/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(name = "a")
"#,
    );

    let ctx = calculation(&fs).await;

    let a = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//juxtaposition:a"))
        .await
        .unwrap();

    assert_eq!(
        &VisibilitySpecification::testing_parse(&["root//aaa/..."]),
        a.visibility().unwrap(),
    );
}

#[tokio::test]
async fn test_package_target_name_glob_visibility() {
    let fs = ProjectRootTemp::new().unwrap();

    fs.write_file("rules.bzl", RULES_BZL);
    fs.write_file(
        "juxtaposition/PACKAGE",
        r#"
package(
    visibility = [target_name_glob(["*-test"], within = ["//consumer/..."])],
)
"#,
    );
    fs.write_file(
        "juxtaposition/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(name = "a")
"#,
    );

    let ctx = calculation(&fs).await;

    let a = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//juxtaposition:a"))
        .await
        .unwrap();

    let visibility = a.visibility().unwrap();
    assert!(
        visibility
            .0
            .matches_target(&TargetLabel::testing_parse("root//consumer:unit-test"))
            .unwrap()
    );
    assert!(
        !visibility
            .0
            .matches_target(&TargetLabel::testing_parse("root//consumer:library"))
            .unwrap()
    );
    assert!(
        !visibility
            .0
            .matches_target(&TargetLabel::testing_parse("root//other:unit-test"))
            .unwrap()
    );
}

#[tokio::test]
async fn test_package_target_name_glob_multi_visibility() {
    let fs = ProjectRootTemp::new().unwrap();

    fs.write_file("rules.bzl", RULES_BZL);
    fs.write_file(
        "juxtaposition/PACKAGE",
        r#"
package(
    visibility = [
        target_name_glob(["*-test", "*-bench"], within = ["//consumer/..."]),
    ],
)
"#,
    );
    fs.write_file(
        "juxtaposition/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(name = "a")
"#,
    );

    let ctx = calculation(&fs).await;

    let a = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//juxtaposition:a"))
        .await
        .unwrap();

    let visibility = a.visibility().unwrap();
    // Any of the listed globs matches inside the scope.
    assert!(
        visibility
            .0
            .matches_target(&TargetLabel::testing_parse("root//consumer:unit-test"))
            .unwrap(),
        "first glob should match"
    );
    assert!(
        visibility
            .0
            .matches_target(&TargetLabel::testing_parse("root//consumer:micro-bench"))
            .unwrap(),
        "second glob should match"
    );
    // Negative case: name doesn't match either glob.
    assert!(
        !visibility
            .0
            .matches_target(&TargetLabel::testing_parse("root//consumer:library"))
            .unwrap(),
        "non-matching name must not pass"
    );
    // Negative case: matching name outside the scope.
    assert!(
        !visibility
            .0
            .matches_target(&TargetLabel::testing_parse("root//other:unit-test"))
            .unwrap(),
        "outside-scope target must not pass"
    );
}

#[tokio::test]
async fn test_package_target_name_glob_multiple_within() {
    let fs = ProjectRootTemp::new().unwrap();

    fs.write_file("rules.bzl", RULES_BZL);
    fs.write_file(
        "juxtaposition/PACKAGE",
        r#"
package(
    visibility = [
        target_name_glob(["*-test"], within = ["//consumer/...", "//other/..."]),
    ],
)
"#,
    );
    fs.write_file(
        "juxtaposition/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(name = "a")
"#,
    );

    let ctx = calculation(&fs).await;

    let a = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//juxtaposition:a"))
        .await
        .unwrap();

    let visibility = a.visibility().unwrap();
    // Matches inside either listed `within` scope (scopes are OR-ed).
    assert!(
        visibility
            .0
            .matches_target(&TargetLabel::testing_parse("root//consumer:unit-test"))
            .unwrap(),
        "first within scope should match"
    );
    assert!(
        visibility
            .0
            .matches_target(&TargetLabel::testing_parse("root//other:unit-test"))
            .unwrap(),
        "second within scope should match"
    );
    // Negative: name matches but package is in neither scope.
    assert!(
        !visibility
            .0
            .matches_target(&TargetLabel::testing_parse("root//elsewhere:unit-test"))
            .unwrap(),
        "target outside all within scopes must not pass"
    );
    // Negative: package in a scope but the name matches no glob.
    assert!(
        !visibility
            .0
            .matches_target(&TargetLabel::testing_parse("root//consumer:library"))
            .unwrap(),
        "non-matching name must not pass"
    );
}

#[tokio::test]
async fn test_target_name_glob_in_buck_visibility() {
    let fs = ProjectRootTemp::new().unwrap();

    fs.write_file("rules.bzl", RULES_BZL);
    // `target_name_glob` directly on a rule's `visibility` exercises the
    // attribute coercer path, distinct from the `package()` PACKAGE parser.
    fs.write_file(
        "juxtaposition/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(
    name = "a",
    visibility = [target_name_glob(["*-test"], within = ["//consumer/..."])],
)
"#,
    );

    let ctx = calculation(&fs).await;

    let a = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//juxtaposition:a"))
        .await
        .unwrap();

    let visibility = a.visibility().unwrap();
    assert!(
        visibility
            .0
            .matches_target(&TargetLabel::testing_parse("root//consumer:unit-test"))
            .unwrap(),
        "name + scope match should be visible"
    );
    // Negative: name matches but package is outside the within scope.
    assert!(
        !visibility
            .0
            .matches_target(&TargetLabel::testing_parse("root//other:unit-test"))
            .unwrap(),
        "out-of-scope package must not be visible"
    );
    // Negative: package in scope but name matches no glob.
    assert!(
        !visibility
            .0
            .matches_target(&TargetLabel::testing_parse("root//consumer:library"))
            .unwrap(),
        "non-matching name must not be visible"
    );
}

#[tokio::test]
async fn test_package_visibility_rejects_non_str() {
    let fs = ProjectRootTemp::new().unwrap();

    fs.write_file("rules.bzl", RULES_BZL);
    // `42` is neither a string nor a `target_name_glob`; the `Either` unpack in
    // `package()` must reject it rather than silently accepting it.
    fs.write_file("juxtaposition/PACKAGE", "package(visibility = [42])\n");
    fs.write_file(
        "juxtaposition/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(name = "a")
"#,
    );

    let ctx = calculation(&fs).await;

    let err = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//juxtaposition:a"))
        .await
        .expect_err("non-str/non-target_name_glob visibility element must be rejected");
    let msg = format!("{err:?}").to_lowercase();
    assert!(
        msg.contains("target_name_glob") || msg.contains("expected") || msg.contains("int"),
        "expected a type-mismatch error, got: {msg}"
    );
}

#[tokio::test]
async fn test_package_target_name_glob_rejects_public_within() {
    let fs = ProjectRootTemp::new().unwrap();

    fs.write_file("rules.bzl", RULES_BZL);
    // `PUBLIC` is a target name, not a package scope; using it in `within` must
    // be rejected with a clear error rather than a confusing parse failure.
    fs.write_file(
        "juxtaposition/PACKAGE",
        r#"
package(
    visibility = [target_name_glob(["*-test"], within = ["PUBLIC"])],
)
"#,
    );
    fs.write_file(
        "juxtaposition/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(name = "a")
"#,
    );

    let ctx = calculation(&fs).await;

    let err = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//juxtaposition:a"))
        .await
        .expect_err("`PUBLIC` in `within` must be rejected");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("`PUBLIC` is not a valid `within`"),
        "expected PublicNotAllowedInWithin error, got: {msg}"
    );
}

#[tokio::test]
async fn test_package_inherit() {
    let fs = ProjectRootTemp::new().unwrap();

    fs.write_file("rules.bzl", RULES_BZL);
    fs.write_file(
        "PACKAGE",
        r#"
package(
    visibility = ["//aaa/..."],
)
"#,
    );
    fs.write_file(
        "juxtaposition/PACKAGE",
        r#"
package(
    visibility = ["//bbb/..."],
    inherit = True,
)
"#,
    );
    fs.write_file(
        "juxtaposition/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(name = "a")
"#,
    );

    let ctx = calculation(&fs).await;

    let a = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//juxtaposition:a"))
        .await
        .unwrap();

    assert_eq!(
        &VisibilitySpecification::testing_parse(&["root//aaa/...", "root//bbb/..."]),
        a.visibility().unwrap(),
    );
}

#[tokio::test]
async fn test_package_visibility_off_ignores_ordinary_visibility() {
    let a = target_a_with_package(
        "package(\n    visibility = [\"//allowed/...\"],\n)\n",
        PackageVisibilityDefaultIntersection::Off,
    )
    .await;

    assert!(
        a.visibility_intersection().is_unrestricted(),
        "off mode without a marker must leave the intersection empty, got: {}",
        a.visibility_intersection(),
    );
}

#[tokio::test]
async fn test_package_visibility_marker_enforces_when_off() {
    let a = target_a_with_package(
        "enforce_visibility_intersection()\npackage(\n    visibility = [\"//allowed/...\"],\n)\n",
        PackageVisibilityDefaultIntersection::Off,
    )
    .await;

    assert!(
        a.visibility_intersection()
            .matches(&TargetLabel::testing_parse("root//allowed:lib"))
            .unwrap(),
        "marker-selected visibility must restrict to //allowed/..."
    );
    assert!(
        !a.visibility_intersection()
            .matches(&TargetLabel::testing_parse("root//other:lib"))
            .unwrap(),
        "marker-selected visibility must block //other/..."
    );
}

#[tokio::test]
async fn test_package_visibility_enforce_without_marker() {
    let a = target_a_with_package(
        "package(\n    visibility = [\"//allowed/...\"],\n)\n",
        PackageVisibilityDefaultIntersection::Enforce,
    )
    .await;

    assert!(
        a.visibility_intersection()
            .matches(&TargetLabel::testing_parse("root//allowed:lib"))
            .unwrap(),
        "enforce mode must restrict to //allowed/... without a marker"
    );
    assert!(
        !a.visibility_intersection()
            .matches(&TargetLabel::testing_parse("root//other:lib"))
            .unwrap(),
        "enforce mode must block //other/... without a marker"
    );
}

#[tokio::test]
async fn test_package_visibility_audit_computes_intersection_like_enforce() {
    let fs = ProjectRootTemp::new().unwrap();

    fs.write_file("rules.bzl", RULES_BZL);
    fs.write_file(
        "juxtaposition/PACKAGE",
        r#"
package(
    visibility = ["//allowed/..."],
)
"#,
    );
    fs.write_file(
        "juxtaposition/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(name = "a")
# Explicit `PUBLIC` own-visibility isolates the intersection: any blocking
# below comes from the intersection layer, not from the target itself.
simple(name = "b", visibility = ["PUBLIC"])
"#,
    );

    let ctx =
        calculation_with_package_visibility_mode(&fs, PackageVisibilityDefaultIntersection::Audit)
            .await;

    let a = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//juxtaposition:a"))
        .await
        .unwrap();

    assert!(
        a.visibility_intersection()
            .matches(&TargetLabel::testing_parse("root//allowed:lib"))
            .unwrap(),
        "audit mode must restrict to //allowed/... like enforce"
    );
    assert!(
        !a.visibility_intersection()
            .matches(&TargetLabel::testing_parse("root//other:lib"))
            .unwrap(),
        "audit mode must block //other/... like enforce"
    );

    // `b` is `PUBLIC`, so the intersection alone decides.
    let b = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//juxtaposition:b"))
        .await
        .unwrap();
    let allowed = TargetLabel::testing_parse("root//allowed:lib");
    let other = TargetLabel::testing_parse("root//other:lib");
    assert!(b.is_visible_to(&allowed).unwrap());
    assert!(!b.is_visible_to(&other).unwrap());

    let enforce_ctx = calculation_with_package_visibility_mode(
        &fs,
        PackageVisibilityDefaultIntersection::Enforce,
    )
    .await;
    let enforce_b = enforce_ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//juxtaposition:b"))
        .await
        .unwrap();
    for consumer in [&allowed, &other] {
        assert_eq!(
            b.visibility_intersection().matches(consumer).unwrap(),
            enforce_b
                .visibility_intersection()
                .matches(consumer)
                .unwrap(),
            "audit and enforce intersections must agree for {consumer}",
        );
    }
}

#[tokio::test]
async fn test_package_visibility_exempt_targets_skip_layer() {
    let fs = ProjectRootTemp::new().unwrap();

    fs.write_file("rules.bzl", RULES_BZL);
    fs.write_file(
        "etc/PACKAGE",
        r#"
enforce_visibility_intersection()
package(
    visibility = ["//etc/allowed/..."],
    visibility_exempt_targets = ["//etc/legacy:"],
)
"#,
    );
    // `PUBLIC`, so any blocking comes from the intersection layer.
    fs.write_file(
        "etc/legacy/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(name = "lib", visibility = ["PUBLIC"])
"#,
    );
    fs.write_file(
        "etc/other/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(name = "lib", visibility = ["PUBLIC"])
"#,
    );

    let ctx = calculation(&fs).await;

    let legacy = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//etc/legacy:lib"))
        .await
        .unwrap();
    let other = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//etc/other:lib"))
        .await
        .unwrap();
    let outside = TargetLabel::testing_parse("root//elsewhere:x");
    let allowed = TargetLabel::testing_parse("root//etc/allowed:x");

    assert!(
        legacy.is_visible_to(&outside).unwrap(),
        "exempt definer must skip the layer"
    );
    assert!(
        !other.is_visible_to(&outside).unwrap(),
        "non-exempt definer must still be restricted"
    );
    assert!(legacy.is_visible_to(&allowed).unwrap());
    assert!(other.is_visible_to(&allowed).unwrap());
}

#[tokio::test]
async fn test_package_visibility_exempt_targets_do_not_grant_visibility() {
    let fs = ProjectRootTemp::new().unwrap();

    fs.write_file("rules.bzl", RULES_BZL);
    fs.write_file(
        "etc/PACKAGE",
        r#"
enforce_visibility_intersection()
package(
    visibility = ["//etc/allowed/..."],
    visibility_exempt_targets = ["//etc/legacy:"],
)
"#,
    );
    // No explicit visibility: the target keeps the restrictive PACKAGE
    // default. The exemption skips the intersection layer but grants nothing.
    fs.write_file(
        "etc/legacy/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(name = "lib")
"#,
    );

    let ctx = calculation(&fs).await;

    let legacy = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//etc/legacy:lib"))
        .await
        .unwrap();

    assert!(
        !legacy
            .is_visible_to(&TargetLabel::testing_parse("root//elsewhere:x"))
            .unwrap(),
        "exemptions must not grant consumer visibility"
    );
    assert!(
        legacy
            .is_visible_to(&TargetLabel::testing_parse("root//etc/allowed:x"))
            .unwrap(),
    );
}

#[tokio::test]
async fn test_package_visibility_exempt_targets_recursive() {
    let fs = ProjectRootTemp::new().unwrap();

    fs.write_file("rules.bzl", RULES_BZL);
    fs.write_file(
        "etc/PACKAGE",
        r#"
enforce_visibility_intersection()
package(
    visibility = ["//etc/allowed/..."],
    visibility_exempt_targets = ["//etc/generated/..."],
)
"#,
    );
    for dir in ["etc/generated", "etc/generated/sub", "etc/handwritten"] {
        fs.write_file(
            &format!("{dir}/BUCK"),
            r#"
load("//:rules.bzl", "simple")
simple(name = "lib", visibility = ["PUBLIC"])
"#,
        );
    }

    let ctx = calculation(&fs).await;
    let outside = TargetLabel::testing_parse("root//elsewhere:x");

    for dir in ["etc/generated", "etc/generated/sub"] {
        let target = ctx
            .ctx()
            .get_target_node(&TargetLabel::testing_parse(&format!("root//{dir}:lib")))
            .await
            .unwrap();
        assert!(
            target.is_visible_to(&outside).unwrap(),
            "recursive exemption must cover {dir}"
        );
    }

    let handwritten = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//etc/handwritten:lib"))
        .await
        .unwrap();
    assert!(
        !handwritten.is_visible_to(&outside).unwrap(),
        "packages outside the recursive exemption must still be restricted"
    );
}

#[tokio::test]
async fn test_package_visibility_exempt_targets_cannot_weaken_ancestor() {
    let fs = ProjectRootTemp::new().unwrap();

    fs.write_file("rules.bzl", RULES_BZL);
    fs.write_file(
        "etc/PACKAGE",
        r#"
enforce_visibility_intersection()
package(
    visibility = ["//etc/allowed/..."],
)
"#,
    );
    fs.write_file(
        "etc/sub/PACKAGE",
        r#"
enforce_visibility_intersection()
package(
    visibility = ["//etc/allowed/...", "//etc/extra/..."],
    visibility_exempt_targets = ["//etc/sub:"],
)
"#,
    );
    fs.write_file(
        "etc/sub/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(name = "lib", visibility = ["PUBLIC"])
"#,
    );

    let ctx = calculation(&fs).await;

    let lib = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//etc/sub:lib"))
        .await
        .unwrap();

    assert!(
        !lib.is_visible_to(&TargetLabel::testing_parse("root//elsewhere:x"))
            .unwrap(),
        "exemption must not weaken an ancestor restriction"
    );
    assert!(
        !lib.is_visible_to(&TargetLabel::testing_parse("root//etc/extra:x"))
            .unwrap(),
        "ancestor layer blocks consumers the descendant would allow"
    );
    assert!(
        lib.is_visible_to(&TargetLabel::testing_parse("root//etc/allowed:x"))
            .unwrap(),
    );
}

#[tokio::test]
async fn test_package_visibility_exempt_targets_deep_nesting_ancestor_wins() {
    let fs = ProjectRootTemp::new().unwrap();

    fs.write_file("rules.bzl", RULES_BZL);
    fs.write_file(
        "etc/PACKAGE",
        r#"
enforce_visibility_intersection()
package(
    visibility = ["//etc/allowed/...", "//etc/extra/..."],
)
"#,
    );
    // Middle layer is narrower than root: it binds consumers root would allow.
    fs.write_file(
        "etc/sub/PACKAGE",
        r#"
enforce_visibility_intersection()
package(
    visibility = ["//etc/allowed/..."],
)
"#,
    );
    fs.write_file(
        "etc/sub/leaf/PACKAGE",
        r#"
enforce_visibility_intersection()
package(
    visibility = ["//etc/allowed/...", "//etc/leaf/..."],
    visibility_exempt_targets = ["//etc/sub/leaf:"],
)
"#,
    );
    fs.write_file(
        "etc/sub/leaf/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(name = "lib", visibility = ["PUBLIC"])
"#,
    );

    let ctx = calculation(&fs).await;

    let lib = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//etc/sub/leaf:lib"))
        .await
        .unwrap();

    assert!(
        !lib.is_visible_to(&TargetLabel::testing_parse("root//elsewhere:x"))
            .unwrap(),
        "inner exemption must not weaken outer layers"
    );
    assert!(
        !lib.is_visible_to(&TargetLabel::testing_parse("root//etc/extra:x"))
            .unwrap(),
        "middle layer must still block consumers the outer layer would allow"
    );
    assert!(
        lib.is_visible_to(&TargetLabel::testing_parse("root//etc/allowed:x"))
            .unwrap(),
    );
}

/// Loads `root//etc:lib` under an `etc/PACKAGE` containing `package_file`
/// and returns the (expected) error.
async fn etc_package_error(package_file: &str) -> String {
    let fs = ProjectRootTemp::new().unwrap();

    fs.write_file("rules.bzl", RULES_BZL);
    fs.write_file("etc/PACKAGE", package_file);
    fs.write_file(
        "etc/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(name = "lib")
"#,
    );

    let ctx = calculation(&fs).await;
    let err = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//etc:lib"))
        .await
        .expect_err("invalid PACKAGE must be rejected");
    format!("{err:?}")
}

#[tokio::test]
async fn test_package_visibility_exempt_targets_rejected() {
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "exemptions without visibility",
            r#"
package(
    visibility_exempt_targets = ["//etc/legacy:"],
)
"#,
            &["requires an explicit `visibility=`"],
        ),
        (
            // Also an invalid target pattern: pins missing-visibility precedence.
            "exemptions with visibility = None",
            r#"
package(
    visibility = None,
    visibility_exempt_targets = ["//etc/legacy:lib"],
)
"#,
            &["requires an explicit `visibility=`"],
        ),
        (
            "duplicate package() call",
            r#"
package(
    visibility = ["//etc/allowed/..."],
)
package(
    visibility_exempt_targets = ["//etc/legacy:"],
)
"#,
            &["at most once"],
        ),
        (
            "target pattern",
            r#"
package(
    visibility = ["//etc/allowed/..."],
    visibility_exempt_targets = ["//etc/legacy:lib"],
)
"#,
            &["target patterns (`cell//pkg:name`) are not accepted"],
        ),
        (
            "garbage",
            r#"
package(
    visibility = ["//etc/allowed/..."],
    visibility_exempt_targets = ["not a pattern"],
)
"#,
            &["not a pattern", "is not a valid pattern"],
        ),
        (
            "unknown cell",
            r#"
package(
    visibility = ["//etc/allowed/..."],
    visibility_exempt_targets = ["nosuchcell//etc:"],
)
"#,
            &["nosuchcell//etc:", "unknown cell alias"],
        ),
        (
            "PUBLIC entry",
            r#"
package(
    visibility = ["//etc/allowed/..."],
    visibility_exempt_targets = ["PUBLIC"],
)
"#,
            &["not a valid `visibility_exempt_targets` entry"],
        ),
        (
            "PUBLIC visibility",
            r#"
package(
    visibility = ["PUBLIC"],
    visibility_exempt_targets = ["//etc/legacy:"],
)
"#,
            &["vacuous with a `visibility` list containing"],
        ),
        (
            "mixed PUBLIC visibility",
            r#"
package(
    visibility = ["PUBLIC", "//etc/allowed/..."],
    visibility_exempt_targets = ["//etc/legacy:"],
)
"#,
            &["vacuous with a `visibility` list containing"],
        ),
        (
            "outside subtree",
            r#"
package(
    visibility = ["//etc/allowed/..."],
    visibility_exempt_targets = ["//other:"],
)
"#,
            &["outside this PACKAGE's subtree"],
        ),
        (
            "outside subtree, recursive",
            r#"
package(
    visibility = ["//etc/allowed/..."],
    visibility_exempt_targets = ["//other/..."],
)
"#,
            &["outside this PACKAGE's subtree"],
        ),
        (
            "covers subtree",
            r#"
package(
    visibility = ["//etc/allowed/..."],
    visibility_exempt_targets = ["//etc/..."],
)
"#,
            &["entire subtree"],
        ),
    ];

    for (name, package_file, expected) in cases {
        let msg = etc_package_error(package_file).await;
        assert!(
            expected.iter().all(|e| msg.contains(e)),
            "{name}: expected error containing {expected:?}, got: {msg}"
        );
    }
}

#[tokio::test]
async fn test_package_visibility_empty_list_with_empty_exempts_succeeds() {
    let fs = ProjectRootTemp::new().unwrap();

    fs.write_file("rules.bzl", RULES_BZL);
    fs.write_file(
        "etc/PACKAGE",
        r#"
package(
    visibility = [],
    visibility_exempt_targets = [],
)
"#,
    );
    fs.write_file(
        "etc/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(name = "lib")
"#,
    );

    let ctx = calculation(&fs).await;

    let lib = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//etc:lib"))
        .await
        .unwrap();
    assert!(
        lib.visibility_intersection().is_unrestricted(),
        "off mode without a marker must leave the empty declaration dormant, got: {}",
        lib.visibility_intersection(),
    );
}

#[tokio::test]
async fn test_package_visibility_exempt_targets_empty_list_with_exempts() {
    let fs = ProjectRootTemp::new().unwrap();

    fs.write_file("rules.bzl", RULES_BZL);
    // `[]` is deny-all, not the `Public` identity.
    fs.write_file(
        "etc/PACKAGE",
        r#"
enforce_visibility_intersection()
package(
    visibility = [],
    visibility_exempt_targets = ["//etc/legacy:"],
)
"#,
    );
    fs.write_file(
        "etc/legacy/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(name = "lib", visibility = ["PUBLIC"])
"#,
    );
    fs.write_file(
        "etc/other/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(name = "lib", visibility = ["PUBLIC"])
"#,
    );

    let ctx = calculation(&fs).await;

    let legacy = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//etc/legacy:lib"))
        .await
        .unwrap();
    let other = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//etc/other:lib"))
        .await
        .unwrap();
    let outside = TargetLabel::testing_parse("root//elsewhere:x");

    assert!(
        legacy.is_visible_to(&outside).unwrap(),
        "exempt definer must skip the deny-all layer"
    );
    assert!(
        !other.is_visible_to(&outside).unwrap(),
        "non-exempt definer must still be restricted by the deny-all layer"
    );
}

#[tokio::test]
async fn test_package_visibility_none_with_empty_exempts_succeeds() {
    let fs = ProjectRootTemp::new().unwrap();

    fs.write_file("rules.bzl", RULES_BZL);
    fs.write_file(
        "etc/PACKAGE",
        r#"
package(
    visibility = None,
    visibility_exempt_targets = [],
)
"#,
    );
    fs.write_file(
        "etc/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(name = "lib")
"#,
    );

    let ctx = calculation(&fs).await;

    let lib = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//etc:lib"))
        .await
        .unwrap();
    assert!(
        lib.visibility_intersection().is_unrestricted(),
        "None visibility must stay Unset and contribute nothing, got: {}",
        lib.visibility_intersection(),
    );
}

#[tokio::test]
async fn test_package_visibility_exempt_targets_exact_declaring_package_accepted() {
    let fs = ProjectRootTemp::new().unwrap();

    fs.write_file("rules.bzl", RULES_BZL);
    // `//etc:` leaves packages below `etc` restricted, so it is not vacuous.
    fs.write_file(
        "etc/PACKAGE",
        r#"
package(
    visibility = ["//etc/allowed/..."],
    visibility_exempt_targets = ["//etc:"],
)
"#,
    );
    fs.write_file(
        "etc/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(name = "lib")
"#,
    );

    let ctx = calculation(&fs).await;

    let node = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//etc:lib"))
        .await
        .unwrap();
    assert!(
        node.visibility_intersection().is_unrestricted(),
        "off mode without a marker must leave exemptions dormant, got: {}",
        node.visibility_intersection(),
    );
}

#[tokio::test]
async fn test_package_visibility_exempt_targets_dormant_when_off() {
    let fs = ProjectRootTemp::new().unwrap();

    fs.write_file("rules.bzl", RULES_BZL);
    fs.write_file(
        "etc/PACKAGE",
        r#"
package(
    visibility = ["//etc/allowed/..."],
    visibility_exempt_targets = ["//etc/legacy:"],
)
"#,
    );
    fs.write_file(
        "etc/legacy/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(name = "lib")
"#,
    );

    let ctx = calculation(&fs).await;

    let legacy = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//etc/legacy:lib"))
        .await
        .unwrap();
    assert!(
        legacy.visibility_intersection().is_unrestricted(),
        "off mode without a marker must leave exemptions dormant, got: {}",
        legacy.visibility_intersection(),
    );
}

#[tokio::test]
async fn test_package_visibility_exempt_targets_enforce_without_marker() {
    let fs = ProjectRootTemp::new().unwrap();

    fs.write_file("rules.bzl", RULES_BZL);
    fs.write_file(
        "etc/PACKAGE",
        r#"
package(
    visibility = ["//etc/allowed/..."],
    visibility_exempt_targets = ["//etc/legacy:"],
)
"#,
    );
    fs.write_file(
        "etc/legacy/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(name = "lib", visibility = ["PUBLIC"])
"#,
    );
    fs.write_file(
        "etc/other/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(name = "lib", visibility = ["PUBLIC"])
"#,
    );

    let ctx = calculation_with_package_visibility_mode(
        &fs,
        PackageVisibilityDefaultIntersection::Enforce,
    )
    .await;

    let legacy = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//etc/legacy:lib"))
        .await
        .unwrap();
    let other = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//etc/other:lib"))
        .await
        .unwrap();
    let outside = TargetLabel::testing_parse("root//elsewhere:x");

    assert!(
        legacy.is_visible_to(&outside).unwrap(),
        "exempt definer must skip the layer in enforce mode"
    );
    assert!(
        !other.is_visible_to(&outside).unwrap(),
        "non-exempt definer must still be restricted in enforce mode"
    );
}

#[tokio::test]
async fn test_package_visibility_exempt_targets_inherit_uses_pre_inherit_list() {
    let fs = ProjectRootTemp::new().unwrap();

    fs.write_file("rules.bzl", RULES_BZL);
    fs.write_file(
        "etc/PACKAGE",
        r#"
package(
    visibility = ["//parent/..."],
)
"#,
    );
    // The layer snapshots this call's explicit list BEFORE `inherit=True`
    // merges the parent's: `//parent/...` must not leak into the layer.
    fs.write_file(
        "etc/child/PACKAGE",
        r#"
enforce_visibility_intersection()
package(
    inherit = True,
    visibility = ["//etc/child/allowed/..."],
    visibility_exempt_targets = ["//etc/child/legacy:"],
)
"#,
    );
    fs.write_file(
        "etc/child/legacy/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(name = "lib", visibility = ["PUBLIC"])
"#,
    );
    fs.write_file(
        "etc/child/other/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(name = "lib", visibility = ["PUBLIC"])
"#,
    );

    let ctx = calculation(&fs).await;

    let legacy = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//etc/child/legacy:lib"))
        .await
        .unwrap();
    let other = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//etc/child/other:lib"))
        .await
        .unwrap();
    let outside = TargetLabel::testing_parse("root//elsewhere:x");
    let parent = TargetLabel::testing_parse("root//parent:x");
    let allowed = TargetLabel::testing_parse("root//etc/child/allowed:x");

    assert!(
        legacy.is_visible_to(&outside).unwrap(),
        "exempt definer must skip the layer"
    );
    assert!(
        !other.is_visible_to(&outside).unwrap(),
        "non-exempt definer must still be restricted"
    );
    assert!(
        !other.is_visible_to(&parent).unwrap(),
        "inherited parent visibility must not leak into the layer"
    );
    assert!(
        other.is_visible_to(&allowed).unwrap(),
        "this call's explicit list forms the layer"
    );
}

#[tokio::test]
async fn test_package_visibility_exempt_targets_audit_without_marker() {
    let fs = ProjectRootTemp::new().unwrap();

    fs.write_file("rules.bzl", RULES_BZL);
    fs.write_file(
        "etc/PACKAGE",
        r#"
package(
    visibility = ["//etc/allowed/..."],
    visibility_exempt_targets = ["//etc/legacy:"],
)
"#,
    );
    fs.write_file(
        "etc/legacy/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(name = "lib", visibility = ["PUBLIC"])
"#,
    );
    fs.write_file(
        "etc/other/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(name = "lib", visibility = ["PUBLIC"])
"#,
    );

    let ctx =
        calculation_with_package_visibility_mode(&fs, PackageVisibilityDefaultIntersection::Audit)
            .await;

    let legacy = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//etc/legacy:lib"))
        .await
        .unwrap();
    let other = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//etc/other:lib"))
        .await
        .unwrap();
    let outside = TargetLabel::testing_parse("root//elsewhere:x");

    assert!(
        legacy.is_visible_to(&outside).unwrap(),
        "exempt definer must skip the layer in audit mode"
    );
    assert!(
        !other.is_visible_to(&outside).unwrap(),
        "non-exempt definer must still be restricted in audit mode"
    );
}

#[tokio::test]
async fn test_package_not_inherit() {
    let fs = ProjectRootTemp::new().unwrap();

    fs.write_file("rules.bzl", RULES_BZL);
    fs.write_file(
        "PACKAGE",
        r#"
package(
    visibility = ["//aaa/..."],
)
"#,
    );
    fs.write_file(
        "juxtaposition/PACKAGE",
        r#"
package(
    visibility = ["//bbb/..."],
)
"#,
    );
    fs.write_file(
        "juxtaposition/BUCK",
        r#"
load("//:rules.bzl", "simple")
simple(name = "a")
"#,
    );

    let ctx = calculation(&fs).await;

    let a = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//juxtaposition:a"))
        .await
        .unwrap();

    assert_eq!(
        &VisibilitySpecification::testing_parse(&["root//bbb/..."]),
        a.visibility().unwrap(),
    );
}
