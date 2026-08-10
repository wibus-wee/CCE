"""Local-only feedback export and benchmark-gated model promotion.

This module deliberately separates raw telemetry from training labels. Merely
showing or opening a result is exposure-biased and is never emitted as a
positive or negative label. The default exporter requires both a cited/edited
positive and an explicit rejection from the same query trajectory.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import sqlite3
import tempfile
import uuid
from collections import Counter, defaultdict
from dataclasses import asdict, dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Literal

from .schema import LearningExampleProvenance, TrainingDocument, TrainingExample

PositiveStage = Literal["edited_or_affected", "cited_or_used"]
QueryKind = Literal["documentation", "symbol_navigation", "change_localization"]

POSITIVE_STAGES: tuple[PositiveStage, ...] = ("edited_or_affected", "cited_or_used")
NEGATIVE_STAGE = "rejected"
RAW_ONLY_STAGES = ("shown_to_model", "opened_by_agent")


@dataclass(frozen=True)
class FeedbackExportSummary:
    output: str
    manifest: str
    dataset_revision: str
    dataset_sha256: str
    trajectories_scanned: int
    examples: int
    skipped_without_positive: int
    skipped_without_explicit_negative: int
    skipped_unmaterialized_documents: int


@dataclass(frozen=True)
class PromotionPolicy:
    quality_max_regression: float = 0.0
    per_kind_max_regression: float = 0.02
    latency_max_ratio: float = 1.10
    memory_max_ratio: float = 1.10
    throughput_min_ratio: float = 0.90

    def validate(self) -> None:
        if self.quality_max_regression < 0 or self.per_kind_max_regression < 0:
            raise ValueError("quality regression tolerances must be non-negative")
        if self.latency_max_ratio < 1 or self.memory_max_ratio < 1:
            raise ValueError("latency and memory ratios must be at least one")
        if not 0 < self.throughput_min_ratio <= 1:
            raise ValueError("throughput_min_ratio must be in (0, 1]")


@dataclass(frozen=True)
class PromotionSummary:
    promoted: bool
    candidate_revision: str
    current_manifest: str | None
    audit_manifest: str
    failures: tuple[str, ...]


@dataclass(frozen=True)
class _Trajectory:
    data_root: Path
    database: Path
    trajectory_id: str
    repository_id: str
    repository_name: str
    snapshot_id: str
    revision: str
    query: str
    intent: str
    trace_digest: str
    trace_relative_path: str
    created_at: str
    events: tuple[dict[str, Any], ...]


def export_feedback_dataset(
    data_roots: list[Path],
    output: Path,
    *,
    cutoff: str | None = None,
    max_negatives: int = 7,
) -> FeedbackExportSummary:
    """Export deterministic, explicit-feedback-only contrastive examples."""
    with tempfile.TemporaryDirectory(prefix="cce-feedback-snapshot-") as temporary:
        return _export_feedback_dataset(
            data_roots,
            output,
            cutoff=cutoff,
            max_negatives=max_negatives,
            snapshot_directory=Path(temporary),
        )


def _export_feedback_dataset(
    data_roots: list[Path],
    output: Path,
    *,
    cutoff: str | None,
    max_negatives: int,
    snapshot_directory: Path,
) -> FeedbackExportSummary:
    if not data_roots:
        raise ValueError("at least one CCE data root is required")
    if not 1 <= max_negatives <= 64:
        raise ValueError("max_negatives must be between 1 and 64")
    normalized_cutoff = _normalize_cutoff(cutoff)
    roots = sorted({path.resolve() for path in data_roots}, key=str)
    database_records: list[dict[str, str]] = []
    trajectories: list[_Trajectory] = []
    event_counts: Counter[str] = Counter()
    for index, root in enumerate(roots):
        database = root / "metadata.sqlite"
        if not database.is_file():
            raise ValueError(f"CCE metadata database does not exist: {database}")
        database_snapshot = snapshot_directory / f"metadata-{index}.sqlite"
        _snapshot_sqlite(database, database_snapshot)
        database_records.append(
            {
                "dataRoot": str(root),
                "metadataDatabase": str(database),
                "metadataMainFileSha256": sha256_file(database),
                "logicalSnapshotSha256": sha256_file(database_snapshot),
            }
        )
        loaded = _load_trajectories(root, database_snapshot, normalized_cutoff)
        trajectories.extend(loaded)
        for trajectory in loaded:
            event_counts.update(str(event["stage"]) for event in trajectory.events)

    policy_payload = {
        "positiveStages": list(POSITIVE_STAGES),
        "negativeStages": [NEGATIVE_STAGE],
        "rawTelemetryOnlyStages": list(RAW_ONLY_STAGES),
        "requireExplicitNegative": True,
        "maxNegatives": max_negatives,
        "cutoff": normalized_cutoff,
    }
    revision_seed = {
        "policy": policy_payload,
        "sourceSignals": [
            {
                "trajectoryId": trajectory.trajectory_id,
                "repositoryId": trajectory.repository_id,
                "snapshotId": trajectory.snapshot_id,
                "revision": trajectory.revision,
                "query": trajectory.query,
                "intent": trajectory.intent,
                "traceDigest": trajectory.trace_digest,
                "createdAt": trajectory.created_at,
                "events": list(trajectory.events),
            }
            for trajectory in sorted(
                trajectories, key=lambda item: (item.created_at, item.trajectory_id)
            )
        ],
    }
    dataset_revision = "feedback-" + _canonical_sha256(revision_seed)[:24]
    examples: list[TrainingExample] = []
    skipped_without_positive = 0
    skipped_without_explicit_negative = 0
    skipped_unmaterialized_documents = 0

    for trajectory in sorted(trajectories, key=lambda item: (item.created_at, item.trajectory_id)):
        trace = _read_json_artifact(
            trajectory.data_root, trajectory.trace_relative_path, trajectory.trace_digest
        )
        hits = {str(hit.get("documentId", "")): hit for hit in trace.get("hits", [])}
        stages_by_document: dict[str, set[str]] = defaultdict(set)
        event_time_by_document: dict[tuple[str, str], str] = {}
        for event in trajectory.events:
            document_id = event.get("document_id")
            if document_id:
                stage = str(event["stage"])
                stages_by_document[str(document_id)].add(stage)
                event_time_by_document[(str(document_id), stage)] = str(event["created_at"])
        positive_ids = [
            document_id
            for document_id, stages in stages_by_document.items()
            if any(stage in stages for stage in POSITIVE_STAGES)
        ]
        if not positive_ids:
            skipped_without_positive += 1
            continue
        rejected_ids = [
            document_id
            for document_id, stages in stages_by_document.items()
            if NEGATIVE_STAGE in stages and document_id not in positive_ids
        ]
        if not rejected_ids:
            skipped_without_explicit_negative += 1
            continue
        ranked_rejected = sorted(
            rejected_ids,
            key=lambda identifier: (
                int(hits.get(identifier, {}).get("rank", 2**31 - 1)),
                identifier,
            ),
        )[:max_negatives]
        document_connection = sqlite3.connect(
            f"file:{trajectory.database.as_posix()}?mode=ro", uri=True
        )
        document_connection.row_factory = sqlite3.Row
        try:
            negative_documents = [
                document
                for identifier in ranked_rejected
                if (
                    document := _materialize_document(
                        trajectory, hits.get(identifier), document_connection
                    )
                )
                is not None
            ]
            if not negative_documents:
                skipped_unmaterialized_documents += 1
                continue
            for positive_id in sorted(positive_ids):
                positive = _materialize_document(
                    trajectory, hits.get(positive_id), document_connection
                )
                if positive is None:
                    skipped_unmaterialized_documents += 1
                    continue
                negatives = [
                    document
                    for document in negative_documents
                    if document.document_id != positive.document_id
                ]
                if not negatives:
                    skipped_unmaterialized_documents += 1
                    continue
                positive_stage = next(
                    stage for stage in POSITIVE_STAGES if stage in stages_by_document[positive_id]
                )
                observed_at = event_time_by_document[(positive_id, positive_stage)]
                example_identity = _canonical_sha256(
                    {
                        "trajectory": trajectory.trajectory_id,
                        "positive": positive.document_id,
                        "negatives": [item.document_id for item in negatives],
                    }
                )[:24]
                examples.append(
                    TrainingExample(
                        example_id=f"feedback_{example_identity}",
                        dataset_revision=dataset_revision,
                        split="train",
                        query=trajectory.query,
                        query_kind=_query_kind(trajectory.intent),
                        positive=positive,
                        negatives=negatives,
                        learning_provenance=LearningExampleProvenance(
                            trajectory_id=trajectory.trajectory_id,
                            snapshot_id=trajectory.snapshot_id,
                            observed_at=observed_at,
                            positive_stage=positive_stage,
                        ),
                    )
                )
        finally:
            document_connection.close()

    examples.sort(key=lambda item: item.example_id)
    output.parent.mkdir(parents=True, exist_ok=True)
    body = "".join(example.model_dump_json() + "\n" for example in examples).encode()
    _atomic_write(output, body)
    dataset_sha256 = hashlib.sha256(body).hexdigest()
    manifest_path = output.with_suffix(output.suffix + ".manifest.json")
    manifest = {
        "schemaVersion": 1,
        "generatedAt": datetime.now(UTC).isoformat(),
        "localOnly": True,
        "datasetRevision": dataset_revision,
        "datasetSha256": dataset_sha256,
        "cutoff": normalized_cutoff,
        "labelPolicy": policy_payload,
        "sources": database_records,
        "eventCounts": dict(sorted(event_counts.items())),
        "traceDigests": sorted(trajectory.trace_digest for trajectory in trajectories),
        "counts": {
            "trajectoriesScanned": len(trajectories),
            "examples": len(examples),
            "skippedWithoutPositive": skipped_without_positive,
            "skippedWithoutExplicitNegative": skipped_without_explicit_negative,
            "skippedUnmaterializedDocuments": skipped_unmaterialized_documents,
        },
    }
    _atomic_write(manifest_path, (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode())
    return FeedbackExportSummary(
        output=str(output.resolve()),
        manifest=str(manifest_path.resolve()),
        dataset_revision=dataset_revision,
        dataset_sha256=dataset_sha256,
        trajectories_scanned=len(trajectories),
        examples=len(examples),
        skipped_without_positive=skipped_without_positive,
        skipped_without_explicit_negative=skipped_without_explicit_negative,
        skipped_unmaterialized_documents=skipped_unmaterialized_documents,
    )


def promote_if_benchmark_passes(
    champion_benchmark: Path,
    challenger_benchmark: Path,
    candidate: Path,
    registry: Path,
    *,
    champion_model: str | None = None,
    challenger_model: str | None = None,
    policy: PromotionPolicy | None = None,
) -> PromotionSummary:
    """Promote a self-contained local bundle only when benchmark gates pass."""
    policy = policy or PromotionPolicy()
    policy.validate()
    champion_payload = _read_json(champion_benchmark)
    challenger_payload = _read_json(challenger_benchmark)
    _validate_comparable_benchmarks(champion_payload, challenger_payload)
    champion = _select_measurement(champion_payload, champion_model)
    challenger = _select_measurement(challenger_payload, challenger_model)
    failures = _regressions(champion, challenger, policy)
    candidate_revision = hash_tree(candidate)
    registry = registry.resolve()
    if registry == candidate.resolve() or registry.is_relative_to(candidate.resolve()):
        raise ValueError("model registry may not be the candidate or live inside it")
    registry.mkdir(parents=True, exist_ok=True)
    audit_directory = registry / "audit"
    audit_directory.mkdir(parents=True, exist_ok=True)
    decision_id = f"{datetime.now(UTC).strftime('%Y%m%dT%H%M%S%fZ')}-{uuid.uuid4().hex[:8]}"
    current_manifest: Path | None = None
    current_payload: dict[str, Any] | None = None
    release_relative: str | None = None
    if not failures:
        release = registry / "releases" / candidate_revision
        release.parent.mkdir(parents=True, exist_ok=True)
        if not release.exists():
            staging = Path(tempfile.mkdtemp(prefix="promotion-", dir=registry))
            try:
                destination = staging / "bundle"
                if candidate.is_dir():
                    _copy_local_tree(candidate, destination)
                elif candidate.is_file() and not candidate.is_symlink():
                    destination.mkdir()
                    shutil.copy2(candidate, destination / candidate.name)
                else:
                    raise ValueError(f"candidate must be a local file or directory: {candidate}")
                if hash_tree(destination) != candidate_revision:
                    raise RuntimeError("candidate changed while it was being promoted")
                os.replace(staging, release)
                _fsync_directory(release.parent)
            finally:
                if staging.exists():
                    shutil.rmtree(staging)
        elif (
            not (release / "bundle").is_dir() or hash_tree(release / "bundle") != candidate_revision
        ):
            raise RuntimeError(f"local registry release is corrupt: {release}")
        release_relative = str(release.relative_to(registry))
        current_manifest = registry / "current.json"
        current_payload = {
            "schemaVersion": 1,
            "localOnly": True,
            "candidateRevision": candidate_revision,
            "release": release_relative,
            "promotedAt": datetime.now(UTC).isoformat(),
            "decisionId": decision_id,
        }
    audit_path = audit_directory / f"{decision_id}.json"
    audit = {
        "schemaVersion": 1,
        "decisionId": decision_id,
        "decidedAt": datetime.now(UTC).isoformat(),
        "localOnly": True,
        "approved": not failures,
        "promoted": False,
        "status": "rejected" if failures else "approved_pending_activation",
        "failures": failures,
        "candidateRevision": candidate_revision,
        "release": release_relative,
        "policy": asdict(policy),
        "championBenchmark": {
            "path": str(champion_benchmark.resolve()),
            "sha256": sha256_file(champion_benchmark),
            "model": champion.get("model"),
        },
        "challengerBenchmark": {
            "path": str(challenger_benchmark.resolve()),
            "sha256": sha256_file(challenger_benchmark),
            "model": challenger.get("model"),
        },
    }
    _atomic_write(audit_path, (json.dumps(audit, indent=2, sort_keys=True) + "\n").encode())
    if current_manifest is not None and current_payload is not None:
        _atomic_write(
            current_manifest,
            (json.dumps(current_payload, indent=2, sort_keys=True) + "\n").encode(),
        )
        audit["promoted"] = True
        audit["status"] = "promoted"
        _atomic_write(audit_path, (json.dumps(audit, indent=2, sort_keys=True) + "\n").encode())
    return PromotionSummary(
        promoted=not failures,
        candidate_revision=candidate_revision,
        current_manifest=str(current_manifest) if current_manifest else None,
        audit_manifest=str(audit_path),
        failures=tuple(failures),
    )


def _load_trajectories(root: Path, database: Path, cutoff: str) -> list[_Trajectory]:
    uri = f"file:{database.as_posix()}?mode=ro"
    connection = sqlite3.connect(uri, uri=True)
    connection.row_factory = sqlite3.Row
    try:
        tables = {
            str(row[0])
            for row in connection.execute("SELECT name FROM sqlite_master WHERE type='table'")
        }
        required = {"trajectories", "learning_events", "artifacts", "snapshots"}
        if missing := required - tables:
            raise ValueError(f"{database}: missing learning tables {sorted(missing)}")
        rows = connection.execute(
            """
            SELECT trajectory.id, trajectory.repository_id, trajectory.snapshot_id,
                   trajectory.query, trajectory.intent, trajectory.artifact_digest,
                   trajectory.created_at, artifact.relative_path, snapshot.base_revision,
                   repository.canonical_root
            FROM trajectories AS trajectory
            JOIN artifacts AS artifact ON artifact.digest=trajectory.artifact_digest
            JOIN snapshots AS snapshot ON snapshot.id=trajectory.snapshot_id
            LEFT JOIN repositories AS repository ON repository.id=trajectory.repository_id
            WHERE trajectory.created_at <= ?
            ORDER BY trajectory.created_at, trajectory.id
            """,
            (cutoff,),
        ).fetchall()
        output: list[_Trajectory] = []
        for row in rows:
            events = connection.execute(
                """
                SELECT stage, document_id, dwell_ms, metadata_json, created_at
                FROM learning_events
                WHERE trajectory_id=? AND created_at <= ?
                ORDER BY created_at, id
                """,
                (row["id"], cutoff),
            ).fetchall()
            parsed_events = tuple(
                {
                    "stage": _decode_json_scalar(event["stage"]),
                    "document_id": event["document_id"],
                    "dwell_ms": event["dwell_ms"],
                    "metadata": json.loads(event["metadata_json"]),
                    "created_at": event["created_at"],
                }
                for event in events
            )
            output.append(
                _Trajectory(
                    data_root=root,
                    database=database,
                    trajectory_id=str(row["id"]),
                    repository_id=str(row["repository_id"]),
                    repository_name=str(row["canonical_root"] or row["repository_id"]),
                    snapshot_id=str(row["snapshot_id"]),
                    revision=_immutable_revision(row["base_revision"], str(row["snapshot_id"])),
                    query=str(row["query"]),
                    intent=str(_decode_json_scalar(row["intent"])),
                    trace_digest=str(row["artifact_digest"]),
                    trace_relative_path=str(row["relative_path"]),
                    created_at=str(row["created_at"]),
                    events=parsed_events,
                )
            )
        return output
    finally:
        connection.close()


def _materialize_document(
    trajectory: _Trajectory,
    hit: dict[str, Any] | None,
    connection: sqlite3.Connection,
) -> TrainingDocument | None:
    if not hit or not hit.get("documentId"):
        return None
    document_id = str(hit["documentId"])
    row = connection.execute(
        """
            SELECT document.body_artifact_digest, artifact.relative_path,
                   document.address_json, entity.name, entity.language
            FROM retrieval_documents AS document
            JOIN artifacts AS artifact ON artifact.digest=document.body_artifact_digest
            LEFT JOIN entities AS entity ON entity.snapshot_id=document.snapshot_id
                                         AND entity.id=document.entity_id
            WHERE document.snapshot_id=? AND document.id=?
            """,
        (trajectory.snapshot_id, document_id),
    ).fetchone()
    address = hit.get("address")
    text: str
    symbol = str(hit.get("symbolName") or hit.get("entityId") or document_id)
    language: str | None = None
    if row:
        text = _read_artifact(
            trajectory.data_root, str(row["relative_path"]), str(row["body_artifact_digest"])
        ).decode("utf-8")
        address = json.loads(row["address_json"]) if row["address_json"] else address
        symbol = str(row["name"] or symbol)
        language = str(row["language"]) if row["language"] else None
    elif isinstance(address, dict):
        source = connection.execute(
            """
                SELECT source.artifact_digest, artifact.relative_path, source.language
                FROM source_files AS source
                JOIN artifacts AS artifact ON artifact.digest=source.artifact_digest
                WHERE source.snapshot_id=? AND source.path=?
                """,
            (trajectory.snapshot_id, str(address.get("path", ""))),
        ).fetchone()
        if not source:
            return None
        source_bytes = _read_artifact(
            trajectory.data_root,
            str(source["relative_path"]),
            str(source["artifact_digest"]),
        )
        start = int(address.get("startByte", 0))
        end = int(address.get("endByte", 0))
        text = source_bytes[start:end].decode("utf-8")
        language = str(source["language"]) if source["language"] else None
    else:
        return None
    if not isinstance(address, dict) or not text:
        return None
    start_byte = int(address.get("startByte", 0))
    end_byte = int(address.get("endByte", start_byte + len(text.encode())))
    if end_byte <= start_byte:
        end_byte = start_byte + len(text.encode())
    return TrainingDocument(
        document_id=document_id,
        repository=trajectory.repository_name,
        revision=trajectory.revision,
        path=str(address["path"]),
        symbol=symbol,
        language=language,
        start_byte=start_byte,
        end_byte=end_byte,
        text=text,
        content_sha256=hashlib.sha256(text.encode()).hexdigest(),
    )


def _regressions(
    champion: dict[str, Any], challenger: dict[str, Any], policy: PromotionPolicy
) -> list[str]:
    failures: list[str] = []
    for metric in ("recall_at_1", "recall_at_5", "recall_at_10", "mrr", "ndcg_at_10"):
        if metric in champion and metric in challenger:
            floor = float(champion[metric]) - policy.quality_max_regression
            if float(challenger[metric]) < floor:
                failures.append(f"{metric} {challenger[metric]} is below required {floor}")
    champion_kinds = champion.get("metrics_by_query_kind", {})
    challenger_kinds = challenger.get("metrics_by_query_kind", {})
    for kind, metrics in champion_kinds.items():
        if kind not in challenger_kinds:
            failures.append(f"challenger is missing query-kind metrics for {kind}")
            continue
        for metric, value in metrics.items():
            if metric in challenger_kinds[kind]:
                floor = float(value) - policy.per_kind_max_regression
                if float(challenger_kinds[kind][metric]) < floor:
                    failures.append(
                        f"{kind}.{metric} {challenger_kinds[kind][metric]} is below {floor}"
                    )
    latency_key = next(
        (key for key in ("single_query_p95_ms", "query_p95_ms") if key in champion), None
    )
    if latency_key and latency_key in challenger:
        ceiling = float(champion[latency_key]) * policy.latency_max_ratio
        if float(challenger[latency_key]) > ceiling:
            failures.append(f"{latency_key} {challenger[latency_key]} exceeds {ceiling}")
    if "peak_memory_bytes" in champion and "peak_memory_bytes" in challenger:
        ceiling = float(champion["peak_memory_bytes"]) * policy.memory_max_ratio
        if float(challenger["peak_memory_bytes"]) > ceiling:
            failures.append(
                f"peak_memory_bytes {challenger['peak_memory_bytes']} exceeds {ceiling}"
            )
    throughput_key = next(
        (key for key in ("documents_per_second", "pairs_per_second") if key in champion), None
    )
    if throughput_key and throughput_key in challenger:
        floor = float(champion[throughput_key]) * policy.throughput_min_ratio
        if float(challenger[throughput_key]) < floor:
            failures.append(f"{throughput_key} {challenger[throughput_key]} is below {floor}")
    return failures


def _validate_comparable_benchmarks(champion: dict[str, Any], challenger: dict[str, Any]) -> None:
    if champion.get("datasetSha256") != challenger.get("datasetSha256"):
        raise ValueError("champion and challenger benchmarks must use the same dataset hash")
    if champion.get("hardware") != challenger.get("hardware"):
        raise ValueError("champion and challenger benchmarks must use identical hardware")


def _select_measurement(payload: dict[str, Any], model: str | None) -> dict[str, Any]:
    measurements = payload.get("measurements")
    if not isinstance(measurements, list) or not measurements:
        raise ValueError("benchmark has no measurements")
    if model is None:
        if len(measurements) != 1:
            raise ValueError("benchmark contains multiple models; select one explicitly")
        return dict(measurements[0])
    matches = [item for item in measurements if item.get("model") == model]
    if len(matches) != 1:
        raise ValueError(f"benchmark does not contain exactly one model named {model}")
    return dict(matches[0])


def hash_tree(path: Path) -> str:
    path = path.resolve()
    if path.is_symlink() or not path.exists():
        raise ValueError(f"model candidate must exist and may not be a symlink: {path}")
    digest = hashlib.sha256()
    if path.is_file():
        digest.update(path.name.encode())
        digest.update(b"\0")
        with path.open("rb") as handle:
            while chunk := handle.read(1024 * 1024):
                digest.update(chunk)
    else:
        for candidate in sorted(path.rglob("*")):
            if candidate.is_symlink():
                raise ValueError(f"model candidate contains a symlink: {candidate}")
            if candidate.is_file():
                relative = candidate.relative_to(path).as_posix()
                digest.update(relative.encode())
                digest.update(b"\0")
                with candidate.open("rb") as handle:
                    while chunk := handle.read(1024 * 1024):
                        digest.update(chunk)
                digest.update(b"\0")
    return digest.hexdigest()


def _copy_local_tree(source: Path, destination: Path) -> None:
    for candidate in source.rglob("*"):
        if candidate.is_symlink():
            raise ValueError(f"model candidate contains a symlink: {candidate}")
    shutil.copytree(source, destination)


def _query_kind(intent: str) -> QueryKind:
    if intent == "exact_entity":
        return "symbol_navigation"
    if intent in {"issue_localization", "impact", "history"}:
        return "change_localization"
    return "documentation"


def _immutable_revision(base_revision: Any, snapshot_id: str) -> str:
    value = str(base_revision or "").lower()
    if len(value) == 40 and all(character in "0123456789abcdef" for character in value):
        return value
    return hashlib.sha1(f"cce-snapshot:{snapshot_id}".encode()).hexdigest()


def _decode_json_scalar(value: Any) -> Any:
    if not isinstance(value, str):
        return value
    try:
        return json.loads(value)
    except json.JSONDecodeError:
        return value


def _read_json(path: Path) -> dict[str, Any]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        raise ValueError(f"{path}: expected a JSON object")
    return payload


def _read_json_artifact(root: Path, relative_path: str, digest: str) -> dict[str, Any]:
    payload = json.loads(_read_artifact(root, relative_path, digest))
    if not isinstance(payload, dict):
        raise ValueError(f"trajectory {digest} is not a JSON object")
    return payload


def _read_artifact(root: Path, relative_path: str, digest: str) -> bytes:
    path = (root / relative_path).resolve()
    if not path.is_relative_to(root.resolve()):
        raise ValueError(f"artifact path escapes CCE data root: {relative_path}")
    if len(digest) != 64 or any(character not in "0123456789abcdef" for character in digest):
        raise ValueError(f"invalid artifact digest: {digest}")
    return path.read_bytes()


def _normalize_cutoff(cutoff: str | None) -> str:
    if cutoff is None:
        return datetime.now(UTC).isoformat()
    parsed = datetime.fromisoformat(cutoff.replace("Z", "+00:00"))
    if parsed.tzinfo is None:
        raise ValueError("cutoff must include an explicit timezone")
    return parsed.astimezone(UTC).isoformat()


def _snapshot_sqlite(source: Path, destination: Path) -> None:
    """Create one transactionally consistent logical snapshot, including source WAL pages."""
    source_connection = sqlite3.connect(f"file:{source.as_posix()}?mode=ro", uri=True)
    destination_connection = sqlite3.connect(destination)
    try:
        source_connection.backup(destination_connection)
    finally:
        destination_connection.close()
        source_connection.close()


def _canonical_sha256(payload: Any) -> str:
    return hashlib.sha256(
        json.dumps(payload, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def _atomic_write(path: Path, body: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(body)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
        _fsync_directory(path.parent)
    finally:
        if temporary.exists():
            temporary.unlink()


def _fsync_directory(path: Path) -> None:
    flags = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0)
    descriptor = os.open(path, flags)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
