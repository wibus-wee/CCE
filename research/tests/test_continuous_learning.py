from __future__ import annotations

import json
import sqlite3
from pathlib import Path

from cce_research.continuous_learning import (
    _snapshot_sqlite,
    export_feedback_dataset,
    hash_tree,
    promote_if_benchmark_passes,
)
from cce_research.schema import TrainingExample, load_jsonl


def _write_artifact(root: Path, digest: str, body: bytes) -> str:
    relative = f"artifacts/blake3/{digest[:2]}/{digest[2:]}"
    path = root / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(body)
    return relative


def _feedback_database(root: Path) -> None:
    root.mkdir()
    database = sqlite3.connect(root / "metadata.sqlite")
    database.executescript(
        """
        CREATE TABLE repositories (
          id TEXT PRIMARY KEY, canonical_root TEXT NOT NULL
        );
        CREATE TABLE snapshots (
          id TEXT PRIMARY KEY, repository_id TEXT NOT NULL, base_revision TEXT
        );
        CREATE TABLE artifacts (
          digest TEXT PRIMARY KEY, relative_path TEXT NOT NULL
        );
        CREATE TABLE trajectories (
          id TEXT PRIMARY KEY, repository_id TEXT NOT NULL, snapshot_id TEXT NOT NULL,
          query TEXT NOT NULL, intent TEXT NOT NULL, artifact_digest TEXT NOT NULL,
          created_at TEXT NOT NULL
        );
        CREATE TABLE learning_events (
          id TEXT PRIMARY KEY, trajectory_id TEXT NOT NULL, stage TEXT NOT NULL,
          document_id TEXT, dwell_ms INTEGER, metadata_json TEXT NOT NULL,
          created_at TEXT NOT NULL
        );
        CREATE TABLE entities (
          id TEXT NOT NULL, snapshot_id TEXT NOT NULL, name TEXT, language TEXT,
          PRIMARY KEY (snapshot_id, id)
        );
        CREATE TABLE retrieval_documents (
          id TEXT NOT NULL, snapshot_id TEXT NOT NULL, entity_id TEXT NOT NULL,
          body_artifact_digest TEXT NOT NULL, address_json TEXT,
          PRIMARY KEY (snapshot_id, id)
        );
        CREATE TABLE source_files (
          snapshot_id TEXT NOT NULL, path TEXT NOT NULL, language TEXT,
          artifact_digest TEXT NOT NULL, PRIMARY KEY (snapshot_id, path)
        );
        """
    )
    database.execute("INSERT INTO repositories VALUES ('repo', '/local/private/repo')")
    database.execute("INSERT INTO snapshots VALUES ('snapshot', 'repo', ?)", ("a" * 40,))
    documents = {
        "positive": b"path: src/positive.ts\nsymbol: positive\nexport function positive() {}",
        "negative": b"path: src/negative.ts\nsymbol: negative\nexport function negative() {}",
    }
    hits = []
    for rank, (identifier, body) in enumerate(documents.items(), start=1):
        digest = ("1" if identifier == "positive" else "2") * 64
        relative = _write_artifact(root, digest, body)
        database.execute("INSERT INTO artifacts VALUES (?, ?)", (digest, relative))
        database.execute(
            "INSERT INTO entities VALUES (?, 'snapshot', ?, 'typescript')",
            (f"entity-{identifier}", identifier),
        )
        address = {
            "repositoryId": "repo",
            "snapshotId": "snapshot",
            "path": f"src/{identifier}.ts",
            "startByte": 0,
            "endByte": len(body),
            "startLine": 1,
            "endLine": 2,
        }
        database.execute(
            "INSERT INTO retrieval_documents VALUES (?, 'snapshot', ?, ?, ?)",
            (identifier, f"entity-{identifier}", digest, json.dumps(address)),
        )
        hits.append(
            {
                "documentId": identifier,
                "entityId": f"entity-{identifier}",
                "symbolName": identifier,
                "rank": rank,
                "address": address,
                "snippet": "truncated and must not be exported",
            }
        )
    for trajectory_id in ("labeled", "no-rejection", "raw-only"):
        trace_digest = {"labeled": "3", "no-rejection": "4", "raw-only": "5"}[trajectory_id] * 64
        trace_relative = _write_artifact(
            root,
            trace_digest,
            json.dumps({"trajectoryId": trajectory_id, "hits": hits}).encode(),
        )
        database.execute("INSERT INTO artifacts VALUES (?, ?)", (trace_digest, trace_relative))
        database.execute(
            "INSERT INTO trajectories VALUES (?, 'repo', 'snapshot', ?, ?, ?, ?)",
            (
                trajectory_id,
                "where is the positive implementation?",
                json.dumps("architecture"),
                trace_digest,
                "2026-08-10T00:00:00+00:00",
            ),
        )
    events = [
        ("e1", "labeled", "shown_to_model", "positive"),
        ("e2", "labeled", "opened_by_agent", "positive"),
        ("e3", "labeled", "cited_or_used", "positive"),
        ("e4", "labeled", "rejected", "negative"),
        ("e5", "no-rejection", "edited_or_affected", "positive"),
        ("e6", "no-rejection", "shown_to_model", "negative"),
        ("e7", "raw-only", "opened_by_agent", "positive"),
        ("e8", "raw-only", "rejected", "negative"),
    ]
    for event_id, trajectory_id, stage, document_id in events:
        database.execute(
            "INSERT INTO learning_events VALUES (?, ?, ?, ?, NULL, '{}', ?)",
            (
                event_id,
                trajectory_id,
                json.dumps(stage),
                document_id,
                "2026-08-10T00:01:00+00:00",
            ),
        )
    database.commit()
    database.close()


