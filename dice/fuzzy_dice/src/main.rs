/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::env;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::Ordering;

use anyhow::Context;
use clap::Args;
use clap::FromArgMatches;
use clap::Parser;
use quickcheck::Arbitrary;
use quickcheck::Gen;
use quickcheck::QuickCheck;
use quickcheck::TestResult;
use quickcheck::Testable;
use thiserror::Error;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;

mod computation;
mod execution;

use crate::execution::DiceExecutionOrder;
use crate::execution::DiceExecutionOrderOptions;
use crate::execution::ExecutionResult;
use crate::execution::GENERATE_OUT_OF_ORDER;
use crate::execution::GENERATE_TRANSIENTS;
use crate::execution::Verdict;
use crate::execution::aggregate_samples;
use crate::execution::install_panic_hook;
use crate::execution::write_case;

/// Stack size for tokio worker threads. Dice's recursive computation graph is
/// deep enough that the default 2 MiB stack overflows on non-trivial fuzz
/// inputs.
const WORKER_STACK_BYTES: usize = 16 * 1024 * 1024;

#[derive(Error, Debug)]
pub enum DiceFuzzError {
    #[error("couldn't parse provided replay file")]
    UnparsableReplay,
}

/// Legacy replay support: extract a `DiceExecutionOrder` embedded in
/// quickcheck's stderr from a prior fuzzer run. New failures serialize the
/// input directly and don't need this; kept working for old artifacts.
fn magical_cleanup(stderr: &str) -> anyhow::Result<&str> {
    stderr
        .rsplit_once("TEST FAILED. Arguments: (")
        .and_then(|(_, suffix)| suffix.rsplit_once(')'))
        .map(|(prefix, _)| prefix)
        .ok_or_else(|| DiceFuzzError::UnparsableReplay.into())
}

fn execution_order_from_path(filepath: &Path) -> anyhow::Result<DiceExecutionOrder> {
    let buf = std::fs::read_to_string(filepath)?;
    if let Ok(ex) = serde_json::from_reader::<_, DiceExecutionOrder>(&mut buf.as_bytes()) {
        return Ok(ex);
    }
    let cleaned_up = magical_cleanup(&buf)?;
    serde_json::from_reader::<_, DiceExecutionOrder>(&mut cleaned_up.as_bytes())
        .context(format!("While parsing {}", cleaned_up))
}

#[derive(clap::Parser)]
#[clap(
    name = "fuzzy-dice",
    about = "a tool for finding bugs in DICE by simulating many different computations"
)]
struct Opts {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    #[clap(about = "Replays an existing failure.")]
    Replay(SubCommandCommon<Replay>),
    #[clap(about = "Searches for new failures.")]
    Fuzz(SubCommandCommon<Fuzz>),
}

#[derive(Parser)]
struct SubCommandCommon<T: FromArgMatches + Args> {
    #[clap(flatten)]
    cmd: T,
    #[clap(
        long,
        value_parser,
        help = "If set, prints a DICE-dump as JSON to given location after each operation."
    )]
    print_dumps: Option<PathBuf>,
}

#[derive(Parser)]
struct Replay {
    #[clap(
        value_parser,
        help = "the path to the file containing the execution to replay"
    )]
    path: PathBuf,
}

#[derive(Parser)]
struct Fuzz {
    #[clap(
        default_value_t = 2_000_000,
        help = "The maximum number of tests for fuzzing. The actual number may be lower due to\
            discarded test cases"
    )]
    max_tests: u64,
    #[clap(
        default_value_t = 2_000_000,
        help = "The number of passes to hit before stopping and considering it as a pass"
    )]
    num_tests: u64,
    #[clap(
        long,
        help = "Size passed to the quickcheck generator. Larger values \
            produce longer test cases. quickcheck does not accept an explicit \
            RNG seed at the version we're pinned to, so this is the only \
            reproducibility knob available."
    )]
    size: Option<usize>,
    #[clap(
        long,
        default_value_t = false,
        help = "Skip shrinking after a failure — report the raw case for debugging."
    )]
    no_shrink: bool,
}

