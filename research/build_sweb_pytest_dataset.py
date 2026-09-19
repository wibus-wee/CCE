"""Build benchmarks/datasets/sweb-pytest.jsonl from SWE-bench pytest instances.

External benchmark arm: real GitHub issue text (problem_statement) as the
query, fix-patch touched files as file-level golds — the standard
issue-localization labeling. Gold symbols are extracted from patch hunk
contexts and verified defined at the indexed checkout (pytest 7.4.4,
commit 33f694f4). gold_facts point at the AST range of verified fix
functions so `claim_support` is exercised deterministically.

Source rows: research/external/swebench-pytest-v7-rows.json (fetched from
princeton-nlp/SWE-bench test split via the HF datasets-server, filtered to
repo == pytest-dev/pytest and version 7.x so the issue text is
contemporaneous with the 7.4.4 checkout).

Run: uv run --project research python research/build_sweb_pytest_dataset.py
"""

from __future__ import annotations

import ast
import json
import re
import sys
from pathlib import Path

RESEARCH = Path(__file__).resolve().parent
REPO = RESEARCH / "external" / "pytest"
ROWS = RESEARCH / "external" / "swebench-pytest-v7-rows.json"
OUT = RESEARCH.parent / "benchmarks" / "datasets" / "sweb-pytest.jsonl"

sys.path.insert(0, str(RESEARCH))
from cce_research.schema import BenchmarkCase  # noqa: E402

REVISION = "33f694f4b30c5c502f21f81cb8ab907b12ad2f65"  # pytest 7.4.4
DATASET_REVISION = "sweb-pytest-v1"
QUERY_MAX = 6000
# Generic names whose symbol match says little about relevance.
GENERIC_SYMBOLS = {"__init__", "__call__", "runtest", "parse", "pytest_configure"}

SELECTED = [
    # version 7.4
    "pytest-dev__pytest-10893",
    "pytest-dev__pytest-11041",
    "pytest-dev__pytest-11044",
    "pytest-dev__pytest-11047",
    "pytest-dev__pytest-11125",
    # version 7.2
    "pytest-dev__pytest-10051",
    "pytest-dev__pytest-10081",
    "pytest-dev__pytest-10115",
    "pytest-dev__pytest-10343",
    "pytest-dev__pytest-10356",
    "pytest-dev__pytest-10442",
    "pytest-dev__pytest-10482",
    "pytest-dev__pytest-10552",
    "pytest-dev__pytest-10624",
    "pytest-dev__pytest-9780",
    "pytest-dev__pytest-9956",
    # version 7.0
    "pytest-dev__pytest-8861",
    "pytest-dev__pytest-8950",
    "pytest-dev__pytest-8952",
    "pytest-dev__pytest-8987",
    "pytest-dev__pytest-9064",
    "pytest-dev__pytest-9359",
    # version 7.1
    "pytest-dev__pytest-9475",
    "pytest-dev__pytest-9646",
    "pytest-dev__pytest-9681",
    "pytest-dev__pytest-9709",
]

# Cases that withhold intent so the system's own classifier is scored.
WITHHOLD_INTENT = {
    "pytest-dev__pytest-10893",
    "pytest-dev__pytest-10115",
    "pytest-dev__pytest-10482",
    "pytest-dev__pytest-8987",
    "pytest-dev__pytest-9475",
    "pytest-dev__pytest-9709",
}


def patch_files(patch: str) -> list[str]:
    seen: set[str] = set()
    out: list[str] = []
    for match in re.finditer(r"^diff --git a/(\S+) b/(\S+)", patch, re.M):
        path = match.group(2)
        if path not in seen:
            seen.add(path)
            out.append(path)
    return out


def file_defs(path: str) -> dict[str, tuple[int, int]]:
    """name -> (lineno, end_lineno) for defs/classes in the checkout."""
    source = (REPO / path).read_text(encoding="utf-8")
    tree = ast.parse(source)
    defs: dict[str, tuple[int, int]] = {}
    for node in ast.walk(tree):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            defs.setdefault(node.name, (node.lineno, node.end_lineno or node.lineno))
    return defs


def hunk_symbols(patch: str) -> list[str]:
    """Symbols named by patch hunk contexts, plus defs the patch adds."""
    out: list[str] = []
    for match in re.finditer(r"^@@ -\d+(?:,\d+)? \+\d+(?:,\d+)? @@\s*(.*)$", patch, re.M):
        context = match.group(1)
        found = re.search(r"\bdef\s+(\w+)|\bclass\s+(\w+)|\basync\s+def\s+(\w+)", context)
        if found:
            out.append(next(group for group in found.groups() if group))
    for match in re.finditer(
        r"^\+\s*(?:async\s+)?def\s+(\w+)|^\+\s*class\s+(\w+)", patch, re.M
    ):
        out.append(next(group for group in match.groups() if group))
    seen: set[str] = set()
    result: list[str] = []
    for symbol in out:
        if symbol not in seen:
            seen.add(symbol)
            result.append(symbol)
    return result


