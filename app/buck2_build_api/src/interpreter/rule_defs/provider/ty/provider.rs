/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_hash::BuckFixedSortedMap;
use starlark::typing::Ty;
use starlark::typing::TyStarlarkValue;
use starlark::typing::TyUser;
use starlark::typing::TyUserFields;
use starlark::typing::TyUserParams;
use starlark::values::typing::TypeInstanceId;
use starlark::values::typing::TypeMatcherFactory;

/// Type of provider instance, builtin or user.
pub(crate) fn ty_provider(
    name: &str,
    type_instance_id: TypeInstanceId,
    base: TyStarlarkValue,
    // Use default matcher from `StarlarkValue` if `None`.
    matcher: Option<TypeMatcherFactory>,
    fields: BuckFixedSortedMap<String, Ty>,
) -> buck2_error::Result<Ty> {
    Ok(Ty::custom(TyUser::new(
        name.to_owned(),
        base,
        type_instance_id,
        TyUserParams {
            matcher,
            fields: TyUserFields {
                known: fields,
                unknown: false,
            },
            ..TyUserParams::default()
        },
    )?))
}
