from pathlib import Path

import pytest
import yaml

from cce_research.adapters import Adapter
from cce_research.schema import BenchmarkCase, Provenance


def _case(**overrides: object) -> BenchmarkCase:
    fields: dict[str, object] = {
        "case_id": "c1",
        "repository": "example/repo",
        "revision": "WORKTREE",
        "query": "q",
        "intent": "impact",
        "gold_files": ["src/a.rs"],
        "provenance": Provenance(
            source_url="https://example.com/repo",
            dataset_revision="test-v1",
            license_spdx="Apache-2.0",
            redistribution="allowed",
            construction_method="test fixture",
        ),
    }
    fields.update(overrides)
    return BenchmarkCase.model_validate(fields)


def _write_adapter(tmp_path: Path, **extra: object) -> Path:
    raw = {
        "name": "test-adapter",
        "command": ["cce", "--json", "search", "{repository}", "{query}", "{intent_args}"],
        "environment": {},
    }
    raw.update(extra)
    path = tmp_path / "adapter.yaml"
    path.write_text(yaml.safe_dump(raw), encoding="utf-8")
    return path


def test_adapter_without_dense_keys_loads(tmp_path: Path) -> None:
    adapter = Adapter.load(_write_adapter(tmp_path))
    assert adapter.dense is None
    assert adapter.embedding_model is None
    assert adapter.model_identity == "none"


def test_dense_adapter_appends_flags_and_derives_identity(tmp_path: Path) -> None:
    adapter = Adapter.load(
        _write_adapter(
            tmp_path,
            dense="local",
            embedding_model="intfloat/multilingual-e5-base",
        )
    )
    command = adapter.build_command(_case(), Path("/repo"))
    assert command[-4:] == [
        "--dense",
        "local",
        "--embedding-model",
        "intfloat/multilingual-e5-base",
    ]
    assert adapter.model_identity == "local:intfloat/multilingual-e5-base"


def test_embedding_model_without_dense_is_rejected(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="require a dense backend"):
        Adapter.load(_write_adapter(tmp_path, embedding_model="intfloat/multilingual-e5-base"))


def test_dense_flags_do_not_disturb_template_expansion(tmp_path: Path) -> None:
    adapter = Adapter.load(
        _write_adapter(
            tmp_path,
            command=[
                "cce",
                "--json",
                "search",
                "{repository}",
                "{query}",
                "{intent_args}",
                "{route_args}",
            ],
            dense="local",
        )
    )
    case = _case(supply_intent=False, routes=["lexical", "dense_summary"])
    command = adapter.build_command(case, Path("/repo"))
    # Withheld intent expands to nothing; routes expand in place; dense flag
    # lands at the end without reordering the template output.
    assert command[:5] == ["cce", "--json", "search", "/repo", "q"]
    assert command[5:9] == ["--route", "lexical", "--route", "dense_summary"]
    assert command[-2:] == ["--dense", "local"]
