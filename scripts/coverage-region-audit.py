#!/usr/bin/env python3
"""Inspect llvm-cov JSON output and summarize low-coverage region instantiations."""

from __future__ import annotations

import argparse
import json
from collections import defaultdict
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="summarize uncovered region-heavy function instantiations from llvm-cov json"
    )
    parser.add_argument(
        "coverage_json",
        type=Path,
        help="path to cargo llvm-cov report --json output",
    )
    parser.add_argument(
        "--file-suffix",
        action="append",
        default=[],
        help="restrict output to files ending with this suffix (repeatable)",
    )
    parser.add_argument(
        "--top",
        type=int,
        default=20,
        help="number of low-coverage function instantiations to print per file",
    )
    parser.add_argument(
        "--include-zero-call",
        action="store_true",
        help="include function instantiations with call count 0",
    )
    return parser.parse_args()


def file_allowed(filename: str, suffixes: list[str]) -> bool:
    if not suffixes:
        return True
    return any(filename.endswith(suffix) for suffix in suffixes)


def main() -> int:
    args = parse_args()
    data = json.loads(args.coverage_json.read_text())
    payload = data["data"][0]

    file_summaries = {}
    for file_entry in payload["files"]:
        filename = file_entry["filename"]
        if not file_allowed(filename, args.file_suffix):
            continue
        file_summaries[filename] = file_entry["summary"]["regions"]

    functions_by_file: dict[str, list[tuple[str, int, int, int]]] = defaultdict(list)
    for function in payload["functions"]:
        filenames = function.get("filenames", [])
        if not filenames:
            continue
        filename = filenames[0]
        if filename not in file_summaries:
            continue
        regions = function.get("regions", [])
        total = len(regions)
        if total == 0:
            continue
        uncovered = sum(1 for region in regions if region[4] == 0)
        if uncovered == 0:
            continue
        count = int(function.get("count", 0))
        if count == 0 and not args.include_zero_call:
            continue
        name = function.get("name", "<unknown>")
        functions_by_file[filename].append((name, uncovered, total, count))

    for filename in sorted(file_summaries):
        summary = file_summaries[filename]
        covered = int(summary["covered"])
        total = int(summary["count"])
        uncovered = int(summary["notcovered"])
        percent = float(summary["percent"])
        print(f"\n{filename}")
        print(
            f"  summary: covered={covered} total={total} uncovered={uncovered} percent={percent:.6f}"
        )

        rows = functions_by_file.get(filename, [])
        rows.sort(key=lambda row: (-row[1], -row[2], row[0]))
        if not rows:
            print("  no function instantiations with uncovered regions in selected data")
            continue
        print(f"  top {min(args.top, len(rows))} function instantiations with uncovered regions:")
        for name, uncovered_count, total_count, call_count in rows[: args.top]:
            print(
                f"    uncovered={uncovered_count:>4} total={total_count:>4} calls={call_count:>6} name={name}"
            )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
