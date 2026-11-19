# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Rewrites a pytest buck e2e test module into a `unittest` one.

The module's `test_*` functions become the methods of one `BuckTestCase` subclass, named
after the module (`test_build_report.py` gives `BuildReportTest`), with `self` as their first
parameter. `buck_test()` supplies the `buck` and `tmp_path` parameters as before.

    pytest.mark.parametrize  ->  parametrize (buck_workspace's)
    pytest.param             ->  param
    pytest.mark.skipif       ->  unittest.skipIf
    pytest.mark.skip         ->  unittest.skip
    pytest.mark.xfail        ->  unittest.expectedFailure
    pytest.raises            ->  self.assertRaises / self.assertRaisesRegex (`.value` -> `.exception`)
    pytest.fail              ->  self.fail
    pytest.skip              ->  self.skipTest

A module that does something this script does not handle (a test defined in an `if` block
beside other code, a `pytest.fixture`, `pytest.approx`, tests made with `globals()[name] = ...`,
a `pytest` use that is left over) is reported and must be finished by hand; the rewrite is still
applied. So is an `xfail`: `expectedFailure` fails a test that passes, where pytest's `xfail`
did not, and takes no condition or reason.

    buck2 run fbcode//buck2/tests/tools/pytest_to_unittest:pytest_to_unittest -- <file>...
"""

import argparse
import re
import sys
from pathlib import Path
from typing import Sequence

import libcst as cst
import libcst.matchers as m

BUCK_WORKSPACE = "buck2.tests.e2e_util.buck_workspace"

_PYTEST_MARK_DECORATORS = {
    "parametrize": "parametrize",
    "skipif": "unittest.skipIf",
    "skip": "unittest.skip",
    "xfail": "unittest.expectedFailure",
}


def _dotted(expr: cst.BaseExpression) -> str | None:
    if isinstance(expr, cst.Name):
        return expr.value
    if isinstance(expr, cst.Attribute):
        base = _dotted(expr.value)
        return None if base is None else f"{base}.{expr.attr.value}"
    return None


def _is_test_function(stmt: cst.CSTNode) -> bool:
    return isinstance(stmt, cst.FunctionDef) and stmt.name.value.startswith("test_")


def _class_name_for(path: Path) -> str:
    stem = path.stem.removeprefix("test_").removesuffix("_test")
    return "".join(part.capitalize() for part in stem.split("_")) + "Test"


class _TestBodyRewriter(cst.CSTTransformer):
    """The pytest API inside one test function or method."""

    def __init__(self) -> None:
        super().__init__()
        self.exception_names: set[str] = set()
        self.uses: set[str] = set()

    def leave_Call(
        self, original_node: cst.Call, updated_node: cst.Call
    ) -> cst.BaseExpression:
        name = _dotted(updated_node.func)
        if name == "pytest.raises":
            args = list(updated_node.args)
            match = [a for a in args if a.keyword and a.keyword.value == "match"]
            if match:
                args = [a for a in args if a not in match] + [
                    match[0].with_changes(keyword=None, equal=cst.MaybeSentinel.DEFAULT)
                ]
                func = "self.assertRaisesRegex"
            else:
                func = "self.assertRaises"
            return updated_node.with_changes(func=cst.parse_expression(func), args=args)
        if name == "pytest.fail":
            return updated_node.with_changes(func=cst.parse_expression("self.fail"))
        if name == "pytest.skip":
            return updated_node.with_changes(func=cst.parse_expression("self.skipTest"))
        if name == "pytest.param":
            self.uses.add("param")
            return updated_node.with_changes(func=cst.Name("param"))
        return updated_node

    def visit_WithItem(self, node: cst.WithItem) -> None:
        if (
            isinstance(node.item, cst.Call)
            and _dotted(node.item.func) == "pytest.raises"
            and node.asname is not None
            and isinstance(node.asname.name, cst.Name)
        ):
            self.exception_names.add(node.asname.name.value)

    def leave_Attribute(
        self, original_node: cst.Attribute, updated_node: cst.Attribute
    ) -> cst.BaseExpression:
        if (
            isinstance(updated_node.value, cst.Name)
            and updated_node.value.value in self.exception_names
            and updated_node.attr.value == "value"
        ):
            return updated_node.with_changes(attr=cst.Name("exception"))
        return updated_node


class _DecoratorRewriter(cst.CSTTransformer):
    def __init__(self) -> None:
        super().__init__()
        self.uses: set[str] = set()
        self.warnings: list[str] = []

    def leave_Decorator(
        self, original_node: cst.Decorator, updated_node: cst.Decorator
    ) -> cst.Decorator:
        expr = updated_node.decorator
        call = expr if isinstance(expr, cst.Call) else None
        name = _dotted(call.func if call else expr)
        if name is None or not name.startswith("pytest.mark."):
            return updated_node
        mark = name.removeprefix("pytest.mark.")
        replacement = _PYTEST_MARK_DECORATORS.get(mark)
        if replacement is None:
            return updated_node
        self.uses.add(replacement.split(".")[0])
        new_func = cst.parse_expression(replacement)
        if mark == "xfail":
            self.warnings.append(
                "an `xfail` is an `expectedFailure` now: it fails when the test passes, and its "
                "condition and reason are dropped"
            )
            return updated_node.with_changes(decorator=new_func)
        if call is None:
            return updated_node.with_changes(decorator=cst.Call(func=new_func))
        return updated_node.with_changes(decorator=call.with_changes(func=new_func))

    def leave_Call(
        self, original_node: cst.Call, updated_node: cst.Call
    ) -> cst.BaseExpression:
        if _dotted(updated_node.func) == "pytest.param":
            self.uses.add("param")
            return updated_node.with_changes(func=cst.Name("param"))
        return updated_node


def _to_method(
    fn: cst.FunctionDef, uses: set[str], warnings: list[str]
) -> cst.FunctionDef:
    params = fn.params
    if not params.params or params.params[0].name.value != "self":
        params = params.with_changes(
            params=[cst.Param(name=cst.Name("self")), *params.params]
        )
    fn = fn.with_changes(params=params)
    decorators = _DecoratorRewriter()
    body = _TestBodyRewriter()
    rewritten = fn.visit(decorators)
    assert isinstance(rewritten, cst.FunctionDef)
    rewritten = rewritten.visit(body)
    assert isinstance(rewritten, cst.FunctionDef)
    uses.update(decorators.uses)
    uses.update(body.uses)
    warnings.extend(f"{fn.name.value}: {w}" for w in decorators.warnings)
    return rewritten


class _Rewrite:
    def __init__(self, path: Path, module: cst.Module) -> None:
        self.path = path
        self.module = module
        self.warnings: list[str] = []
        self.uses: set[str] = set()

    def run(self) -> cst.Module:
        kept: list[cst.BaseStatement] = []
        methods: list[cst.BaseStatement] = []
        for stmt in self.module.body:
            if isinstance(stmt, cst.FunctionDef) and _is_test_function(stmt):
                methods.append(_to_method(stmt, self.uses, self.warnings))
            elif isinstance(stmt, cst.If) and self._has_test(stmt):
                methods.append(self._if_to_methods(stmt))
            elif isinstance(stmt, cst.ClassDef) and self._has_test(stmt):
                kept.append(self._convert_class(stmt))
            else:
                kept.append(stmt)
        if methods:
            self.uses.add("BuckTestCase")
            kept.append(self._test_class(methods))
        module = self.module.with_changes(body=kept)
        return self._fix_imports(module)

    def _has_test(self, node: cst.CSTNode) -> bool:
        found = False

        class Finder(cst.CSTVisitor):
            def visit_FunctionDef(self, node: cst.FunctionDef) -> bool:
                nonlocal found
                found = found or _is_test_function(node)
                return False

        node.visit(Finder())
        return found

    def _if_to_methods(self, node: cst.If) -> cst.If:
        def convert(block: cst.BaseSuite) -> cst.BaseSuite:
            assert isinstance(block, cst.IndentedBlock)
            body: list[cst.BaseStatement] = []
            for stmt in block.body:
                if isinstance(stmt, cst.FunctionDef) and _is_test_function(stmt):
                    body.append(_to_method(stmt, self.uses, self.warnings))
                elif isinstance(stmt, cst.If) and self._has_test(stmt):
                    body.append(self._if_to_methods(stmt))
                elif m.matches(stmt, m.SimpleStatementLine([m.Pass()])):
                    body.append(stmt)
                else:
                    self.warnings.append(
                        f"line {self._line(stmt)}: an `if` block holds tests beside other "
                        "code, which is now in the class body too"
                    )
                    body.append(stmt)
            return block.with_changes(body=body)

        orelse = node.orelse
        if isinstance(orelse, cst.If):
            orelse = self._if_to_methods(orelse)
        elif isinstance(orelse, cst.Else):
            orelse = orelse.with_changes(body=convert(orelse.body))
        return node.with_changes(body=convert(node.body), orelse=orelse)

    def _convert_class(self, node: cst.ClassDef) -> cst.ClassDef:
        name = node.name.value
        if name.startswith("Test") and not name.endswith("Test"):
            name = name.removeprefix("Test") + "Test"
        body: list[cst.BaseSmallStatement | cst.BaseStatement] = []
        for stmt in node.body.body:
            if isinstance(stmt, cst.FunctionDef) and _is_test_function(stmt):
                body.append(_to_method(stmt, self.uses, self.warnings))
            else:
                if isinstance(stmt, cst.FunctionDef) and any(
                    "fixture" in (_dotted(d.decorator) or "")
                    or (
                        isinstance(d.decorator, cst.Call)
                        and "fixture" in (_dotted(d.decorator.func) or "")
                    )
                    for d in stmt.decorators
                ):
                    self.warnings.append(
                        f"line {self._line(stmt)}: a `pytest.fixture` in class {node.name.value}"
                    )
                body.append(stmt)
        self.uses.add("BuckTestCase")
        assert isinstance(node.body, cst.IndentedBlock)
        return node.with_changes(
            name=cst.Name(name),
            bases=[cst.Arg(cst.Name("BuckTestCase"))],
            body=node.body.with_changes(body=body),
        )

    def _test_class(self, methods: Sequence[cst.BaseStatement]) -> cst.ClassDef:
        name = _class_name_for(self.path)
        return cst.ClassDef(
            name=cst.Name(name),
            bases=[cst.Arg(cst.Name("BuckTestCase"))],
            body=cst.IndentedBlock(body=list(methods)),
            leading_lines=[cst.EmptyLine(), cst.EmptyLine()],
        )

    def _fix_imports(self, module: cst.Module) -> cst.Module:
        code = module.code
        names = sorted(n for n in self.uses if n != "unittest")
        body = list(module.body)
        import_index = None
        for i, stmt in enumerate(body):
            if not isinstance(stmt, cst.SimpleStatementLine) or not isinstance(
                stmt.body[0], (cst.ImportFrom, cst.Import)
            ):
                continue
            import_index = i
            imp = stmt.body[0]
            if (
                isinstance(imp, cst.ImportFrom)
                and imp.module is not None
                and _dotted(imp.module) == BUCK_WORKSPACE
                and isinstance(imp.names, Sequence)
            ):
                existing = [
                    module.code_for_node(
                        a.with_changes(comma=cst.MaybeSentinel.DEFAULT)
                    ).strip()
                    for a in imp.names
                ]
                added = [n for n in names if n not in existing]
                if added:
                    # One line whatever the shape of the old statement; the formatter wraps it.
                    body[i] = cst.ensure_type(
                        cst.parse_statement(
                            f"from {BUCK_WORKSPACE} import {', '.join(existing + added)}"
                        ),
                        cst.SimpleStatementLine,
                    ).with_changes(leading_lines=stmt.leading_lines)
                names = []
        if names:
            line = cst.parse_statement(
                f"from {BUCK_WORKSPACE} import {', '.join(names)}"
            )
            body.insert((import_index or 0) + 1, line)
            import_index = (import_index or 0) + 1
        if "unittest" in self.uses:
            body.insert((import_index or 0) + 1, cst.parse_statement("import unittest"))
        module = module.with_changes(body=body)
        code = module.code
        for n, line in enumerate(code.splitlines(), 1):
            if "globals()[" in line:
                self.warnings.append(
                    f"line {n}: a test made with `globals()`; make the factory return the "
                    "decorated method and assign it in the class body"
                )
        if "pytest." not in re.sub(r"#.*", "", code):
            body = [
                s
                for s in module.body
                if not m.matches(
                    s,
                    m.SimpleStatementLine(
                        [m.Import([m.ImportAlias(m.Name("pytest"))])]
                    ),
                )
            ]
            module = module.with_changes(body=body)
        else:
            for n, line in enumerate(code.splitlines(), 1):
                if "pytest." in line and not line.lstrip().startswith("#"):
                    self.warnings.append(
                        f"line {n}: pytest is still used: {line.strip()}"
                    )
        return module

    def _line(self, node: cst.CSTNode) -> int:
        wrapper = cst.MetadataWrapper(self.module, unsafe_skip_copy=True)
        positions = wrapper.resolve(cst.metadata.PositionProvider)
        pos = positions.get(node)
        return pos.start.line if pos else 0


def rewrite(path: Path) -> list[str]:
    source = path.read_text()
    module = cst.parse_module(source)
    rewriter = _Rewrite(path, module)
    new = rewriter.run()
    if new.code != source:
        path.write_text(new.code)
    return rewriter.warnings


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("files", nargs="+", type=Path)
    args = parser.parse_args(argv)
    status = 0
    for path in args.files:
        try:
            warnings = rewrite(path)
        except Exception as e:  # noqa: BLE001
            print(f"{path}: FAILED: {e}", file=sys.stderr)
            status = 1
            continue
        for w in warnings:
            print(f"{path}: {w}", file=sys.stderr)
    return status


if __name__ == "__main__":
    sys.exit(main())
