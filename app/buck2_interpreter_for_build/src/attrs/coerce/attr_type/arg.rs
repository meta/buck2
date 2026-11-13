/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Debug;
use std::mem;
use std::sync::LazyLock;

use buck2_core::provider::label::ProvidersLabel;
use buck2_hash::BuckMutSet;
use buck2_node::attrs::arena::AttrToken;
use buck2_node::attrs::attr_type::arg::ArgAttrType;
use buck2_node::attrs::attr_type::arg::MacroBase;
use buck2_node::attrs::attr_type::arg::MacroDepKind;
use buck2_node::attrs::attr_type::arg::QueryExpansion;
use buck2_node::attrs::attr_type::arg::StringWithMacrosPart;
use buck2_node::attrs::attr_type::arg::UnconfiguredMacro;
use buck2_node::attrs::attr_type::arg::UnconfiguredStringWithMacros;
use buck2_node::attrs::attr_type::arg::parser;
use buck2_node::attrs::attr_type::arg::parser::ParsedMacro;
use buck2_node::attrs::attr_type::arg::parser::parse_macros;
use buck2_node::attrs::attr_type::query::QueryAttrType;
use buck2_node::attrs::attr_type::query::QueryMacroBase;
use buck2_node::attrs::coercion_context::AttrCoercionContext;
use buck2_node::attrs::configurable::AttrIsConfigurable;
use starlark::typing::Ty;
use starlark::values::Value;

use crate::attrs::coerce::AttrTypeCoerce;
use crate::attrs::coerce::attr_type::query::QueryAttrTypeExt;
use crate::attrs::coerce::attr_type::ty_maybe_select::TyMaybeSelect;

// These are the macros we haven't yet implemented yet, we should make sure not
// to try and resolve them to user defined macros with a target parameter,
// because some of them don't take a target.
// Taken from https://buck.build/function/string_parameter_macros.html.
static UNIMPLEMENTED_MACROS: LazyLock<BuckMutSet<&'static str>> = LazyLock::new(|| {
    ["classpath_abi", "maven_coords", "output", "query_paths"]
        .into_iter()
        .collect()
});

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum MacroError {
    #[error("Expected a single target label argument. Got `[{}]`", (.0).join(", "))]
    ExpectedSingleTargetArgument(Vec<String>),
    #[error("Expected a single path argument. Got `{}`", (.0).join(", "))]
    ExpectedSinglePathArgument(Vec<String>),
    #[error("Incorrect number of args to macro `{0}` (had {1} args)")]
    InvalidNumberOfArgs(String, usize),
    #[error(
        "Unrecognized macro `{0}` (with {1} args). If this is shell command substitution rather than a buck macro, escape it as `\\$({0} ...)`"
    )]
    UnrecognizedMacro(String, usize),
}

