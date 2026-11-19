#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""Sokoban level generation pipeline.

Subcommands:
  generate  - Generate levels using sokoban_gen binary
  solve     - Solve all levels in a file and annotate with push counts
  sort      - Sort levels within each size group by difficulty

Example full pipeline:
  python3 games/bin/sokoban_pipeline.py generate --boxes 5 --sizes 25x19,35x19 --count 5 --time-limit 60 --out /tmp/5box_raw.txt
  python3 games/bin/sokoban_pipeline.py solve --file /tmp/5box_raw.txt --out /tmp/5box_solved.txt
  python3 games/bin/sokoban_pipeline.py sort --file /tmp/5box_solved.txt --out games/src/games/sokoban_levels_5box_large.txt
"""

import argparse
import os
import re
import subprocess
import sys
import tempfile


def find_buck2_root():
    """Find the fbcode directory to run buck commands from."""
    d = os.path.dirname(os.path.abspath(__file__))
    while d != "/":
        if os.path.exists(os.path.join(d, "BUCK")) or os.path.exists(
            os.path.join(d, ".buckconfig")
        ):
            # Go up one more to fbcode
            parent = os.path.dirname(d)
            if os.path.basename(d) == "buck2" and os.path.basename(parent) == "fbcode":
                return parent
        d = os.path.dirname(d)
    # Fallback
    return os.environ.get("FBCODE_DIR", "/home/cjhopman/fbsource-3/fbcode")


FBCODE = find_buck2_root()
GEN_TARGET = "fbcode//buck2/games:sokoban_gen"
SOKOBAN_TARGET = "fbcode//buck2/games:sokoban"


def run_buck(target, args, capture=True):
    """Run a buck2 target with args."""
    cmd = ["buck2", "run", target, "--"] + args
    if capture:
        result = subprocess.run(
            cmd, capture_output=True, text=True, cwd=FBCODE, timeout=300
        )
        return result.stdout, result.stderr, result.returncode
    else:
        result = subprocess.run(cmd, cwd=FBCODE, timeout=300)
        return "", "", result.returncode


def parse_size(s):
    """Parse '25x19' into (template_w, template_h).
    Room size = template * 2, so template = room / 2."""
    m = re.match(r"(\d+)x(\d+)", s)
    if not m:
        raise ValueError(f"Invalid size: {s}")
    room_w, room_h = int(m.group(1)), int(m.group(2))
    # Template size is half the room size (rounded up)
    return (room_w + 1) // 2, (room_h + 1) // 2, s


def cmd_generate(args):
    """Generate levels."""
    sizes = [parse_size(s) for s in args.sizes.split(",")]
    outdir = tempfile.mkdtemp(prefix="sokoban_gen_")

    print(f"Generating {args.boxes}-box levels", file=sys.stderr)
    print(f"Sizes: {args.sizes}", file=sys.stderr)
    print(f"Count per size: {args.count}", file=sys.stderr)
    print(f"Time limit per level: {args.time_limit}s", file=sys.stderr)
    print(f"Working dir: {outdir}", file=sys.stderr)

    # Build first
    subprocess.run(
        ["buck2", "build", GEN_TARGET],
        cwd=FBCODE,
        capture_output=True,
        timeout=300,
    )

    all_levels = []
    for tw, th, name in sizes:
        print(f"\n=== {name} ===", file=sys.stderr)
        for i in range(1, args.count + 1):
            print(f"  {name} #{i}: ", end="", flush=True, file=sys.stderr)
            gen_args = [
                "--width",
                str(tw),
                "--height",
                str(th),
                "--boxes",
                str(args.boxes),
                "--time-limit",
                str(args.time_limit),
            ]
            stdout, stderr, rc = run_buck(GEN_TARGET, gen_args)
            if rc != 0 or not stdout.strip():
                print("FAILED", file=sys.stderr)
                continue
            # Extract depth from stderr
            depth_match = re.search(r"best depth: (\d+)", stderr)
            depth = depth_match.group(1) if depth_match else "?"
            print(f"depth {depth}", file=sys.stderr)

            level_text = stdout.strip()
            all_levels.append((name, i, level_text))

    # Write output
    lines = [
        f"; {args.boxes}-box large sokoban levels",
        "; Generated at various room sizes",
        "",
    ]
    for name, num, level_text in all_levels:
        lines.append("")
        lines.append(f"; {name} #{num}")
        lines.append(level_text)
        lines.append("")

    output = "\n".join(lines) + "\n"
    if args.out:
        with open(args.out, "w") as f:
            f.write(output)
        print(f"\nWrote {len(all_levels)} levels to {args.out}", file=sys.stderr)
    else:
        sys.stdout.write(output)


def parse_levels_from_file(path):
    """Parse a levels file into list of (comment, level_lines)."""
    with open(path) as f:
        text = f.read()

    levels = []
    header_lines = []
    current_comment = None
    current_lines = []

    for line in text.split("\n"):
        stripped = line.rstrip()
        if stripped.startswith("; ") and re.match(r"; \d+x\d+", stripped):
            # New level comment
            if current_comment is not None and current_lines:
                levels.append((current_comment, "\n".join(current_lines)))
            elif current_comment is None and not levels:
                header_lines = [
                    line
                    for line in (current_lines if current_lines else [])
                    if line.startswith(";")
                ]
            current_comment = stripped
            current_lines = []
        elif current_comment is not None:
            if stripped or current_lines:  # skip leading blanks
                current_lines.append(stripped)
        else:
            if stripped.startswith(";") or not stripped:
                header_lines.append(stripped)

    if current_comment is not None and current_lines:
        # Strip trailing empty lines
        while current_lines and not current_lines[-1]:
            current_lines.pop()
        levels.append((current_comment, "\n".join(current_lines)))

    return header_lines, levels


def cmd_solve(args):
    """Solve all levels in a file and annotate comments with push counts."""
    header_lines, levels = parse_levels_from_file(args.file)

    print(f"Solving {len(levels)} levels from {args.file}", file=sys.stderr)

    # Build solver
    subprocess.run(
        ["buck2", "build", SOKOBAN_TARGET],
        cwd=FBCODE,
        capture_output=True,
        timeout=300,
    )

    # Write levels to a temp file for the solver
    tmp = tempfile.NamedTemporaryFile(mode="w", suffix=".txt", delete=False)
    for comment, level_text in levels:
        tmp.write(f"\n{comment}\n{level_text}\n")
    tmp.close()

    solved_levels = []
    for i, (comment, level_text) in enumerate(levels):
        level_num = i + 1
        print(f"  Level {level_num}: ", end="", flush=True, file=sys.stderr)
        stdout, stderr, rc = run_buck(
            SOKOBAN_TARGET,
            ["--levels-file", tmp.name, "--solve", str(level_num)],
        )
        pushes_match = re.search(r"solved in (\d+) pushes", stderr)
        if pushes_match:
            pushes = int(pushes_match.group(1))
            print(f"{pushes} pushes", file=sys.stderr)
        elif "no solution" in stderr:
            pushes = -1
            print("NO SOLUTION", file=sys.stderr)
        else:
            pushes = 0
            print(f"ERROR: {stderr.strip()}", file=sys.stderr)

        # Update comment with push count
        # Remove existing push count if present
        base_comment = re.sub(r" \(\d+ pushes\)$", "", comment)
        if pushes > 0:
            new_comment = f"{base_comment} ({pushes} pushes)"
        else:
            new_comment = base_comment
        solved_levels.append((new_comment, level_text, pushes))

    os.unlink(tmp.name)

    # Write output
    output_lines = list(header_lines)
    for comment, level_text, _ in solved_levels:
        output_lines.append("")
        output_lines.append(comment)
        output_lines.append(level_text)
        output_lines.append("")

    output = "\n".join(output_lines) + "\n"
    out_path = args.out or args.file
    with open(out_path, "w") as f:
        f.write(output)
    print(f"\nWrote {len(solved_levels)} levels to {out_path}", file=sys.stderr)


def cmd_sort(args):
    """Sort levels within each size group by difficulty."""
    header_lines, levels = parse_levels_from_file(args.file)

    # Group by size
    groups = {}
    for comment, level_text in levels:
        m = re.match(r"; (\d+x\d+)", comment)
        if not m:
            continue
        size = m.group(1)
        # Extract pushes
        pm = re.search(r"\((\d+) pushes\)", comment)
        pushes = int(pm.group(1)) if pm else 999
        if size not in groups:
            groups[size] = []
        groups[size].append((pushes, level_text, comment))

    # Sort each group by pushes, renumber
    output_lines = list(header_lines)
    size_order = sorted(
        groups.keys(), key=lambda s: tuple(int(x) for x in s.split("x"))
    )

    for size in size_order:
        group = sorted(groups[size], key=lambda x: x[0])
        for i, (pushes, level_text, _) in enumerate(group):
            output_lines.append("")
            if pushes < 999:
                output_lines.append(f"; {size} #{i + 1} ({pushes} pushes)")
            else:
                output_lines.append(f"; {size} #{i + 1}")
            output_lines.append(level_text)
            output_lines.append("")

        pushes_list = [str(p) for p, _, _ in group]
        print(f"  {size}: {' -> '.join(pushes_list)}", file=sys.stderr)

    output = "\n".join(output_lines) + "\n"
    out_path = args.out or args.file
    with open(out_path, "w") as f:
        f.write(output)
    print(f"\nWrote {len(levels)} levels to {out_path}", file=sys.stderr)


def cmd_pipeline(args):
    """Run the full pipeline: generate, solve, sort."""
    raw_file = tempfile.NamedTemporaryFile(
        mode="w", suffix="_raw.txt", delete=False
    ).name
    solved_file = tempfile.NamedTemporaryFile(
        mode="w", suffix="_solved.txt", delete=False
    ).name

    # Generate
    print("=" * 60, file=sys.stderr)
    print("STAGE 1: Generate", file=sys.stderr)
    print("=" * 60, file=sys.stderr)
    gen_args = argparse.Namespace(
        boxes=args.boxes,
        sizes=args.sizes,
        count=args.count,
        time_limit=args.time_limit,
        out=raw_file,
    )
    cmd_generate(gen_args)

    # Solve
    print("\n" + "=" * 60, file=sys.stderr)
    print("STAGE 2: Solve", file=sys.stderr)
    print("=" * 60, file=sys.stderr)
    solve_args = argparse.Namespace(file=raw_file, out=solved_file)
    cmd_solve(solve_args)

    # Sort
    print("\n" + "=" * 60, file=sys.stderr)
    print("STAGE 3: Sort", file=sys.stderr)
    print("=" * 60, file=sys.stderr)
    sort_args = argparse.Namespace(file=solved_file, out=args.out)
    cmd_sort(sort_args)

    # Cleanup
    os.unlink(raw_file)
    os.unlink(solved_file)
    print(f"\nPipeline complete: {args.out}", file=sys.stderr)


def main():
    parser = argparse.ArgumentParser(description="Sokoban level generation pipeline")
    sub = parser.add_subparsers(dest="command", required=True)

    # generate
    p_gen = sub.add_parser("generate", help="Generate levels using sokoban_gen")
    p_gen.add_argument("--boxes", type=int, required=True, help="Number of boxes")
    p_gen.add_argument(
        "--sizes",
        type=str,
        default="25x19,35x19",
        help="Comma-separated room sizes (e.g. 25x19,35x19)",
    )
    p_gen.add_argument(
        "--count", type=int, default=5, help="Levels per size (default: 5)"
    )
    p_gen.add_argument(
        "--time-limit", type=int, default=60, help="Seconds per level (default: 60)"
    )
    p_gen.add_argument("--out", type=str, help="Output file (default: stdout)")

    # solve
    p_solve = sub.add_parser("solve", help="Solve levels and annotate with push counts")
    p_solve.add_argument("--file", type=str, required=True, help="Input levels file")
    p_solve.add_argument(
        "--out", type=str, help="Output file (default: overwrite input)"
    )

    # sort
    p_sort = sub.add_parser("sort", help="Sort levels by difficulty within size groups")
    p_sort.add_argument("--file", type=str, required=True, help="Input levels file")
    p_sort.add_argument(
        "--out", type=str, help="Output file (default: overwrite input)"
    )

    # pipeline (all-in-one)
    p_pipe = sub.add_parser("pipeline", help="Generate, solve, and sort in one step")
    p_pipe.add_argument("--boxes", type=int, required=True, help="Number of boxes")
    p_pipe.add_argument(
        "--sizes",
        type=str,
        default="25x19,35x19",
        help="Comma-separated room sizes",
    )
    p_pipe.add_argument(
        "--count", type=int, default=5, help="Levels per size (default: 5)"
    )
    p_pipe.add_argument(
        "--time-limit", type=int, default=60, help="Seconds per level (default: 60)"
    )
    p_pipe.add_argument("--out", type=str, required=True, help="Output file")

    args = parser.parse_args()
    if args.command == "generate":
        cmd_generate(args)
    elif args.command == "solve":
        cmd_solve(args)
    elif args.command == "sort":
        cmd_sort(args)
    elif args.command == "pipeline":
        cmd_pipeline(args)


if __name__ == "__main__":
    main()
