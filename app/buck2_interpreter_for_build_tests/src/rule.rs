/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_build_api::interpreter::rule_defs::transitive_set::transitive_set_definition::register_transitive_set;
use buck2_core::bzl::ImportPath;
use buck2_interpreter::file_loader::LoadedModules;
use buck2_interpreter_for_build::interpreter::testing::Tester;
use buck2_interpreter_for_build::nodes::attr_spec::AttributeSpecExt;
use buck2_node::attrs::inspect_options::AttrInspectOptions;
use buck2_node::attrs::spec::AttributeSpec;
use buck2_node::nodes::unconfigured::testing::targets_to_json;
use indoc::indoc;
use serde_json::json;
use starlark::docs::DocFunction;
use starlark::docs::DocItem;
use starlark::docs::DocMember;
use starlark::docs::DocParam;
use starlark::docs::DocReturn;
use starlark::docs::DocString;
use starlark::docs::DocStringKind;
use starlark::typing::Ty;

fn rule_tester() -> Tester {
    let mut tester = Tester::new().unwrap();
    tester.additional_globals(register_transitive_set);
    tester
}

#[test]
fn rule_creates_callable() -> buck2_error::Result<()> {
    let mut tester = rule_tester();
    tester.run_starlark_test(indoc!(
        r#"
        def impl(ctx):
            pass

        string_attr = attrs.string(default="something", doc = "foo")
        frozen_rule = rule(
            impl=impl,
            attrs={
                "param1": string_attr,
                "param2": attrs.list(string_attr, default=[], doc = "foo"),
            }
        )

        def test():
            assert_eq(None, frozen_rule(name="target_name"))
            assert_eq("frozen_rule()", repr(frozen_rule))
        "#
    ))?;
    Ok(())
}

#[test]
fn freeze_fails_if_not_assigned() {
    let mut tester = rule_tester();
    tester.run_starlark_bzl_test_expecting_error(
        indoc!(
            r#"
        def impl(ctx):
            pass

        def test():
            string_attr = attrs.string(default="something", doc = "foo")
            nonfrozen_rule = rule(
                impl=impl,
                attrs={
                    "param1": string_attr,
                }
            )
            nonfrozen_rule(name="target_name")
        "#
        ),
        "may only be called after the module is loaded",
    );
}

#[test]
fn rule_disallows_reserved_names() {
    let mut tester = rule_tester();
    tester.run_starlark_bzl_test_expecting_error(
        indoc!(
            r#"
        def impl(ctx):
            pass

        string_attr = attrs.string(default="something", doc = "foo")
        frozen_rule = rule(
            impl=impl,
            attrs={
                "name": string_attr,
                "param1": string_attr,
            }
        )
        def test():
            frozen_rule(name="target_name")
        "#
        ),
        "not a valid attribute",
    );
}

#[test]
fn rule_unbound() {
    let mut tester = rule_tester();
    tester.run_starlark_bzl_test_expecting_error(
        indoc!(
            r#"
        def impl(ctx):
            pass
        rules = []
        rules.append(rule(
            impl=impl,
            attrs={}
        ))
        def test():
            pass
        "#
        ),
        "must be assigned to a variable",
    );
}

#[test]
fn udr_is_recorded() -> buck2_error::Result<()> {
    let content = indoc!(
        r#"
        def impl(ctx):
            pass

        string_attr = attrs.string(default="some_default", doc="")
        mandatory_string_attr = attrs.string(default="", doc="")
        dep_attr = attrs.dep(default="//foo:baz")
        src_attr = attrs.source(default="//foo:baz")
        foo_binary = rule(
            impl=impl,
            attrs={
                "optional": string_attr,
                "other_optional": string_attr,
                "mandatory": mandatory_string_attr,
                "dep": dep_attr,
                "src": src_attr,
            },
        )

        def hide_type(v): return v

        def test():
            foo_binary(
                name="target1",
                mandatory="m1",
                optional="o1",
                dep=":bar",
                # TODO(nga): function type should accept `None` for optional parameters.
                other_optional=hide_type(None),
                src="file1.java",
            )
            foo_binary(name="target2", mandatory="m2", other_optional="o1")
        "#
    );
    let mut tester = rule_tester();
    let result = tester.run_starlark_test(content)?;

    let expected = json!({
        "target1": {
            "name": "target1",
            "__type__": "root//some/package/defs.bzl:foo_binary",
            "compatible_with": [],
            "default_target_platform": null,
            "mandatory": "m1",
            "optional": "o1",
            "other_optional": "some_default",
            "dep": "root//some/package:bar",
            "exec_compatible_with": [],
            "src": "root//some/package/file1.java",
            "target_compatible_with": [],
            "modifiers": [],
            "tests": [],
            "visibility": [],
            "within_view": ["PUBLIC"],
            "metadata": {},
        },
        "target2": {
            "name": "target2",
            "__type__": "root//some/package/defs.bzl:foo_binary",
            "compatible_with": [],
            "default_target_platform": null,
            "mandatory": "m2",
            "optional": "some_default",
            "other_optional": "o1",
            "dep": "root//foo:baz",
            "exec_compatible_with": [],
            "src": "root//foo:baz",
            "target_compatible_with": [],
            "modifiers": [],
            "tests": [],
            "visibility": [],
            "within_view": ["PUBLIC"],
            "metadata": {},
        },
    });
    let actual = targets_to_json(
        &result,
        Tester::build_file_path().package(),
        AttrInspectOptions::All,
    )?;
    assert_eq!(expected, actual, "`{expected:#?}` != `{actual:#?}`");
    Ok(())
}

