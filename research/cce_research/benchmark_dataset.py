"""Verify that a metadata-only benchmark still matches its frozen source checkout."""

from __future__ import annotations

import argparse
import subprocess
from dataclasses import dataclass
from pathlib import Path, PurePosixPath

from .schema import BenchmarkCase, load_jsonl


@dataclass(frozen=True)
class DatasetSourceVerification:
    cases: int
    repositories: int
    revisions: int
    files: int
    symbols: int
    ranges: int


def verify_dataset_sources(dataset: Path, repository_root: Path) -> DatasetSourceVerification:
    """Fail closed when gold paths, symbols, ranges, or the pinned revision have drifted."""

    cases = list(load_jsonl(dataset, BenchmarkCase))
    if not cases:
        raise ValueError(f"benchmark dataset is empty: {dataset}")
    repositories = {case.repository for case in cases}
    revisions = {case.revision for case in cases}
    if len(repositories) != 1 or len(revisions) != 1:
        raise ValueError("source verification requires one repository and revision per dataset")
    expected_revision = next(iter(revisions))
    if expected_revision == "WORKTREE":
        raise ValueError("source verification requires an immutable revision, not WORKTREE")
    actual_revision = _git(repository_root, "rev-parse", "HEAD").strip()
    if actual_revision != expected_revision:
        raise ValueError(
            f"checkout revision {actual_revision} != dataset revision {expected_revision}"
        )

    checked_files: set[str] = set()
    symbol_count = 0
    range_count = 0
    for case in cases:
        paths = {
            *case.gold_files,
            *(item.path for item in case.gold_ranges),
            *(item.path for item in case.supporting_ranges),
        }
        contents: dict[str, str] = {}
        for relative in sorted(paths):
            source = _source_path(repository_root, relative)
            if not source.is_file():
                raise ValueError(f"{case.case_id}: gold source file does not exist: {relative}")
            contents[relative] = source.read_text(encoding="utf-8", errors="replace")
            checked_files.add(relative)
        joined_gold = "\n".join(contents[path] for path in case.gold_files)
        for symbol in case.gold_symbols:
            if symbol not in joined_gold:
                raise ValueError(f"{case.case_id}: gold symbol {symbol!r} absent from gold files")
            symbol_count += 1
        for item in (*case.gold_ranges, *case.supporting_ranges):
            line_count = len(contents[item.path].splitlines())
            if item.end_line > line_count:
                raise ValueError(
                    f"{case.case_id}: {item.path}:{item.end_line} exceeds {line_count} lines"
                )
            range_count += 1
    return DatasetSourceVerification(
        cases=len(cases),
        repositories=len(repositories),
        revisions=len(revisions),
        files=len(checked_files),
        symbols=symbol_count,
        ranges=range_count,
    )


def _source_path(repository_root: Path, relative: str) -> Path:
    path = PurePosixPath(relative)
    if path.is_absolute() or ".." in path.parts:
        raise ValueError(f"unsafe repository-relative path: {relative}")
    return repository_root.joinpath(*path.parts)


def _git(repository: Path, *arguments: str) -> str:
    completed = subprocess.run(
        ["git", *arguments],
        cwd=repository,
        check=True,
        capture_output=True,
        text=True,
        timeout=30,
    )
    return completed.stdout


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dataset", type=Path)
    parser.add_argument("repository", type=Path)
    arguments = parser.parse_args()
    summary = verify_dataset_sources(arguments.dataset, arguments.repository)
    print(
        f"verified {summary.cases} cases, {summary.files} files, "
        f"{summary.symbols} symbols, and {summary.ranges} ranges"
    )


if __name__ == "__main__":
    main()
