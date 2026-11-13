# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""Classify pre-dexed library classes for Buck's primary/secondary dex planning.

merge_pre_dexed_libs invokes this tool in batches with per-library class names,
weights, reference counts, and primary-dex patterns. The JSON output feeds
sort_pre_dexed_files.py, which packs classes into final dex groups. This tool
classifies metadata; it does not read or split dex bytecode.

With --dex-units, each library also supplies a TSV manifest of indivisible
class-owned dex units, including synthetic companions and measured counts. If
any member or its synthesizing context matches a primary pattern, the whole
unit is primary. Unit membership and counts are forwarded for atomic packing.

Without --dex-units, the legacy path classifies individual class names and
forwards library totals plus any available per-class reference counts. In both
modes, optional --synthetic-contexts metadata lets synthetics follow their
synthesizing context into primary dex without parsing D8's mangled names.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
from collections.abc import Iterable
from typing import TypedDict


class DexUnit(TypedDict):
    class_names: list[str]
    primary: bool
    weight_estimate: int
    method_ref_count: int
    field_ref_count: int
    type_ref_count: int


PREFIX_MARKER = "^"
SUFFIX_MARKER = "^"
REGEX_MARKER = "^-"


class ClassNameFilter:
    def __init__(self, primary_dex_patterns):
        prefixes = []
        suffixes = []
        substrings = []
        exact_matches = []
        regular_expressions = []

        for pattern in primary_dex_patterns:
            if pattern.startswith(REGEX_MARKER):
                regular_expressions.append(pattern[2:])
            else:
                is_prefix = pattern[0] == PREFIX_MARKER
                is_suffix = pattern[-1] == SUFFIX_MARKER
                if is_prefix and is_suffix:
                    exact_matches.append(pattern[1:-1])
                elif is_prefix:
                    prefixes.append(pattern[1:])
                elif is_suffix:
                    suffixes.append(pattern[:-1])
                else:
                    substrings.append(pattern)

        self.prefixes = prefixes
        self.suffixes = suffixes
        self.substrings = substrings
        self.exact_matches = exact_matches
        self.regular_expressions = [
            re.compile(regular_expression) for regular_expression in regular_expressions
        ]

    def class_name_matches_filter(self, class_name):
        if class_name in self.exact_matches:
            return True

        for prefix in self.prefixes:
            if class_name.startswith(prefix):
                return True

        for suffix in self.suffixes:
            if class_name.endswith(suffix):
                return True

        for substring in self.substrings:
            if substring in class_name:
                return True

        for regular_expression in self.regular_expressions:
            if regular_expression.match(class_name):
                return True

        return False


def _parse_args():
    parser = argparse.ArgumentParser(
        description="Tool to filter a dex for primary class names.",
        fromfile_prefix_chars="@",
    )

    parser.add_argument(
        "--primary-dex-patterns",
        type=pathlib.Path,
        required=True,
        help="a path to a list of primary dex patterns",
    )
    parser.add_argument(
        "--dex-target-identifiers",
        type=str,
        required=True,
        nargs="+",
        help="a list of dex target identifiers",
    )
    parser.add_argument(
        "--class-names",
        type=pathlib.Path,
        required=True,
        nargs="+",
        help="a path to a list of class names",
    )
    parser.add_argument(
        "--weight-estimates",
        type=pathlib.Path,
        required=True,
        nargs="+",
        help="a path to a weight estimate",
    )
    parser.add_argument(
        "--ref-counts",
        type=pathlib.Path,
        required=True,
        nargs="+",
        help=(
            "a path to a ref count file ('<method_count> <field_count> <type_count>' for the"
            " whole lib, optionally followed by one '<method_count> <field_count> <type_count>"
            " <class>' line per class)"
        ),
    )
    parser.add_argument(
        "--synthetic-contexts",
        type=pathlib.Path,
        nargs="+",
        default=[],
        help="a path to a synthetic-to-context file (one '<synthetic> <context>' pair per line)",
    )
    parser.add_argument(
        "--dex-units",
        type=pathlib.Path,
        nargs="+",
        default=[],
        help="per-library TSV files describing indivisible class-owned dex units",
    )
    parser.add_argument(
        "--output",
        type=pathlib.Path,
        required=True,
        help="a path to an output. The output is a JSON mapping of dex target names to a map of primary dex classes, secondary dex classes, and weight estimate.",
    )

    return parser.parse_args()


