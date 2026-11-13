/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_build_api::interpreter::rule_defs::register_rule_defs;
use buck2_build_api::interpreter::rule_defs::transitive_set::globals::register_transitive_set_types;
use buck2_build_api::interpreter::rule_defs::transitive_set::transitive_set_definition::register_transitive_set;
use buck2_core::bzl::ImportPath;
use buck2_interpreter_for_build::interpreter::testing::Tester;
use buck2_interpreter_for_build::interpreter::testing::expect_error;
use indoc::indoc;

use crate::interpreter::rule_defs::artifact::testing::artifactory;
use crate::interpreter::rule_defs::cmd_args::testing::command_line_stringifier;
use crate::interpreter::transitive_set::testing::tset_factory;

fn transitive_set_tester() -> Tester {
    let mut tester = Tester::new().unwrap();
    tester.additional_globals(register_transitive_set);
    tester.additional_globals(register_transitive_set_types);
    tester.additional_globals(tset_factory);
    tester.additional_globals(artifactory);
    tester.additional_globals(command_line_stringifier);
    tester.additional_globals(register_rule_defs);
    tester
}

#[test]
fn test_define_transitive_set() -> buck2_error::Result<()> {
    let mut tester = transitive_set_tester();
    tester.run_starlark_bzl_test(indoc!(
        r#"
        FooSet = transitive_set()
        FooSet2 = FooSet

        def test():
            assert_eq("FooSet", str(FooSet))
            assert_eq("Value(TransitiveSetDefinition(FooSet declared in root//some/package/defs.bzl))", debug(FooSet))
            assert_eq("FooSet", str(FooSet2))
            assert_eq("unnamed transitive set", repr(transitive_set()))
        "#
    ))?;

    Ok(())
}

#[test]
fn test_hash_transitive_set() -> buck2_error::Result<()> {
    let mut tester = transitive_set_tester();

    tester.add_import(
        &ImportPath::testing_new("root//test:def1.bzl"),
        indoc!(
            r#"
            FooSet = transitive_set()
            dict = {FooSet: 1}
            "#
        ),
    )?;

    tester.run_starlark_bzl_test(indoc!(
        r#"
        load("//test:def1.bzl", "FooSet", "dict")

        def test():
            assert_eq(dict[FooSet], 1)
        "#
    ))?;

    Ok(())
}

#[test]
fn test_define_transitive_set_projections() -> buck2_error::Result<()> {
    let mut tester = transitive_set_tester();
    tester.run_starlark_test(indoc!(
        r#"
        def project1(_value):
            pass

        FooSet = transitive_set(args_projections = { "foo": project1 })

        def test():
            pass
        "#
    ))?;

    Ok(())
}

// With debug assertions, starlark checks what a conversion accepts against the type it
// declares, and panics here: the FIXME below is such a disagreement.
#[test]
#[cfg_attr(
    debug_assertions,
    should_panic(expected = "`get` disagrees with the declared type")
)]
fn test_define_transitive_set_wrong_signature() {
    for (param, bad) in [
        ("args_projections", "def bad(_value, _extra):"),
        ("json_projections", "def bad():"),
        ("reductions", "def bad(_children):"),
    ] {
        let contents = format!(
            indoc!(
                r#"
                {bad}
                    pass

                FooSet = transitive_set({param} = {{ "foo": bad }})

                def test():
                    pass
                "#
            ),
            bad = bad,
            param = param,
        );

        let err = transitive_set_tester()
            .run_starlark_bzl_test(&contents)
            .unwrap_err();
        let err = format!("{err:?}");
        // FIXME: The dict is of the type; what is wrong is the signature of an entry.
        assert!(
            err.contains(&format!("Type of parameter `{param}` doesn't match")),
            "{err}"
        );
    }
}

#[test]
fn test_create_transitive_set() -> buck2_error::Result<()> {
    let mut tester = transitive_set_tester();

    tester.run_starlark_bzl_test(indoc!(
        r#"
        FooSet = transitive_set()
        BarSet = transitive_set()

        def test():
            f1 = make_tset(FooSet, value = 1)
            f2 = make_tset(FooSet, value = 2)
            f3 = make_tset(FooSet, value = 3, children = [f1, f2])
        "#
    ))?;

    let contents = indoc!(
        r#"
        FooSet = transitive_set()
        BarSet = transitive_set()

        def test():
            f1 = make_tset(FooSet, value = 1)
            make_tset(BarSet, value = None, children = [f1])
        "#
    );

    expect_error(
        tester.run_starlark_bzl_test(contents),
        contents,
        "expected: `TransitiveSetDefinition(BarSet declared in root//some/package/defs.bzl)`, \
         got: `TransitiveSetDefinition(FooSet declared in root//some/package/defs.bzl)`",
    );

    Ok(())
}

