from __future__ import annotations

import hashlib
import json

import pytest

from cce_research.model_benchmark import evaluate_ranks
from cce_research.schema import TrainingDocument, TrainingExample
from cce_research.train_reranker import (
    promote_reranker_bundle,
    reranker_rows,
)
from cce_research.train_retriever import TrainingRecipe, training_row, validate_splits
from cce_research.training_data import (
    dense_document_text,
    mine_hard_negatives,
    preceding_comment,
    split_identifier,
    terms,
)


def document(identifier: str, repository: str = "repo") -> TrainingDocument:
    text = dense_document_text(
        f"src/{identifier}.rs", f"fn {identifier}() {{ perform_repository_operation(); }}"
    )
    return TrainingDocument(
        document_id=identifier,
        repository=repository,
        revision="a" * 40,
        path=f"src/{identifier}.rs",
        symbol=identifier,
        language="rust",
        start_byte=0,
        end_byte=len(text.encode()),
        text=text,
        content_sha256=hashlib.sha256(text.encode()).hexdigest(),
    )


def example(identifier: str, repository: str = "repo") -> TrainingExample:
    return TrainingExample(
        example_id=f"example_{identifier}",
        dataset_revision="dataset-v1",
        split="train",
        query=f"find {identifier}",
        query_kind="symbol_navigation",
        positive=document(identifier, repository),
        negatives=[document(f"negative_{identifier}", repository)],
    )


def test_training_row_preserves_runtime_prefixes_and_explicit_negatives() -> None:
    recipe = TrainingRecipe(
        base_model="model",
        base_revision="b" * 40,
        code_revision=None,
        dataset_directory="dataset",
        output_directory="output",
        same_repository_hard_negatives=1,
    )
    row = training_row(example("target"), recipe)
    assert row["query"].startswith("query: ")
    assert row["positive"].startswith("passage: path: src/target.rs")
    assert "negative_1" in row


def test_repository_split_leakage_is_rejected() -> None:
    with pytest.raises(ValueError, match="repository leakage"):
        validate_splits([example("train")], [example("validation")], 1)


def test_hard_negative_mining_is_repository_local_and_deterministic() -> None:
    documents = [document("target"), document("target_cache"), document("unrelated")]
    index: dict[str, set[int]] = {}
    for position, candidate in enumerate(documents):
        for term in terms(candidate.text):
            index.setdefault(term, set()).add(position)
    first = mine_hard_negatives("target cache operation", 0, documents, index, 2)
    second = mine_hard_negatives("target cache operation", 0, documents, index, 2)
    assert [item.document_id for item in first] == [item.document_id for item in second]
    assert all(item.document_id != "target" for item in first)


def test_rank_metrics_reward_earlier_retrieval() -> None:
    perfect = evaluate_ranks([1, 1])
    weaker = evaluate_ranks([2, 10])
    assert perfect["mrr"] > weaker["mrr"]
    assert perfect["ndcg@10"] > weaker["ndcg@10"]
    assert perfect["recall@1"] == 1.0


def test_identifier_splitting_matches_code_conventions() -> None:
    assert split_identifier("workspaceOverlayHash") == ["workspace", "overlay", "hash"]
    assert split_identifier("source_address") == ["source", "address"]


def test_preceding_c_documentation_is_recovered_without_code_generation() -> None:
    assert (
        preceding_comment("int prior;\n/** Open the database and validate its header. */\n")
        == "Open the database and validate its header."
    )


def test_reranker_rows_pair_each_query_with_positive_and_hard_negative() -> None:
    rows = reranker_rows([example("target")], 1)
    assert [row["label"] for row in rows] == [1.0, 0.0]
    assert all(row["query"] == "find target" for row in rows)
    assert "path: src/target.rs" in str(rows[0]["document"])


def test_promoted_reranker_bundle_is_hash_bound(tmp_path) -> None:
    source = tmp_path / "source"
    for relative in [
        "config.json",
        "special_tokens_map.json",
        "tokenizer.json",
        "tokenizer_config.json",
        "onnx/model.onnx",
    ]:
        path = source / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(relative, encoding="utf-8")
    output = tmp_path / "output"
    summary = promote_reranker_bundle(
        source,
        output,
        "jinaai/jina-reranker-v1-turbo-en",
        "a" * 40,
    )
    manifest = json.loads((output / "cce-reranker-manifest.json").read_text())
    assert manifest["bundleRevision"] == summary.bundle_revision
    assert len(manifest["files"]) == 5