def test_feedback_export_uses_only_explicit_high_confidence_labels(tmp_path: Path) -> None:
    data_root = tmp_path / "data"
    _feedback_database(data_root)
    output = tmp_path / "feedback.jsonl"
    summary = export_feedback_dataset([data_root], output, cutoff="2026-08-11T00:00:00+00:00")
    examples = list(load_jsonl(output, TrainingExample))
    assert summary.examples == 1
    assert summary.skipped_without_positive == 1
    assert summary.skipped_without_explicit_negative == 1
    assert examples[0].positive.document_id == "positive"
    assert [item.document_id for item in examples[0].negatives] == ["negative"]
    assert "export function positive" in examples[0].positive.text
    assert "truncated" not in examples[0].positive.text
    assert examples[0].learning_provenance is not None
    assert examples[0].learning_provenance.positive_stage == "cited_or_used"
    manifest = json.loads(Path(summary.manifest).read_text())
    assert manifest["localOnly"] is True
    assert manifest["datasetSha256"] == summary.dataset_sha256
    assert manifest["labelPolicy"]["rawTelemetryOnlyStages"] == [
        "shown_to_model",
        "opened_by_agent",
    ]
    assert manifest["eventCounts"]["shown_to_model"] == 2

    database = sqlite3.connect(data_root / "metadata.sqlite")
    database.execute(
        "INSERT INTO learning_events VALUES ('future', 'labeled', ?, 'negative', NULL, '{}', ?)",
        (json.dumps("opened_by_agent"), "2026-08-12T00:00:00+00:00"),
    )
    database.commit()
    database.close()
    repeated_output = tmp_path / "feedback-repeated.jsonl"
    repeated = export_feedback_dataset(
        [data_root], repeated_output, cutoff="2026-08-11T00:00:00+00:00"
    )
    assert repeated.dataset_revision == summary.dataset_revision
    assert repeated.dataset_sha256 == summary.dataset_sha256


def test_sqlite_snapshot_includes_committed_wal_pages(tmp_path: Path) -> None:
    source = tmp_path / "source.sqlite"
    writer = sqlite3.connect(source)
    writer.execute("PRAGMA journal_mode=WAL")
    writer.execute("PRAGMA wal_autocheckpoint=0")
    writer.execute("CREATE TABLE events(value TEXT)")
    writer.execute("INSERT INTO events VALUES ('committed-in-wal')")
    writer.commit()
    assert source.with_name(source.name + "-wal").is_file()
    snapshot = tmp_path / "snapshot.sqlite"
    _snapshot_sqlite(source, snapshot)
    reader = sqlite3.connect(snapshot)
    assert reader.execute("SELECT value FROM events").fetchone() == ("committed-in-wal",)
    reader.close()
    writer.close()


def _benchmark(path: Path, *, quality: float, latency: float, memory: int) -> None:
    measurement = {
        "model": path.stem,
        "recall_at_1": quality,
        "recall_at_5": quality,
        "recall_at_10": quality,
        "mrr": quality,
        "ndcg_at_10": quality,
        "metrics_by_query_kind": {"documentation": {"mrr": quality}},
        "single_query_p95_ms": latency,
        "peak_memory_bytes": memory,
        "documents_per_second": 100.0,
    }
    path.write_text(
        json.dumps(
            {
                "datasetSha256": "d" * 64,
                "hardware": {"machine": "test"},
                "measurements": [measurement],
            }
        )
    )


def test_promotion_is_atomic_and_regressions_do_not_replace_champion(tmp_path: Path) -> None:
    champion = tmp_path / "champion.json"
    challenger = tmp_path / "challenger.json"
    _benchmark(champion, quality=0.8, latency=100, memory=1_000)
    _benchmark(challenger, quality=0.82, latency=105, memory=1_050)
    candidate = tmp_path / "candidate"
    candidate.mkdir()
    (candidate / "model.onnx").write_bytes(b"candidate-one")
    registry = tmp_path / "registry"
    passed = promote_if_benchmark_passes(champion, challenger, candidate, registry)
    assert passed.promoted is True
    assert passed.candidate_revision == hash_tree(candidate)
    current_before = (registry / "current.json").read_bytes()
    current = json.loads(current_before)
    assert (registry / current["release"] / "bundle" / "model.onnx").is_file()

    regressed = tmp_path / "regressed.json"
    _benchmark(regressed, quality=0.7, latency=150, memory=2_000)
    second_candidate = tmp_path / "candidate-two"
    second_candidate.mkdir()
    (second_candidate / "model.onnx").write_bytes(b"candidate-two")
    failed = promote_if_benchmark_passes(champion, regressed, second_candidate, registry)
    assert failed.promoted is False
    assert failed.failures
    assert (registry / "current.json").read_bytes() == current_before
    audit = json.loads(Path(failed.audit_manifest).read_text())
    assert audit["promoted"] is False
    assert audit["candidateRevision"] == hash_tree(second_candidate)