#[test]
fn udr_rejects_invalid_parameters() {
    let prefix = indoc!(
        r#"
        def impl(ctx):
            pass

        string_attr = attrs.string(default="some_default")
        mandatory_string_attr = attrs.string(doc="")
        foo_binary = rule(
            impl=impl,
            attrs={"optional":string_attr, "mandatory": mandatory_string_attr},
        )
        def hide_type(v): return v

        def test():
        "#
    );

    let missing_name = r#"    hide_type(foo_binary)(mandatory="s")"#;
    let invalid_name = r#"    hide_type(foo_binary)(name="bad name", mandatory="s")"#;
    let missing_mandatory = r#"    hide_type(foo_binary)(name="t1")"#;
    let wrong_type = r#"    hide_type(foo_binary)(name="t1", mandatory=1)"#;
    let unknown_param = r#"    hide_type(foo_binary)(name="t1", mandatory="m1", unknown="")"#;
    let duplicate_targets = format!(
        "    {}\n    {}",
        r#"hide_type(foo_binary)(name="t1", mandatory="m1")"#,
        r#"hide_type(foo_binary)(name="t1", mandatory="m2")"#,
    );

    let run = |content: &str, msg: &str| {
        let mut tester = rule_tester();
        tester.run_starlark_test_expecting_error(&format!("{prefix}\n{content}"), msg);
    };

    run(
        missing_name,
        "Missing named-only parameter `name` for call to `foo_binary`",
    );
    run(invalid_name, "Invalid target name `bad name`.");
    run(
        missing_mandatory,
        "Missing named-only parameter `mandatory`",
    );
    run(wrong_type, "coercing attribute `mandatory`");
    run(unknown_param, "Found `unknown` extra named parameter");
    run(
        &duplicate_targets,
        "Attempted to register target root//some/package:t1 twice",
    );
}

#[test]
fn option_allows_none() -> buck2_error::Result<()> {
    let mut tester = rule_tester();
    tester.run_starlark_test_expecting_error(
        "def test():\n attrs.option(attrs.string(), default = 'test')",
        "parameter must be `None` or absent",
    );
    let mut tester = rule_tester();
    tester.run_starlark_test("def test():\n attrs.option(attrs.string(), default = None)")?;
    let mut tester = rule_tester();
    tester.run_starlark_test("def test():\n attrs.option(attrs.string())")?;
    let mut tester = rule_tester();
    tester.run_starlark_test_expecting_error(
        "def test():\n attrs.option(attrs.string(), default = select({'DEFAULT': 'test'}))",
        "parameter must be `None` or absent",
    );
    let mut tester = rule_tester();
    tester.run_starlark_test(
        "def test():\n attrs.option(attrs.string(), default = select({'DEFAULT': None}))",
    )?;
    Ok(())
}