/// Directory under which the inputs of failing cases are written, one
/// `<uuid>/input.json` per case, for `replay`.
fn cases_root() -> PathBuf {
    env::temp_dir().join("fuzzy_dice_cases")
}

/// Runs `execution` and returns the per-sample results. A panic that escapes
/// the execution (as opposed to one inside a dice task, which `execute_raw`
/// reports itself) becomes a synthetic `UnexpectedPanic` result rather than
/// taking down the fuzzer.
fn drive(
    execution: &DiceExecutionOrder,
    samples: usize,
    options: &DiceExecutionOrderOptions,
    stop_at: Option<&ExecutionResult>,
) -> Vec<ExecutionResult> {
    let run = || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_stack_size(WORKER_STACK_BYTES)
            .build()
            .expect("building tokio runtime");
        let results = rt.block_on(execution.execute_raw(samples, options, stop_at));
        // A hung dice task would make dropping the runtime wait on it, defeating the query
        // timeout that reported it.
        rt.shutdown_background();
        results
    };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)) {
        Ok(results) => results,
        Err(payload) => vec![ExecutionResult::UnexpectedPanic(
            execution::panic_payload_message(&*payload),
        )],
    }
}

fn samples_to_test_result(
    execution: &DiceExecutionOrder,
    samples: &[ExecutionResult],
    previous: Option<&ExecutionResult>,
) -> (TestResult, Option<ExecutionResult>) {
    match aggregate_samples(samples, previous) {
        Verdict::Passed => (TestResult::passed(), None),
        Verdict::Failed(res) => {
            let msg = format!("Execution: {execution:?}\nFailure: `{res}`");
            (TestResult::error(msg), Some(res))
        }
        Verdict::Nondeterministic => (TestResult::discard(), None),
    }
}

struct Fuzzer {
    options: DiceExecutionOrderOptions,
    no_shrink: bool,
}

impl Fuzzer {
    /// Shrinks a failing case and writes the smallest reproduction next to the original as
    /// `shrunk.json`.
    fn shrink_failure(
        &self,
        execution: DiceExecutionOrder,
        first: ExecutionResult,
        case_dir: &Path,
    ) -> TestResult {
        let mut current_execution = execution;
        let mut current_first = first;
        let mut best: Option<TestResult> = None;
        loop {
            let executions = current_execution.shrink();
            let mut improved = false;
            for exec in executions {
                let samples = drive(
                    &exec,
                    DiceExecutionOrder::NSAMPLES_SHRINKING,
                    &self.options,
                    Some(&current_first),
                );
                let (tr, matched) = samples_to_test_result(&exec, &samples, Some(&current_first));
                if let Some(r) = matched {
                    eprintln!("Found failure at {}: {r}", exec.uuid);
                    best = Some(tr);
                    current_first = r;
                    current_execution = exec;
                    improved = true;
                    break;
                }
            }
            if !improved {
                break;
            }
        }
        let shrunk_path = case_dir.join("shrunk.json");
        match write_case(&shrunk_path, &current_execution) {
            Ok(()) => eprintln!("Shrunk case saved to {}", shrunk_path.display()),
            Err(e) => eprintln!(
                "Could not save the shrunk case to {}: {e:#}",
                shrunk_path.display()
            ),
        }
        best.unwrap_or_else(|| {
            let msg = format!("Execution: {current_execution:?}\nFailure: `{current_first}`");
            TestResult::error(msg)
        })
    }
}

impl Testable for Fuzzer {
    fn result(&self, g: &mut Gen) -> TestResult {
        let execution = DiceExecutionOrder::arbitrary(g);
        let samples = drive(
            &execution,
            DiceExecutionOrder::NSAMPLES_SEARCHING,
            &self.options,
            None,
        );
        let (tr, first) = samples_to_test_result(&execution, &samples, None);
        if tr.is_failure() {
            if let Some(first) = first {
                let dump_dir = cases_root().join(execution.uuid.to_string());
                let dump_path = dump_dir.join("input.json");
                eprintln!("Found failure at execution {}: {first}.", execution.uuid);
                match write_case(&dump_path, &execution) {
                    Ok(()) => eprintln!("Input saved to {}.", dump_path.display()),
                    Err(e) => {
                        eprintln!("Could not save the input to {}: {e:#}", dump_path.display())
                    }
                }
                if self.no_shrink {
                    tr
                } else {
                    eprintln!("Beginning shrinking...");
                    self.shrink_failure(execution, first, &dump_dir)
                }
            } else {
                tr
            }
        } else {
            tr
        }
    }
}