#[test]
fn test_frozen_transitive_sets() -> buck2_error::Result<()> {
    let mut tester = transitive_set_tester();

    tester.add_import(
        &ImportPath::testing_new("root//test:def1.bzl"),
        "FooSet = transitive_set()",
    )?;

    tester.add_import(
        &ImportPath::testing_new("root//test:use1.bzl"),
        indoc!(
            r#"
            load("//test:def1.bzl", "FooSet")

            f1 = make_tset(FooSet, value = 1)
            "#
        ),
    )?;

    tester.run_starlark_bzl_test(indoc!(
        r#"
        load("//test:def1.bzl", "FooSet")
        load("//test:use1.bzl", "f1")

        def test():
            f2 = make_tset(FooSet, value = 2)
            f3 = make_tset(FooSet, value = 3, children = [f1, f2])
        "#
    ))?;

    Ok(())
}

#[test]
fn test_transitive_set_display() -> buck2_error::Result<()> {
    let mut tester = transitive_set_tester();

    tester.run_starlark_bzl_test(indoc!(
        r#"
        FooSet = transitive_set()

        def test():
            f1 = make_tset(FooSet, value = 1)
            f2 = make_tset(FooSet, value = 2, children = [f1])
            f3 = make_tset(FooSet, children = [f1])

            assert_eq("FooSet(value=1, 0 children)", repr(f1))
            assert_eq("FooSet(value=2, 1 children)", repr(f2))
            assert_eq("FooSet(1 children)", repr(f3))

            assert_eq("FooSet(\n  value=1,\n  0 children\n)", prepr(f1))
            assert_eq("FooSet( 1 children )", prepr(f3))
        "#
    ))?;

    Ok(())
}

#[test]
fn test_transitive_sets_projection() -> buck2_error::Result<()> {
    let mut tester = transitive_set_tester();

    tester.run_starlark_bzl_test(indoc!(
        r#"
        def project1(value):
            return str(value)

        FooSet = transitive_set(args_projections = {
            "foo": project1
        })

        def test():
            f1 = make_tset(FooSet, value = 1)
            f1.project_as_args("foo")
        "#
    ))?;

    let contents = indoc!(
        r#"
        FooSet = transitive_set()

        def test():
            f1 = make_tset(FooSet, value = 1)
            f1.project_as_args("foo")
        "#
    );

    expect_error(
        tester.run_starlark_bzl_test(contents),
        contents,
        "requested projection `foo` does not exist.",
    );

    Ok(())
}

#[test]
fn test_transitive_sets_iteration() -> buck2_error::Result<()> {
    let mut tester = transitive_set_tester();

    /* Validate on a simple tree which validates transitive links:
     *
     *  4 -> 3 -> 2 -> 1
     *        \--------^
     */
    tester.run_starlark_bzl_test(indoc!(
        r#"
        FooSet = transitive_set()

        def test():
            f1 = make_tset(FooSet, value = 1)
            f2 = make_tset(FooSet, value = 2, children = [f1])
            f3 = make_tset(FooSet, value = 3, children = [f1, f2])
            f4 = make_tset(FooSet, children = [f3])

            assert_eq([3, 1, 2], list(f4.traverse()))
            assert_eq([3, 1, 2], list(f4.traverse(ordering = "preorder")))

            assert_eq([1, 2, 3], list(f4.traverse(ordering = "postorder")))

            assert_eq([3, 2, 1], list(f4.traverse(ordering = "topological")))

            assert_eq([3, 1, 2], list(f4.traverse(ordering = "bfs")))
        "#
    ))?;

    /* Validate on a more complex tree with:
     * - More than 2 children per node.
     * - Nodes that depend on lower levels of the tree.
     * - Nodes that depend on siblings.
     *
     *      ┌─────┬─9─┬───┐
     *      │     │   │   │
     *    ┌─5─┐ ┌─6─┐ 7◄──8
     *    │   │ │   │     │
     *    │   2 │   3     4
     *    1◄────┘
     */
    tester.run_starlark_bzl_test(indoc!(
        r#"
        FooSet = transitive_set()

        def test():
            f1 = make_tset(FooSet, value = 1)
            f2 = make_tset(FooSet, value = 2)
            f3 = make_tset(FooSet, value = 3)
            f4 = make_tset(FooSet, value = 4)
            f5 = make_tset(FooSet, value = 5, children = [f1, f2])
            f6 = make_tset(FooSet, value = 6, children = [f1, f3])
            f7 = make_tset(FooSet, value = 7)
            f8 = make_tset(FooSet, value = 8, children = [f4, f7])
            f9 = make_tset(FooSet, value = 9, children = [f5, f6, f7, f8])

            assert_eq([9, 5, 1, 2, 6, 3, 7, 8, 4], list(f9.traverse()))
            assert_eq([9, 5, 1, 2, 6, 3, 7, 8, 4], list(f9.traverse(ordering = "preorder")))

            assert_eq([1, 2, 5, 3, 6, 7, 4, 8, 9], list(f9.traverse(ordering = "postorder")))

            assert_eq([9, 5, 2, 6, 1, 3, 8, 4, 7], list(f9.traverse(ordering = "topological")))

            assert_eq([9, 5, 6, 7, 8, 1, 2, 3, 4], list(f9.traverse(ordering = "bfs")))
        "#
    ))?;

    Ok(())
}

