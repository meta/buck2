# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

ROOT_SYMBOL = "//"
_TARGET_SYMBOL = ":"
_RECURSIVE_SYMBOL = "..."
_PATH_SYMBOL = "/"

# https://www.internalfb.com/intern/staticdocs/buck2/docs/concepts/build_target/
# Anchors are needed here to ensure every character in "name" is valid
_NAME_REGEX_PATTERN = "^[A-Za-z0-9_/.=,@~+-]+$"
_NAME_REGEX = regex(_NAME_REGEX_PATTERN)

_BuildTargetPatternKind = enum(
    "single",
    "package",
    "recursive",
)

BuildTargetPattern = record(
    kind = field(_BuildTargetPatternKind),
    cell = field([str, None], None),
    path = field(str),
    name = field([str, None], None),
    matches = field(typing.Callable),
    as_string = field(typing.Callable),
)

BuildTargetPatternParseResult = record(
    build_target_pattern = field([BuildTargetPattern, None], None),
    error = field([str, None], default = None),
)

def try_parse_build_target_pattern(pattern: str) -> BuildTargetPatternParseResult:
    """
    This function try to parse build target pattern. If parse fails, it will return the error message.
    """
    if not (len(pattern) >= len(ROOT_SYMBOL) + 1):
        err_msg = "Invalid build target pattern, pattern too short: {}".format(pattern)
        return BuildTargetPatternParseResult(error = err_msg)

    root_position = pattern.find(ROOT_SYMBOL)
    if not (root_position >= 0):
        err_msg = "Invalid build target pattern, pattern should started with `{}` or a cell name followed by `{}`: ".format(ROOT_SYMBOL, ROOT_SYMBOL, pattern)
        return BuildTargetPatternParseResult(error = err_msg)

    cell = None
    if root_position > 0:
        cell = pattern[0:root_position]

    name = None
    if pattern.endswith(_TARGET_SYMBOL):
        kind = _BuildTargetPatternKind("package")
        end_of_path_position = len(pattern) - 1
    elif pattern.endswith(_RECURSIVE_SYMBOL):
        kind = _BuildTargetPatternKind("recursive")
        end_of_path_position = len(pattern) - len(_RECURSIVE_SYMBOL) - 1
        if not (pattern[end_of_path_position] == _PATH_SYMBOL):
            err_msg = "Invalid build target pattern, `{}` should be preceded by a `{}`: {}".format(_RECURSIVE_SYMBOL, _PATH_SYMBOL, pattern)
            return BuildTargetPatternParseResult(error = err_msg)
    else:
        kind = _BuildTargetPatternKind("single")
        end_of_path_position = pattern.rfind(_TARGET_SYMBOL)
        if end_of_path_position < 0:
            # Pattern does not have a target delimiter and thus a target name
            # Assume target name to be the same as the last component of the package
            end_of_path_position = len(pattern)
            start_of_package = pattern.rfind(_PATH_SYMBOL)
            name = pattern[start_of_package + len(_PATH_SYMBOL) :]
        elif end_of_path_position < root_position:
            err_msg = "Invalid build target pattern, cell name should not contain `{}`: {}".format(_PATH_SYMBOL, pattern)
            return BuildTargetPatternParseResult(error = err_msg)
        else:
            name = pattern[end_of_path_position + len(_TARGET_SYMBOL) :]

        valid_name = _NAME_REGEX.match(name)
        if not valid_name:
            err_msg = "Invalid build target pattern, target name `{}` contains invalid characters. Valid characters are `{}`.".format(name, _NAME_REGEX_PATTERN)
            return BuildTargetPatternParseResult(error = err_msg)

    start_of_path_position = root_position + len(ROOT_SYMBOL)

    if not (pattern[start_of_path_position] != _PATH_SYMBOL):
        err_msg = "Invalid build target pattern, path cannot start with `{}`: {}".format(_PATH_SYMBOL, pattern)
        return BuildTargetPatternParseResult(error = err_msg)

    path = pattern[start_of_path_position:end_of_path_position]
    if not (path.find(ROOT_SYMBOL) < 0):
        err_msg = "Invalid build target pattern, `{}` can only appear once: {}".format(ROOT_SYMBOL, pattern)
        return BuildTargetPatternParseResult(error = err_msg)
    if not (path.find(_RECURSIVE_SYMBOL) < 0):
        err_msg = "Invalid build target pattern, `{}` can only appear once: {}".format(_RECURSIVE_SYMBOL, pattern)
        return BuildTargetPatternParseResult(error = err_msg)
    if not (path.find(_TARGET_SYMBOL) < 0):
        err_msg = "Invalid build target pattern, `{}` can only appear once: {}".format(_TARGET_SYMBOL, pattern)
        return BuildTargetPatternParseResult(error = err_msg)
    if not (len(path) == 0 or path[-1:] != _PATH_SYMBOL):
        err_msg = "Invalid build target pattern, path cannot end with `{}`: {}".format(_PATH_SYMBOL, pattern)
        return BuildTargetPatternParseResult(error = err_msg)

    # One closure per kind, chosen here rather than on every call: a link
    # group filter calls `matches` once per node of an application graph.
    # The types are left unchecked for the same reason; `label` is a `Label`
    # or a `TargetLabel`.
    if kind == _BuildTargetPatternKind("single"):

        def matches(label):
            if cell and cell != label.cell:
                return False
            return path == label.package and name == label.name
    elif kind == _BuildTargetPatternKind("package"):

        def matches(label):
            if cell and cell != label.cell:
                return False
            return path == label.package
    elif len(path) == 0:
        # This is a recursive pattern of the cell: cell//...
        def matches(label):
            return not cell or cell == label.cell
    else:
        path_with_path_symbol = path + _PATH_SYMBOL

        def matches(label):
            if cell and cell != label.cell:
                return False
            package = label.package
            if len(package) > len(path):
                # pattern cell//package/... matches label cell//package/subpackage:target
                return package.startswith(path_with_path_symbol)
            else:
                return path == package

    # buildifier: disable=uninitialized - self is initialized
    def as_string() -> str:
        normalized_cell = self.cell if self.cell else ""
        if self.kind == _BuildTargetPatternKind("single"):
            return "{}//{}:{}".format(normalized_cell, self.path, self.name)
        elif self.kind == _BuildTargetPatternKind("package"):
            return "{}//{}:".format(normalized_cell, self.path)
        elif self.kind == _BuildTargetPatternKind("recursive"):
            return "{}//{}...".format(normalized_cell, self.path + _PATH_SYMBOL if self.path else "")
        else:
            fail("Unknown build target pattern kind.")

    self = BuildTargetPattern(kind = kind, cell = cell, path = path, name = name, matches = matches, as_string = as_string)

    return BuildTargetPatternParseResult(build_target_pattern = self)

def parse_build_target_pattern(pattern: str) -> BuildTargetPattern:
    parse_res = try_parse_build_target_pattern(pattern)
    if parse_res.error != None:
        fail(parse_res.error)
    return parse_res.build_target_pattern
