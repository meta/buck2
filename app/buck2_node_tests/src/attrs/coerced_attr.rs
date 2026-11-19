/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;

use buck2_core::configuration::config_setting::ConfigSettingData;
use buck2_core::configuration::constraints::ConstraintKey;
use buck2_core::configuration::constraints::ConstraintValue;
use buck2_node::attrs::arena::CoercedAttrArena;
use buck2_node::attrs::arena::OwnedCoercedAttr;
use buck2_node::attrs::attr_type::AttrType;
use buck2_node::attrs::display::AttrDisplayWithContextExt;
use buck2_node::attrs::fmt_context::AttrFmtContext;
use buck2_node::attrs::testing::configuration_ctx_with_matched_keys;
use buck2_node::configuration::resolved::ConfigurationSettingKey;
use dupe::Dupe;

fn key(s: &str) -> ConfigurationSettingKey {
    ConfigurationSettingKey::testing_parse(s)
}

fn to_json(value: &OwnedCoercedAttr) -> String {
    value
        .as_ref()
        .to_json(&AttrFmtContext::NO_CONTEXT)
        .unwrap()
        .to_string()
}

#[test]
fn selector_identity_accounts_for_ordering() -> buck2_error::Result<()> {
    let mut b = CoercedAttrArena::builder();
    let t = b.encode_bool(true)?;
    let f = b.encode_bool(false)?;
    let s1 = b.encode_selector(
        &[
            (key("cell1//pkg1:target1"), t),
            (key("cell2//pkg2:target2"), f),
        ],
        Some(f),
    )?;
    let s2 = b.encode_selector(
        &[
            (key("cell1//pkg1:target1"), t),
            (key("cell2//pkg2:target2"), f),
        ],
        Some(f),
    )?;
    let reordered = b.encode_selector(
        &[
            (key("cell2//pkg2:target2"), f),
            (key("cell1//pkg1:target1"), t),
        ],
        Some(f),
    )?;
    let nested_s1 = b.encode_list(&[s1])?;
    let nested_s2 = b.encode_list(&[s2])?;
    let arena = b.finish();

    assert_eq!(s1, s2);
    assert_eq!(nested_s1, nested_s2);
    assert_ne!(s1.word(), reordered.word());
    assert!(
        !arena
            .view(s1.word())
            .content_eq(&arena.view(reordered.word()))
    );
    Ok(())
}

#[test]
fn test_to_json_concat() -> buck2_error::Result<()> {
    let concat = OwnedCoercedAttr::new_with(|b| {
        let items = ["a", "b", "c", "d"]
            .iter()
            .map(|s| b.encode_string(s))
            .collect::<buck2_error::Result<Vec<_>>>()?;
        b.encode_concat(&items)
    })?;
    assert_eq!(
        r#"{"__type":"concat","items":["a","b","c","d"]}"#,
        to_json(&concat)
    );
    Ok(())
}

#[test]
fn test_to_json_selector() -> buck2_error::Result<()> {
    let selector = OwnedCoercedAttr::new_with(|b| {
        let t = b.encode_bool(true)?;
        let ten = b.encode_int(10)?;
        let ddd = b.encode_string("ddd")?;
        b.encode_selector(
            &[(key("config//:a"), t), (key("config//:b"), ten)],
            Some(ddd),
        )
    })?;
    assert_eq!(
        r#"{"__type":"selector","entries":{"DEFAULT":"ddd","config//:a":true,"config//:b":10}}"#,
        to_json(&selector)
    );
    Ok(())
}

/// Resolve a string `select()` whose every entry matches the configuration:
/// the entries carry the setting data that decides specificity. Returns the
/// chosen value, or the resolution error rendered.
fn resolve(entries: Vec<(&str, ConfigSettingData, &str)>) -> Result<String, String> {
    let selector = OwnedCoercedAttr::new_with(|b| {
        let mut encoded = Vec::with_capacity(entries.len());
        for (k, _, v) in &entries {
            encoded.push((key(k), b.encode_string(v)?));
        }
        b.encode_selector(&encoded, None)
    })
    .unwrap();
    let ctx =
        configuration_ctx_with_matched_keys(entries.into_iter().map(|(k, d, _)| (key(k), Some(d))));
    selector
        .as_ref()
        .configure(&AttrType::string(), &ctx, None)
        .require_compatible()
        .map(|v| v.as_display_no_ctx().to_string())
        .map_err(|e| format!("{e:#}"))
}

const AMBIGUOUS: &str = "Both select keys `config//:linux-arm64` and `config//:linux-x86_64` \
    match the configuration, but neither is more specific and they have different values";

