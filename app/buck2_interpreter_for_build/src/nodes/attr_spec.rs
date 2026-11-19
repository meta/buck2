/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::target::label::label::TargetLabelRef;
use buck2_core::target::name::TargetNameRef;
use buck2_error::BuckErrorContext;
use buck2_error::internal_error;
use buck2_hash::BuckMutMap;
use buck2_hash::IntentionallyStdHashMap;
use buck2_node::attrs::arena::AttrToken;
use buck2_node::attrs::attr::Attribute;
use buck2_node::attrs::attr::CoercedValue;
use buck2_node::attrs::coerced::kinds;
use buck2_node::attrs::coercion_context::AttrCoercionContext;
use buck2_node::attrs::configurable::AttrIsConfigurable;
use buck2_node::attrs::display::AttrDisplayWithContextExt;
use buck2_node::attrs::inspect_options::AttrInspectOptions;
use buck2_node::attrs::spec::AttributeId;
use buck2_node::attrs::spec::AttributeSpec;
use buck2_node::attrs::spec::internal::NAME_ATTRIBUTE;
use buck2_node::attrs::spec::internal::VISIBILITY_ATTRIBUTE;
use buck2_node::attrs::spec::internal::WITHIN_VIEW_ATTRIBUTE;
use buck2_node::attrs::spec::internal::attr_is_configurable;
use buck2_node::attrs::values::UnsealedAttrValues;
use starlark::docs::DocString;
use starlark::eval::ParametersParser;
use starlark::eval::ParametersSpec;
use starlark::eval::ParametersSpecParam;
use starlark::typing::ParamIsRequired;
use starlark::typing::ParamSpec;
use starlark::typing::Ty;
use starlark::typing::TyFunction;
use starlark::values::Value;

use crate::attrs::AttributeCoerceExt;
use crate::interpreter::module_internals::ModuleInternals;
use crate::nodes::check_within_view::check_within_view;

pub trait AttributeSpecExt {
    fn start_parse<'a, 'v>(
        &'a self,
        param_parser: &mut ParametersParser<'v, '_>,
        size_hint: usize,
        internals: &ModuleInternals,
    ) -> buck2_error::Result<(
        // "name" attribute value.
        &'v TargetNameRef,
        // Remaining attributes.
        impl ExactSizeIterator<Item = (&'a str, AttributeId, &'a Attribute)> + 'a,
        // Attribute values, populated with name.
        UnsealedAttrValues,
    )>;

    fn parse_params<'v>(
        &self,
        param_parser: &mut ParametersParser<'v, '_>,
        arg_count: usize,
        internals: &ModuleInternals,
        coercion_ctx: &dyn AttrCoercionContext<'v>,
    ) -> buck2_error::Result<(&'v TargetNameRef, UnsealedAttrValues)>;

    /// Returns a starlark Parameters for the rule callable, but not default values.
    fn signature<'v>(&self, rule_name: String) -> ParametersSpec<Value<'v>>;

    /// Returns a starlark Parameters for the rule callable, with default
    /// values pre-rendered for documentation.
    fn signature_with_default_value(&self, rule_name: String) -> ParametersSpec<String>;

    fn ty_function(&self) -> TyFunction;

    fn starlark_types(&self) -> Vec<Ty>;
    fn docstrings(&self) -> IntentionallyStdHashMap<String, Option<DocString>>;
}

/// The value an attribute takes when the target does not set it and the
/// attribute's own default does not apply: `visibility` and `within_view`
/// default to what the enclosing `PACKAGE` files declare.
fn package_default(
    attr_name: &str,
    internals: &ModuleInternals,
) -> Option<buck2_error::Result<AttrToken>> {
    if attr_name == VISIBILITY_ATTRIBUTE.name {
        Some(
            internals.with_attr_arena_builder(|b| {
                b.encode_visibility(internals.super_package.visibility())
            }),
        )
    } else if attr_name == WITHIN_VIEW_ATTRIBUTE.name {
        Some(internals.with_attr_arena_builder(|b| {
            b.encode_within_view(internals.super_package.within_view())
        }))
    } else {
        None
    }
}

impl AttributeSpecExt for AttributeSpec {
    fn start_parse<'a, 'v>(
        &'a self,
        param_parser: &mut ParametersParser<'v, '_>,
        size_hint: usize,
        internals: &ModuleInternals,
    ) -> buck2_error::Result<(
        &'v TargetNameRef,
        impl ExactSizeIterator<Item = (&'a str, AttributeId, &'a Attribute)> + 'a,
        UnsealedAttrValues,
    )> {
        let mut attr_values = UnsealedAttrValues::with_capacity(size_hint);

        let mut indices = self.attr_specs();
        let name = match indices.next() {
            Some((name_name, attr_idx, _attr)) if name_name == NAME_ATTRIBUTE.name => {
                let name = param_parser.next()?;
                internals.with_attr_arena_builder(|builder| {
                    let name = builder.encode_string(name)?;
                    attr_values.push(builder, attr_idx, name);
                    buck2_error::Ok(())
                })?;
                name
            }
            _ => {
                return Err(internal_error!("First attribute is `name`, it is known"));
            }
        };
        let name = TargetNameRef::new(name)?;
        Ok((name, indices, attr_values))
    }

