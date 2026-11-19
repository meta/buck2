/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! End-to-end acceptance over the built binary: the exit-code contract
//! (docs/contract.md) and the shape of the headless surfaces.

use std::process::Command;
use std::process::Output;

use retrace::testdata as fixture;

/// The binary under test; the buck test target passes it in
/// `RETRACE_BIN`.
fn bin() -> String {
    std::env::var("RETRACE_BIN")
        .ok()
        .or_else(|| option_env!("CARGO_BIN_EXE_retrace").map(str::to_owned))
        .expect("Buck supplies RETRACE_BIN; Cargo supplies CARGO_BIN_EXE_retrace")
}

#[test]
fn cli_syntax_is_validated_before_conversion() {
    for args in [vec![""], vec!["ingest", ""]] {
        let output = run(&args);
        assert_eq!(output.status.code(), Some(2));
        assert!(stderr(&output).contains("--last"), "{}", stderr(&output));
    }
    for args in [
        vec!["missing.pb.zst", "extra"],
        vec!["missing.pb.zst", "-run=2"],
        vec!["missing.pb.zst", "--last"],
        vec!["missing.pb.zst", "--threads=1", "--threads=2"],
        vec!["missing.pb.zst", "--threads=no"],
        vec!["missing.pb.zst", "--json=typo"],
        vec!["missing.pb.zst", "--json=cp-rank", "--simulate="],
        vec!["missing.pb.zst", "--json=cp-rank", "--simulate-config=typo"],
        vec!["missing.pb.zst", "--json=cp-rank", "--scale=0.5"],
        vec!["missing.pb.zst", "--repair-cycles"],
        vec!["ingest", "missing.pb.zst", "extra"],
    ] {
        let output = run(&args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}: {}",
            stderr(&output)
        );
        assert!(
            !stderr(&output).contains("No such file"),
            "validation performed I/O: {args:?}"
        );
    }
    assert!(run(&["--help"]).status.success());
    assert!(run(&["-h"]).status.success());
    assert!(run(&["ingest", "--help"]).status.success());
}

#[test]
fn output_modes_refuse_conflicts_but_bench_can_extend_the_summary() {
    for (left, right, diagnostic) in [
        ("--json", "--stats", "choose one output mode"),
        ("--json=overview", "--summary", "choose one output mode"),
        ("--stats", "--summary", "choose one output mode"),
        ("--json", "--bench", "--bench adds timing"),
        ("--stats", "--bench", "--bench adds timing"),
    ] {
        for flags in [[left, right], [right, left]] {
            let output = run(&["missing.pb.zst", flags[0], flags[1]]);
            assert_eq!(
                output.status.code(),
                Some(2),
                "{flags:?}: {}",
                stderr(&output)
            );
            assert!(
                stderr(&output).contains(diagnostic),
                "{flags:?}: {}",
                stderr(&output)
            );
            assert!(output.stdout.is_empty(), "{flags:?}: {}", stdout(&output));
            assert!(
                !stderr(&output).contains("No such file"),
                "must refuse before input I/O"
            );
        }
    }
    let trace = fixture("smoke-trace.json");
    for flags in [vec!["--bench"], vec!["--summary", "--bench"]] {
        let mut args = vec![trace.as_str()];
        args.extend(flags);
        let output = run(&args);
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        assert!(stdout(&output).contains("actions ("), "{}", stdout(&output));
        assert!(
            stdout(&output).contains("fit-all-lanes"),
            "{}",
            stdout(&output)
        );
    }
}

fn run(args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .output()
        .expect("binary runs")
}

fn run_env(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut c = Command::new(bin());
    c.args(args);
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().expect("binary runs")
}
fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// Run selection applies to the recorded command model, including a
/// measured zero, without reconstructing it from the selected actions.
#[test]
fn command_profile_json_exports_only_the_selected_runs_recorded_model() {
    let path = std::env::temp_dir().join(format!(
        "retrace-command-profile-cli-{}.json",
        std::process::id()
    ));
    let mut body = String::from("[\n");
    for (run, wall) in [("first", 20), ("second", 0)] {
        for row in [
            serde_json::json!({"ph":"M","cat":"run","name":"run_start","ts":0,"args":{"run":run,"v":1}}),
            serde_json::json!({"ph":"X","cat":"recipe","name":run,"ts":0,"dur":10,"args":{"run":run}}),
            serde_json::json!({"ph":"M","cat":"command-profile","name":"command_profile","ts":0,
                "args":{"run":run,"profile":{"schema_version":1,"origin_us":0,"run":run,
                    "command_kind":"install","client_wall_us":wall,
                    "lifecycle":[{"kind":"command","start_offset_us":0,"duration_us":wall}]}}}),
            serde_json::json!({"ph":"M","cat":"run","name":"run_end","ts":20,"args":{"run":run,"v":1,"outcome":"success","exit":0}}),
        ] {
            if run == "second" && row["cat"] == "recipe" {
                continue; // Metadata-only run selection must not need an action.
            }
            body.push_str(&serde_json::to_string(&row).unwrap());
            body.push_str(",\n");
        }
    }
    body.push_str("]\n");
    std::fs::write(&path, body).unwrap();
    let output = run(&[
        path.to_str().unwrap(),
        "--run=second",
        "--json=command-profile",
        "--compact",
    ]);
    std::fs::remove_file(&path).unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let value: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    let profile = &value["command_profile"];
    assert_eq!(profile["status"], "available", "{profile}");
    assert_eq!(profile["data"]["run"], "second");
    assert_eq!(profile["data"]["command_kind"], "install");
    assert_eq!(profile["data"]["client_wall_us"], 0);
    assert_eq!(profile["data"]["lifecycle"][0]["duration_us"], 0);
    assert!(profile.get("bound").is_none());
}