def truncate(text: str) -> str:
    text = text.strip()
    if len(text) <= QUERY_MAX:
        return text
    cut = text[:QUERY_MAX]
    # Prefer ending on a line boundary.
    if "\n" in cut[QUERY_MAX - 200 :]:
        cut = cut[: cut.rfind("\n")]
    return cut + "\n[... issue truncated ...]"


def main() -> None:
    rows = {row["instance_id"]: row for row in json.loads(ROWS.read_text(encoding="utf-8"))}
    missing = [iid for iid in SELECTED if iid not in rows]
    if missing:
        raise SystemExit(f"instances missing from source rows: {missing}")

    cases: list[BenchmarkCase] = []
    seen_queries: set[str] = set()
    for iid in SELECTED:
        row = rows[iid]
        issue = iid.rsplit("-", 1)[-1]
        query = truncate(row["problem_statement"])
        if query[:200] in seen_queries:
            print(f"SKIP {iid}: duplicate query")
            continue
        seen_queries.add(query[:200])

        files = patch_files(row["patch"])
        absent = [f for f in files if not (REPO / f).is_file()]
        if absent:
            print(f"SKIP {iid}: gold files absent at checkout {absent}")
            continue

        defs_per_file = {f: file_defs(f) for f in files}
        verified: list[tuple[str, str, int, int]] = []  # symbol, path, start, end
        for symbol in hunk_symbols(row["patch"]):
            for path in files:
                if symbol in defs_per_file[path]:
                    start, end = defs_per_file[path][symbol]
                    verified.append((symbol, path, start, end))
                    break

        gold_symbols = [
            symbol
            for symbol, _, _, _ in verified
            if symbol not in GENERIC_SYMBOLS
        ][:5]

        # Claim-level gold: the fix lives inside these verified functions.
        facts = []
        for symbol, path, start, end in verified:
            if symbol in GENERIC_SYMBOLS:
                continue
            facts.append(
                {
                    "claim": (
                        f"the fix for this issue is implemented in `{symbol}` in `{path}`"
                    ),
                    "evidence": [
                        {"path": path, "start_line": start, "end_line": end, "symbol": symbol}
                    ],
                    "symbols": [symbol],
                }
            )
            if len(facts) == 2:
                break

        case = BenchmarkCase(
            case_id=f"sweb-pytest-{issue}",
            repository="pytest-dev/pytest",
            revision=REVISION,
            query=query,
            intent="issue_localization",
            gold_files=files,
            gold_symbols=gold_symbols,
            gold_facts=facts,
            no_context=False,
            supply_intent=iid not in WITHHOLD_INTENT,
            provenance={
                "source_url": f"https://github.com/pytest-dev/pytest/pull/{issue}",
                "dataset_revision": DATASET_REVISION,
                "license_spdx": "MIT",
                "redistribution": "allowed",
                "construction_method": (
                    f"SWE-bench test instance {iid} (pytest {row['version']}): "
                    "query = GitHub problem_statement"
                    + (" truncated to 6000 chars" if len(row["problem_statement"]) > QUERY_MAX else "")
                    + "; gold_files = fix-patch touched paths, verified present at "
                    "checkout 7.4.4; gold_symbols = patch hunk-context symbols verified "
                    "defined at checkout via ast; gold_facts evidence = ast ranges of "
                    "verified fix functions"
                ),
            },
            tags=["swebench", "external", "pytest", f"pytest-{row['version']}"],
        )
        cases.append(case)
        print(
            f"{case.case_id}: files={files} syms={gold_symbols} "
            f"facts={len(facts)} qlen={len(query)}"
        )

    cases.append(
        BenchmarkCase(
            case_id="sweb-pytest-negative-control",
            repository="pytest-dev/pytest",
            revision=REVISION,
            query=(
                "Where is the hyperdrive calibration routine that syncs the "
                "flux compensator before test collection?"
            ),
            intent="issue_localization",
            no_context=True,
            provenance={
                "source_url": "https://github.com/pytest-dev/pytest",
                "dataset_revision": DATASET_REVISION,
                "license_spdx": "MIT",
                "redistribution": "allowed",
                "construction_method": (
                    "hand-authored negative control using concepts absent from the "
                    "pytest repository (mirrors self-no-context-control)"
                ),
            },
            tags=["external", "negative-control", "abstention"],
        )
    )

    OUT.write_text(
        "".join(case.model_dump_json() + "\n" for case in cases), encoding="utf-8"
    )
    print(f"\nwrote {len(cases)} cases -> {OUT}")


if __name__ == "__main__":
    main()
