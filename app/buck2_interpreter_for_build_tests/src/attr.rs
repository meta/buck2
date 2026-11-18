/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_common::package_listing::listing::PackageListing;
use buck2_common::package_listing::listing::testing::PackageListingExt;
use buck2_core::cells::cell_path_with_allowed_relative_dir::CellPathWithAllowedRelativeDir;
use buck2_core::cells::name::CellName;
use buck2_core::cells::paths::CellRelativePath;
use buck2_core::package::PackageLabel;
use buck2_core::package::package_relative_path::PackageRelativePathBuf;
use buck2_core::pattern::pattern::InferTargetNames;
use buck2_core::plugins::PluginKindSet;
use buck2_interpreter_for_build::attrs::coerce::attr_type::AttrTypeExt;
use buck2_interpreter_for_build::attrs::coerce::coerced_attr::coerce_attr;
use buck2_interpreter_for_build::attrs::coerce::ctx::BuildAttrCoercionContext;
use buck2_interpreter_for_build::attrs::coerce::testing::CoerceSealed;
use buck2_interpreter_for_build::attrs::coerce::testing::coercion_ctx_listing;
use buck2_interpreter_for_build::interpreter::testing::Tester;
use buck2_interpreter_for_build::interpreter::testing::cells;
use buck2_node::attrs::arena::OwnedCoercedAttr;
use buck2_node::attrs::attr_type::AttrType;
use buck2_node::attrs::coercion_context::AttrCoercionContext;
use buck2_node::attrs::coercion_context::CoerceMemoKey;
use buck2_node::attrs::coercion_context::CoerceMemoTable;
use buck2_node::attrs::configurable::AttrIsConfigurable;
use buck2_node::attrs::fmt_context::AttrFmtContext;
use buck2_node::provider_id_set::ProviderIdSet;
use dupe::Dupe;
use indoc::indoc;
use starlark::values::FrozenHeap;
use starlark::values::Heap;
use starlark::values::dict::AllocDict;
use starlark::values::list::AllocList;

fn value_to_string(value: &OwnedCoercedAttr, pkg: PackageLabel) -> buck2_error::Result<String> {
    let ctx = AttrFmtContext {
        package: Some(pkg),
        options: Default::default(),
    };
    match value.as_ref().to_json(&ctx)?.as_str() {
        Some(s) => Ok(s.to_owned()),
        None => Err(buck2_error::buck2_error!(
            buck2_error::ErrorTag::Input,
            "Expected a string, did not get one",
        )),
    }
}

#[test]
fn string_works() -> buck2_error::Result<()> {
    let mut tester = Tester::new().unwrap();
    tester.run_starlark_bzl_test(indoc!(
        r#"
        frozen = attrs.string(default="something", doc = "foo")
        def test():
            assert_eq('attrs.string(default="something")', repr(attrs.string(default="something", doc = "foo")))
            assert_eq('attrs.string(default="something")', repr(frozen))
        "#
    ))
}

#[test]
fn boolean_works() -> buck2_error::Result<()> {
    let mut tester = Tester::new().unwrap();
    tester.run_starlark_bzl_test(indoc!(
        r#"
        frozen = attrs.bool(default=False)
        def test():
            assert_eq('attrs.bool(default=True)', repr(attrs.bool(default=True, doc = "foo")))
            assert_eq('attrs.bool(default=False)', repr(frozen))
        "#
    ))
}

#[test]
fn test_attr_module_registered() -> buck2_error::Result<()> {
    let mut tester = Tester::new().unwrap();
    tester.run_starlark_bzl_test(indoc!(
        r#"
        def test():
            assert_eq(True, getattr(attrs, "string") != None)
        "#
    ))
}