def _parse_ref_counts(ref_count_path):
    """Parse a ref count file: '<method_count> <field_count> <type_count>' for the whole lib,
    then, for a lib over a 64K limit, one '<method_count> <field_count> <type_count> <class>'
    line per class."""
    with open(ref_count_path) as ref_count_file:
        first_line, *class_lines = ref_count_file.read().strip().split("\n")
        parts = first_line.split()
        if len(parts) != 3:
            raise ValueError(
                f"Expected 3 values (method field type) in ref count file "
                f"{ref_count_path}, got {len(parts)}: {parts!r}"
            )
        class_ref_counts = {}
        for line in class_lines:
            class_parts = line.split()
            if len(class_parts) != 4:
                return parts[0], parts[1], parts[2], {}
            method_count, field_count, type_count, class_name = class_parts
            try:
                counts = [int(method_count), int(field_count), int(type_count)]
            except ValueError:
                return parts[0], parts[1], parts[2], {}
            class_ref_counts[class_name + ".class"] = counts
        return parts[0], parts[1], parts[2], class_ref_counts


def _parse_synthetic_contexts(synthetic_contexts_path):
    """Parse '<synthetic> <context>' lines, as written by the pre-dex step, into a dict."""
    synthetic_to_context = {}
    with open(synthetic_contexts_path) as synthetic_contexts_file:
        for line in synthetic_contexts_file:
            line = line.strip()
            if not line:
                continue
            synthetic, _, context = line.partition(" ")
            if not context:
                raise ValueError(
                    f"Malformed line in {synthetic_contexts_path}: {line!r}"
                )
            synthetic_to_context[synthetic] = context
    return synthetic_to_context


def _resolve_synthesizing_context(java_class, synthetic_to_context):
    """Follow a synthetic back to the class it was synthesized from.

    D8 can synthesize from a synthetic, so the chain is walked to its end. The seen-set guards
    against a cycle in D8's output, which would otherwise hang the build.
    """
    seen = set()
    while java_class in synthetic_to_context and java_class not in seen:
        seen.add(java_class)
        java_class = synthetic_to_context[java_class]
    return java_class


def _belongs_in_primary_dex(java_class, class_name_filter, synthetic_to_context):
    if class_name_filter.class_name_matches_filter(java_class):
        return True

    # A synthetic has to land in the same dex as the class it was synthesized from, which
    # references it directly. Splitting them puts the reference across a dex boundary that the
    # primary dex cannot resolve. primary_dex_patterns are written against real class names, so
    # match the context rather than the synthetic's mangled name.
    context = _resolve_synthesizing_context(java_class, synthetic_to_context)
    return context != java_class and class_name_filter.class_name_matches_filter(
        context
    )


def _parse_dex_unit(line: str, location: str) -> DexUnit:
    parts = line.split("\t")
    if len(parts) < 6:
        raise ValueError(
            f"Expected owner, four counts and members in {location}: {line!r}"
        )
    owner = parts[0]
    members = parts[5:]
    for name in [owner, *members]:
        # Validate nonempty slash-separated internal-name components and reject
        # common malformed formats (dots, descriptors, arrays, or whitespace).
        # This is a format sanity check, not Java source-identifier validation.
        if re.fullmatch(r"[^.;\[\s/]+(?:/[^.;\[\s/]+)*", name) is None:
            raise ValueError(f"Invalid internal class name in {location}: {name!r}")
    if owner not in members:
        raise ValueError(f"Dex unit owner {owner!r} is not among members in {location}")
    if len(set(members)) != len(members):
        raise ValueError(f"Duplicate dex unit members in {location}: {members!r}")
    if any(re.fullmatch(r"[0-9]+", value) is None for value in parts[1:5]):
        raise ValueError(
            f"Expected non-negative integer counts in {location}: {parts[1:5]!r}"
        )
    weight, methods, fields, types = map(int, parts[1:5])
    return {
        "class_names": [member + ".class" for member in members],
        "primary": False,
        "weight_estimate": weight,
        "method_ref_count": methods,
        "field_ref_count": fields,
        "type_ref_count": types,
    }