#[test]
fn test_frozen_transitive_sets_iteration() -> buck2_error::Result<()> {
    let mut tester = transitive_set_tester();

    tester.add_import(
        &ImportPath::testing_new("root//test:def1.bzl"),
        "FooSet = transitive_set()",
    )?;

    tester.add_import(
        &ImportPath::testing_new("root//test:use1.bzl"),
        indoc!(
            r#"
            load("//test:def1.bzl", "FooSet")

            f1 = make_tset(FooSet, value = 1)
            "#
        ),
    )?;

    tester.run_starlark_bzl_test(indoc!(
        r#"
        load("//test:def1.bzl", "FooSet")
        load("//test:use1.bzl", "f1")

        def test():
            f2 = make_tset(FooSet, value = 2, children = [f1])

            assert_eq([1], list(f1.traverse()))
            assert_eq([2, 1], list(f2.traverse()))
        "#
    ))?;

    Ok(())
}

#[test]
fn test_projection_args() -> buck2_error::Result<()> {
    let mut tester = transitive_set_tester();

    tester.add_import(
        &ImportPath::testing_new("root//test:decl.bzl"),
        indoc!(
            r#"
            "#
        ),
    )?;

    tester.run_starlark_bzl_test(indoc!(
        r#"
        def project(val):
            if val == 1:
                return "foo"
            if val == 2:
                return cmd_args("bar")
            if val == 3:
                return ["baz"]

        FooSet = transitive_set(args_projections = {
            "project": project
        })

        def test():
            f3 = make_tset(FooSet, value = 3)
            f2 = make_tset(FooSet, value = 2)
            f1 = make_tset(FooSet, value = 1, children = [f2, f3])
            proj = f1.project_as_args("project")

            assert_eq(["foo", "bar", "baz"], get_args(proj))
        "#
    ))?;

    Ok(())
}

#[test]
fn test_projection_inputs() -> buck2_error::Result<()> {
    let mut tester = transitive_set_tester();

    tester.run_starlark_bzl_test(indoc!(
        r#"
        def project(value):
            return value

        FooSet = transitive_set(args_projections = {
            "project": project
        })

        def test():
            a1 = source_artifact("a1", "a1.h")
            a2 = source_artifact("a2", "a2.h")
            a3 = source_artifact("a3", "a3.h")

            f2 = make_tset(FooSet, value = [a2, a3])
            f1 = make_tset(FooSet, value = [a1], children = [f2])

            proj2 = f2.project_as_args("project")
            proj1 = f1.project_as_args("project")

            # Always just 1 input, no matter the structure.
            assert_eq(1, len(cmd_args().add(proj2).inputs))
            assert_eq(1, len(cmd_args().add(proj1).inputs))
        "#
    ))?;

    Ok(())
}

