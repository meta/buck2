/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Benchmark wrapper that shells out to the bench binary with a fresh
//! temporary SQLite DB for each run.
//!
//! Bench-specific args (e.g. `--num-keys`, `--value-size`) are passed
//! through after `--`:
//!
//!   bench_runner -o /tmp/results -- --num-keys 50000 --value-size 20000

use std::process::Command;

use clap::Parser;
use json_value::OrderedValue;

mod table;

#[derive(Parser)]
struct Cli {
    /// Output directory for benchmark results and profiling data.
    /// If not specified, a temporary directory is created.
    #[arg(long, short)]
    output: Option<String>,
    /// Wrap the child process with `perf record`.
    #[arg(long)]
    perf: bool,
    /// Dump jemalloc heap profiles and run `jeprof --dot` to produce DOT graphs.
    /// Convert to SVG with `dot -Tsvg`.
    #[arg(long)]
    jeprof_dot: bool,
    /// Extra args passed through to the bench binary (after --).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    bench_args: Vec<String>,
}

fn pagable_bin() -> String {
    std::env::var("PAGABLE_BIN").expect("PAGABLE_BIN env var must be set")
}

fn perf_command(bin: &str, perf_path: &str) -> Command {
    let mut cmd = Command::new("perf");
    cmd.args([
        "record",
        "-g",
        "--call-graph",
        "dwarf",
        "-o",
        perf_path,
        "--",
        bin,
    ]);
    cmd
}

struct Benchmark {
    bin: String,
    db_path: String,
    output: String,
    perf: bool,
    jeprof_dot: bool,
    bench_args: Vec<String>,
}

impl Benchmark {
    fn new(cli: Cli) -> Self {
        let pid = std::process::id();
        let output = cli.output.unwrap_or_else(|| {
            std::env::temp_dir()
                .join(format!("dice-bench-output-{}", pid))
                .to_string_lossy()
                .into_owned()
        });
        Self {
            bin: pagable_bin(),
            db_path: std::env::temp_dir()
                .join(format!("dice-bench-{}.db", pid))
                .to_string_lossy()
                .into_owned(),
            output,
            perf: cli.perf,
            jeprof_dot: cli.jeprof_dot,
            bench_args: cli.bench_args,
        }
    }

    fn run_bench(&self, json_lines: &mut String) -> anyhow::Result<()> {
        let mut cmd = if self.perf {
            perf_command(&self.bin, &format!("{}/perf.data", self.output))
        } else {
            Command::new(&self.bin)
        };
        cmd.env("BUCK2_DICE_DB_PATH", &self.db_path);
        if self.jeprof_dot {
            cmd.env("MALLOC_CONF", "prof:true");
            cmd.args(["--heap-dump", &self.output]);
        }
        cmd.args(&self.bench_args);

        eprintln!("Running: {:?}", cmd);
        let result = cmd.output()?;
        eprint!("{}", String::from_utf8_lossy(&result.stderr));

        if !result.status.success() {
            return Err(anyhow::anyhow!(
                "bench command failed with {}",
                result.status
            ));
        }

        let stdout = String::from_utf8_lossy(&result.stdout);
        json_lines.push_str(&stdout);

        if self.jeprof_dot {
            run_jeprof_dot(&self.bin, &self.output)?;
        }

        Ok(())
    }
}

fn run_jeprof_dot(bin: &str, prefix: &str) -> anyhow::Result<()> {
    let (dir, file_prefix) = prefix
        .rsplit_once('/')
        .ok_or(anyhow::anyhow!("prefix is not a path"))?;

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(rest) = name.strip_prefix(&format!("{}.", file_prefix)) {
            if let Some(stage) = rest.strip_suffix(".heap") {
                let dump = format!("{}/{}", dir, name);
                let dot_path = format!("{}.{}.dot", prefix, stage);
                eprintln!("[jeprof] {} -> {}", dump, dot_path);
                let output = Command::new("jeprof")
                    .args(["--dot", bin, &dump])
                    .output()?;
                if !output.status.success() {
                    eprintln!(
                        "[jeprof] failed: {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                } else {
                    std::fs::write(&dot_path, &output.stdout)?;
                    eprintln!("[jeprof] wrote {}", dot_path);
                }
            }
        }
    }
    Ok(())
}

fn write_benchmark_md(output_dir: &str, content: &str) -> anyhow::Result<()> {
    let md_path = format!("{}/benchmark.md", output_dir);

    let mut params = OrderedValue::Null;
    let mut totals = OrderedValue::Null;
    let mut stage_lines = Vec::new();
    for line in content.lines() {
        let v: OrderedValue = serde_json::from_str(line)?;
        match v.get("type").and_then(|t| t.as_str()) {
            Some("params") => {
                if params.is_null() {
                    params = v;
                }
            }
            Some("metrics") => {
                totals = v;
            }
            _ => {
                stage_lines.push(line);
            }
        }
    }

    // Merge params and metrics into one summary row.
    if let (Some(p), Some(t)) = (params.as_object_mut(), totals.as_object()) {
        for (k, v) in t {
            p.insert(k.clone(), v.clone());
        }
    }

    let mut md = String::new();
    if !params.is_null() {
        md.push_str(&table::kv_table(&params));
        md.push('\n');
    }

    md.push_str(&table::stages_table(&stage_lines)?);

    std::fs::write(&md_path, &md)?;
    eprint!("{}", md);
    eprintln!("Wrote {}", md_path);
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let bench = Benchmark::new(Cli::parse());

    let _ignored = std::fs::remove_dir_all(&bench.output);
    std::fs::create_dir_all(&bench.output)?;

    let mut json_lines = String::new();
    bench.run_bench(&mut json_lines)?;

    write_benchmark_md(&bench.output, &json_lines)?;
    std::fs::remove_dir_all(&bench.db_path)?;
    Ok(())
}
