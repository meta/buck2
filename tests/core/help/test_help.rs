/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::LazyLock;

use buck2_e2e::prelude::*;
use futures::FutureExt;
use futures::future::LocalBoxFuture;
use regex::NoExpand;
use regex::Regex;
use tokio::sync::Semaphore;

fn normalize(s: &str) -> String {
    static VERSION: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"buck2 [a-z0-9]{16,64} (<build-id>|<exe-hash>)").unwrap());
    static EXE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"buck2\.exe").unwrap());
    let s = VERSION.replace_all(s, NoExpand("buck2 <version> <version-source>"));
    let s = EXE.replace_all(&s, NoExpand("buck2"));
    s.lines().map(str::trim_end).collect::<Vec<_>>().join("\n") + "\n"
}

fn find_subcommands(help: &str) -> Vec<String> {
    static BEFORE_SUBCOMMANDS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s).*SUBCOMMANDS:").unwrap());
    static SUBCOMMAND: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^  ([a-z][a-z0-9_-]*)").unwrap());
    let help = BEFORE_SUBCOMMANDS.replace_all(help, "");
    SUBCOMMAND
        .captures_iter(&help)
        .map(|c| c[1].to_owned())
        .collect()
}

fn check_help<'a>(
    buck: &'a Buck,
    semaphore: &'a Semaphore,
    command_stack: Vec<String>,
) -> LocalBoxFuture<'a, anyhow::Result<usize>> {
    async move {
        let args: Vec<&str> = command_stack.iter().map(String::as_str).collect();
        let result = {
            let _permit = semaphore.acquire().await?;
            buck.help(&args).await?
        };

        let name = ["help"]
            .into_iter()
            .chain(args.iter().copied())
            .collect::<Vec<_>>()
            .join("-");
        golden(
            &normalize(&result.stdout),
            &format!("buck2-{name}.golden.txt"),
        );

        let subtasks = find_subcommands(&result.stdout)
            .into_iter()
            .map(|subcommand| {
                let mut stack = command_stack.clone();
                stack.push(subcommand);
                check_help(buck, semaphore, stack)
            });
        let subresults = futures::future::try_join_all(subtasks).await?;

        Ok(subresults.iter().sum::<usize>() + 1)
    }
    .boxed_local()
}

#[buck_test]
async fn test_help(buck: &Buck) -> anyhow::Result<()> {
    let semaphore = Semaphore::new(10);
    let total = check_help(buck, &semaphore, Vec::new()).await?;
    assert!(total > 4);
    Ok(())
}