#[test]
fn list_works() -> buck2_error::Result<()> {
    let mut tester = Tester::new().unwrap();
    tester.run_starlark_bzl_test(indoc!(
        r#"
        frozen = attrs.list(
            attrs.string(default = "something", doc = "foo"),
            default=["1", "2"],
            doc = "foo",
        )
        def test():
            not_frozen = attrs.list(
                attrs.string(default = "something", doc = "foo"),
                default=[],
                doc = "foo",
            )

            assert_eq('attrs.list(attrs.string(), default=[])', repr(not_frozen))
            assert_eq('attrs.list(attrs.string(), default=["1", "2"])', repr(frozen))
        "#
    ))
}

#[test]
fn enum_works() -> buck2_error::Result<()> {
    let mut tester = Tester::new().unwrap();
    tester.run_starlark_bzl_test(indoc!(
        r#"
        frozen = attrs.enum(["red", "green", "blue"])
        def test():
            not_frozen = attrs.enum(["yes", "no"], default="no")
            assert_eq('attrs.enum(["red","green","blue"])', repr(frozen))
            assert_eq('attrs.enum(["yes","no"], default="no")', repr(not_frozen))
        "#
    ))
}

#[test]
fn attr_coercer_coerces() -> buck2_error::Result<()> {
    Heap::temp(|heap| {
        let some_cells = cells(None)?;
        let cell_resolver = some_cells.1;
        let cell_alias_resolver = some_cells.0;
        let package = PackageLabel::new(
            CellName::testing_new("root"),
            CellRelativePath::unchecked_new("foo"),
        )?;
        let enclosing_package = (package.dupe(), PackageListing::testing_empty());
        let coercer_ctx = BuildAttrCoercionContext::new_with_package(
            cell_resolver,
            cell_alias_resolver,
            enclosing_package,
            CellPathWithAllowedRelativeDir::backwards_relative_not_supported(
                package.as_cell_path().to_owned(),
            ),
            InferTargetNames::No,
        );
        let label_coercer = AttrType::dep(ProviderIdSet::EMPTY, PluginKindSet::EMPTY);
        let string_coercer = AttrType::string();
        let enum_coercer = AttrType::enumeration(vec![
            "red".to_owned(),
            "green".to_owned(),
            "blue".to_owned(),
        ])?;
        assert!(AttrType::enumeration(vec!["UPPER".to_owned()]).is_err());
        assert!(
            AttrType::enumeration(vec![
                "repeated".to_owned(),
                "and".to_owned(),
                "repeated".to_owned()
            ])
            .is_err()
        );

        let label_value1 =
            coercer_ctx.coerce_sealed(&label_coercer, heap.alloc("root//foo:bar").to_value())?;
        let label_value2 = coercer_ctx
            .coerce_sealed(&label_coercer, heap.alloc("root//foo:bar[baz]").to_value())?;
        let label_value3 =
            coercer_ctx.coerce_sealed(&label_coercer, heap.alloc(":bar").to_value())?;
        let label_value4 =
            coercer_ctx.coerce_sealed(&label_coercer, heap.alloc(":bar[baz]").to_value())?;
        let invalid_label_value1 =
            coercer_ctx.coerce_sealed(&label_coercer, heap.alloc("root//foo/...").to_value());
        let invalid_label_value2 =
            coercer_ctx.coerce_sealed(&label_coercer, heap.alloc("root//foo:").to_value());
        let invalid_label_value3 =
            coercer_ctx.coerce_sealed(&label_coercer, heap.alloc("1").to_value());

        assert_eq!(
            "root//foo:bar",
            value_to_string(&label_value1, package.dupe())?
        );
        assert_eq!(
            "root//foo:bar[baz]",
            value_to_string(&label_value2, package.dupe())?
        );
        assert_eq!(
            "root//foo:bar",
            value_to_string(&label_value3, package.dupe())?
        );
        assert_eq!(
            "root//foo:bar[baz]",
            value_to_string(&label_value4, package.dupe())?
        );
        assert!(invalid_label_value1.is_err());
        assert!(invalid_label_value2.is_err());
        assert!(invalid_label_value3.is_err());

        let string_value1 =
            coercer_ctx.coerce_sealed(&string_coercer, heap.alloc("str").to_value())?;
        assert_eq!("str", value_to_string(&string_value1, package.dupe())?);

        let enum_valid1 = coercer_ctx.coerce_sealed(&enum_coercer, heap.alloc("red").to_value())?;
        let enum_valid2 =
            coercer_ctx.coerce_sealed(&enum_coercer, heap.alloc("green").to_value())?;
        let enum_valid3 = coercer_ctx.coerce_sealed(&enum_coercer, heap.alloc("RED").to_value())?;
        let enum_invalid1 =
            coercer_ctx.coerce_sealed(&enum_coercer, heap.alloc("orange").to_value());
        let enum_invalid2 = coercer_ctx.coerce_sealed(&enum_coercer, heap.alloc(false).to_value());
        assert_eq!("red", value_to_string(&enum_valid1, package.dupe())?);
        assert_eq!("green", value_to_string(&enum_valid2, package.dupe())?);
        assert_eq!("red", value_to_string(&enum_valid3, package.dupe())?);
        assert!(enum_invalid1.is_err());
        assert!(enum_invalid2.is_err());

        Ok(())
    })
}