#[test]
fn test_projection_iteration() -> buck2_error::Result<()> {
    let mut tester = transitive_set_tester();

    /*
     *  1 -> 2 -> 3
     *   \--------^
     */
    tester.run_starlark_bzl_test(indoc!(
        r#"
        def project(value):
            return value

        FooSet = transitive_set(args_projections = {
            "project": project
        })

        def test():
            f3 = make_tset(FooSet, value = "baz")
            f2 = make_tset(FooSet, value = "bar", children = [f3])
            f1 = make_tset(FooSet, value = "foo", children = [f3, f2])

            proj = f1.project_as_args("project")
            l = list(proj.traverse())

            assert_eq(3, len(l))
            assert_eq(["foo"], get_args(l[0]))
            assert_eq(["baz"], get_args(l[1]))
            assert_eq(["bar"], get_args(l[2]))

            proj2 = f1.project_as_args("project", ordering = "topological")
            l2 = list(proj2.traverse())

            assert_eq(3, len(l))
            assert_eq(["foo"], get_args(l2[0]))
            assert_eq(["bar"], get_args(l2[1]))
            assert_eq(["baz"], get_args(l2[2]))
        "#
    ))?;

    Ok(())
}

#[test]
fn test_json_projection() -> buck2_error::Result<()> {
    let _guard = buck2_util::threads::ignore_stack_overflow_checks_for_current_thread();
    let mut tester = transitive_set_tester();

    /*
     *  1 -> 2 -> 3
     *   \--------^
     */
    tester.run_starlark_bzl_test(indoc!(
        r#"
        def project(value):
            return struct(value = value)

        FooSet = transitive_set(json_projections = {
            "project": project
        })

        def test():
            f3 = make_tset(FooSet, value = "baz")
            f2 = make_tset(FooSet, value = "bar", children = [f3])
            f1 = make_tset(FooSet, value = "foo", children = [f3, f2])

            proj = f1.project_as_json("project")
            l = list(proj.traverse())

            assert_eq(3, len(l))
            assert_eq(struct(value="foo"), l[0])
            assert_eq(struct(value="baz"), l[1])
            assert_eq(struct(value="bar"), l[2])

            proj2 = f1.project_as_json("project", ordering = "topological")
            l2 = list(proj2.traverse())

            assert_eq(3, len(l2))
            assert_eq(struct(value="foo"), l2[0])
            assert_eq(struct(value="bar"), l2[1])
            assert_eq(struct(value="baz"), l2[2])
        "#
    ))?;

    Ok(())
}

#[test]
fn test_reduction() -> buck2_error::Result<()> {
    let mut tester = transitive_set_tester();

    tester.run_starlark_bzl_test(indoc!(
        r#"
        def depth(children, _value):
            return max(children + [0]) + 1

        def truthyness(children, value):
            c = all(children)
            if value == None:
                return c
            return c and value

        FooSet = transitive_set(reductions = {
            "depth": depth,
            "truthyness": truthyness,
        })

        def test():
            f3 = make_tset(FooSet, value = False)
            f2 = make_tset(FooSet, value = True)
            f1 = make_tset(FooSet, value = True, children = [f2])
            f0 = make_tset(FooSet, children = [f1, f3])

            assert_eq(1, f3.reduce("depth"))
            assert_eq(1, f2.reduce("depth"))
            assert_eq(2, f1.reduce("depth"))
            assert_eq(3, f0.reduce("depth"))

            assert_eq(False, f3.reduce("truthyness"))
            assert_eq(True, f2.reduce("truthyness"))
            assert_eq(True, f1.reduce("truthyness"))
            assert_eq(False, f0.reduce("truthyness"))
        "#
    ))?;

    Ok(())
}

#[test]
fn test_definition_type() -> buck2_error::Result<()> {
    let mut tester = transitive_set_tester();

    tester.add_import(
        &ImportPath::testing_new("root//test:def.bzl"),
        "FooSet = transitive_set()",
    )?;

    tester.run_starlark_bzl_test(indoc!(
        r#"
        load("//test:def.bzl", "FooSet")

        BarSet = transitive_set()

        def test():
            QuxSet = transitive_set();
            assert_eq("FooSet", FooSet.type)
            assert_eq("BarSet", BarSet.type)
            assert_eq("TransitiveSetDefinition", QuxSet.type)
        "#
    ))?;

    Ok(())
}

#[test]
fn test_type_1() -> buck2_error::Result<()> {
    let mut tester = transitive_set_tester();

    tester.run_starlark_bzl_test(indoc!(
        r#"
        FooSet = transitive_set()

        def wants_foo_set(f: FooSet):
            pass

        def wants_transitive_set(f: TransitiveSet):
            pass

        def test():
            s = make_tset(FooSet)
            wants_foo_set(s)
            wants_transitive_set(s)
        "#
    ))?;
    Ok(())
}

