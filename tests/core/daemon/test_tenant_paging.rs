/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use buck2_e2e::prelude::*;
use buck2_e2e::process::wait_for_exit;
use rusqlite::OptionalExtension;
use serde_json::Value;

async fn tenant_value(buck: &Buck, config: &Path) -> anyhow::Result<String> {
    let result = buck
        .build(&[
            "//:tenant_value",
            "--config-file",
            config.to_str().unwrap(),
            "--build-report=-",
        ])
        .await?;
    Ok(std::fs::read_to_string(
        result
            .build_report()?
            .output_for_target("//:tenant_value")?,
    )?)
}

async fn page_out(buck: &Buck, require_new_values: bool) -> anyhow::Result<u64> {
    let out = buck.debug(&["hydration", "page-out"]).await?.stdout;
    let summary: Value = serde_json::from_str(&out)
        .map_err(|e| anyhow::anyhow!("page-out did not print a JSON summary:\n{out}\n{e}"))?;
    assert!(summary["error"].is_null(), "page-out failed:\n{out}");
    let data_keys_out = summary.get("data_keys_out").map_or(Some(0), Value::as_u64);
    if require_new_values {
        assert!(
            data_keys_out.unwrap() > 0,
            "expected positive paged values:\n{out}"
        );
    }
    assert!(
        summary["daemon_data_keys_out"].as_u64().unwrap() > 0,
        "{out}"
    );
    Ok(summary["daemon_data_keys_in"].as_u64().unwrap())
}

fn materializer_identity(
    buck: &Buck,
    isolation: &str,
) -> anyhow::Result<Option<rusqlite::types::Value>> {
    let db = buck
        .cwd()
        .join("buck-out")
        .join(isolation)
        .join("cache/materializer_state/db.sqlite");
    if !db.is_file() {
        return Ok(None);
    }
    let connection = rusqlite::Connection::open(db)?;
    Ok(connection
        .query_row(
            "SELECT value FROM created_by WHERE key = 'timestamp_on_initialization'",
            [],
            |row| row.get(0),
        )
        .optional()?
        .filter(|value| *value != rusqlite::types::Value::Null))
}

fn tenant_dice_store(buck: &Buck, isolation: &str) -> PathBuf {
    buck.cwd()
        .join("buck-out")
        .join(isolation)
        .join("cache/dice_state")
}

fn daemon_shared_dice_dirs(buck: &Buck) -> anyhow::Result<Vec<PathBuf>> {
    let parent = buck
        .cwd()
        .join("buck-out")
        .join("._buck2")
        .join("dice_state");
    if !parent.is_dir() {
        return Ok(Vec::new());
    }
    let mut dirs = Vec::new();
    for entry in std::fs::read_dir(parent)? {
        let path = entry?.path();
        if path.is_dir() {
            dirs.push(path);
        }
    }
    dirs.sort();
    Ok(dirs)
}

fn is_non_empty_dir(path: &Path) -> anyhow::Result<bool> {
    Ok(std::fs::read_dir(path)?.next().is_some())
}

async fn status_pid(buck: &Buck) -> anyhow::Result<u32> {
    let status: Value = serde_json::from_str(&buck.status(&[]).await?.stdout)?;
    Ok(u32::try_from(
        status["process_info"]["pid"].as_u64().unwrap(),
    )?)
}

async fn assert_daemon_exits(pid: u32) {
    assert!(
        wait_for_exit(pid, Duration::from_secs(10)).await,
        "Daemon survived final tenant eviction"
    );
}

