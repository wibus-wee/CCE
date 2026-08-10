"""Build leakage-controlled, repository-local retriever training data through CCE.

The builder intentionally indexes each pinned source tree with the production CCE binary. This
keeps training documents byte-for-byte aligned with the representation used by local inference.
It emits docstring/symbol queries plus recent change-localization queries and mines explicit hard
negatives only inside the same repository.
"""

from __future__ import annotations

import ast
import hashlib
import json
import re
import sqlite3
import subprocess
from collections import defaultdict
from collections.abc import Iterable
from dataclasses import dataclass
from pathlib import Path

import yaml

from .schema import (
    CorpusManifest,
    CorpusRepository,
    TrainingDocument,
    TrainingExample,
)

_IGNORED_PATH_PARTS = {
    ".git",
    "dist",
    "generated",
    "node_modules",
    "target",
    "third_party",
    "vendor",
    "vendored",
}
_GENERIC_SYMBOLS = {
    "build",
    "default",
    "get",
    "index",
    "main",
    "new",
    "run",
    "set",
    "test",
    "tests",
    "update",
}
_TOKEN = re.compile(r"[A-Za-z_][A-Za-z0-9_]{1,}")
_COMMENT_LINE = re.compile(r"^\s*(?://[/!]?|#|--|;|\*)\s?(.*)$")
_CONVENTIONAL_PREFIX = re.compile(
    r"^(?:build|chore|ci|docs|feat|fix|perf|refactor|revert|style|test)(?:\([^)]*\))?!?:\s*",
    re.IGNORECASE,
)


@dataclass(frozen=True)
class DatasetBuildSummary:
    dataset_revision: str
    repositories: int
    documents: int
    examples: int
    split_examples: dict[str, int]
    output_directory: str


def load_corpus_manifest(path: Path) -> CorpusManifest:
    payload = yaml.safe_load(path.read_text(encoding="utf-8"))
    return CorpusManifest.model_validate(payload)