#[test]
fn test_type_2() -> buck2_error::Result<()> {
    let mut tester = transitive_set_tester();
    let contents = indoc!(
        r#"
        FooSet = transitive_set()

        def wants_not_foo_set(f: str):
            pass

        # Separate function to erase type and check runtime error instead of compile-time error.
        def mk():
            return make_tset(FooSet)

        def test():
            wants_not_foo_set(mk())
        "#
    );

    expect_error(
        tester.run_starlark_bzl_test(contents),
        contents,
        "does not match the type annotation",
    );

    Ok(())
}

#[test]
fn test_transitive_set_ordering_docs() -> buck2_error::Result<()> {
    let mut tester = transitive_set_tester();

    /*       [A] (root)
     *       / \
     *     [B] [C]
     *     / \   \
     *   [D]  \   \
     *   / \   \  /
     *   \ [F] / /
     *    \   / /
     *     \ / /
     *      [E]
     */
    tester.run_starlark_bzl_test(indoc!(
        r#"
        def project(value: str) -> str:
            return value.lower()

        MySet = transitive_set(args_projections = {
            "project": project
        })

        ctx = struct(
            actions = struct(
                tset = make_tset,
            ),
        )

        def test():
            F = ctx.actions.tset(MySet, value = "F")
            E = ctx.actions.tset(MySet, value = "E")
            D = ctx.actions.tset(MySet, value = "D", children = [E, F])
            C = ctx.actions.tset(MySet, value = "C", children = [E])
            B = ctx.actions.tset(MySet, value = "B", children = [D, E])
            A = ctx.actions.tset(MySet, value = "A", children = [B, C])

            values = list(A.traverse(ordering = "topological"))

            # This also works for projections
            args = A.project_as_args("project", ordering = "topological")
            assert_eq(["a", "b", "d", "f", "c", "e"], get_args(args))

            # Test all orderings which show up in the table.
            assert_eq(["A", "B", "D", "F", "E", "C"], list(A.traverse()))
            assert_eq(["A", "B", "D", "F", "E", "C"], list(A.traverse(ordering = "preorder")))
            assert_eq(["E", "F", "D", "B", "C", "A"], list(A.traverse(ordering = "postorder")))
            assert_eq(["A", "B", "D", "F", "C", "E"], list(A.traverse(ordering = "topological")))
            assert_eq(["A", "B", "C", "D", "E", "F"], list(A.traverse(ordering = "bfs")))
            assert_eq(["A", "B", "D", "E", "F", "C"], list(A.traverse(ordering = "dfs")))
        "#
    ))?;

    Ok(())
}

#[test]
fn test_accessors() -> buck2_error::Result<()> {
    let _guard = buck2_util::threads::ignore_stack_overflow_checks_for_current_thread();
    let mut tester = transitive_set_tester();

    tester.run_starlark_bzl_test(indoc!(
        r#"
        def project1(_value):
            return []

        FooSet = transitive_set(
            args_projections = { "foo": project1 },
            json_projections = { "bar": project1 },
        )

        def test():
            s = make_tset(FooSet)
            assert_eq(s.definition, FooSet)
            assert_eq(s.value,  None)

            proj = s.project_as_args("foo")
            assert_eq(proj.transitive_set, s)
            assert_eq(proj.projection_name, "foo")

            json = s.project_as_json("bar")
            assert_eq(json.transitive_set, s)
            assert_eq(json.projection_name, "bar")

            s2 = make_tset(FooSet, value = 1)
            assert_eq(s2.value,  1)

            f4 = make_tset(FooSet, value = "baz")
            assert_eq([], f4.children)
            assert_eq([], [x.value for x in f4.children])

            f3 = make_tset(FooSet, value = "bar", children = [f4])
            assert_eq([f4], f3.children)
            assert_eq(["baz"], [x.value for x in f3.children])

            f2 = make_tset(FooSet, children = [f4, f3])
            assert_eq([f4, f3], f2.children)
            assert_eq(["baz", "bar"], [x.value for x in f2.children])

            f1 = make_tset(FooSet, children = [f4, f3, f2])
            assert_eq([f4, f3, f2], f1.children)
            assert_eq(["baz", "bar"], filter(None, [x.value for x in f1.children])) 
            "#
    ))?;

    Ok(())
}
