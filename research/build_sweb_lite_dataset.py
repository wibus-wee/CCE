"""Build per-repo SWE-bench-Lite issue-localization datasets.

Extends the sweb-pytest arm to the five largest SWE-bench_Lite repositories.
Real GitHub issue text (problem_statement) is the query; the fix-patch's
touched files are file-level golds — the standard issue-localization
labeling, no hand-labeling required. Gold symbols are extracted from patch
hunk contexts and verified defined at the indexed checkout via `ast`.
gold_facts point at the AST range of verified fix functions so
`claim_support` is exercised deterministically.

One representative checkout is indexed per repository (the newest version
tag present in the case set); gold paths are verified present at that
checkout and cases whose golds are absent are skipped — the same
version-skew caveat as the sweb-pytest arm.

Source rows: research/external/sweb-lite-rows.json (princeton-nlp/
SWE-bench_Lite test split via the HF datasets-server, 300 instances).

Run: uv run --project research python research/build_sweb_lite_dataset.py
"""

from __future__ import annotations

import ast
import json
import re
import sys
from pathlib import Path

RESEARCH = Path(__file__).resolve().parent
ROWS = RESEARCH / "external" / "sweb-lite-rows.json"
OUT_DIR = RESEARCH.parent / "benchmarks" / "datasets"

sys.path.insert(0, str(RESEARCH))
from cce_research.schema import BenchmarkCase  # noqa: E402

# repo slug -> (checkout dir name, revision label, tag used for the checkout)
REPOS = {
    "django/django": ("django", "5.0.9"),
    "sympy/sympy": ("sympy", "1.13.3"),
    "matplotlib/matplotlib": ("matplotlib", "v3.7.5"),
    "scikit-learn/scikit-learn": ("scikit-learn", "0.22.2.post1"),
    "sphinx-doc/sphinx": ("sphinx", "v7.1.2"),
    "pytest-dev/pytest": ("pytest", "7.4.4"),
}

DATASET_REVISION = "sweb-lite-v1"
QUERY_MAX = 6000
GENERIC_SYMBOLS = {"__init__", "__call__", "runtest", "parse", "pytest_configure"}
LICENSES = {
    "django/django": "BSD-3-Clause",
    "sympy/sympy": "BSD-3-Clause",
    "matplotlib/matplotlib": "PSF",
    "scikit-learn/scikit-learn": "BSD-3-Clause",
    "sphinx-doc/sphinx": "BSD-2-Clause",
    "pytest-dev/pytest": "MIT",
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


def file_defs(path: Path) -> dict[str, tuple[int, int]]:
    source = path.read_text(encoding="utf-8")
    tree = ast.parse(source)
    defs: dict[str, tuple[int, int]] = {}
    for node in ast.walk(tree):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            defs.setdefault(node.name, (node.lineno, node.end_lineno or node.lineno))
    return defs


def hunk_symbols(patch: str) -> list[str]:
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
    if "\n" in cut[QUERY_MAX - 200 :]:
        cut = cut[: cut.rfind("\n")]
    return cut + "\n[... issue truncated ...]"


def checkout_revision(root: Path, tag: str) -> str:
    try:
        import subprocess

        out = subprocess.run(
            ["git", "rev-parse", "HEAD"], cwd=root, capture_output=True, text=True
        )
        if out.returncode == 0:
            return out.stdout.strip()
    except OSError:
        pass
    return tag


def build_repo(repo_slug: str, rows: list[dict]) -> list[BenchmarkCase]:
    dirname, tag = REPOS[repo_slug]
    root = RESEARCH / "external" / dirname
    revision = checkout_revision(root, tag)
    short = dirname.replace("-", "")
    cases: list[BenchmarkCase] = []
    seen_queries: set[str] = set()
    for index, row in enumerate(rows):
        iid = row["instance_id"]
        issue = iid.rsplit("-", 1)[-1]
        query = truncate(row["problem_statement"])
        if query[:200] in seen_queries:
            print(f"  SKIP {iid}: duplicate query")
            continue
        seen_queries.add(query[:200])

        files = patch_files(row["patch"])
        absent = [f for f in files if not (root / f).is_file()]
        if absent:
            print(f"  SKIP {iid}: gold files absent at checkout {absent}")
            continue

        try:
            defs_per_file = {f: file_defs(root / f) for f in files}
        except (SyntaxError, UnicodeDecodeError) as exc:
            print(f"  SKIP {iid}: unparseable gold file ({exc})")
            continue
        verified: list[tuple[str, str, int, int]] = []
        for symbol in hunk_symbols(row["patch"]):
            for path in files:
                if symbol in defs_per_file[path]:
                    start, end = defs_per_file[path][symbol]
                    verified.append((symbol, path, start, end))
                    break

        gold_symbols = [
            symbol for symbol, _, _, _ in verified if symbol not in GENERIC_SYMBOLS
        ][:5]

        facts = []
        for symbol, path, start, end in verified:
            if symbol in GENERIC_SYMBOLS:
                continue
            facts.append(
                {
                    "claim": f"the fix for this issue is implemented in `{symbol}` in `{path}`",
                    "evidence": [
                        {"path": path, "start_line": start, "end_line": end, "symbol": symbol}
                    ],
                    "symbols": [symbol],
                }
            )
            if len(facts) == 2:
                break

        cases.append(
            BenchmarkCase(
                case_id=f"sweb-{short}-{issue}",
                repository=repo_slug,
                revision=revision,
                query=query,
                intent="issue_localization",
                gold_files=files,
                gold_symbols=gold_symbols,
                gold_facts=facts,
                no_context=False,
                supply_intent=index % 5 != 4,
                provenance={
                    "source_url": f"https://github.com/{repo_slug}/pull/{issue}",
                    "dataset_revision": DATASET_REVISION,
                    "license_spdx": LICENSES[repo_slug],
                    "redistribution": "allowed",
                    "construction_method": (
                        f"SWE-bench_Lite test instance {iid} (version "
                        f"{row['version']}): query = GitHub problem_statement"
                        + (
                            " truncated to 6000 chars"
                            if len(row["problem_statement"]) > QUERY_MAX
                            else ""
                        )
                        + f"; gold_files = fix-patch touched paths, verified present at "
                        f"checkout {revision} ({tag}); gold_symbols = patch hunk-context symbols "
                        f"verified defined at checkout via ast; gold_facts evidence = "
                        f"ast ranges of verified fix functions"
                    ),
                },
                tags=["swebench", "swebench-lite", "external", short],
            )
        )
    return cases


def main() -> None:
    rows = json.loads(ROWS.read_text(encoding="utf-8"))
    by_repo: dict[str, list[dict]] = {}
    for row in rows:
        by_repo.setdefault(row["repo"], []).append(row)

    for slug, (dirname, tag) in REPOS.items():
        repo_rows = by_repo.get(slug, [])
        if not repo_rows:
            continue
        print(f"{slug} @ {tag}: {len(repo_rows)} source rows")
        cases = build_repo(slug, repo_rows)
        out = OUT_DIR / f"sweb-lite-{dirname}.jsonl"
        out.write_text(
            "".join(case.model_dump_json() + "\n" for case in cases), encoding="utf-8"
        )
        print(f"  wrote {len(cases)} cases -> {out}")


if __name__ == "__main__":
    main()