def build_training_dataset(
    manifest_path: Path,
    workspace: Path,
    output_directory: Path,
    cce_binary: Path,
    hard_negatives: int = 7,
) -> DatasetBuildSummary:
    if hard_negatives < 1 or hard_negatives > 64:
        raise ValueError("hard_negatives must be in 1..=64")
    manifest = load_corpus_manifest(manifest_path)
    workspace.mkdir(parents=True, exist_ok=True)
    output_directory.mkdir(parents=True, exist_ok=True)
    by_split: dict[str, list[TrainingExample]] = defaultdict(list)
    document_count = 0
    for repository in manifest.repositories:
        source_root = materialize_repository(repository, workspace / "repositories")
        data_root = workspace / "indexes" / repository.name
        snapshot_id = index_repository(cce_binary, source_root, data_root)
        documents = load_indexed_documents(
            data_root / "metadata.sqlite",
            snapshot_id,
            repository,
        )
        document_count += len(documents)
        examples = build_repository_examples(
            repository,
            source_root,
            documents,
            manifest.dataset_revision,
            hard_negatives,
        )
        by_split[repository.split].extend(examples)

    for split in ("train", "validation", "test"):
        path = output_directory / f"{split}.jsonl"
        with path.open("w", encoding="utf-8") as handle:
            for example in sorted(by_split[split], key=lambda item: item.example_id):
                handle.write(example.model_dump_json() + "\n")
    summary = DatasetBuildSummary(
        dataset_revision=manifest.dataset_revision,
        repositories=len(manifest.repositories),
        documents=document_count,
        examples=sum(map(len, by_split.values())),
        split_examples={split: len(by_split[split]) for split in ("train", "validation", "test")},
        output_directory=str(output_directory.resolve()),
    )
    (output_directory / "manifest.json").write_text(
        json.dumps(
            {
                **summary.__dict__,
                "sourceManifestSha256": sha256_file(manifest_path),
                "files": {
                    f"{split}.jsonl": sha256_file(output_directory / f"{split}.jsonl")
                    for split in ("train", "validation", "test")
                },
            },
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    return summary


def materialize_repository(repository: CorpusRepository, repositories_root: Path) -> Path:
    repositories_root.mkdir(parents=True, exist_ok=True)
    target = repositories_root / repository.name
    if not target.exists():
        subprocess.run(
            [
                "git",
                "clone",
                "--filter=blob:none",
                "--no-checkout",
                repository.url,
                str(target),
            ],
            check=True,
            timeout=900,
        )
    if not (target / ".git").is_dir():
        raise ValueError(f"refusing to reuse non-Git path: {target}")
    origin = run_git(target, "remote", "get-url", "origin").strip()
    if origin.rstrip("/") != repository.url.rstrip("/"):
        raise ValueError(f"{target} origin is {origin!r}, expected {repository.url!r}")
    has_materialized_files = any(path.name != ".git" for path in target.iterdir())
    if has_materialized_files and run_git(target, "status", "--porcelain"):
        raise ValueError(f"refusing to replace local changes in corpus checkout: {target}")
    subprocess.run(
        [
            "git",
            "fetch",
            "--no-tags",
            f"--depth={repository.history_depth}",
            "origin",
            repository.revision,
        ],
        cwd=target,
        check=True,
        timeout=900,
    )
    subprocess.run(
        ["git", "checkout", "--detach", repository.revision],
        cwd=target,
        check=True,
        timeout=120,
    )
    actual = run_git(target, "rev-parse", "HEAD").strip()
    if actual != repository.revision:
        raise ValueError(f"{repository.name}: checkout {actual} != pinned {repository.revision}")
    return target


def index_repository(cce_binary: Path, source_root: Path, data_root: Path) -> str:
    completed = subprocess.run(
        [
            str(cce_binary.resolve()),
            "--data-dir",
            str(data_root.resolve()),
            "--json",
            "index",
            str(source_root.resolve()),
        ],
        check=True,
        capture_output=True,
        text=True,
        timeout=1800,
    )
    payload = json.loads(completed.stdout)
    return str(payload["snapshot"]["id"])


def load_indexed_documents(
    database: Path,
    snapshot_id: str,
    repository: CorpusRepository,
) -> list[TrainingDocument]:
    connection = sqlite3.connect(f"file:{database.resolve()}?mode=ro", uri=True)
    try:
        rows = connection.execute(
            """
            SELECT d.id, f.path, f.name, f.body, e.language, d.address_json
            FROM retrieval_documents d
            JOIN documents_fts f
              ON f.document_id=d.id AND f.snapshot_id=d.snapshot_id
            JOIN entities e
              ON e.id=d.entity_id AND e.snapshot_id=d.snapshot_id
            WHERE d.snapshot_id=? AND d.representation='\"raw_code\"'
              AND d.address_json IS NOT NULL
              AND length(f.body) BETWEEN 80 AND 64000
            ORDER BY f.path, f.name, d.id
            """,
            (snapshot_id,),
        )
        output: list[TrainingDocument] = []
        for document_id, path, symbol, body, language_json, address_json in rows:
            if ignored_path(str(path)) or str(symbol) == str(path):
                continue
            language = str(language_json) if language_json else None
            if repository.languages and language not in repository.languages:
                continue
            text = dense_document_text(str(path), str(body))
            address = json.loads(str(address_json))
            output.append(
                TrainingDocument(
                    document_id=str(document_id),
                    repository=repository.name,
                    revision=repository.revision,
                    path=str(path),
                    symbol=str(symbol),
                    language=language,
                    start_byte=int(address["startByte"]),
                    end_byte=int(address["endByte"]),
                    text=text,
                    content_sha256=hashlib.sha256(text.encode()).hexdigest(),
                )
            )
            if len(output) >= repository.max_documents:
                break
        return output
    finally:
        connection.close()


def build_repository_examples(
    repository: CorpusRepository,
    source_root: Path,
    documents: list[TrainingDocument],
    dataset_revision: str,
    hard_negatives: int,
) -> list[TrainingExample]:
    if len(documents) <= hard_negatives:
        return []
    token_index: dict[str, set[int]] = defaultdict(set)
    source_cache: dict[str, bytes] = {}
    for index, document in enumerate(documents):
        for token in document_terms(document):
            token_index[token].add(index)

    output: list[TrainingExample] = []
    for index, document in enumerate(documents):
        documentation = documentation_query(document, source_root, source_cache)
        if documentation:
            query, query_kind = documentation, "documentation"
        elif useful_symbol(document.symbol):
            query = symbol_navigation_query(repository.name, document)
            query_kind = "symbol_navigation"
        else:
            continue
        negatives = mine_hard_negatives(
            query,
            index,
            documents,
            token_index,
            hard_negatives,
        )
        if len(negatives) != hard_negatives:
            continue
        output.append(
            training_example(dataset_revision, repository, query, query_kind, document, negatives)
        )

    by_path: dict[str, list[TrainingDocument]] = defaultdict(list)
    for document in documents:
        by_path[document.path].append(document)
    for subject, paths in recent_changes(source_root, repository.history_depth):
        positive = next(
            (
                min(by_path[path], key=lambda item: (len(item.text), item.document_id))
                for path in paths
                if path in by_path
            ),
            None,
        )
        if positive is None:
            continue
        positive_index = documents.index(positive)
        negatives = mine_hard_negatives(
            subject,
            positive_index,
            documents,
            token_index,
            hard_negatives,
        )
        if len(negatives) != hard_negatives:
            continue
        output.append(
            training_example(
                dataset_revision,
                repository,
                subject,
                "change_localization",
                positive,
                negatives,
            )
        )
    return output


def mine_hard_negatives(
    query: str,
    positive_index: int,
    documents: list[TrainingDocument],
    token_index: dict[str, set[int]],
    maximum: int,
) -> list[TrainingDocument]:
    query_terms = terms(query)
    candidates: set[int] = set()
    for term in query_terms:
        candidates.update(token_index.get(term, ()))
    if len(candidates) < maximum * 3:
        stride = max(1, len(documents) // max(1, maximum * 8))
        candidates.update(range(0, len(documents), stride))
    positive = documents[positive_index]
    ranked: list[tuple[float, int]] = []
    for candidate_index in candidates:
        if candidate_index == positive_index:
            continue
        candidate = documents[candidate_index]
        candidate_terms = document_terms(candidate)
        overlap = len(query_terms & candidate_terms) / max(1, len(query_terms | candidate_terms))
        same_language = float(candidate.language == positive.language) * 0.08
        same_directory = float(
            Path(candidate.path).parent == Path(positive.path).parent
            and candidate.path != positive.path
        ) * 0.06
        ranked.append((overlap + same_language + same_directory, candidate_index))
    ranked.sort(key=lambda item: (-item[0], documents[item[1]].document_id))
    return [documents[index] for _, index in ranked[:maximum]]


def documentation_query(
    document: TrainingDocument,
    source_root: Path,
    source_cache: dict[str, bytes],
) -> str | None:
    raw = document.text.split("\n", 2)[-1]
    if document.language == "python":
        try:
            tree = ast.parse(raw)
            first = tree.body[0] if tree.body else None
            node: ast.AsyncFunctionDef | ast.FunctionDef | ast.ClassDef | ast.Module = (
                first
                if isinstance(first, (ast.AsyncFunctionDef, ast.FunctionDef, ast.ClassDef))
                else tree
            )
            value = ast.get_docstring(node, clean=True)
            if value:
                return normalize_query(value)
        except (SyntaxError, TypeError):
            pass
    source_path = source_root / document.path
    if source_path.is_file():
        source = source_cache.get(document.path)
        if source is None:
            source = source_path.read_bytes()
            source_cache[document.path] = source
        prefix = source[: document.start_byte].decode("utf-8", errors="ignore")
        if prefix:
            comment = preceding_comment(prefix)
            if comment:
                return normalize_query(comment)
    comments: list[str] = []
    for line in raw.splitlines()[:12]:
        match = _COMMENT_LINE.match(line)
        if match:
            comments.append(match.group(1).strip(" */"))
        elif line.strip() or comments:
            break
    return normalize_query(" ".join(comments)) if comments else None


def preceding_comment(prefix: str) -> str | None:
    prefix = prefix.rstrip()
    if prefix.endswith("*/"):
        start = prefix.rfind("/*")
        if start >= 0:
            block = prefix[start + 2 : -2]
            lines = [
                re.sub(r"^\s*\*+\s?", "", line).strip()
                for line in block.splitlines()
            ]
            value = " ".join(line for line in lines if line and not line.startswith("@"))
            return value or None
    lines = prefix.splitlines()
    comments: list[str] = []
    for line in reversed(lines[-24:]):
        match = re.match(r"^\s*(?://[/!]?|#)\s?(.*)$", line)
        if match:
            comments.append(match.group(1).strip())
        elif line.strip():
            break
    value = " ".join(reversed(comments)).strip()
    return value or None


def normalize_query(value: str) -> str | None:
    first_paragraph = re.split(r"\n\s*\n|(?<=[.!?])\s+", value.strip(), maxsplit=1)[0]
    compact = " ".join(first_paragraph.split())[:512].strip()
    if len(terms(compact)) < 3 or compact.lower().startswith(("todo", "fixme")):
        return None
    return compact


def symbol_navigation_query(repository: str, document: TrainingDocument) -> str:
    words = " ".join(split_identifier(document.symbol))
    module = " ".join(
        part for part in Path(document.path).parts[-3:-1] if part not in {"src", "lib"}
    )
    suffix = f" in the {module} module" if module else ""
    return f"How is {words} implemented{suffix} in {repository}?"


def recent_changes(source_root: Path, maximum: int) -> Iterable[tuple[str, list[str]]]:
    payload = run_git(
        source_root,
        "log",
        f"--max-count={maximum}",
        "--format=%x1e%s",
        "--name-only",
        "--diff-filter=AMR",
    )
    for record in payload.split("\x1e"):
        lines = [line.strip() for line in record.splitlines() if line.strip()]
        if len(lines) < 2:
            continue
        subject = _CONVENTIONAL_PREFIX.sub("", lines[0]).strip()
        query = normalize_query(subject)
        if query and not query.lower().startswith(("merge ", "release ", "bump ")):
            yield query, lines[1:]


def training_example(
    dataset_revision: str,
    repository: CorpusRepository,
    query: str,
    query_kind: str,
    positive: TrainingDocument,
    negatives: list[TrainingDocument],
) -> TrainingExample:
    identity = "\0".join((repository.name, repository.revision, query, positive.document_id))
    return TrainingExample.model_validate(
        {
            "example_id": "example_" + hashlib.sha256(identity.encode()).hexdigest()[:32],
            "dataset_revision": dataset_revision,
            "split": repository.split,
            "query": query,
            "query_kind": query_kind,
            "positive": positive.model_dump(),
            "negatives": [negative.model_dump() for negative in negatives],
        }
    )


def dense_document_text(path: str, body: str) -> str:
    return f"path: {path}\nrepresentation: RawCode\n{body}"


def document_terms(document: TrainingDocument) -> set[str]:
    return terms(f"{document.path} {document.symbol} {document.text[:4000]}")


def terms(value: str) -> set[str]:
    return {token.lower() for token in _TOKEN.findall(value) if len(token) > 1}


def split_identifier(value: str) -> list[str]:
    expanded = re.sub(r"([a-z0-9])([A-Z])", r"\1 \2", value.replace("_", " "))
    return [part.lower() for part in expanded.split() if part]


def useful_symbol(symbol: str) -> bool:
    words = split_identifier(symbol)
    return bool(words) and not (len(words) == 1 and words[0] in _GENERIC_SYMBOLS)


def ignored_path(value: str) -> bool:
    parts = {part.lower() for part in Path(value).parts}
    return bool(parts & _IGNORED_PATH_PARTS)


def run_git(repository: Path, *arguments: str) -> str:
    completed = subprocess.run(
        ["git", *arguments],
        cwd=repository,
        check=True,
        capture_output=True,
        text=True,
        timeout=300,
    )
    return completed.stdout


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()