impl AttrTypeCoerce for ArgAttrType {
    fn coerce_item<'v>(
        &self,
        _configurable: AttrIsConfigurable,
        ctx: &dyn AttrCoercionContext<'v>,
        value: Value<'v>,
    ) -> buck2_error::Result<AttrToken> {
        let value = value.unpack_str_err()?;

        let mut items = parse_macros(value)?.into_items();
        if let [parser::ArgItem::String(val)] = items.as_mut_slice() {
            // Specialize single-item StringWithMacros, which are in fact the common case.
            let arg = UnconfiguredStringWithMacros::StringPart(ctx.intern_str(&mem::take(val)));
            return ctx.attr_arena().encode_arg(&arg);
        }

        let mut parts = Vec::with_capacity(items.len());

        for item in items {
            match item {
                parser::ArgItem::String(val) => {
                    parts.push(StringWithMacrosPart::String(ctx.intern_str(&val)))
                }
                parser::ArgItem::Macro(ParsedMacro {
                    write_to_file,
                    macro_type,
                    args,
                }) => {
                    let part = match macro_type.as_ref() {
                        "location" | "location-platform" if args.len() == 1 => {
                            UnconfiguredMacro::new_location(ctx, args, MacroDepKind::Regular)?
                        }
                        "location_exec" => {
                            UnconfiguredMacro::new_location(ctx, args, MacroDepKind::Exec)?
                        }
                        "location_toolchain" => {
                            UnconfiguredMacro::new_location(ctx, args, MacroDepKind::Toolchain)?
                        }
                        "exe" => UnconfiguredMacro::new_exe(ctx, args, true)?,
                        "exe_target" => UnconfiguredMacro::new_exe(ctx, args, false)?,
                        "source" => UnconfiguredMacro::new_source(ctx, args)?,
                        "query_outputs" | "query_targets" | "query_targets_and_outputs" => {
                            UnconfiguredMacro::new_query(ctx, &macro_type, args)?
                        }
                        _ if args.is_empty() => {
                            UnconfiguredMacro::new_user_unkeyed_placeholder(macro_type)
                        }
                        name if (args.len() == 1 || args.len() == 2)
                            && !UNIMPLEMENTED_MACROS.contains(name) =>
                        {
                            UnconfiguredMacro::new_user_keyed_placeholder(ctx, macro_type, args)?
                        }
                        _ => {
                            return Err(
                                MacroError::UnrecognizedMacro(macro_type, args.len()).into()
                            );
                        }
                    };
                    parts.push(StringWithMacrosPart::Macro(write_to_file, part));
                }
            }
        }

        let arg = UnconfiguredStringWithMacros::ManyParts(parts.into_boxed_slice());
        ctx.attr_arena().encode_arg(&arg)
    }

    fn starlark_type(&self) -> TyMaybeSelect {
        TyMaybeSelect::Basic(Ty::string())
    }
}

/// A simple helper for a macro that accepts a single target argument (the most common case, by far).
fn get_single_target_arg<'v>(
    args: Vec<String>,
    ctx: &dyn AttrCoercionContext<'v>,
) -> buck2_error::Result<ProvidersLabel> {
    if args.len() != 1 {
        return Err(MacroError::ExpectedSingleTargetArgument(args).into());
    }
    ctx.coerce_providers_label(&args[0])
}

pub trait UnconfiguredMacroExt {
    fn new_location<'v>(
        ctx: &dyn AttrCoercionContext<'v>,
        args: Vec<String>,
        dep_kind: MacroDepKind,
    ) -> buck2_error::Result<UnconfiguredMacro> {
        Ok(UnconfiguredMacro::Location {
            label: get_single_target_arg(args, ctx)?,
            dep_kind,
        })
    }

    fn new_exe<'v>(
        ctx: &dyn AttrCoercionContext<'v>,
        args: Vec<String>,
        exec_dep: bool,
    ) -> buck2_error::Result<UnconfiguredMacro> {
        Ok(UnconfiguredMacro::Exe {
            label: get_single_target_arg(args, ctx)?,
            exec_dep,
        })
    }

    fn new_source<'v>(
        ctx: &dyn AttrCoercionContext<'v>,
        args: Vec<String>,
    ) -> buck2_error::Result<UnconfiguredMacro> {
        if args.len() != 1 {
            return Err(MacroError::ExpectedSinglePathArgument(args).into());
        }

        ctx.coerce_path(&args[0], /* allow_directory */ true)
            .map(UnconfiguredMacro::Source)
    }

    fn new_user_keyed_placeholder<'v>(
        ctx: &dyn AttrCoercionContext<'v>,
        var_name: String,
        mut args: Vec<String>,
    ) -> buck2_error::Result<UnconfiguredMacro> {
        // args should've already been checked to have len == 1 or 2
        let arg = if args.len() == 2 {
            Some(args.pop().unwrap())
        } else {
            None
        };
        Ok(UnconfiguredMacro::UserKeyedPlaceholder(Box::new((
            var_name.into_boxed_str(),
            ctx.coerce_providers_label(&args[0])?,
            arg.map(|a| a.into_boxed_str()),
        ))))
    }

    fn new_query<'v>(
        ctx: &dyn AttrCoercionContext<'v>,
        expansion_type: &str,
        args: Vec<String>,
    ) -> buck2_error::Result<UnconfiguredMacro> {
        if args.is_empty() || args.len() > 2 {
            return Err(
                MacroError::InvalidNumberOfArgs(expansion_type.to_owned(), args.len()).into(),
            );
        }
        let mut args_iter = args.into_iter();
        let (query, separator) = match (args_iter.next(), args_iter.next()) {
            (Some(query), None) => (query, None),
            (separator, Some(query)) => (query, separator),
            _ => unreachable!(),
        };

        let expansion_type = match expansion_type {
            "query_targets" => QueryExpansion::Target,
            "query_outputs" => QueryExpansion::Output,
            "query_targets_and_outputs" => QueryExpansion::TargetAndOutput(separator),
            _ => panic!("invalid expansion type {expansion_type}"),
        };

        // TODO(cjhopman): errors when args aren't the right size (too many, or separator for non-separator query)

        Ok(MacroBase::Query(Box::new(QueryMacroBase {
            expansion_type,
            query: QueryAttrType::coerce(ctx, query)?,
        })))
    }

    fn new_user_unkeyed_placeholder(var_name: String) -> UnconfiguredMacro {
        UnconfiguredMacro::UserUnkeyedPlaceholder(var_name.into_boxed_str())
    }
}

