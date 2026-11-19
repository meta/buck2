/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The Manifold CLI, for tests that read back what buck2 uploaded.
//!
//! The test's `BUCK` names the CLI in `MANIFOLD_CLI` as
//! `$(exe_target fbcode//manifold/clients/cli:manifold)`: the par on Linux and macOS, and
//! `python.exe <par>` on Windows, where a par is not an executable.

use std::path::Path;
use std::process::Output;

use anyhow::Context;

use crate::buck::Buck;

/// The certificates the macOS test hosts need named, as an attempted fix for uploads failing there:
/// <https://fb.workplace.com/groups/fbpython/permalink/5214295275278464/>.
const SSL_CERT_FILE: &str = "/etc/ssl/cert.pem";

/// Points buck2 at the certificates, where the host has them, for a test that uploads.
pub fn use_ssl_cert_file(buck: &mut Buck) {
    if Path::new(SSL_CERT_FILE).exists() {
        buck.set_env("SSL_CERT_FILE", SSL_CERT_FILE);
    }
}

/// A Manifold bucket, read with its API key.
pub struct Bucket {
    name: String,
    apikey: String,
}

impl Bucket {
    pub fn new(name: &str, apikey: &str) -> Self {
        Self {
            name: name.to_owned(),
            apikey: apikey.to_owned(),
        }
    }

    /// Runs `manifold --apikey <apikey> <command> <bucket>/<path>`, echoing its output to stderr.
    pub async fn run(&self, command: &str, path: &str) -> anyhow::Result<Output> {
        let cli = std::env::var("MANIFOLD_CLI")
            .context("`MANIFOLD_CLI` is not set; the test's `BUCK` sets it")?;
        let mut cli = cli.split_whitespace();
        let program = cli
            .next()
            .context("`MANIFOLD_CLI` is empty; the test's `BUCK` sets it")?;
        let mut process = tokio::process::Command::new(std::path::absolute(program)?);
        for arg in cli {
            process.arg(std::path::absolute(arg)?);
        }
        process.args([
            "--apikey",
            &self.apikey,
            command,
            &format!("{}/{path}", self.name),
        ]);
        if Path::new(SSL_CERT_FILE).exists() {
            process.env("SSL_CERT_FILE", SSL_CERT_FILE);
        }
        let output = process.output().await?;
        eprintln!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(output)
    }

    /// Whether `path` exists in the bucket. A failure of the CLI is an error, not an answer.
    pub async fn exists(&self, path: &str) -> anyhow::Result<bool> {
        let bucket_path = format!("{}/{path}", self.name);
        let output = self.run("exists", path).await?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        if output.status.success() && stdout.contains(&format!("{bucket_path} EXISTS")) {
            Ok(true)
        } else if stdout.contains(&format!("{bucket_path} DOES NOT EXIST")) {
            Ok(false)
        } else {
            anyhow::bail!("`manifold exists {bucket_path}` failed: {}", output.status)
        }
    }

    /// The size of `path` in the bucket, as the CLI prints it.
    pub async fn file_size(&self, path: &str) -> anyhow::Result<String> {
        let output = self.run("getFileSize", path).await?;
        anyhow::ensure!(
            output.status.success(),
            "`manifold getFileSize {}/{path}` failed: {}",
            self.name,
            output.status
        );
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }
}