#[test]
fn any_nested_dict_is_interned() -> buck2_error::Result<()> {
    Heap::temp(|heap| {
        let cell_resolver = cells(None)?.1;
        let ctx = BuildAttrCoercionContext::new_no_package(
            cell_resolver.dupe(),
            CellName::testing_new("root"),
            cell_resolver.root_cell_cell_alias_resolver().dupe(),
            InferTargetNames::No,
        );
        let attr = AttrType::any();
        let first = attr.coerce(
            AttrIsConfigurable::Yes,
            &ctx,
            heap.alloc(AllocList([AllocDict([("key", 1i32)])]))
                .to_value(),
        )?;
        let second = attr.coerce(
            AttrIsConfigurable::Yes,
            &ctx,
            heap.alloc(AllocList([AllocDict([("key", 1i32)])]))
                .to_value(),
        )?;
        assert_eq!(first, second);
        assert_eq!(first.word(), second.word());
        Ok(())
    })
}

/// The coercion memo keys every input a coercion's result depends on. The same
/// frozen value coerces differently as a string and as a source path, and is a
/// different entry under another type, configurability or default; repeating a
/// coercion's inputs finds its entry.
#[test]
fn coerce_memo_keys_every_input() -> buck2_error::Result<()> {
    use AttrIsConfigurable::No;
    use AttrIsConfigurable::Yes;

    FrozenHeap::temp(|frozen| {
        let ctx = coercion_ctx_listing(PackageListing::testing_files(&["foo.cpp"]));
        let value = frozen.alloc("foo.cpp").to_value();
        let string = AttrType::string();
        let source = AttrType::source(false);
        let default_a = OwnedCoercedAttr::new_with(|b| b.encode_int(1))?;
        let default_b = OwnedCoercedAttr::new_with(|b| b.encode_int(2))?;
        let key = |attr: &AttrType, default: Option<&OwnedCoercedAttr>, c| {
            CoerceMemoKey::new(value, attr, default.map(|d| d.as_ref()), c)
                .expect("a frozen value under a root default is memoizable")
        };

        let as_string = coerce_attr(&string, Yes, &ctx, value, None)?;
        let as_source = coerce_attr(&source, Yes, &ctx, value, None)?;
        {
            let arena = ctx.attr_arena();
            assert!(
                !arena
                    .view(as_string.word())
                    .content_eq(&arena.view(as_source.word())),
                "a string and a source path coerce differently"
            );
        }

        let memo = CoerceMemoTable::default();
        memo.insert(key(&string, None, Yes), as_string);
        assert_eq!(memo.lookup(key(&string, None, Yes)), Some(as_string));
        for (other, why) in [
            (key(&source, None, Yes), "the coercer"),
            (key(&string, None, No), "configurability"),
            (key(&string, Some(&default_a), Yes), "the default"),
        ] {
            assert_eq!(memo.lookup(other), None, "{why} is part of the key");
        }
        assert_ne!(
            key(&string, Some(&default_a), Yes),
            key(&string, Some(&default_b), Yes),
            "defaults with different content are different keys"
        );
        Ok(())
    })
}

