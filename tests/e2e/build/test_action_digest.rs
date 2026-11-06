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
use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::Context;
use buck2_e2e::mode::Mode;
use buck2_e2e::mode::get_mode_from_platform;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::read_what_ran;
use serde_json::Value;

// Every action is normally an action-cache hit, so a build takes well under a
// minute. After a digest change the whole graph, including the in-graph Rust
// toolchain bootstrap, has to execute on RE, which takes far longer. The test
// fails on its own clock rather than letting TPX time it out, because on diffs
// a TPX timeout is reported as a warning instead of a failure.
const BUILD_DEADLINE: Duration = Duration::from_secs(20 * 60);

async fn build_and_collect_digests(
    buck: &Buck,
    arm_name: &str,
) -> anyhow::Result<BTreeMap<String, Vec<String>>> {
    buck.kill().await?;
    let build = buck.build(&[
        get_mode_from_platform(Mode::Dev),
        "fbcode//buck2/tests/targets/rules/rust/hello_world:welcome",
        "--remote-only",
    ]);
    match tokio::time::timeout(BUILD_DEADLINE, build).await {
        Ok(result) => {
            result?;
        }
        Err(_) => panic!(
            "Build with {arm_name} did not finish within {}s. \
             Every action should be an action-cache hit; a build this slow almost \
             always means the action digests changed and the graph is being \
             rebuilt. Refer to comment on this test for next steps.",
            BUILD_DEADLINE.as_secs()
        ),
    }
    identities_by_digest(&read_what_ran(buck, &[]).await?)
}

#[track_caller]
fn assert_same_digests(
    left_name: &str,
    left: &BTreeMap<String, Vec<String>>,
    right_name: &str,
    right: &BTreeMap<String, Vec<String>>,
) {
    let left_keys: BTreeSet<&String> = left.keys().collect();
    let right_keys: BTreeSet<&String> = right.keys().collect();
    let only_left: BTreeSet<&String> = left_keys.difference(&right_keys).copied().collect();
    let only_right: BTreeSet<&String> = right_keys.difference(&left_keys).copied().collect();
    assert!(
        only_left.is_empty() && only_right.is_empty(),
        "Action Digest was modified, refer to comment on this test for next steps.\n\
         Only in {left_name} ({}):\n{}Only in {right_name} ({}):\n{}",
        only_left.len(),
        format_digests(left, &only_left),
        only_right.len(),
        format_digests(right, &only_right),
    );
}

/// Map each action digest to the identities of the actions that produced it.
///
/// The graph contains identical actions in several configurations, which
/// share a digest. Whichever finishes first seeds the local dep-file cache,
/// and the others are then served from it without a reproducer digest, so
/// only the set of digests is stable between two builds.
fn identities_by_digest(what_ran: &[Value]) -> anyhow::Result<BTreeMap<String, Vec<String>>> {
    let mut digests: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in what_ran {
        let details = entry
            .get("reproducer")
            .and_then(|r| r.get("details"))
            .with_context(|| format!("No `reproducer.details` in {entry}"))?;
        let Some(digest) = details.get("digest").filter(|d| !d.is_null()) else {
            continue;
        };
        let digest = digest
            .as_str()
            .with_context(|| format!("A digest that is not a string in {entry}"))?;
        let identity = entry
            .get("identity")
            .and_then(Value::as_str)
            .with_context(|| format!("No `identity` in {entry}"))?;
        digests
            .entry(digest.to_owned())
            .or_default()
            .push(identity.to_owned());
    }
    Ok(digests)
}

fn format_digests(digests: &BTreeMap<String, Vec<String>>, selected: &BTreeSet<&String>) -> String {
    selected
        .iter()
        .map(|digest| format!("  {digest}  {}\n", digests[*digest].join("; ")))
        .collect()
}

// If this test fails, it means that a change that modifies action digest was made.
// Background in this post:
// https://fb.workplace.com/groups/buck2eng/permalink/3452581371706005/
// Changes should instead be deployed by:
//   1: Create a new buck2 flag and hide the changes behind it (Ex. D59503359)
//   2: Wait for bvb that contains #1 to land. To be safe, wait for a second to land
//      as well so you're guaranteed that the first bump can no longer be
//      fast-reverted.
//   3: Activate the flag via .buckconfig (Ex. D59648609)
//       3.1: Fix/followup on any CI failures caused by cache invalidation
//   4: Observe for a couple of days to ensure that there are no issues
//   5. Remove the code associated with the config flag but NOT the config itself,
//      this way this test wouldn't need to be changed at all (Ex. D59864942)
//   6: Wait for bvb that contains #5 to land. Optionally wait for a second as above.
//   7: Remove the config flag (Ex. D59988979)
//
// Three buck2 binaries are compared pairwise: the one compiled from this
// checkout, the deployed `stable` channel, and the deployed `previous`
// channel. On a version-bump diff `stable` is the candidate and `previous`
// is the version it replaces, so the stable/previous pair is the gate that
// keeps a digest change out of production even when trunk already contains it.
#[buck_test(inplace = true)]
async fn test_action_digest(buck: &mut Buck) -> anyhow::Result<()> {
    let compiled_digests = build_and_collect_digests(buck, "compiled buck2").await?;

    buck.set_executable("buck2");
    let deployed_digests = build_and_collect_digests(buck, "deployed buck2").await?;

    buck.set_env("BUCK2_CHANNEL", "previous");
    let previous_digests = build_and_collect_digests(buck, "previous buck2").await?;

    assert_same_digests(
        "compiled buck2",
        &compiled_digests,
        "deployed buck2",
        &deployed_digests,
    );
    assert_same_digests(
        "deployed buck2",
        &deployed_digests,
        "previous buck2",
        &previous_digests,
    );
    Ok(())
}