/// Resolution picks the most specific matching entry, independent of order,
/// and rejects two unrelated matches unless they carry the same value.
fn check_most_specific(
    linux: impl Fn() -> ConfigSettingData,
    linux_arm64: impl Fn() -> ConfigSettingData,
    linux_x86_64: impl Fn() -> ConfigSettingData,
) {
    // More specific wins whether it comes second...
    assert_eq!(
        Ok("\"x86\"".to_owned()),
        resolve(vec![
            ("config//:linux", linux(), "any"),
            ("config//:linux-x86_64", linux_x86_64(), "x86"),
        ])
    );
    // ...or first.
    assert_eq!(
        Ok("\"x86\"".to_owned()),
        resolve(vec![
            ("config//:linux-x86_64", linux_x86_64(), "x86"),
            ("config//:linux", linux(), "any"),
        ])
    );
    // Unrelated matches with different values are an error.
    let err = resolve(vec![
        ("config//:linux-arm64", linux_arm64(), "arm"),
        ("config//:linux-x86_64", linux_x86_64(), "x86"),
    ])
    .unwrap_err();
    assert!(err.contains(AMBIGUOUS), "{err}");
    // Unrelated matches with the same value are fine.
    assert_eq!(
        Ok("\"same\"".to_owned()),
        resolve(vec![
            ("config//:linux-arm64", linux_arm64(), "same"),
            ("config//:linux-x86_64", linux_x86_64(), "same"),
        ])
    );
}

#[test]
fn select_the_most_specific() {
    let c_os = ConstraintKey::testing_new("config//c:os");
    let c_linux = ConstraintValue::testing_new("config//c:linux", None);
    let c_cpu = ConstraintKey::testing_new("config//c:cpu");
    let c_arm64 = ConstraintValue::testing_new("config//c:arm64", None);
    let c_x86_64 = ConstraintValue::testing_new("config//c:x86_64", None);

    check_most_specific(
        || ConfigSettingData::testing_new(BTreeMap::from_iter([(c_os.dupe(), c_linux.dupe())])),
        || {
            ConfigSettingData::testing_new(BTreeMap::from_iter([
                (c_os.dupe(), c_linux.dupe()),
                (c_cpu.dupe(), c_arm64.dupe()),
            ]))
        },
        || {
            ConfigSettingData::testing_new(BTreeMap::from_iter([
                (c_os.dupe(), c_linux.dupe()),
                (c_cpu.dupe(), c_x86_64.dupe()),
            ]))
        },
    );
}

/// The same, with constraint values spelled as subtargets.
#[test]
fn select_the_most_specific_with_subtargets() {
    let c_os = ConstraintKey::testing_new("config//c:os");
    let c_linux = ConstraintValue::testing_new("config//c:os", Some("linux"));
    let c_cpu = ConstraintKey::testing_new("config//c:cpu");
    let c_arm64 = ConstraintValue::testing_new("config//c:cpu", Some("arm64"));
    let c_x86_64 = ConstraintValue::testing_new("config//c:cpu", Some("x86_64"));

    check_most_specific(
        || ConfigSettingData::testing_new(BTreeMap::from_iter([(c_os.dupe(), c_linux.dupe())])),
        || {
            ConfigSettingData::testing_new(BTreeMap::from_iter([
                (c_os.dupe(), c_linux.dupe()),
                (c_cpu.dupe(), c_arm64.dupe()),
            ]))
        },
        || {
            ConfigSettingData::testing_new(BTreeMap::from_iter([
                (c_os.dupe(), c_linux.dupe()),
                (c_cpu.dupe(), c_x86_64.dupe()),
            ]))
        },
    );
}

#[test] // T177093673
fn test_select_refines_bug() {
    let c_windows = (
        ConstraintKey::testing_new("config//c:os"),
        ConstraintValue::testing_new("config//c:windows", None),
    );
    let c_x86_64 = (
        ConstraintKey::testing_new("config//c:cpu"),
        ConstraintValue::testing_new("config//c:x86_64", None),
    );
    assert_eq!(
        Ok("\"windows-x86_64\"".to_owned()),
        resolve(vec![
            (
                "config//:windows",
                ConfigSettingData::testing_new(BTreeMap::from_iter([c_windows.dupe()])),
                "windows",
            ),
            (
                "config//:x86_64",
                ConfigSettingData::testing_new(BTreeMap::from_iter([c_x86_64.dupe()])),
                "x86_64",
            ),
            (
                "config//:windows-x86_64",
                ConfigSettingData::testing_new(BTreeMap::from_iter([c_windows, c_x86_64])),
                "windows-x86_64",
            ),
        ])
    );
}
