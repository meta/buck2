/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::prelude::*;

/// Bare Select type annotations should pass type checking.
#[buck_test(inplace = false)]
async fn test_bare_select_typecheck(buck: &Buck) -> anyhow::Result<()> {
    buck.starlark(&["typecheck", "select_types.bzl"]).await?;
    Ok(())
}

/// Select[str] | str accepts both select expressions and plain strings.
///
/// TODO(D94266514): Currently vacuous because Select[T] resolves to Any.
#[buck_test(inplace = false)]
async fn test_select_union_accepts_both(buck: &Buck) -> anyhow::Result<()> {
    buck.starlark(&["typecheck", "select_union_good.bzl"])
        .await?;
    Ok(())
}

/// Select[str] rejects a plain str argument during type checking.
///
/// TODO(D94266514): Once Select[T] type parameterization lands, this should
/// use expect_failure with stderr_regex="Detected 2 errors".
/// Currently Select[T] resolves to Any so no type error is produced.
#[buck_test(inplace = false)]
async fn test_select_rejects_plain_value(buck: &Buck) -> anyhow::Result<()> {
    // TODO(D94266514): expect_failure once Select[T] lands
    buck.starlark(&["typecheck", "select_union_bad.bzl"])
        .await?;
    Ok(())
}

/// Nested type parameters like Select[list[str]] pass type checking.
///
/// TODO(D94266514): Currently vacuous because Select[T] resolves to Any.
#[buck_test(inplace = false)]
async fn test_nested_type_params(buck: &Buck) -> anyhow::Result<()> {
    buck.starlark(&["typecheck", "select_nested_good.bzl"])
        .await?;
    Ok(())
}

/// Select[list[str]] rejects plain values during type checking.
///
/// TODO(D94266514): Once Select[T] type parameterization lands, this should
/// use expect_failure with stderr_regex="Detected 2 errors".
#[buck_test(inplace = false)]
async fn test_nested_type_params_reject_plain(buck: &Buck) -> anyhow::Result<()> {
    // TODO(D94266514): expect_failure once Select[T] lands
    buck.starlark(&["typecheck", "select_nested_bad.bzl"])
        .await?;
    Ok(())
}

/// Select[str | None] accepts select expressions resolving to str or None.
///
/// TODO(D94266514): Currently vacuous because Select[T] resolves to Any.
#[buck_test(inplace = false)]
async fn test_optional_type_params(buck: &Buck) -> anyhow::Result<()> {
    buck.starlark(&["typecheck", "select_optional_good.bzl"])
        .await?;
    Ok(())
}

/// Select[str | None] rejects plain values during type checking.
///
/// TODO(D94266514): Once Select[T] type parameterization lands, this should
/// use expect_failure with stderr_regex="Detected 2 errors".
#[buck_test(inplace = false)]
async fn test_optional_type_params_reject_plain(buck: &Buck) -> anyhow::Result<()> {
    // TODO(D94266514): expect_failure once Select[T] lands
    buck.starlark(&["typecheck", "select_optional_bad.bzl"])
        .await?;
    Ok(())
}

/// Select[bool] accepts select expressions with boolean branches.
///
/// TODO(D94266514): Currently vacuous because Select[T] resolves to Any.
#[buck_test(inplace = false)]
async fn test_select_bool_parameterization(buck: &Buck) -> anyhow::Result<()> {
    buck.starlark(&["typecheck", "select_bool_good.bzl"])
        .await?;
    Ok(())
}

/// Select[bool] rejects plain boolean and string values.
///
/// TODO(D94266514): Once Select[T] type parameterization lands, this should
/// use expect_failure with stderr_regex="Detected 2 errors".
#[buck_test(inplace = false)]
async fn test_select_bool_rejects_plain(buck: &Buck) -> anyhow::Result<()> {
    // TODO(D94266514): expect_failure once Select[T] lands
    buck.starlark(&["typecheck", "select_bool_bad.bzl"]).await?;
    Ok(())
}