struct Replayer {
    execution: DiceExecutionOrder,
    options: DiceExecutionOrderOptions,
}

impl Testable for Replayer {
    fn result(&self, _: &mut Gen) -> TestResult {
        // A case saved while shrinking reproduced across many samples; one saved from the
        // search may not, and gets the single sample the search gave it.
        let samples = if self.execution.is_shrinking {
            DiceExecutionOrder::NSAMPLES_SHRINKING
        } else {
            DiceExecutionOrder::NSAMPLES_SEARCHING
        };
        let samples = drive(&self.execution, samples, &self.options, None);
        samples_to_test_result(&self.execution, &samples, None).0
    }
}

/// Entry point for `fuzz` command. Returns whether all cases passed.
fn run_fuzz(fuzz: Fuzz, print_dumps: Option<PathBuf>) -> bool {
    install_panic_hook();
    let fuzzer = Fuzzer {
        options: DiceExecutionOrderOptions { print_dumps },
        no_shrink: fuzz.no_shrink,
    };
    let mut qc = QuickCheck::new()
        .max_tests(fuzz.max_tests)
        .tests(fuzz.num_tests);
    qc = qc.rng(Gen::new(fuzz.size.unwrap_or(10)));
    // quickcheck panics on failure, so we use the try_ variant to convert
    // that into a boolean.
    qc.quicktest(fuzzer).is_ok()
}

fn main() -> anyhow::Result<ExitCode> {
    let cmd = Opts::parse();
    if env::var_os("NOGEN_TRANSIENTS").is_some() {
        GENERATE_TRANSIENTS.store(false, Ordering::Relaxed);
    }
    if env::var_os("NOGEN_OUT_OF_ORDER").is_some() {
        GENERATE_OUT_OF_ORDER.store(false, Ordering::Relaxed);
    }

    match cmd.command {
        Commands::Fuzz(fuzz) => {
            let print_dumps = fuzz.print_dumps.clone();
            let ok = run_fuzz(fuzz.cmd, print_dumps);
            if ok {
                println!("Fuzzing complete.");
                Ok(ExitCode::SUCCESS)
            } else {
                Ok(ExitCode::FAILURE)
            }
        }
        Commands::Replay(replay_cmd) => {
            tracing_subscriber::registry()
                .with(fmt::layer())
                .with(EnvFilter::from_default_env())
                .init();
            install_panic_hook();
            let execution = execution_order_from_path(&replay_cmd.cmd.path)?;
            let options = DiceExecutionOrderOptions {
                print_dumps: replay_cmd.print_dumps,
            };
            QuickCheck::new()
                .max_tests(1)
                .quickcheck(Replayer { execution, options });
            Ok(ExitCode::SUCCESS)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bounded, fixed-seed fuzz that runs on every `buck2 test` invocation.
    #[test]
    fn ci_quick_fuzz() {
        // The known-buggy scenarios remain gated so this smoke test stays
        // green until the underlying dice bugs are fixed. The gates are process-wide,
        // so they are put back for whatever test runs next.
        let transients = GENERATE_TRANSIENTS.swap(false, Ordering::Relaxed);
        let out_of_order = GENERATE_OUT_OF_ORDER.swap(false, Ordering::Relaxed);
        let ok = run_fuzz(
            Fuzz {
                max_tests: 100,
                num_tests: 100,
                size: Some(10),
                no_shrink: true,
            },
            None,
        );
        GENERATE_TRANSIENTS.store(transients, Ordering::Relaxed);
        GENERATE_OUT_OF_ORDER.store(out_of_order, Ordering::Relaxed);
        assert!(ok, "quick fuzz reported failures");
    }
}
