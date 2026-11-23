/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::time::Duration;

use buck2_e2e::prelude::*;
use serde_json::Value;

/// The file name and script of a helper that saves what it reads to a file named after its keys,
/// renamed into place once its input closes.
#[cfg(unix)]
const HELPER: (&str, &str) = (
    "helper.sh",
    r#"#!/bin/sh
cat > "forwarded.$BUCK2_EVENT_FORWARDING_KEY.pb.zst.part"
mv "forwarded.$BUCK2_EVENT_FORWARDING_KEY.pb.zst.part" "forwarded.$BUCK2_EVENT_FORWARDING_KEY.pb.zst"
"#,
);
#[cfg(windows)]
const HELPER: (&str, &str) = (
    "helper.bat",
    concat!(
        r#"@"%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive"#,
        r#" -Command "$name = 'forwarded.' + $env:BUCK2_EVENT_FORWARDING_KEY + '.pb.zst';"#,
        r#" $file = [IO.File]::Create($name + '.part');"#,
        r#" [Console]::OpenStandardInput().CopyTo($file); $file.Close();"#,
        r#" Move-Item -Force -LiteralPath ($name + '.part') -Destination $name""#,
        "\r\n",
    ),
);

/// Waits for the helper of the command with `keys` to save what it read, and returns the records
/// of that event log as `buck2 log show` prints them.
async fn forwarded(buck: &Buck, keys: &str) -> anyhow::Result<Vec<Value>> {
    let path = buck.cwd().join(format!("forwarded.{keys}.pb.zst"));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    while !path.exists() {
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "the helper saved nothing for `{keys}`"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let shown = buck
        .log(&["show", path.to_str().expect("the path is UTF-8")])
        .await?;
    shown
        .stdout
        .lines()
        .map(|line| Ok(serde_json::from_str(line)?))
        .collect()
}

#[buck_test]
async fn test_a_keyed_command_reaches_the_helper(buck: &mut Buck) -> anyhow::Result<()> {
    let (helper, script) = HELPER;
    std::fs::write(
        buck.cwd().join(".bucksettings.toml"),
        format!("[event_forwarding]\nenabled = true\nhelper = \"{helper}\"\n"),
    )?;
    let helper = buck.cwd().join(helper);
    std::fs::write(&helper, script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755))?;
    }
    // Only the flags below give the commands keys.
    buck.remove_env("BUCK2_EVENT_FORWARDING_KEY");

    buck.audit(&["cell"]).await?;
    buck.audit(&[
        "cell",
        "--event-forwarding-key=first",
        "--event-forwarding-key=second",
    ])
    .await?;

    let records = forwarded(buck, "first,second").await?;
    let (header, rest) = records.split_first().expect("the log has a header");
    let args: Vec<_> = header["command_line_args"]
        .as_array()
        .expect("the header holds the command line")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        args.contains(&"audit") && args.contains(&"cell"),
        "{header}"
    );
    let (result, events) = rest.split_last().expect("the log has a result");
    assert!(result.get("Result").is_some(), "{result}");
    assert!(
        !events.is_empty() && events.iter().all(|event| event.get("Event").is_some()),
        "{events:?}"
    );

    // The helper of the command without keys would have saved its log before the one above.
    let saved: Vec<String> = std::fs::read_dir(buck.cwd())?
        .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .filter(|name| name.starts_with("forwarded."))
        .collect();
    assert_eq!(saved, ["forwarded.first,second.pb.zst"]);
    Ok(())
}