def _parse_dex_units(
    dex_units_path: pathlib.Path,
    all_class_names: Iterable[str],
    class_name_filter: ClassNameFilter,
    synthetic_to_context: dict[str, str],
) -> list[DexUnit]:
    units: list[DexUnit] = []
    seen_members = set()
    with open(dex_units_path) as dex_units_file:
        for line_number, line in enumerate(dex_units_file, 1):
            line = line.rstrip("\r\n")
            if not line:
                continue
            location = f"{dex_units_path}:{line_number}"
            unit = _parse_dex_unit(line, location)
            members = [name.removesuffix(".class") for name in unit["class_names"]]
            overlap = seen_members.intersection(members)
            if overlap:
                raise ValueError(
                    f"Members appear in multiple dex units in {location}: {sorted(overlap)!r}"
                )
            seen_members.update(members)
            unit["primary"] = any(
                _belongs_in_primary_dex(member, class_name_filter, synthetic_to_context)
                for member in members
            )
            units.append(unit)
    expected_members = set(all_class_names)
    if seen_members != expected_members:
        raise ValueError(
            f"Dex unit members do not match class names in {dex_units_path}: "
            f"missing={sorted(expected_members - seen_members)!r}, "
            f"unexpected={sorted(seen_members - expected_members)!r}"
        )
    return units


def main():
    args = _parse_args()

    primary_dex_patterns_path = args.primary_dex_patterns
    with open(primary_dex_patterns_path) as primary_dex_patterns_file:
        all_primary_dex_patterns = [line.rstrip() for line in primary_dex_patterns_file]

    class_name_filter = ClassNameFilter(all_primary_dex_patterns)

    dex_target_identifiers = args.dex_target_identifiers
    class_names_paths = args.class_names
    weight_estimate_paths = args.weight_estimates
    ref_count_paths = args.ref_counts
    synthetic_contexts_paths = args.synthetic_contexts
    dex_units_paths = args.dex_units
    output = args.output

    assert len(dex_target_identifiers) == len(class_names_paths), (
        "Must provide same number of class names files as dex target identifiers!"
    )

    assert len(dex_target_identifiers) == len(weight_estimate_paths), (
        "Must provide same number of weight estimate files as dex target identifiers!"
    )

    assert len(dex_target_identifiers) == len(ref_count_paths), (
        "Must provide same number of ref count files as dex target identifiers!"
    )

    assert not synthetic_contexts_paths or len(dex_target_identifiers) == len(
        synthetic_contexts_paths
    ), "Must provide same number of synthetic context files as dex target identifiers!"

    if dex_units_paths and len(dex_units_paths) != len(dex_target_identifiers):
        raise ValueError(
            "Must provide same number of dex unit files as dex target identifiers!"
        )

    json_output = {}
    for i in range(len(dex_target_identifiers)):
        dex_target_name = dex_target_identifiers[i]
        weight_estimate_path = weight_estimate_paths[i]
        with open(weight_estimate_path) as weight_estimate_file:
            weight_estimate = weight_estimate_file.read().strip()

        method_ref_count, field_ref_count, type_ref_count, class_ref_counts = (
            _parse_ref_counts(ref_count_paths[i])
        )

        class_names_path = class_names_paths[i]
        with open(class_names_path) as class_names_file:
            all_class_names = [line.rstrip() for line in class_names_file]

        synthetic_to_context = (
            _parse_synthetic_contexts(synthetic_contexts_paths[i])
            if synthetic_contexts_paths
            else {}
        )

        dex_units = (
            _parse_dex_units(
                dex_units_paths[i],
                all_class_names,
                class_name_filter,
                synthetic_to_context,
            )
            if dex_units_paths
            else None
        )
        primary_unit_members = {
            name
            for unit in dex_units or []
            if unit["primary"]
            for name in unit["class_names"]
        }
        primary_dex_class_names = []
        secondary_dex_class_names = []
        for java_class in all_class_names:
            is_primary = (
                java_class + ".class" in primary_unit_members
                if dex_units is not None
                else _belongs_in_primary_dex(
                    java_class, class_name_filter, synthetic_to_context
                )
            )
            if is_primary:
                primary_dex_class_names.append(java_class + ".class")
            else:
                secondary_dex_class_names.append(java_class + ".class")

        json_output[dex_target_name] = {
            "primary_dex_class_names": primary_dex_class_names,
            "secondary_dex_class_names": secondary_dex_class_names,
            "weight_estimate": weight_estimate,
            "method_ref_count": method_ref_count,
            "field_ref_count": field_ref_count,
            "type_ref_count": type_ref_count,
        }
        if class_ref_counts:
            json_output[dex_target_name]["class_ref_counts"] = class_ref_counts
        if dex_units is not None:
            json_output[dex_target_name]["dex_units"] = dex_units

    with open(output, "w") as output_file:
        json.dump(json_output, output_file, indent=4)


if __name__ == "__main__":
    main()