// With a shared engine, page-in critical path entries can report time spans
// that overlap their neighbours (`critical_path_entry_overlap`). This test
// checks paging storage lifetime, not critical path accounting.
async fn tenant_paging_storage_lifetime(
    buck: &Buck,
    tmp_path: &Path,
    share_dice: bool,
) -> anyhow::Result<()> {
    std::fs::OpenOptions::new()
        .append(true)
        .open(buck.cwd().join(".buckconfig"))?
        .write_all(format!("\n[buck2]\nshare_dice = {share_dice}\n").as_bytes())?;
    std::fs::write(
        buck.settings_home_dir().join(".bucksettings.local.toml"),
        "[hydration]\n\
         enable_paging = true\n\
         page_out_on_idle = false\n\
         page_out_policy = \"once_per_key\"\n\
         eviction_policy = \"on_idle\"\n",
    )?;
    let initial_config = tmp_path.join("initial.bcfg");
    std::fs::write(&initial_config, "[tenant]\nvalue = initial\n")?;
    let sibling_config = tmp_path.join("sibling.bcfg");
    std::fs::write(&sibling_config, "[tenant]\nvalue = sibling\n")?;
    let other_config = tmp_path.join("other.bcfg");
    std::fs::write(&other_config, "[tenant]\nvalue = other\n")?;
    let tenant = buck;
    let tenant_isolation = "v2";
    let mut sibling = buck.clone();
    sibling.set_isolation_prefix("survivor");
    let mut other = buck.clone();
    other.set_env("BUCK2_SHARE_DAEMON", "false");
    other.set_isolation_prefix("other");
    let result = async {
        assert_eq!(tenant_value(tenant, &initial_config).await?, "initial");
        assert_eq!(tenant_value(&sibling, &sibling_config).await?, "sibling");
        let pid = status_pid(&sibling).await?;
        assert_eq!(status_pid(tenant).await?, pid);

        let initial_keys_in = page_out(tenant, true).await?;
        let identity = materializer_identity(tenant, tenant_isolation)?;
        assert!(identity.is_some());

        let paging_path = if share_dice {
            let shared_dirs = daemon_shared_dice_dirs(buck)?;
            assert_eq!(shared_dirs.len(), 1);
            let paging_path = shared_dirs[0].clone();
            assert!(is_non_empty_dir(&paging_path)?);
            paging_path
        } else {
            assert_eq!(daemon_shared_dice_dirs(buck)?, Vec::<PathBuf>::new());
            let paging_path = tenant_dice_store(tenant, tenant_isolation);
            assert!(is_non_empty_dir(&paging_path)?);
            paging_path
        };

        other
            .build(&[
                "//:tenant_value",
                "--config-file",
                other_config.to_str().unwrap(),
            ])
            .await?;
        page_out(&other, true).await?;
        let other_pid = status_pid(&other).await?;
        assert_ne!(other_pid, pid);
        let other_dirs: Vec<PathBuf> = if share_dice {
            daemon_shared_dice_dirs(buck)?
                .into_iter()
                .filter(|p| *p != paging_path)
                .collect()
        } else {
            Vec::new()
        };
        if share_dice {
            assert_eq!(other_dirs.len(), 1);
        }

        tenant.kill().await?;
        assert_eq!(status_pid(&sibling).await?, pid);
        assert_eq!(tenant_value(tenant, &initial_config).await?, "initial");
        assert_eq!(materializer_identity(tenant, tenant_isolation)?, identity);
        if share_dice {
            let recreated_keys_in = page_out(tenant, false).await?;
            assert!(recreated_keys_in > initial_keys_in);
            assert!(daemon_shared_dice_dirs(buck)?.contains(&paging_path));
            assert!(is_non_empty_dir(&paging_path)?);
        }

        tenant.kill().await?;
        tenant.clean(&[]).await?;
        if share_dice {
            assert!(daemon_shared_dice_dirs(buck)?.contains(&paging_path));
            assert!(is_non_empty_dir(&paging_path)?);
        } else {
            assert!(!tenant_dice_store(tenant, tenant_isolation).exists());
        }
        assert_eq!(tenant_value(tenant, &initial_config).await?, "initial");
        let new_identity = materializer_identity(tenant, tenant_isolation)?;
        assert!(new_identity.is_some());
        assert_ne!(new_identity, identity);
        let clean_keys_in = page_out(tenant, !share_dice).await?;
        assert_eq!(tenant_value(tenant, &initial_config).await?, "initial");
        let replayed_keys_in = page_out(tenant, false).await?;
        if !share_dice {
            assert!(replayed_keys_in > clean_keys_in);
        }
        assert_eq!(tenant_value(&sibling, &sibling_config).await?, "sibling");
        page_out(&sibling, !share_dice).await?;
        assert_eq!(status_pid(tenant).await?, pid);

        tenant.kill().await?;
        sibling.kill().await?;
        assert_daemon_exits(pid).await;
        if share_dice {
            assert_eq!(tenant_value(tenant, &initial_config).await?, "initial");
            assert!(!daemon_shared_dice_dirs(buck)?.contains(&paging_path));
            for p in &other_dirs {
                assert!(p.is_dir());
            }
        }
        other.kill().await?;
        assert_daemon_exits(other_pid).await;
        if share_dice {
            tenant.kill().await?;
            assert_eq!(tenant_value(tenant, &initial_config).await?, "initial");
            for p in &other_dirs {
                assert!(!p.exists());
            }
        }
        anyhow::Ok(())
    }
    .await;
    let killed = async {
        tenant.kill().await?;
        sibling.kill().await?;
        other.kill().await?;
        anyhow::Ok(())
    }
    .await;
    result?;
    killed
}

#[buck_test(
    data_dir = "test_daemon_data",
    skip_for_os = ["windows"],
    allow_soft_errors = true,
    env = [("BUCK2_SHARE_DAEMON", "true")],
)]
#[expect(
    non_snake_case,
    reason = "named like the Python harness's parametrized case"
)]
async fn test_tenant_paging_storage_lifetime_False(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    tenant_paging_storage_lifetime(buck, tmp_path, false).await
}

#[buck_test(
    data_dir = "test_daemon_data",
    skip_for_os = ["windows"],
    allow_soft_errors = true,
    env = [("BUCK2_SHARE_DAEMON", "true")],
)]
#[expect(
    non_snake_case,
    reason = "named like the Python harness's parametrized case"
)]
async fn test_tenant_paging_storage_lifetime_True(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    tenant_paging_storage_lifetime(buck, tmp_path, true).await
}