/// Select[str] alone accepts select expressions but not plain values.
///
/// TODO(D94266514): Currently vacuous because Select[T] resolves to Any.
#[buck_test(inplace = false)]
async fn test_select_str_not_equal_to_union(buck: &Buck) -> anyhow::Result<()> {
    buck.starlark(&["typecheck", "select_semantics_good.bzl"])
        .await?;
    Ok(())
}

/// Select[str] is not Select[str] | str: plain values are rejected.
///
/// TODO(D94266514): Once Select[T] type parameterization lands, this should
/// use expect_failure with stderr_regex="Detected 2 errors".
#[buck_test(inplace = false)]
async fn test_select_str_rejects_plain_proving_not_union(buck: &Buck) -> anyhow::Result<()> {
    // TODO(D94266514): expect_failure once Select[T] lands
    buck.starlark(&["typecheck", "select_semantics_bad.bzl"])
        .await?;
    Ok(())
}

/// select() branch values matching the type parameter pass type checking.
///
/// TODO(D94266514): Currently vacuous because Select[T] resolves to Any.
#[buck_test(inplace = false)]
async fn test_branch_type_matching(buck: &Buck) -> anyhow::Result<()> {
    buck.starlark(&["typecheck", "select_branch_good.bzl"])
        .await?;
    Ok(())
}

/// select() branch values not matching the type parameter are rejected.
///
/// TODO(D94266514): Once Select[T] type parameterization lands, this should
/// use expect_failure with stderr_regex="Detected 2 errors".
#[buck_test(inplace = false)]
async fn test_branch_type_mismatch(buck: &Buck) -> anyhow::Result<()> {
    // TODO(D94266514): expect_failure once Select[T] lands
    buck.starlark(&["typecheck", "select_branch_bad.bzl"])
        .await?;
    Ok(())
}

/// Bare Select type accepts any select() regardless of branch value types.
#[buck_test(inplace = false)]
async fn test_backward_compat_bare_select(buck: &Buck) -> anyhow::Result<()> {
    buck.starlark(&["typecheck", "select_compat_good.bzl"])
        .await?;
    Ok(())
}

/// list + Select[T] type-checks via rbin_op dispatch on custom types.
///
/// TODO(D94266514): Currently vacuous because Select[T] resolves to Any.
#[buck_test(inplace = false)]
async fn test_rbin_op_list_plus_select(buck: &Buck) -> anyhow::Result<()> {
    buck.starlark(&["typecheck", "select_rbin_op_good.bzl"])
        .await?;
    Ok(())
}

/// Select[str] | list[str] accepts both select expressions and plain lists.
///
/// TODO(D94266514): Currently vacuous because Select[T] resolves to Any.
#[buck_test(inplace = false)]
async fn test_select_or_list_union_accepts_both(buck: &Buck) -> anyhow::Result<()> {
    buck.starlark(&["typecheck", "select_union_param_good.bzl"])
        .await?;
    Ok(())
}

/// Select[str] | list[str] rejects plain int and str values.
///
/// TODO(D94266514): Once Select[T] type parameterization lands, this should
/// use expect_failure with stderr_regex="Detected 2 errors".
#[buck_test(inplace = false)]
async fn test_select_or_list_union_rejects_wrong_type(buck: &Buck) -> anyhow::Result<()> {
    // TODO(D94266514): expect_failure once Select[T] lands
    buck.starlark(&["typecheck", "select_union_param_bad.bzl"])
        .await?;
    Ok(())
}

/// Select[str] | Select[int] accepts select expressions with str or int branches.
#[buck_test(inplace = false)]
async fn test_select_intersect_accepts_matching(buck: &Buck) -> anyhow::Result<()> {
    buck.starlark(&["typecheck", "select_intersect_good.bzl"])
        .await?;
    Ok(())
}

/// Select[str] | Select[int] rejects Select[bool] and plain values.
///
/// TODO(D94266514): Once Select[T] type parameterization lands, this should
/// use expect_failure with stderr_regex="Detected 2 errors".
#[buck_test(inplace = false)]
async fn test_select_intersect_rejects_non_overlapping(buck: &Buck) -> anyhow::Result<()> {
    // TODO(D94266514): expect_failure once Select[T] lands
    buck.starlark(&["typecheck", "select_intersect_bad.bzl"])
        .await?;
    Ok(())
}
