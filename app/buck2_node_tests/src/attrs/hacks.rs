/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::cells::name::CellName;
use buck2_core::cells::paths::CellRelativePath;
use buck2_core::package::PackageLabel;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_node::attrs::arena::OwnedCoercedAttr;
use buck2_node::attrs::fmt_context::AttrFmtContext;
use dupe::Dupe;

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
fn stringifies_correctly() -> buck2_error::Result<()> {
    let coerced = OwnedCoercedAttr::new_with(|b| b.encode_string("Hello, world!"))?;

    let package = PackageLabel::new(
        CellName::testing_new("root"),
        CellRelativePath::new(ForwardRelativePath::new("foo/bar").unwrap()),
    )?;

    assert_eq!(
        "Hello, world!".to_owned(),
        value_to_string(&coerced, package.dupe())?
    );

    let list = OwnedCoercedAttr::new_with(|b| {
        let item = b.encode_string("Hello, world!")?;
        b.encode_list(&[item])
    })?;
    assert!(value_to_string(&list, package.dupe()).is_err());
    Ok(())
}