impl UnconfiguredMacroExt for UnconfiguredMacro {}

#[cfg(test)]
mod tests {
    use buck2_core::configuration::data::ConfigurationData;
    use buck2_core::package::PackageLabel;
    use buck2_core::target::label::label::TargetLabel;
    use buck2_error::buck2_error;
    use buck2_hash::buck_mut_insertion_set;
    use buck2_node::attrs::attr_type::AttrType;
    use buck2_node::attrs::attr_type::arg::MacroBase;
    use buck2_node::attrs::coerced::CoercedAttrRefKind;
    use buck2_node::attrs::coerced_deps_collector::CoercedDepsCollector;
    use buck2_node::attrs::configuration_context::AttrConfigurationContext;
    use buck2_node::attrs::configured_attr_info_for_tests::ConfiguredAttrInfoForTests;
    use buck2_node::attrs::display::AttrDisplayWithContextExt;
    use buck2_node::attrs::testing::configuration_ctx;
    use dupe::Dupe;
    use starlark::environment::GlobalsBuilder;
    use starlark::environment::Module;

    use super::*;
    use crate::attrs::coerce::testing::CoerceSealed;
    use crate::attrs::coerce::testing::coercion_ctx;
    use crate::attrs::coerce::testing::to_value;
    use crate::interpreter::selector::register_select;

    trait GetMacroDeps {
        type DepsType;
        fn get_deps(&self) -> buck2_error::Result<Vec<Self::DepsType>>;
    }

    impl GetMacroDeps for UnconfiguredMacro {
        type DepsType = TargetLabel;
        fn get_deps(&self) -> buck2_error::Result<Vec<Self::DepsType>> {
            let mut visitor = CoercedDepsCollector::new();
            self.traverse(&mut visitor, Some(PackageLabel::testing_new("root", "")))?;
            let CoercedDepsCollector {
                deps,
                exec_deps,
                toolchain_deps,
                ..
            } = visitor;
            Ok(deps
                .into_iter()
                .chain(exec_deps)
                .chain(toolchain_deps)
                .collect())
        }
    }

    #[test]
    fn test_concat() -> buck2_error::Result<()> {
        // Test
        // ast-grep-ignore: rust/buck2-no-starlark-module
        Module::with_temp_heap(|env| {
            let globals = GlobalsBuilder::standard().with(register_select).build();
            let attr = AttrType::arg(true);
            let value = to_value(
                &env,
                &globals,
                r#""$(exe //:foo) " + select({"DEFAULT": "$(location //:bar)"})"#,
            );

            let coerced = coercion_ctx().coerce_sealed(&attr, value)?;
            let configured = coerced
                .as_ref()
                .configure(&attr, &configuration_ctx(), None)
                .require_compatible()?;
            assert_eq!(
                format!(
                    r#""$(exe root//:foo ({})) $(location root//:bar ({}))""#,
                    configuration_ctx().base_exec_cfg()?,
                    ConfigurationData::testing_new(),
                ),
                configured.as_display_no_ctx().to_string(),
            );

            Ok(())
        })
    }

