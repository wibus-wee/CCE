from __future__ import annotations

import json
import re
import subprocess
from pathlib import Path

import pytest
import yaml

from cce_research.benchmark_dataset import verify_dataset_sources
from cce_research.schema import BenchmarkCase, load_jsonl
from cce_research.training_data import load_corpus_manifest


def test_production_corpus_is_repository_held_out_and_stratified() -> None:
    root = Path(__file__).parents[2]
    manifest = load_corpus_manifest(root / "research/corpora/production-repositories.yaml")
    assert manifest.split_policy.repository_held_out
    assert manifest.split_policy.future_feedback_evaluation_only
    assert {repository.split for repository in manifest.repositories} == {
        "train",
        "validation",
        "test",
    }
    assert {repository.scale_tier for repository in manifest.repositories} == {
        "small",
        "medium",
        "large",
        "enterprise",
    }
    languages = {
        language for repository in manifest.repositories for language in repository.languages
    }
    assert {"javascript", "typescript", "rust"} <= languages
    assert any(len(repository.languages) > 1 for repository in manifest.repositories)
    assert all(
        re.fullmatch(r"[0-9a-f]{40}", repository.revision) for repository in manifest.repositories
    )


def test_external_benchmarks_are_frozen_and_distinguish_completion_proxies() -> None:
    root = Path(__file__).parents[2]
    payload = yaml.safe_load((root / "benchmarks/external-datasets.yaml").read_text())
    assert payload["policy"]["evaluation_only"]
    manifest = load_corpus_manifest(root / "research/corpora/production-repositories.yaml")
    excluded = {
        repository.lower() for repository in payload["policy"]["excluded_training_repositories"]
    }
    trained = {
        repository.url.removesuffix(".git").removeprefix("https://github.com/").lower()
        for repository in manifest.repositories
        if repository.split == "train"
    }
    assert trained.isdisjoint(excluded)
    datasets = {item["short_name"]: item for item in payload["datasets"]}
    assert datasets["arb-v2"]["samples"] == 427
    assert datasets["swe-explore-v1"]["samples"] == 848
    assert datasets["crosscodeeval"]["classification"] == "secondary_completion_proxy"
    assert datasets["repobench-v1.1"]["classification"] == "secondary_completion_proxy"
    assert all(
        re.fullmatch(r"[0-9a-f]{40}", item["source_revision"]) for item in payload["datasets"]
    )


def test_redux_toolkit_suite_covers_production_retrieval_intents() -> None:
    root = Path(__file__).parents[2]
    cases = list(load_jsonl(root / "benchmarks/datasets/redux-toolkit-js-ts.jsonl", BenchmarkCase))
    assert {"exact_entity", "architecture", "history", "impact", "precise_dataflow"} <= {
        case.intent for case in cases
    }
    assert any(case.no_context for case in cases)
    assert len({case.revision for case in cases}) == 1


def test_metadata_dataset_can_be_verified_against_pinned_source(tmp_path: Path) -> None:
    repository = tmp_path / "repository"
    repository.mkdir()
    source = repository / "src/engine.ts"
    source.parent.mkdir()
    source.write_text("export function buildEngine() { return 1 }\n", encoding="utf-8")
    subprocess.run(["git", "init", "-q"], cwd=repository, check=True)
    subprocess.run(["git", "add", "."], cwd=repository, check=True)
    subprocess.run(
        [
            "git",
            "-c",
            "user.name=CCE Test",
            "-c",
            "user.email=cce@example.invalid",
            "commit",
            "-qm",
            "fixture",
        ],
        cwd=repository,
        check=True,
    )
    revision = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=repository,
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    case = {
        "case_id": "fixture-exact",
        "repository": "fixture/repository",
        "revision": revision,
        "query": "buildEngine",
        "intent": "exact_entity",
        "gold_files": ["src/engine.ts"],
        "gold_symbols": ["buildEngine"],
        "gold_ranges": [],
        "supporting_ranges": [],
        "no_context": False,
        "budget_tokens": 1024,
        "provenance": {
            "source_url": "https://example.invalid/fixture/repository",
            "dataset_revision": "fixture-v1",
            "license_spdx": "MIT",
            "redistribution": "metadata_only",
            "construction_method": "unit-test fixture",
        },
        "tags": ["fixture"],
    }
    dataset = tmp_path / "dataset.jsonl"
    dataset.write_text(json.dumps(case) + "\n", encoding="utf-8")
    summary = verify_dataset_sources(dataset, repository)
    assert summary.cases == 1
    assert summary.files == 1
    assert summary.symbols == 1

    case["gold_symbols"] = ["missingSymbol"]
    dataset.write_text(json.dumps(case) + "\n", encoding="utf-8")
    with pytest.raises(ValueError, match="missingSymbol"):
        verify_dataset_sources(dataset, repository)
