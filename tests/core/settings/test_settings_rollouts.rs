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
use serde_json::Value;

async fn report(buck: &Buck) -> anyhow::Result<Value> {
    let result = buck.audit(&["settings", "--json"]).await?;
    Ok(serde_json::from_str(&result.stdout)?)
}

fn array(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}

// The unknown key below is reported as a soft error.
#[buck_test(allow_soft_errors = true)]
async fn test_stanza_selection_and_per_key_tolerance(buck: &Buck) -> anyhow::Result<()> {
    let report_before = report(buck).await?;
    let version = array(&report_before["sections"])
        .find(|s| s["name"] == "analysis")
        .and_then(|s| s["version"].as_i64())
        .expect("a version of the `analysis` section");
    assert!(
        version >= 1,
        "the test needs an older version to write a stanza for"
    );

    // Only the stanza at or below the compiled version applies, and within
    // it a key this binary does not know is dropped on its own.
    let rollouts = buck.settings_home_dir().join(".bucksettings.rollouts");
    std::fs::write(
        &rollouts,
        format!(
            "[analysis.{}]\nrecord_requested_anon_targets = false\nnot_a_setting = true\n[analysis.{}]\nrecord_requested_anon_targets = true\n",
            version - 1,
            version + 1
        ),
    )?;

    let report = report(buck).await?;
    let anon = array(&report["sections"])
        .filter(|section| section["name"] == "analysis")
        .flat_map(|section| array(&section["settings"]))
        .find(|setting| setting["name"] == "record_requested_anon_targets")
        .expect("the `analysis.record_requested_anon_targets` setting");
    assert_eq!(anon["value"], false);
    assert_eq!(anon["origin"]["kind"], "rollout");

    let rollout_layer = array(&report["layers"])
        .find(|layer| layer["origin"]["kind"] == "rollout")
        .expect("a rollout layer");
    assert_eq!(rollout_layer["applied"], true);
    assert_eq!(
        array(&rollout_layer["ignored"])
            .map(|ignored| &ignored["setting"])
            .collect::<Vec<_>>(),
        vec!["analysis.not_a_setting"]
    );
    Ok(())
}