#[test]
fn returns_documentation() -> buck2_error::Result<()> {
    let bzl = indoc::indoc!(
        r#"def impl(ctx):
            pass

        any_attr = attrs.any(doc="any docs")
        arg_attr = attrs.arg(default="arg", doc="arg docs")
        bool_attr = attrs.bool(default=True, doc="bool docs")
        string_attr = attrs.string(default="string", doc="string docs")
        default_only_attr = attrs.default_only(attrs.string(default="default_only"), doc="default_only docs")
        dep_attr = attrs.dep(default="//:dep", doc="dep docs")
        dict_attr = attrs.dict(attrs.string(), attrs.bool(), default={"dict": True}, doc="dict docs")
        list_attr = attrs.list(attrs.string(), default=["list"], doc="list docs")
        one_of_attr = attrs.one_of(attrs.bool(), attrs.string(), default="", doc="one_of docs")
        option_attr = attrs.option(attrs.string(), default=None, doc="option docs")
        query_attr = attrs.query(doc="query docs")
        source_attr = attrs.source(default="//:src", doc="source docs")
        tuple_attr = attrs.tuple(attrs.bool(), attrs.string(), default=(True, "some string"), doc="tuple docs")

        doc = """Summary for foo_binary

        Details for foo_binary
        """

        foo_binary = rule(
            impl=impl,
            attrs={
                "any": any_attr,
                "arg": arg_attr,
                "bool": bool_attr,
                "default_only": default_only_attr,
                "dep": dep_attr,
                "dict": dict_attr,
                "list": list_attr,
                "one_of": one_of_attr,
                "option": option_attr,
                "query": query_attr,
                "source": source_attr,
                "string": string_attr,
                "tuple": tuple_attr,
            },
            doc = doc,
        )
        "#
    );

    fn arg(name: &str, raw_type: Ty, default: Option<&str>) -> DocParam {
        DocParam {
            name: name.to_owned(),
            docs: DocString::from_docstring(DocStringKind::Starlark, &format!("{name} docs")),
            typ: raw_type,
            default_value: default.map(String::from),
        }
    }

    // Grab the default parameters that are inserted into every rule.
    let empty_spec = AttributeSpec::testing_new(Default::default());
    let mut params = empty_spec
        .signature("foo_binary".to_owned())
        .documentation(empty_spec.starlark_types(), empty_spec.docstrings());

    for param in &mut params.named_only {
        match param.name.as_str() {
            "default_target_platform" => param.default_value = Some("None".to_owned()),
            "target_compatible_with" => param.default_value = Some("[]".to_owned()),
            "compatible_with" => param.default_value = Some("[]".to_owned()),
            "exec_compatible_with" => param.default_value = Some("[]".to_owned()),
            "visibility" => param.default_value = Some("[]".to_owned()),
            "within_view" => param.default_value = Some("[\"PUBLIC\"]".to_owned()),
            "metadata" => param.default_value = Some("{}".to_owned()),
            "tests" => param.default_value = Some("[]".to_owned()),
            "modifiers" => param.default_value = Some("[]".to_owned()),
            _ => {}
        }
    }

    params.named_only.extend(vec![
        arg("any", Ty::any(), None),
        arg("arg", Ty::string(), Some("\"arg\"")),
        arg("bool", Ty::bool(), Some("True")),
        arg("default_only", Ty::string(), Some("\"default_only\"")),
        arg("dep", Ty::string(), Some("\"root//:dep\"")),
        arg(
            "dict",
            Ty::dict(Ty::string(), Ty::bool()),
            Some("{\"dict\": True}"),
        ),
        arg("list", Ty::list(Ty::string()), Some("[\"list\"]")),
        arg("one_of", Ty::union2(Ty::bool(), Ty::string()), Some("\"\"")),
        arg("option", Ty::union2(Ty::none(), Ty::string()), Some("None")),
        arg("query", Ty::string(), None),
        arg("source", Ty::string(), Some("\"root//:src\"")),
        arg("string", Ty::string(), Some("\"string\"")),
        arg(
            "tuple",
            Ty::tuple2(Ty::bool(), Ty::string()),
            Some("(True, \"some string\")"),
        ),
    ]);

    let expected_docs = DocItem::Member(DocMember::Function(DocFunction {
        docs: DocString::from_docstring(
            DocStringKind::Starlark,
            "Summary for foo_binary\n\nDetails for foo_binary",
        ),
        params,
        ret: DocReturn {
            docs: None,
            typ: Ty::none(),
        },
    }));

    let tester = rule_tester();
    let res = tester.eval_import(
        &ImportPath::testing_new("root//:defs.bzl"),
        bzl,
        LoadedModules::default(),
    )?;
    let docs = res
        .env()
        .get("foo_binary")
        .expect("foo_binary to exist")
        .as_ref()
        .value()
        .documentation();

    assert_eq!(expected_docs, docs);

    Ok(())
}