    #[test]
    fn test_unrecognized_macro_fails_coercion() {
        // ast-grep-ignore: rust/buck2-no-starlark-module
        Module::with_temp_heap(|env| {
            let attr = AttrType::arg(true);
            for (input, name, arg_count) in [
                ("$(output foo.yaml)", "output", 1),
                ("$(@output foo.yaml)", "output", 1),
                ("$(classpath_abi //:foo)", "classpath_abi", 1),
                ("$(maven_coords //:foo)", "maven_coords", 1),
                ("$(query_paths 'deps(//:foo)')", "query_paths", 1),
                ("$(find . -name foo)", "find", 3),
            ] {
                let value = env.heap().alloc(input).to_value();
                let error = coercion_ctx().coerce_sealed(&attr, value).unwrap_err();
                assert!(
                    format!("{error:#}").contains(&format!(
                        "Unrecognized macro `{name}` (with {arg_count} args). If this is shell command substitution rather than a buck macro, escape it as `\\$({name} ...)`"
                    )),
                    "{input}: {error:#}",
                );
            }
        })
    }

    #[test]
    fn test_escaped_macro_coerces_as_string() -> buck2_error::Result<()> {
        // ast-grep-ignore: rust/buck2-no-starlark-module
        Module::with_temp_heap(|env| {
            let attr = AttrType::arg(true);
            let value = env.heap().alloc(r"\$(find . -name foo)").to_value();
            let coerced = coercion_ctx().coerce_sealed(&attr, value)?;
            let CoercedAttrRefKind::ArgSimple(part) = coerced.as_ref().kind() else {
                panic!("expected a plain-string arg");
            };
            assert_eq!("$(find . -name foo)", part);
            Ok(())
        })
    }

    #[test]
    fn test_user_defined_macros_coerce() -> buck2_error::Result<()> {
        // ast-grep-ignore: rust/buck2-no-starlark-module
        Module::with_temp_heap(|env| {
            let attr = AttrType::arg(true);
            for (input, expected) in [
                ("$(CXX)", "$(CXX)"),
                ("$(cxxppflags //:foo)", "$(cxxppflags root//:foo)"),
                (
                    "$(cxxppflags //:foo shared)",
                    "$(cxxppflags root//:foo shared)",
                ),
            ] {
                let value = env.heap().alloc(input).to_value();
                let coerced = coercion_ctx().coerce_sealed(&attr, value)?;
                assert_eq!(
                    coerced.as_ref().as_display_no_ctx().to_string(),
                    format!(r#""{expected}""#)
                );
            }
            Ok(())
        })
    }

    #[test]
    fn test_location() -> buck2_error::Result<()> {
        let ctx = coercion_ctx();
        let location = UnconfiguredMacro::new_location(
            &ctx,
            vec!["//some:target".to_owned()],
            MacroDepKind::Regular,
        )?;
        let deps = location
            .get_deps()?
            .iter()
            .map(|t| t.to_string())
            .collect::<Vec<_>>();
        assert_eq!(vec!["root//some:target".to_owned()], deps);

        let configured = location.configure(&configuration_ctx(), None)?;

        if let MacroBase::Location {
            label: target,
            dep_kind: MacroDepKind::Regular,
        } = &configured
        {
            let mut info = ConfiguredAttrInfoForTests::new();
            configured.traverse(&mut info, PackageLabel::testing_new("root", ""))?;
            assert_eq!(buck_mut_insertion_set![target.dupe()], info.deps);
        } else {
            return Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "Expected Location"
            ));
        }

