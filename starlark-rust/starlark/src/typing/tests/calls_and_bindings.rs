/*
 * Copyright 2019 The Starlark in Rust Authors.
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

//! The lint typechecker (`AstModule::typecheck`, used by `buck2 starlark typecheck`), the
//! compiler typechecker (`enable_static_typechecking`, on for every `.bxl` file and the prelude)
//! and the runtime, compared on the same programs.

use std::collections::HashMap;

use crate::environment::Globals;
use crate::environment::GlobalsBuilder;
use crate::environment::Module;
use crate::eval::Evaluator;
use crate::syntax::AstModule;
use crate::syntax::Dialect;
use crate::typing::AstModuleTypecheck;

fn globals() -> Globals {
    GlobalsBuilder::extended().build()
}

fn parse(code: &str) -> AstModule {
    AstModule::parse("t.star", code.to_owned(), &Dialect::AllOptionsInternal).unwrap()
}

/// First lint-typechecker error, or `None`.
fn lint(code: &str) -> Option<String> {
    let (errors, _, _, _) = parse(code).typecheck(&globals(), &HashMap::new());
    errors.first().map(|e| format!("{e:#}"))
}

/// Evaluate; `Err(message)` on failure.
fn eval(code: &str, static_typecheck: bool) -> Result<String, String> {
    Module::with_temp_heap(|module| {
        let mut eval = Evaluator::new(&module);
        eval.enable_static_typechecking(static_typecheck);
        let g = globals();
        Ok::<_, crate::Error>(match eval.eval_module(parse(code), &g) {
            Ok(v) => Ok(v.to_string()),
            Err(e) => Err(format!("{e:#}")),
        })
    })
    .unwrap()
}

/// `defs` is accepted by both typecheckers and `defs + call` runs.
#[track_caller]
fn accepted_and_runs(defs: &str, call: &str) {
    assert_eq!(lint(defs), None);
    eval(defs, true).expect("compiler typechecker rejected");
    eval(&format!("{defs}\n{call}"), false).expect("runtime failed");
}

/// `<undefined>.append(..)` inside a def is a scope error for the lint typechecker, like it is
/// for the compiler typechecker, not a panic.
#[test]
fn test_lint_reports_unresolved_append() {
    for method in ["append(1)", "insert(0, 1)", "extend([1])"] {
        let err = lint(&format!("def t():\n    y.{method}\n")).expect("no error");
        assert!(err.contains("Variable `y` not found"), "{method}: {err}");
    }
    assert!(
        eval("def t():\n    y.append(1)\n", true)
            .unwrap_err()
            .contains("Variable `y` not found")
    );
}

/// A name passed both positionally and by keyword is a typing error naming the parameter, and
/// a union of callables where another alternative accepts the call is accepted.
#[test]
fn test_duplicate_argument_is_a_typing_error() {
    let err = lint("def f(a, b): pass\ndef use():\n    f(1, a = 2)\n").unwrap();
    assert!(err.contains("Argument `a` occurs more than once"), "{err}");
    assert!(!err.contains("Internal error"), "{err}");
    let err = eval("def f(a, b): pass\nf(1, a = 2)", false).unwrap_err();
    assert!(err.contains("occurs more than once"), "{err}");
    accepted_and_runs(
        "def f(a, b): pass\ndef h(*args, **kwargs): pass\ndef t(c):\n    k = f if c else h\n    k(1, a = 2)\n",
        "t(False)",
    );
}

/// Membership tests and dict/set lookups widen int and float like `==` does.
#[test]
fn test_int_float_membership_and_lookup_are_accepted() {
    let cases = [
        (
            "def f(x: float) -> bool:\n    return x in [1, 2]\n",
            "f(1.0)",
            "Binary operator `in` is not available",
        ),
        (
            "def f(d: dict[int, str], k: float) -> str:\n    return d[k]\n",
            "f({1: \"a\"}, 1.0)",
            "does not have [] operator",
        ),
        (
            "def f(d: dict[int, str], k: float) -> bool:\n    return k in d\n",
            "f({1: \"a\"}, 1.0)",
            "Binary operator `in` is not available",
        ),
        (
            "def f(s: set[int], k: float) -> bool:\n    return k in s\n",
            "f(set([1]), 1.0)",
            "Binary operator `in` is not available",
        ),
    ];
    for (defs, call, _msg) in cases {
        accepted_and_runs(defs, call);
    }
    // A string key is still rejected.
    let err = lint("def f(d: dict[int, str], k: str) -> str:\n    return d[k]\n").unwrap();
    assert!(err.contains("does not have [] operator"), "{err}");
}