#[test]
fn help_prints_to_stdout_and_succeeds() {
    let long = run(&["--help"]);
    assert_eq!(long.status.code(), Some(0));
    let text = String::from_utf8_lossy(&long.stdout).into_owned();
    for heading in [
        "Usage:",
        "Commands:",
        "Arguments:",
        "Output Options:",
        "Examples:",
    ] {
        assert!(text.contains(heading), "--help has {heading}");
    }
    assert!(text.contains("--cascade-steps=<N>") && text.contains("cp-cascade"));
    assert!(
        !text.contains('\x1b'),
        "no styling when stdout is not a terminal"
    );

    let short = run(&["-h"]);
    assert_eq!(short.status.code(), Some(0));
    let summary = String::from_utf8_lossy(&short.stdout).into_owned();
    assert!(summary.contains("see more with '--help'"));
    assert!(
        summary.lines().count() < text.lines().count(),
        "-h is the shorter form"
    );
}

#[test]
fn usage_and_unknown_topic_refuse_with_2() {
    let o = run(&[]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        stderr(&o).contains("Usage: retrace"),
        "a usage error shows the summary"
    );

    let f = fixture("smoke-trace.json");
    let o = run(&[&f, "--json=nope"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("unknown --json topic"));
    assert!(stderr(&o).contains("critical-path"), "lists the known set");
    assert!(
        stderr(&o).contains("command-profile"),
        "lists the known set"
    );

    for retired in ["--json=serial-chain", "--json=levers"] {
        let o = run(&[&f, retired]);
        assert_eq!(o.status.code(), Some(2));
        assert!(stderr(&o).contains("unknown --json topic"));
    }
    assert!(stderr(&o).contains("cp-rank"), "lists the known set");

    // a mistyped flag must refuse, not silently fall through to the
    // default output as if it had worked
    let o = run(&[&f, "--stets"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        stderr(&o).contains("unknown option --stets"),
        "{}",
        stderr(&o)
    );

    let o = run(&[&f, "--json=cp-rank", "--simulate=x", "--scale=nan"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("finite factor"), "{}", stderr(&o));
    let o = run(&[&f, "--json=cp-rank", "--simulate=x", "--scale=-1"]);
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn command_only_logs_open_and_export_without_synthetic_actions() {
    use retrace::ingest::testlog::CommandOnlyResult as R;
    use retrace::ingest::testlog::command_only_log;
    for (result, phases, outcome, kind) in [
        (R::Success, true, "success", "aquery"),
        (R::Error, false, "failed", "uquery"),
        (R::TestFailure, false, "failed", "test"),
        (R::Missing, false, "unknown", "aquery"),
        (R::Unrecognized, false, "unknown", "aquery"),
        (R::Truncated, true, "unknown", "aquery"),
    ] {
        let dir = std::env::temp_dir().join(format!(
            "retrace-command-only-cli-{}-{result:?}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let log = command_only_log(&dir, result, phases);
        let out_arg = format!("--out={}", dir.display());
        let output = run(&[log.to_str().unwrap(), &out_arg, "--json=all", "--compact"]);
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        let value: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
        assert_eq!(value["trace"]["actions"], 0);
        assert_eq!(value["trace"]["outcome"], outcome);
        assert!(value["trace"]["graph"].is_null());
        assert_eq!(value["command_profile"]["data"]["command_kind"], kind);
        assert!(
            value["command_profile"]["data"]["snapshots"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        for topic in [
            "critical_path",
            "cp_cascade",
            "whatif",
            "longest",
            "top_memory",
        ] {
            assert_eq!(value[topic]["status"], "refused", "{value}");
            assert!(
                value[topic]["limitations"][0]
                    .as_str()
                    .unwrap()
                    .contains("Not applicable")
            );
        }
        if result == R::Error {
            assert!(
                value["command_profile"]["data"]["result_error"]
                    .as_str()
                    .unwrap()
                    .contains("`%s` placeholder")
            );
        }
        if phases {
            assert_eq!(
                value["command_profile"]["data"]["phases"][0]["completed"],
                1
            );
        }
        let trace = dir.join(format!("{}-trace.json", retrace::ingest::testlog::RUN_ID));
        let selected = run(&[
            trace.to_str().unwrap(),
            "--run=1",
            "--json=command-profile",
            "--compact",
        ]);
        assert_eq!(selected.status.code(), Some(0), "{}", stderr(&selected));
        assert_eq!(
            run(&[
                trace.to_str().unwrap(),
                "--run=wrong",
                "--json=command-profile"
            ])
            .status
            .code(),
            Some(2)
        );
        let cached = run(&[log.to_str().unwrap(), &out_arg, "--json=all", "--compact"]);
        assert_eq!(cached.status.code(), Some(0), "{}", stderr(&cached));
        let mut cached_value: serde_json::Value = serde_json::from_str(&stdout(&cached)).unwrap();
        let mut fresh_value = value.clone();
        for v in [&mut cached_value, &mut fresh_value] {
            v["health"]["data"]
                .as_object_mut()
                .unwrap()
                .remove("parse_ms");
            v["health"]["data"]
                .as_object_mut()
                .unwrap()
                .remove("index_ms");
        }
        assert_eq!(cached_value, fresh_value);
        if matches!(result, R::Success | R::Error | R::TestFailure) {
            assert!(stderr(&cached).contains("reusing cached conversion"));
        }
        let stats = run(&[trace.to_str().unwrap(), "--stats"]);
        assert_eq!(stats.status.code(), Some(0));
        assert!(stdout(&stats).contains("not applicable: no completed actions"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[test]
fn cp_cascade_json_recomputes_the_chain_and_honors_the_step_limit() {
    let f = fixture("smoke-trace.json");
    let first = run(&[&f, "--json=cp-cascade", "--cascade-steps=1", "--compact"]);
    assert_eq!(first.status.code(), Some(0), "{}", stderr(&first));
    let one: serde_json::Value = serde_json::from_str(&stdout(&first)).unwrap();
    let d = &one["cp_cascade"]["data"];
    assert_eq!(d["base_cp_us"], 1300);
    assert_eq!(d["steps_completed"], 1);
    assert_eq!(d["final_cp_us"], 800);
    assert_eq!(d["rows"][0]["target"], "z (cxx_link)");
    assert_eq!(d["rows"][0]["marginal_saved_us"], 500);
    assert!(one["cp_cascade"].get("bound").is_none());
    let full = run(&[
        &f,
        "--json=cp-cascade",
        "--cascade-steps=20",
        "--topk=999",
        "--compact",
    ]);
    assert_eq!(full.status.code(), Some(0), "{}", stderr(&full));
    let all: serde_json::Value = serde_json::from_str(&stdout(&full)).unwrap();
    let d = &all["cp_cascade"]["data"];
    assert_eq!(d["step_limit"], 20);
    assert_eq!(d["steps_completed"], 3);
    assert_eq!(d["final_cp_us"], 0);
    assert_eq!(d["total_saved_us"], 1300);
    assert_eq!(d["rows"][0], one["cp_cascade"]["data"]["rows"][0]);
    let excessive = run(&[&f, "--json=cp-cascade", "--cascade-steps=21"]);
    assert_eq!(excessive.status.code(), Some(2));
    assert!(stderr(&excessive).contains("--cascade-steps wants an integer from 1 to 20"));
}

#[test]
fn cp_cascade_json_refuses_simulation_flags_instead_of_ignoring_them() {
    let f = fixture("smoke-trace.json");
    for option in [
        "--simulate=z",
        "--scale=0.5",
        "--simulate-config=free-admission",
    ] {
        let o = run(&[&f, "--json=cp-cascade", option, "--compact"]);
        // Mode errors are rejected before any source is read or converted.
        assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
        assert!(stdout(&o).is_empty());
    }
}

#[test]
fn missing_file_and_cap_refuse_with_2() {
    let o = run(&["/nonexistent/trace.json", "--stats"]);
    assert_eq!(o.status.code(), Some(2));

    let f = fixture("smoke-trace.json");
    let o = run(&[&f, "--stats", "--max-input-bytes=10"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        stderr(&o).contains("--max-input-bytes"),
        "refusal names the flag"
    );
}

#[test]
fn stats_renders_the_overview_with_cp() {
    let f = fixture("smoke-trace.json");
    let o = run(&[&f, "--stats"]);
    assert_eq!(o.status.code(), Some(0));
    let s = stdout(&o);
    // a failed BUILD is data, not a tool failure (exit stays 0)
    assert!(s.contains("exit 1 — FAILED"), "{s}");
    // the sibling graph was auto-detected and stitched
    assert!(s.contains("1.3ms over 2 steps"), "{s}");
    assert!(s.contains("Action model CP"), "{s}");
    assert!(!s.contains("Serial chain"), "{s}");
    assert!(!s.contains("Parallelism bound"), "{s}");
    assert!(s.contains("work by type:"), "{s}");
    assert!(
        s.contains("RE queue: 1 of 3 actions have measurements"),
        "{s}"
    );
    let output = run(&[&f, "--json=overview", "--compact"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    for row in value["overview"]["data"].as_array().unwrap() {
        assert!(s.contains(row["key"].as_str().unwrap()), "{row}: {s}");
        assert!(s.contains(row["value"].as_str().unwrap()), "{row}: {s}");
    }
}

#[test]
fn json_all_topics_carry_the_envelope() {
    let f = fixture("smoke-trace.json");
    let o = run(&[&f, "--json", "--compact"]);
    assert_eq!(o.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).expect("valid JSON");
    let obj = v.as_object().unwrap();
    // every topic except the trace header carries the envelope
    for (k, t) in obj {
        if k == "trace" {
            continue;
        }
        let status = t.get("status").and_then(|s| s.as_str());
        assert!(
            matches!(status, Some("available" | "partial" | "refused")),
            "{k} lacks an envelope status: {t}"
        );
        assert!(t.get("limitations").is_some(), "{k} lacks limitations");
    }
    // spot-check honesty: queue coverage is 1 of 3 -> partial, and a
    // refused topic is an explicit envelope, not an omission
    assert_eq!(v["re_queue"]["status"], "partial");
    assert_eq!(v["top_memory"]["status"], "partial");
    assert_eq!(v["critical_path"]["status"], "available");
    assert_eq!(v["critical_path"]["data"]["steps"], 2);
    assert_eq!(v["command_profile"]["status"], "refused");
    assert!(v["command_profile"]["data"].is_null());
    assert!(v.get("levers").is_none());
    assert!(v.get("serial_chain").is_none());
}

/// Two actions, a graph that defines only one: the critical path over
/// that graph is a lower bound and says so, but a saving replayed on it
/// is not a bound in either direction, so the savings topics carry none.
#[test]
fn savings_topics_never_carry_a_bound_on_a_partial_graph() {
    let f = fixture("partial-trace.json");
    let o = run(&[
        &f,
        "--json=critical-path,whatif,cp-rank,cp-cascade,findings",
        "--compact",
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).expect("valid JSON");
    assert_eq!(v["critical_path"]["status"], "partial");
    assert_eq!(v["critical_path"]["bound"], "lower");
    for topic in ["whatif", "cp_rank", "cp_cascade", "findings"] {
        assert!(
            v[topic].get("bound").is_none(),
            "{topic} must not label a partial-graph saving a bound: {}",
            v[topic]
        );
    }
}
#[test]
fn corrupt_graph_explicit_refuses_and_discovered_degrades() {
    // a trace whose sibling graph has undefined refs and no torn lines
    let dir = std::env::temp_dir().join(format!("retrace-accept-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let t = dir.join("x-trace.json");
    let g = dir.join("x-graph.json");
    std::fs::write(
        &t,
        "[\n{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"a\",\"ts\":0,\"dur\":5,\"args\":{}},\n",
    )
    .unwrap();
    std::fs::write(
        &g,
        "{\"graph\":{\"enc\":2}}\n{\"t\":\"a\",\"d\":[\"tset:dead\"]}\n",
    )
    .unwrap();
    // explicitly named: the user's input, loud refusal
    let o = run(&[
        t.to_str().unwrap(),
        "--stats",
        &format!("--graph={}", g.to_str().unwrap()),
    ]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        stderr(&o).contains("refusing to emit a silently-holed graph"),
        "{}",
        stderr(&o)
    );
    // auto-discovered: graph-independent analyses keep working, with a
    // warning naming what was skipped
    let o = run(&[t.to_str().unwrap(), "--stats"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(
        stderr(&o).contains("ignoring auto-discovered graph sidecar"),
        "{}",
        stderr(&o)
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_selection_contract_at_the_cli() {
    // two appended runs: no selector refuses listing candidates; either
    // an id or a 1-based ordinal selects
    let dir = std::env::temp_dir().join(format!("retrace-accept-runs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let t = dir.join("two-trace.json");
    std::fs::write(
        &t,
        concat!(
            "[\n",
            "{\"ph\":\"M\",\"cat\":\"run\",\"name\":\"run_start\",\"ts\":0,\"args\":{\"run\":\"r1\",\"tool\":\"buck2\",\"host\":\"buck2\"}},\n",
            "{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"a\",\"ts\":1,\"dur\":5,\"args\":{\"run\":\"r1\"}},\n",
            "{\"ph\":\"M\",\"cat\":\"run\",\"name\":\"run_end\",\"ts\":10,\"args\":{\"run\":\"r1\",\"exit\":0}},\n",
            "{\"ph\":\"M\",\"cat\":\"run\",\"name\":\"run_start\",\"ts\":100,\"args\":{\"run\":\"r2\",\"tool\":\"buck2\",\"host\":\"buck2\"}},\n",
            "{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"b\",\"ts\":101,\"dur\":7,\"args\":{\"run\":\"r2\"}},\n",
            "{\"ph\":\"M\",\"cat\":\"run\",\"name\":\"run_end\",\"ts\":110,\"args\":{\"run\":\"r2\",\"exit\":0}},\n",
        ),
    )
    .unwrap();
    let o = run(&[t.to_str().unwrap(), "--stats"]);
    assert_eq!(o.status.code(), Some(2), "ambiguity refuses");
    assert!(
        stderr(&o).contains("r2"),
        "candidates listed: {}",
        stderr(&o)
    );

    let o = run(&[t.to_str().unwrap(), "--stats", "--run=r2"]);
    assert_eq!(o.status.code(), Some(0));
    let o2 = run(&[t.to_str().unwrap(), "--stats", "--run=2"]);
    assert_eq!(o2.status.code(), Some(0));
    assert_eq!(
        stdout(&o),
        stdout(&o2),
        "id and ordinal select the same run"
    );
    // A selected run must not inherit the file-wide action count or
    // the first run's origin. All exported action timestamps share the
    // absolute client clock, including graph-based topics.
    std::fs::write(
        dir.join("two-graph.json"),
        concat!(
            "{\"graph\":{\"kind\":\"full\",\"enc\":2,\"run\":\"r2\",\"coverage\":{\"nodes\":1,\"edges\":0,\"dangling_deps\":0,\"dangling_tsets\":0}}}\n",
            "{\"t\":\"b\",\"d\":[]}\n",
        ),
    )
    .unwrap();
    let selected = run(&[
        t.to_str().unwrap(),
        "--run=r2",
        "--json=longest,critical-path,cp-cascade,admission,top-memory",
        "--compact",
    ]);
    assert_eq!(selected.status.code(), Some(0), "{}", stderr(&selected));
    let value: serde_json::Value = serde_json::from_slice(&selected.stdout).unwrap();
    assert_eq!(value["trace"]["actions"], 1);
    assert_eq!(value["trace"]["span_us"], 7);
    assert_eq!(value["trace"]["t0_abs_us"], 100);
    for row in [
        &value["longest"]["data"][0],
        &value["critical_path"]["data"]["path"][0],
        &value["cp_cascade"]["data"]["rows"][0],
    ] {
        assert_eq!(row["target"], "b", "{value}");
        assert_eq!(row["start_us"], 101, "{value}");
    }
    let args = value["longest"]["coverage"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["scope"] == "actions-with-args")
        .unwrap();
    assert_eq!(args["numerator"], 1);
    assert_eq!(args["denominator"], 1);
    assert_eq!(value["admission"]["status"], "refused");
    assert_eq!(value["top_memory"]["status"], "refused");
    std::fs::remove_dir_all(&dir).ok();
}

/// A scratch dir with a synthetic event log in it, cleaned by name so
/// reruns start fresh.
fn ingest_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "retrace-accept-ingest-{}-{name}",
        std::process::id()
    ));
    std::fs::remove_dir_all(&d).ok();
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn cached_result_stages_and_nested_timings_reach_cli_without_extending_the_path() {
    use serde_json::json;

    let dir = std::env::temp_dir().join(format!(
        "retrace-accept-cached-result-stages-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let trace = dir.join("cache-trace.json");
    let rows = [
        json!({"ph":"M", "cat":"run", "name":"run_start", "ts":0,
            "args":{"run":"cached", "tool":"buck2"}}),
        json!({"ph":"X", "cat":"recipe", "name":"cached action", "ts":0, "dur":100,
        "args":{"run":"cached", "cache":{"hit":true},
            "stages":{"prepare_us":5, "cache_query_us":30, "cache_hit_us":60, "daemon_us":5},
            "activity":{"version":1, "spans":[
                ["Prepare",0,5,false], ["CacheQuery",5,30,false], ["CacheHit",35,60,false]
            ]},
            "re":{
                "cache_query":{"permit_wait_us":20, "sdk_us":10},
                "cache_hit":{"tree_admission_us":0, "tree_fetch_us":10,
                    "local_bookkeeping_us":25, "declare_us":25, "tree_count":1}
            }
        }}),
        json!({"ph":"M", "cat":"run", "name":"run_end", "ts":100,
            "args":{"run":"cached", "exit":0}}),
    ];
    let body = rows
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .join(",\n");
    std::fs::write(&trace, format!("[\n{body}\n]\n")).unwrap();
    std::fs::write(
        dir.join("cache-graph.json"),
        concat!(
            "{\"graph\":{\"kind\":\"full\",\"enc\":2,\"run\":\"cached\",\"coverage\":{\"nodes\":1,\"edges\":0,\"dangling_deps\":0,\"dangling_tsets\":0}}}\n",
            "{\"t\":\"cached action\",\"d\":[]}\n",
        ),
    )
    .unwrap();
    let output = run(&[
        trace.to_str().unwrap(),
        "--json=critical-path,action-detail,stall",
        "--action=cached action",
        "--compact",
    ]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["critical_path"]["data"]["cp_us"], 100);
    assert_eq!(value["stall"]["data"]["staged_us"], 95);
    let detail = &value["action_detail"]["data"];
    assert_eq!(detail["activity"]["covered_us"], 95);
    assert_eq!(detail["activity"]["uncovered_us"], 5);
    let lines = detail["stage_lines"].as_array().unwrap();
    for key in ["prepare_us", "cache_query_us", "cache_hit_us"] {
        assert!(lines.iter().any(|line| line["stage"] == key), "{detail}");
    }
    for key in ["cache_query_us", "cache_hit_us"] {
        assert!(
            lines.iter().any(|line| line["parent_stage"] == key),
            "missing nested measurements: {detail}"
        );
    }
    assert!(
        lines.iter().any(|line| line["text"]
            .as_str()
            .is_some_and(|t| t.contains("output metadata construction and ownership claim"))),
        "the cache-hit bookkeeping phase is read: {detail}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn ingest_verb_exports_the_pair_and_the_pair_feeds_the_pipeline() {
    let d = ingest_dir("verb");
    let log = retrace::ingest::testlog::demo_log(&d, Some(1), true, true);
    let out_flag = format!("--out={}", d.display());

    let o = run(&["ingest", &log.display().to_string(), &out_flag]);
    assert_eq!(o.status.code(), Some(0), "stderr: {}", stderr(&o));
    assert!(stdout(&o).contains("actions ("), "stdout: {}", stdout(&o));
    assert!(
        stdout(&o).contains("full dependency graph"),
        "{}",
        stdout(&o)
    );

    let trace = d.join(format!("{}-trace.json", retrace::ingest::testlog::RUN_ID));
    let graph = d.join(format!("{}-graph.json", retrace::ingest::testlog::RUN_ID));
    assert!(trace.exists() && graph.exists());

    // The emitted pair drives the ordinary headless pipeline.
    let o2 = run(&[&trace.display().to_string(), "--json=overview"]);
    assert_eq!(o2.status.code(), Some(0), "stderr: {}", stderr(&o2));
    let v: serde_json::Value = serde_json::from_str(&stdout(&o2)).unwrap();
    assert!(v["overview"]["status"].is_string());
    std::fs::remove_dir_all(&d).ok();
}
/// Declared counts and the rows actually loaded remain visible together.
#[test]
fn the_summary_line_reconciles_declared_counts() {
    let (t, g) = (
        fixture("review-trace.json"),
        fixture("review-declared-missing-graph.jsonl"),
    );
    let o = run(&[&t, &format!("--graph={g}"), "--summary"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let s = stdout(&o);
    assert!(
        s.contains("declared 200 nodes / 100 edges — PARTIAL"),
        "{s}"
    );
}

/// A graph that names another run is the user's input when passed
/// explicitly (refused, exit 2) and a stray file when discovered
/// (dropped with a warning; graph topics refuse, the rest run).
#[test]
fn an_explicit_graph_for_another_run_exits_2_and_a_discovered_one_is_dropped_with_a_warning() {
    let g = fixture("review-mismatched-run-graph.jsonl");
    let d = ingest_dir("runpair");
    let t = d.join("x-trace.json");
    std::fs::write(
        &t,
        concat!(
            "[\n",
            r#"{"ph":"M","cat":"run","name":"run_start","ts":0,"pid":1,"args":{"v":1,"run":"trace-run"}},"#,
            "\n",
            r#"{"ph":"X","cat":"recipe","name":"a","ts":0,"dur":10,"args":{"run":"trace-run"}},"#,
            "\n",
            r#"{"ph":"X","cat":"recipe","name":"b","ts":10,"dur":5,"args":{"run":"trace-run"}},"#,
            "\n",
            r#"{"ph":"M","cat":"run","name":"run_end","ts":20,"pid":1,"args":{"v":1,"run":"trace-run","exit":0}}"#,
            "\n]\n"
        ),
    )
    .unwrap();
    let o = run(&[
        t.to_str().unwrap(),
        &format!("--graph={g}"),
        "--json=critical-path",
        "--compact",
    ]);
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    assert!(
        stderr(&o).contains("another-run") && stderr(&o).contains("trace-run"),
        "{}",
        stderr(&o)
    );

    std::fs::copy(&g, d.join("x-graph.jsonl")).unwrap();
    let o = run(&[
        t.to_str().unwrap(),
        "--json=critical-path,overview",
        "--compact",
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(
        stderr(&o).contains("ignoring the graph beside this trace"),
        "{}",
        stderr(&o)
    );
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["critical_path"]["status"], "refused");
    assert_eq!(v["overview"]["status"], "available");
    std::fs::remove_dir_all(&d).ok();
}

/// A cyclic graph is refused unless repaired, and a repaired or
/// malformed graph never yields an `available` critical path.
#[test]
fn cyclic_and_malformed_graphs_never_report_available() {
    let (t, cyc, mal) = (
        fixture("review-trace.json"),
        fixture("review-cycle-graph.jsonl"),
        fixture("review-malformed-graph.jsonl"),
    );
    let topics = "--json=critical-path";
    let o = run(&[&t, &format!("--graph={cyc}"), topics, "--compact"]);
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    assert!(
        stderr(&o).contains("dependency cycle") && stderr(&o).contains("--repair-cycles"),
        "{}",
        stderr(&o)
    );

    let o = run(&[
        &t,
        &format!("--graph={cyc}"),
        "--repair-cycles",
        topics,
        "--compact",
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    let cp = &v["critical_path"];
    assert_eq!(cp["status"], "partial", "{cp}");
    assert!(cp.get("bound").is_none(), "{cp}");
    assert!(
        cp["limitations"].as_array().unwrap().iter().any(|l| {
            let l = l.as_str().unwrap();
            l.contains("repaired graph") && l.contains("not a bound")
        }),
        "{cp}"
    );
    assert!(
        cp["coverage"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["scope"] == "dependency-edges-kept"),
        "{cp}"
    );
    assert_eq!(cp["data"]["cp_us"], 15);

    // the malformed row for `a` is dropped and leaves no name behind,
    // so `a` is an event without a graph node
    let o = run(&[&t, &format!("--graph={mal}"), topics, "--compact"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    let cp = &v["critical_path"];
    assert_ne!(cp["status"], "available", "{cp}");
    let cov = cp["coverage"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["scope"] == "actions-with-graph-nodes")
        .unwrap();
    assert_eq!(
        (cov["numerator"].as_u64(), cov["denominator"].as_u64()),
        (Some(1), Some(2))
    );

    // the flag against a clean graph changes nothing
    {
        let f = fixture("smoke-trace.json");
        let o = run(&[&f, "--repair-cycles", topics, "--compact"]);
        assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
        let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
        assert_eq!(v["critical_path"]["status"], "available");
    }
}

/// A complete log with an undefined projection is refused; the refusal
/// exits 2 and publishes neither half of the pair nor any staging file.
#[test]
fn a_graph_refusal_exits_2_and_publishes_nothing() {
    let d = ingest_dir("refusal");
    let log = retrace::ingest::testlog::demo_log(&d, Some(1), false, true);
    let out_flag = format!("--out={}", d.display());
    let o = run(&["ingest", &log.display().to_string(), &out_flag]);
    assert_eq!(o.status.code(), Some(2), "stderr: {}", stderr(&o));
    assert!(
        stderr(&o).contains("refusing to emit a silently-holed graph"),
        "{}",
        stderr(&o)
    );
    let left: Vec<String> = std::fs::read_dir(&d)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        left.iter().all(|n| !n.ends_with("-trace.json")
            && !n.ends_with("-graph.json")
            && !n.ends_with(".partial")),
        "{left:?}"
    );
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn viewing_a_log_defaults_its_artifacts_to_the_cache_dir_not_cwd() {
    let d = ingest_dir("cachedefault");
    let log = retrace::ingest::testlog::demo_log(&d, Some(1), true, true);
    let cache = d.join("xdg-cache");

    // No --out: the auto-route must not write into the working
    // directory — a `retrace --last` runs from inside a repo.
    let o = run_env(
        &[&log.display().to_string(), "--json=critical-path"],
        &[("XDG_CACHE_HOME", &cache.display().to_string())],
    );
    assert_eq!(o.status.code(), Some(0), "stderr: {}", stderr(&o));
    let trace = cache
        .join("retrace")
        .join(format!("{}-trace.json", retrace::ingest::testlog::RUN_ID));
    assert!(trace.exists(), "expected {}", trace.display());
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn a_log_positional_ingests_then_analyzes_with_clean_stdout() {
    let d = ingest_dir("route");
    let log = retrace::ingest::testlog::demo_log(&d, Some(1), true, true);
    let out_flag = format!("--out={}", d.display());

    let o = run(&[
        &log.display().to_string(),
        "--json=critical-path",
        &out_flag,
    ]);
    assert_eq!(o.status.code(), Some(0), "stderr: {}", stderr(&o));
    // stdout is pure JSON; the ingest status went to stderr.
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert!(v["critical_path"]["status"].is_string());
    assert!(stderr(&o).contains("actions ("), "stderr: {}", stderr(&o));
    std::fs::remove_dir_all(&d).ok();
}

#[test]
#[cfg(unix)]
fn reopening_a_log_reuses_the_pair_and_explicit_ingest_refreshes_it() {
    use std::os::unix::fs::MetadataExt;
    let d = ingest_dir("reuse-conversion");
    let log = retrace::ingest::testlog::demo_log(&d, Some(1), true, true);
    let log_arg = log.display().to_string();
    let out_flag = format!("--out={}", d.display());
    let args = [&*log_arg, "--json=critical-path", &*out_flag];
    let first = run(&args);
    assert_eq!(first.status.code(), Some(0), "{}", stderr(&first));
    assert!(stderr(&first).contains("converting"));
    let trace = d.join(format!("{}-trace.json", retrace::ingest::testlog::RUN_ID));
    let graph = d.join(format!("{}-graph.json", retrace::ingest::testlog::RUN_ID));
    let stamp = |p: &std::path::PathBuf| {
        let m = std::fs::metadata(p).unwrap();
        (m.modified().unwrap(), m.ino(), m.ctime(), m.ctime_nsec())
    };
    let times = [&trace, &graph].map(stamp);
    let second = run(&args);
    assert_eq!(second.status.code(), Some(0), "{}", stderr(&second));
    assert!(stderr(&second).contains("reusing cached conversion"));
    assert!(!stderr(&second).contains("retrace: converting"));
    assert_eq!(
        [&trace, &graph].map(stamp),
        times,
        "a cache hit does not rewrite the artifact pair"
    );
    let a: serde_json::Value = serde_json::from_str(&stdout(&first)).unwrap();
    let b: serde_json::Value = serde_json::from_str(&stdout(&second)).unwrap();
    assert_eq!(a["critical_path"]["data"], b["critical_path"]["data"]);
    let forced = run(&["ingest", &log_arg, &out_flag]);
    assert_eq!(forced.status.code(), Some(0), "{}", stderr(&forced));
    assert!(!stdout(&forced).contains("reusing cached conversion"));
    let phases = run(&[&log_arg, "--json=critical-path", &out_flag, "--phases"]);
    assert_eq!(phases.status.code(), Some(0), "{}", stderr(&phases));
    assert!(stderr(&phases).contains("converting"));
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn ingest_refuses_a_source_it_cannot_classify() {
    let o = run(&["ingest", "not-a-log.txt"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("not an event log"), "{}", stderr(&o));

    let o2 = run(&["ingest"]);
    assert_eq!(o2.status.code(), Some(2));
    assert!(stderr(&o2).contains("--last"), "{}", stderr(&o2));
}

#[test]
fn ingest_refuses_flags_outside_its_own_set() {
    let d = ingest_dir("flags");
    let log = retrace::ingest::testlog::demo_log(&d, Some(1), true, true);
    let out_flag = format!("--out={}", d.display());
    let log_arg = log.display().to_string();

    // Analysis flags belong to the viewer, not to the verb: accepting
    // one silently would export the pair and drop the flag on the
    // floor, which reads as if it had been honoured.
    for bad in ["--json=overview", "--stats", "--stets"] {
        let o = run(&["ingest", &log_arg, &out_flag, bad]);
        assert_eq!(o.status.code(), Some(2), "{bad}: {}", stderr(&o));
        assert!(
            stderr(&o).contains(&format!("unknown option {bad}")),
            "{bad}: {}",
            stderr(&o)
        );
    }

    // ... while its own three still work.
    let o = run(&["ingest", &log_arg, &out_flag, "--phases"]);
    assert_eq!(o.status.code(), Some(0), "stderr: {}", stderr(&o));
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn cp_rank_prices_an_exact_action_and_refuses_ambiguous_or_unmeasured_inputs() {
    let trace = fixture("smoke-trace.json");
    let output = run(&[
        &trace,
        "--json=cp-rank",
        "--simulate=z (cxx_link)",
        "--scale=0.5",
        "--compact",
    ]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["cp_rank"]["status"], "available");
    let result = &value["cp_rank"]["data"]["simulate"];
    assert_eq!(result["target"], "z (cxx_link)");
    assert_eq!(result["matched"], 1);
    assert_eq!(result["simulated_duration_us"], 400);
    assert_eq!(result["cp_us"], 900);
    assert_eq!(result["saved_us"], 400);

    for (option, reason) in [
        ("--simulate=(", "ambiguous"),
        ("--simulate=missing", "no action matches"),
        ("--simulate-config=free-admission", "no action node carries"),
    ] {
        let output = run(&[&trace, "--json=cp-rank", option, "--compact"]);
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let topic = &value["cp_rank"];
        assert_eq!(topic["status"], "refused", "{value}");
        assert!(topic["data"].is_null());
        assert!(
            topic["limitations"]
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l.as_str().unwrap().contains(reason)),
            "{value}"
        );
    }
}

#[test]
fn blast_target_is_resolved_and_missing_targets_are_explicit_refusals() {
    let trace = fixture("smoke-trace.json");
    for target in ["fbcode//x:y", "x:y"] {
        let output = run(&[
            &trace,
            "--json=blast",
            &format!("--target={target}"),
            "--compact",
        ]);
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_ne!(value["blast"]["status"], "refused", "{value}");
        let scoped = &value["blast"]["data"]["target"];
        assert_eq!(scoped["descendant_path_us"], 1300, "{value}");
        assert_eq!(scoped["downstream_cache_hits"], 1);
        assert!(scoped.get("warm_wall_us").is_none());
    }
    let output = run(&[&trace, "--json=blast", "--target=not-present", "--compact"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["blast"]["status"], "refused");
    assert!(value["blast"]["data"].is_null());
}

#[test]
fn action_detail_reads_one_record_without_loading_the_graph_or_all_extras() {
    let source = fixture("activity-trace.json");
    let dir = ingest_dir("action-detail");
    let trace = dir.join("detail-trace.json");
    std::fs::copy(source, &trace).unwrap();
    // A graph-independent request must not even try this discovered
    // sidecar. Asking for it explicitly still validates and refuses it.
    let graph = dir.join("detail-graph.json");
    std::fs::write(
        &graph,
        "{\"graph\":{\"enc\":2}}\n{\"t\":\"fallback (rustc)\",\"d\":[\"tset:dead\"]}\n",
    )
    .unwrap();
    let output = run(&[
        trace.to_str().unwrap(),
        "--json=action-detail",
        "--action=fallback (rustc)",
        "--compact",
    ]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(!stderr(&output).contains("graph sidecar"));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(value["trace"]["graph"].is_null());
    assert_eq!(value["action_detail"]["status"], "available", "{value}");
    let detail = &value["action_detail"]["data"];
    assert_eq!(detail["target"], "fallback (rustc)");
    assert_eq!(detail["start_us"], 1_000_000);
    assert_eq!(detail["duration_us"], 600_000_000);
    assert_eq!(detail["activity"]["covered_us"], 599_999_000);
    assert_eq!(detail["activity"]["uncovered_us"], 1_000);
    assert!(detail["stage_lines"].is_array());
    assert!(detail["rows"].is_array());

    let index = detail["action_index"].as_u64().unwrap();
    let by_index = run(&[
        trace.to_str().unwrap(),
        "--json=action-detail",
        &format!("--action=action:{index}"),
        "--compact",
    ]);
    assert_eq!(by_index.status.code(), Some(0), "{}", stderr(&by_index));
    let by_index: serde_json::Value = serde_json::from_slice(&by_index.stdout).unwrap();
    assert_eq!(by_index["action_detail"], value["action_detail"]);
    for selector in [
        None,
        Some("--action=fallback"),
        Some("--action=action:999999"),
    ] {
        let mut args = vec![trace.to_str().unwrap(), "--json=action-detail", "--compact"];
        args.extend(selector);
        let output = run(&args);
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["action_detail"]["status"], "refused");
    }
    let explicit = run(&[
        trace.to_str().unwrap(),
        "--json=action-detail",
        "--action=fallback (rustc)",
        &format!("--graph={}", graph.display()),
    ]);
    assert_eq!(explicit.status.code(), Some(2), "{}", stderr(&explicit));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn last_and_command_id_resolve_a_local_log_from_inside_the_checkout() {
    use retrace::ingest::testlog::CommandOnlyResult;
    use retrace::ingest::testlog::RUN_ID;
    use retrace::ingest::testlog::command_only_log;

    let dir = ingest_dir("local-resolution");
    std::fs::write(dir.join(".buckconfig"), "[project]\n").unwrap();
    let logs = dir.join("buck-out/v2/log");
    std::fs::create_dir_all(&logs).unwrap();
    let source = command_only_log(&dir, CommandOnlyResult::Success, true);
    let log = logs.join(format!("20260918-000000_aquery_{RUN_ID}_events.pb"));
    std::fs::rename(source, log).unwrap();
    let cwd = dir.join("nested/working-directory");
    std::fs::create_dir_all(&cwd).unwrap();
    // Buck may supply a checkout-relative artifact path. Resolve it before
    // changing the child's working directory to the fixture checkout.
    let executable = std::fs::canonicalize(bin()).unwrap();
    for (selector, suffix) in [("--last", "last"), (RUN_ID, "id")] {
        // Separate outputs prevent the UUID path from reusing the --last
        // receipt, so both local resolution routes are exercised.
        let out = dir.join(suffix);
        let output = Command::new(&executable)
            .current_dir(&cwd)
            // A regression cannot silently fall back to a real download.
            .env("PATH", dir.join("no-helper-binaries"))
            .args([
                selector,
                "--json=command-profile",
                "--compact",
                &format!("--out={}", out.display()),
            ])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["command_profile"]["data"]["run"], RUN_ID);
        assert_eq!(value["command_profile"]["data"]["command_kind"], "aquery");
        assert!(out.join(format!("{RUN_ID}-trace.json")).is_file());
        assert!(!stderr(&output).contains("downloading"));
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn admission_distinguishes_recorded_zero_from_an_unmeasured_action() {
    let dir = ingest_dir("zero-admission");
    let trace = dir.join("zero-trace.json");
    std::fs::write(
        &trace,
        concat!(
            "[\n",
            "{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"measured\",\"ts\":100,\"dur\":5,\"args\":{\"re\":{\"rs_exec_sem_wait_us\":0}}},\n",
            "{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"unmeasured\",\"ts\":106,\"dur\":7,\"args\":{}}\n",
            "]\n",
        ),
    )
    .unwrap();
    let output = run(&[trace.to_str().unwrap(), "--json=admission", "--compact"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let topic = &value["admission"];
    assert_eq!(topic["status"], "partial", "{value}");
    assert_eq!(topic["data"]["actions"], 1);
    assert_eq!(topic["data"]["exec_slot_wait_us"], 0);
    let coverage = topic["coverage"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["scope"] == "actions-with-admission-measurements")
        .unwrap();
    assert_eq!(coverage["numerator"], 1);
    assert_eq!(coverage["denominator"], 2);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
#[cfg(unix)]
fn non_utf8_arguments_refuse_without_panicking() {
    use std::os::unix::ffi::OsStringExt as _;

    let output = Command::new(bin())
        .arg(std::ffi::OsString::from_vec(b"bad-\xff.json".to_vec()))
        .arg("--stats")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(stderr(&output).contains("UTF-8"), "{}", stderr(&output));
    assert!(!stderr(&output).contains("panicked"));
    assert!(output.stdout.is_empty());
}

#[test]
fn findings_without_a_graph_keep_replay_unavailability_visible() {
    let source = fixture("activity-trace.json");
    let dir = ingest_dir("findings-no-graph");
    let trace = dir.join("activity-trace.json");
    std::fs::copy(source, &trace).unwrap();
    let output = run(&[trace.to_str().unwrap(), "--json=findings", "--compact"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["findings"]["status"], "partial", "{value}");
    assert!(value["findings"]["data"]["base_cp_us"].is_null());
    assert!(
        value["findings"]["limitations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap().contains("no dependency graph"))
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
#[cfg(unix)]
fn a_closed_stdout_exits_successfully_without_panicking() {
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::process::Stdio;

    let trace = fixture("smoke-trace.json");
    // Close the reader before spawning to avoid racing a short JSON
    // write. The real child sees EPIPE on its stdout descriptor.
    let (reader, writer) = UnixStream::pair().unwrap();
    drop(reader);
    let output = Command::new(bin())
        .args([&trace, "--json=longest", "--compact"])
        .stdout(Stdio::from(OwnedFd::from(writer)))
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(!stderr(&output).contains("panicked"));
}