        Ok(())
    }

    #[test]
    fn test_location_exec() -> buck2_error::Result<()> {
        let ctx = coercion_ctx();
        let location = UnconfiguredMacro::new_location(
            &ctx,
            vec!["//some:target".to_owned()],
            MacroDepKind::Exec,
        )?;
        let deps = location
            .get_deps()?
            .iter()
            .map(|t| t.to_string())
            .collect::<Vec<_>>();
        assert_eq!(vec!["root//some:target".to_owned()], deps);
        assert_eq!("location_exec root//some:target", &location.to_string());

        let config_ctx = configuration_ctx();
        let configured = location.configure(&config_ctx, None)?;

        if let MacroBase::Location { label, .. } = &configured {
            let mut info = ConfiguredAttrInfoForTests::new();
            configured.traverse(&mut info, PackageLabel::testing_new("root", ""))?;
            assert_eq!(label.cfg(), config_ctx.base_exec_cfg()?.cfg());
            assert_eq!(buck_mut_insertion_set![label.dupe()], info.execution_deps);
            assert_eq!(buck_mut_insertion_set![], info.deps);
        } else {
            return Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "Expected Location"
            ));
        }

        Ok(())
    }

    #[test]
    fn test_location_toolchain() -> buck2_error::Result<()> {
        let ctx = coercion_ctx();
        let location = UnconfiguredMacro::new_location(
            &ctx,
            vec!["//some:target".to_owned()],
            MacroDepKind::Toolchain,
        )?;
        let deps = location
            .get_deps()?
            .iter()
            .map(|t| t.to_string())
            .collect::<Vec<_>>();
        assert_eq!(vec!["root//some:target".to_owned()], deps);
        assert_eq!(
            "location_toolchain root//some:target",
            &location.to_string()
        );

        let config_ctx = configuration_ctx();
        let configured = location.configure(&config_ctx, None)?;

        if let MacroBase::Location { label, .. } = &configured {
            let mut info = ConfiguredAttrInfoForTests::new();
            configured.traverse(&mut info, PackageLabel::testing_new("root", ""))?;
            assert_eq!(label.cfg(), config_ctx.toolchain_cfg().cfg());
            assert_eq!(buck_mut_insertion_set![label.dupe()], info.toolchain_deps);
            assert_eq!(buck_mut_insertion_set![], info.deps);
            assert_eq!(buck_mut_insertion_set![], info.execution_deps);
        } else {
            return Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "Expected Location"
            ));
        }

        Ok(())
    }

    #[test]
    fn test_exe() -> buck2_error::Result<()> {
        let ctx = coercion_ctx();
        let exe = UnconfiguredMacro::new_exe(&ctx, vec!["//some:target".to_owned()], true)?;
        let deps = exe
            .get_deps()?
            .iter()
            .map(|t| t.to_string())
            .collect::<Vec<_>>();
        assert_eq!(vec!["root//some:target".to_owned()], deps);
        assert_eq!("exe root//some:target", &exe.to_string());

        let config_ctx = configuration_ctx();
        let configured = exe.configure(&config_ctx, None)?;

        if let MacroBase::Exe { label, .. } = &configured {
            let mut info = ConfiguredAttrInfoForTests::new();
            configured.traverse(&mut info, PackageLabel::testing_new("root", ""))?;
            assert_eq!(label.cfg(), config_ctx.base_exec_cfg()?.cfg());
            assert_eq!(buck_mut_insertion_set![label.dupe()], info.execution_deps);
            assert_eq!(buck_mut_insertion_set![], info.deps);
        } else {
            return Err(buck2_error!(buck2_error::ErrorTag::Input, "Expected Exe"));
        }

        Ok(())
    }

    #[test]
    fn test_exe_target() -> buck2_error::Result<()> {
        let ctx = coercion_ctx();
        let exe = UnconfiguredMacro::new_exe(&ctx, vec!["//some:target".to_owned()], false)?;
        let deps = exe
            .get_deps()?
            .iter()
            .map(|t| t.to_string())
            .collect::<Vec<_>>();
        assert_eq!(vec!["root//some:target".to_owned()], deps);
        assert_eq!("exe_target root//some:target", &exe.to_string());

        let config_ctx = configuration_ctx();
        let configured = exe.configure(&config_ctx, None)?;

        if let MacroBase::Exe { label, .. } = &configured {
            let mut info = ConfiguredAttrInfoForTests::new();
            configured.traverse(&mut info, PackageLabel::testing_new("root", ""))?;
            assert_eq!(label.cfg(), config_ctx.cfg().cfg());
            assert_eq!(buck_mut_insertion_set![], info.execution_deps);
            assert_eq!(buck_mut_insertion_set![label.dupe()], info.deps);
        } else {
            return Err(buck2_error!(buck2_error::ErrorTag::Input, "Expected Exe"));
        }

        Ok(())
    }
}
