/*
 * Copyright 2018 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Memory benchmark for starlark-rust.
//!
//! Generates large Starlark programs and measures memory consumption
//! during parse, eval, and freeze phases.

use std::fmt::Write as _;
use std::mem::size_of;
use std::time::Instant;

use clap::Parser;
use starlark::environment::Globals;
use starlark::environment::Module;
use starlark::eval::Evaluator;
use starlark::singleton_heap_name;
use starlark::syntax::AstModule;
use starlark::syntax::Dialect;
use starlark::values::FrozenHeapName;
use starlark_syntax::syntax::ast::AssignP;
use starlark_syntax::syntax::ast::AssignTargetP;
use starlark_syntax::syntax::ast::AstExprP;
use starlark_syntax::syntax::ast::AstLiteral;
use starlark_syntax::syntax::ast::AstNoPayload;
use starlark_syntax::syntax::ast::AstStmtP;
use starlark_syntax::syntax::ast::CallArgsP;
use starlark_syntax::syntax::ast::ClauseP;
use starlark_syntax::syntax::ast::DefP;
use starlark_syntax::syntax::ast::ExprP;
use starlark_syntax::syntax::ast::FStringP;
use starlark_syntax::syntax::ast::ForClauseP;
use starlark_syntax::syntax::ast::ForP;
use starlark_syntax::syntax::ast::LoadP;
use starlark_syntax::syntax::ast::StmtP;
use starlark_syntax::syntax::ast::TypeExprP;

#[derive(Parser, Debug)]
#[command(name = "starlark_bench_mem")]
#[command(about = "Memory benchmark for starlark-rust parser and evaluator")]
struct Args {
    /// Number of function definitions to generate.
    #[arg(long, default_value_t = 50_000)]
    num_functions: usize,

    /// Number of statements per function body.
    #[arg(long, default_value_t = 6)]
    body_size: usize,

    /// Also evaluate the code (not just parse).
    #[arg(long, default_value_t = false)]
    eval: bool,

    /// Also freeze the module after evaluation (requires --eval).
    #[arg(long, default_value_t = false)]
    freeze: bool,

    /// Measure lambda instantiation instead of function definitions:
    /// one lambda site instantiated `num_instances` times.
    #[arg(long, default_value_t = false)]
    lambdas: bool,

    /// Number of lambda instances to create (requires --lambdas).
    #[arg(long, default_value_t = 1_000_000, requires = "lambdas")]
    num_instances: usize,

    /// Number of parameters on the lambda (requires --lambdas).
    #[arg(long, default_value_t = 2, requires = "lambdas")]
    lambda_params: usize,

    /// Give every lambda parameter a default value (requires --lambdas).
    #[arg(long, default_value_t = false, requires = "lambdas")]
    lambda_defaults: bool,
}

/// Read RSS (Resident Set Size) in bytes from /proc/self/status on Linux.
fn rss_bytes() -> usize {
    let status = match std::fs::read_to_string("/proc/self/status") {
        Ok(s) => s,
        Err(_) => return 0,
    };
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            // Value is in kB, e.g. "VmRSS:    12345 kB"
            let trimmed = rest.trim().trim_end_matches(" kB");
            let kb: usize = trimmed.parse().unwrap_or(0);
            return kb * 1024;
        }
    }
    0
}

fn format_mb(bytes: usize) -> String {
    format!("{} MB", bytes / (1024 * 1024))
}

/// Generate Starlark source code with many function definitions.
///
/// Each function resembles a Buck/bzl macro with unique variable names
/// to prevent identifier deduplication from reducing memory.
fn generate_source(num_functions: usize, body_size: usize) -> String {
    // Pre-estimate capacity: ~420 bytes per function with body_size=6
    let estimated_bytes = num_functions * (200 + body_size * 60);
    let mut source = String::with_capacity(estimated_bytes);

    for i in 0..num_functions {
        writeln!(source, "def rule_{i}(name, srcs, deps, visibility):",).unwrap();

        // Core body statements — always included
        writeln!(
            source,
            "    config_{i} = {{\"name\": name, \"srcs\": srcs, \"deps\": deps}}"
        )
        .unwrap();
        writeln!(source, "    processed_{i} = [s + \"_out\" for s in srcs]").unwrap();
        writeln!(source, "    config_{i}[\"processed\"] = processed_{i}").unwrap();

        // Additional body statements up to body_size
        for j in 3..body_size {
            writeln!(
                source,
                "    extra_{i}_{j} = config_{i}.get(\"name\", \"{i}_{j}\")"
            )
            .unwrap();
        }

        writeln!(source, "    if visibility:").unwrap();
        writeln!(source, "        config_{i}[\"visibility\"] = visibility").unwrap();
        writeln!(source, "    return config_{i}").unwrap();
        writeln!(source).unwrap();
    }

    source
}

/// Generate a program with a single lambda site instantiated many times.
///
/// Sharing one site isolates the per-instance cost (`Def`, its
/// `ParametersSpec`, captured slots) from the per-site cost (`DefInfo`),
/// which is paid only once. Each instance captures `i` so instances cannot
/// collapse into a constant.
fn generate_lambda_source(num_instances: usize, params: usize, defaults: bool) -> String {
    let mut source = String::new();
    writeln!(source, "def mk(i):").unwrap();
    if params == 0 {
        writeln!(source, "    return lambda: i").unwrap();
    } else {
        let param_list = (0..params)
            .map(|p| {
                if defaults {
                    format!("p{p} = i")
                } else {
                    format!("p{p}")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(source, "    return lambda {param_list}: i").unwrap();
    }
    writeln!(
        source,
        "instances = [mk(i) for i in range({num_instances})]"
    )
    .unwrap();
    source
}

fn main() {
    // Print AST type sizes for analysis
    println!("=== Type Sizes ===");
    println!("ExprP:        {} bytes", size_of::<ExprP<AstNoPayload>>());
    println!(
        "AstExprP:     {} bytes",
        size_of::<AstExprP<AstNoPayload>>()
    );
    println!("StmtP:        {} bytes", size_of::<StmtP<AstNoPayload>>());
    println!(
        "AstStmtP:     {} bytes",
        size_of::<AstStmtP<AstNoPayload>>()
    );
    println!("AssignP:      {} bytes", size_of::<AssignP<AstNoPayload>>());
    println!("DefP:         {} bytes", size_of::<DefP<AstNoPayload>>());
    println!("ForP:         {} bytes", size_of::<ForP<AstNoPayload>>());
    println!("LoadP:        {} bytes", size_of::<LoadP<AstNoPayload>>());
    println!("AstLiteral:   {} bytes", size_of::<AstLiteral>());
    println!(
        "FStringP:     {} bytes",
        size_of::<FStringP<AstNoPayload>>()
    );
    println!(
        "TypeExprP:    {} bytes",
        size_of::<TypeExprP<AstNoPayload>>()
    );
    println!("String:       {} bytes", size_of::<String>());
    println!("Box<str>:     {} bytes", size_of::<Box<str>>());
    println!(
        "AstString:    {} bytes",
        size_of::<starlark_syntax::syntax::ast::AstString>()
    );
    println!(
        "AstIdent:     {} bytes",
        size_of::<starlark_syntax::syntax::ast::AstIdent>()
    );
    println!(
        "CallArgsP:    {} bytes",
        size_of::<CallArgsP<AstNoPayload>>()
    );
    println!(
        "AssignTarget: {} bytes",
        size_of::<AssignTargetP<AstNoPayload>>()
    );
    println!(
        "ForClauseP:   {} bytes",
        size_of::<ForClauseP<AstNoPayload>>()
    );
    println!("ClauseP:      {} bytes", size_of::<ClauseP<AstNoPayload>>());
    println!(
        "LambdaP:      {} bytes",
        size_of::<starlark_syntax::syntax::ast::LambdaP<AstNoPayload>>()
    );
    println!("Vec<u8>:      {} bytes", size_of::<Vec<u8>>());
    println!("Box<[u8]>:    {} bytes", size_of::<Box<[u8]>>());
    println!();

    let args = Args::parse();

    if args.freeze && !args.eval {
        eprintln!("Error: --freeze requires --eval");
        std::process::exit(1);
    }

    if args.lambdas && args.num_instances == 0 {
        eprintln!("Error: --num-instances must be at least 1");
        std::process::exit(1);
    }

    println!("=== Starlark Memory Benchmark ===");
    if args.lambdas {
        println!(
            "Configuration: {} lambda instances, lambda_params={}, lambda_defaults={}",
            args.num_instances, args.lambda_params, args.lambda_defaults
        );
    } else {
        println!(
            "Configuration: {} functions, body_size={}",
            args.num_functions, args.body_size
        );
    }

    // Generate source code
    let gen_start = Instant::now();
    let source = if args.lambdas {
        generate_lambda_source(args.num_instances, args.lambda_params, args.lambda_defaults)
    } else {
        generate_source(args.num_functions, args.body_size)
    };
    let gen_time = gen_start.elapsed();
    println!(
        "Source code: {} ({} bytes), generated in {:.1}s",
        format_mb(source.len()),
        source.len(),
        gen_time.as_secs_f64()
    );

    // Parse phase
    println!("\n--- Parse Phase ---");
    let rss_before_parse = rss_bytes();
    println!("RSS before parse: {}", format_mb(rss_before_parse));

    let parse_start = Instant::now();
    let ast = AstModule::parse("bench.star", source, &Dialect::Extended)
        .expect("generated code should parse without errors");
    let parse_time = parse_start.elapsed();

    // Return freed memory (intern table, intermediate allocations) to the OS
    // so RSS reflects only live data.
    #[cfg(target_os = "linux")]
    unsafe {
        libc::malloc_trim(0);
    }

    let rss_after_parse = rss_bytes();
    let rss_delta_parse = rss_after_parse.saturating_sub(rss_before_parse);
    println!("RSS after parse:  {}", format_mb(rss_after_parse));
    println!("RSS delta:        {}", format_mb(rss_delta_parse));
    println!("Parse time:       {:.1}s", parse_time.as_secs_f64());

    // Eval phase (optional)
    if args.eval {
        println!("\n--- Eval Phase ---");

        Module::with_temp_heap(|module| {
            let globals = Globals::standard();

            let eval_start = Instant::now();
            {
                let mut evaluator = Evaluator::new(&module);
                evaluator
                    .eval_module(ast, &globals)
                    .expect("generated code should evaluate without errors");
            }
            let eval_time = eval_start.elapsed();

            let heap_allocated = module.heap().allocated_bytes();
            let heap_peak = module.heap().peak_allocated_bytes();
            let rss_after_eval = rss_bytes();

            println!("Starlark heap:    {}", format_mb(heap_allocated));
            println!("Peak heap:        {}", format_mb(heap_peak));
            println!("RSS after eval:   {}", format_mb(rss_after_eval));
            println!("Eval time:        {:.1}s", eval_time.as_secs_f64());
            if args.lambdas {
                println!(
                    "Heap per instance: {} bytes",
                    heap_allocated / args.num_instances
                );
            }

            // Freeze phase (optional)
            if args.freeze {
                println!("\n--- Freeze Phase ---");
                let freeze_start = Instant::now();
                let frozen = module
                    .freeze_named(FrozenHeapName::Singleton(singleton_heap_name!()))
                    .expect("freeze should succeed");
                let freeze_time = freeze_start.elapsed();

                let frozen_heap_bytes = frozen.frozen_heap().allocated_bytes();
                let rss_after_freeze = rss_bytes();

                println!("Frozen heap:      {}", format_mb(frozen_heap_bytes));
                println!("RSS after freeze: {}", format_mb(rss_after_freeze));
                println!("Freeze time:      {:.1}s", freeze_time.as_secs_f64());
                if args.lambdas {
                    println!(
                        "Frozen heap per instance: {} bytes",
                        frozen_heap_bytes / args.num_instances
                    );
                }

                // Keep frozen alive until after measurement
                drop(frozen);
            }
        });
    }

    println!("\nDone.");
}