#[test]
fn dep_works() -> buck2_error::Result<()> {
    let mut t = Tester::new().unwrap();
    t.run_starlark_bzl_test(indoc!(
        r#"
        frozen1 = attrs.dep(default="root//foo:bar")
        frozen2 = attrs.dep(default="//foo:bar")
        def test():
            assert_eq('attrs.dep(default="root//foo:bar")', repr(attrs.dep(default="//foo:bar")))
            assert_eq('attrs.dep(default="root//foo:bar")', repr(frozen1))
            assert_eq('attrs.dep(default="root//foo:bar")', repr(frozen2))
        "#
    ))?;

    let mut t = Tester::new().unwrap();
    t.run_starlark_bzl_test_expecting_error(
        indoc!(
            r#"
        def test():
            attrs.dep(default="notatarget")
        "#
        ),
        "Error parsing target pattern",
    );

    // Relative targets are disallowed; there is no build file for them to be relative to
    let mut t = Tester::new().unwrap();
    t.run_starlark_bzl_test_expecting_error(
        indoc!(
            r#"
        def test():
            attrs.dep(default=":reltarget")
        "#
        ),
        "Target pattern must be absolute",
    );
    Ok(())
}

#[test]
fn source_works() -> buck2_error::Result<()> {
    let mut t = Tester::new().unwrap();
    t.run_starlark_bzl_test(indoc!(
        r#"
        frozen1 = attrs.source(default="root//foo:bar")
        frozen2 = attrs.source(default="//foo:bar")
        def test():
            assert_eq('attrs.source(default="root//foo:bar")', repr(attrs.source(default="root//foo:bar")))
            assert_eq('attrs.source(default="root//foo:bar")', repr(frozen1))
            assert_eq('attrs.source(default="root//foo:bar")', repr(frozen2))
        "#
    ))?;

    // Relative targets are disallowed; there is no build file for them to be relative to
    let mut t = Tester::new().unwrap();
    t.run_starlark_bzl_test_expecting_error(
        indoc!(
            r#"
        def test():
            attrs.source(default=":reltarget")
        "#
        ),
        "Target pattern must be absolute",
    );
    Ok(())
}

#[test]
fn coercing_src_to_path_works() -> buck2_error::Result<()> {
    let cell_resolver = cells(None).unwrap().1;
    let cell_alias_resolver = cells(None).unwrap().0;
    let package = PackageLabel::new(
        CellName::testing_new("root"),
        CellRelativePath::unchecked_new("foo/bar"),
    )?;
    let package_ctx = BuildAttrCoercionContext::new_with_package(
        cell_resolver.dupe(),
        cell_alias_resolver.dupe(),
        (
            package.dupe(),
            PackageListing::testing_files(&["baz/quz.cpp"]),
        ),
        CellPathWithAllowedRelativeDir::backwards_relative_not_supported(
            package.as_cell_path().to_owned(),
        ),
        InferTargetNames::No,
    );
    let no_package_ctx = BuildAttrCoercionContext::new_no_package(
        cell_resolver,
        CellName::testing_new("root"),
        cell_alias_resolver,
        InferTargetNames::No,
    );

    let err = no_package_ctx
        .coerce_path("baz/quz.cpp", false)
        .unwrap_err();
    assert!(err.to_string().contains("Expected a package"));

    let err = package_ctx
        .coerce_path("/invalid/absolute/path", false)
        .unwrap_err();
    assert!(format!("{err:#}").contains("absolute path"), "{err:?}");

    let err = package_ctx
        .coerce_path("../upward/traversal", false)
        .unwrap_err();
    assert!(err.to_string().contains("normalized path"));

    let expected = PackageRelativePathBuf::unchecked_new("baz/quz.cpp".to_owned());
    assert_eq!(
        expected.as_path(),
        &**package_ctx
            .coerce_path("baz/quz.cpp", false)
            .unwrap()
            .path()
    );
    Ok(())
}
