/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::deprecation;
use buck2_node::attrs::arena::AttrToken;
use buck2_node::attrs::attr_type::any::AnyAttrType;
use buck2_node::attrs::coercion_context::AttrCoercionContext;
use buck2_node::attrs::configurable::AttrIsConfigurable;
use starlark::typing::Ty;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::dict::DictRef;
use starlark::values::int::IntView;
use starlark::values::list::ListRef;
use starlark::values::none::NoneType;
use starlark::values::tuple::TupleRef;

use crate::attrs::coerce::AttrTypeCoerce;
use crate::attrs::coerce::attr_type::ty_maybe_select::TyMaybeSelect;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum AnyError {
    #[error("Cannot coerce value of type `{0}` to any: `{1}`")]
    CannotCoerce(&'static str, String),
}

/// The values that `attrs.any()` keeps as they are. Any other value is kept as its `str()`.
#[derive(UnpackValue)]
enum Literal<'v> {
    None(NoneType),
    Bool(bool),
    Int(IntView<'v>),
    Dict(DictRef<'v>),
    Tuple(&'v TupleRef<'v>),
    List(&'v ListRef<'v>),
    Str(&'v str),
}

fn to_literal<'v>(
    value: Value<'v>,
    ctx: &dyn AttrCoercionContext<'v>,
) -> buck2_error::Result<AttrToken> {
    match Literal::unpack_value_opt(value) {
        Some(Literal::None(_)) => ctx.attr_arena().encode_none(),
        Some(Literal::Bool(x)) => ctx.attr_arena().encode_bool(x),
        Some(Literal::Int(x)) => ctx.attr_arena().encode_int(x.to_integer()?),
        Some(Literal::Dict(x)) => {
            let pairs = x
                .iter()
                .map(|(k, v)| Ok((to_literal(k, ctx)?, to_literal(v, ctx)?)))
                .collect::<buck2_error::Result<Vec<_>>>()?;
            ctx.attr_arena().encode_dict(&pairs)
        }
        Some(Literal::Tuple(x)) => {
            let items = x
                .iter()
                .map(|v| to_literal(v, ctx))
                .collect::<buck2_error::Result<Vec<_>>>()?;
            ctx.attr_arena().encode_tuple(&items)
        }
        Some(Literal::List(x)) => {
            let items = x
                .iter()
                .map(|v| to_literal(v, ctx))
                .collect::<buck2_error::Result<Vec<_>>>()?;
            ctx.attr_arena().encode_list(&items)
        }
        Some(Literal::Str(s)) => ctx.attr_arena().encode_string(s),
        None => {
            deprecation!(
                "coerce_to_any",
                AnyError::CannotCoerce(value.get_type(), value.to_repr()).into(),
                owner: "jtbraun", started: "2023-04-29"
            );
            ctx.attr_arena().encode_string(&value.to_str())
        }
    }
}

impl AttrTypeCoerce for AnyAttrType {
    fn coerce_item<'v>(
        &self,
        _configurable: AttrIsConfigurable,
        ctx: &dyn AttrCoercionContext<'v>,
        value: Value<'v>,
    ) -> buck2_error::Result<AttrToken> {
        to_literal(value, ctx)
    }

    fn starlark_type(&self) -> TyMaybeSelect {
        TyMaybeSelect::Basic(Ty::any())
    }
}