    /// Parses params extracting the TargetName and the attribute values to store in the TargetNode.
    fn parse_params<'v>(
        &self,
        param_parser: &mut ParametersParser<'v, '_>,
        arg_count: usize,
        internals: &ModuleInternals,
        coercion_ctx: &dyn AttrCoercionContext<'v>,
    ) -> buck2_error::Result<(&'v TargetNameRef, UnsealedAttrValues)> {
        let (name, indices, mut attr_values) =
            self.start_parse(param_parser, arg_count, internals)?;

        let target_label = TargetLabelRef::new(internals.buildfile_path().package(), name);

        let mut default_allowed_deps = BuckMutMap::default();

        for (attr_name, attr_idx, attribute) in indices {
            let configurable = attr_is_configurable(attr_name);

            let user_value: Option<Value> = match attribute.default_as_ref() {
                Some(_) => param_parser.next_opt()?,
                None => Some(param_parser.next()?),
            };

            let mut coerced = match user_value {
                Some(v) => attribute
                    .coerce(attr_name, configurable, coercion_ctx, v)
                    .with_buck_error_context(|| {
                        format!("Error coercing attribute `{attr_name}` of `{target_label}`",)
                    })?,
                None => CoercedValue::Default,
            };
            if coerced == CoercedValue::Default
                && let Some(value) = package_default(attr_name, internals)
            {
                coerced = CoercedValue::Custom(value?);
            }

            if let CoercedValue::Custom(v) = coerced {
                internals.with_attr_arena_builder(|builder| attr_values.push(builder, attr_idx, v));
                default_allowed_deps.insert(attr_name, attribute.default_allowed_deps());
            }
        }

        // For now `within_view` is always set, but let's make code more robust.
        if let Some(within_view_word) = attr_values.get(WITHIN_VIEW_ATTRIBUTE.id) {
            internals.with_attr_arena_ref(|arena_builder| -> buck2_error::Result<()> {
                let within_view = match arena_builder
                    .view(within_view_word)
                    .downcast::<kinds::WithinView>()
                {
                    Some(within_view) => within_view,
                    _ => return Err(internal_error!("`within_view` coerced incorrectly")),
                };
                for a in self.unsealed_attrs(
                    arena_builder,
                    &attr_values,
                    AttrInspectOptions::DefinedOnly,
                ) {
                    let default_deps = default_allowed_deps.get(&a.name).copied().flatten();
                    check_within_view(
                        a.value,
                        internals.buildfile_path().package(),
                        a.attr.coercer(),
                        within_view,
                        internals.super_package.within_view_cap(),
                        default_deps,
                    )
                    .with_buck_error_context(|| {
                        format!(
                            "checking `within_view` for attribute `{}` of `{}`",
                            a.name, target_label,
                        )
                    })?;
                }
                Ok(())
            })?;
        }

        Ok((name, attr_values))
    }

    /// Returns a starlark Parameters for the rule callable, but not default values.
    fn signature<'v>(&self, rule_name: String) -> ParametersSpec<Value<'v>> {
        ParametersSpec::new_named_only(
            &rule_name,
            self.attr_specs().map(|(name, _idx, attribute)| {
                let default = attribute.default_as_ref();
                (
                    name,
                    match default {
                        Some(_) => ParametersSpecParam::Optional,
                        None => ParametersSpecParam::Required,
                    },
                )
            }),
        )
    }

    /// Returns a starlark Parameters for the rule callable, with default
    /// values pre-rendered for documentation.
    fn signature_with_default_value(&self, rule_name: String) -> ParametersSpec<String> {
        ParametersSpec::new_named_only(
            &rule_name,
            self.attr_specs().map(|(name, _idx, attribute)| {
                let default = attribute.default_as_ref();
                (
                    name,
                    match default {
                        Some(default) => {
                            ParametersSpecParam::Defaulted(default.as_display_no_ctx().to_string())
                        }
                        None => ParametersSpecParam::Required,
                    },
                )
            }),
        )
    }

    fn ty_function(&self) -> TyFunction {
        let mut params = Vec::with_capacity(self.attr_specs().len());
        for (name, _idx, attribute) in self.attr_specs() {
            let ty = match attr_is_configurable(name) {
                AttrIsConfigurable::Yes => attribute.starlark_type().to_ty_with_select(),
                AttrIsConfigurable::No => attribute.starlark_type().to_ty(),
            };
            let required = match attribute.default_as_ref() {
                Some(_) => ParamIsRequired::No,
                None => ParamIsRequired::Yes,
            };
            params.push((starlark::util::ArcStr::from(name), required, ty));
        }
        let params = ParamSpec::new_named_only(params).unwrap();
        TyFunction::new(params, Ty::none())
    }

    fn starlark_types(&self) -> Vec<Ty> {
        self.attr_specs()
            .map(|(_, _, a)| a.starlark_type().to_ty())
            .collect()
    }

    fn docstrings(&self) -> IntentionallyStdHashMap<String, Option<DocString>> {
        self.attr_specs()
            .map(|(name, _idx, attr)| (name.to_owned(), attr.docstring()))
            .collect()
    }
}
