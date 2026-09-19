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


def test_reranker_adapter_appends_equals_form_and_identity(tmp_path: Path) -> None:
    adapter = Adapter.load(
        _write_adapter(
            tmp_path,
            dense="local",
            embedding_model="jinaai/jina-embeddings-v2-base-code",
            reranker="rozgo/bge-reranker-v2-m3",
        )
    )
    command = adapter.build_command(_case(), Path("/repo"))
    # `=` form: an optional-value flag must not consume a trailing token.
    assert command[-1] == "--reranker=rozgo/bge-reranker-v2-m3"
    assert (
        adapter.model_identity
        == "local:jinaai/jina-embeddings-v2-base-code+rerank:rozgo/bge-reranker-v2-m3"
    )


def test_reranker_only_identity(tmp_path: Path) -> None:
    adapter = Adapter.load(_write_adapter(tmp_path, reranker="BAAI/bge-reranker-base"))
    assert adapter.model_identity == "rerank:BAAI/bge-reranker-base"


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


def test_context_pack_preserves_verdict_and_delivery_report() -> None:
    from cce_research.adapters import normalize_payload

    payload = {
        "items": [
            {
                "id": "d1",
                "kind": "source",
                "title": "src/a.rs:1-5",
                "body": "…",
                "estimatedTokens": 40,
                "provenance": {
                    "route": "lexical",
                    "rank": 1,
                    "score": 1.0,
                    "snapshotId": "snap",
                    "verifiedCurrent": True,
                    "sourceAddress": {
                        "path": "src/a.rs",
                        "startLine": 1,
                        "endLine": 5,
                    },
                },
            }
        ],
        "searchVerdict": {"state": "weak_witness", "reasons": ["gap"]},
        "deliveryReport": {
            "includedItemIds": ["d1"],
            "omittedHits": [{"documentId": "d2", "reason": "budget"}],
            "deliveredTerms": ["zebra"],
            "missingTerms": ["koala"],
            "witnessVerification": "not_verified",
        },
        "usedTokens": 40,
    }
    normalized = normalize_payload(payload)
    assert normalized.result_kind == "context"
    # Both signal layers survive: the corpus verdict and the delivery gaps.
    assert normalized.verdict_state == "weak_witness"
    assert normalized.delivery_report is not None
    assert normalized.delivery_report["omittedHits"][0]["reason"] == "budget"
    assert normalized.delivery_report["missingTerms"] == ["koala"]


def test_context_pack_without_delivery_report_reads_none() -> None:
    from cce_research.adapters import normalize_payload

    normalized = normalize_payload({"items": [], "usedTokens": 0})
    assert normalized.result_kind == "context"
    assert normalized.delivery_report is None
    assert normalized.verdict_state is None
