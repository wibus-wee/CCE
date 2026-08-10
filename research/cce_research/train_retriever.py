"""Local contrastive fine-tuning and ONNX export for CCE retrievers."""

from __future__ import annotations

import hashlib
import json
import os
import platform
import random
import shutil
from dataclasses import asdict, dataclass
from importlib import import_module
from pathlib import Path
from typing import Any

from .schema import TrainingExample, load_jsonl


@dataclass(frozen=True)
class TrainingRecipe:
    base_model: str
    base_revision: str
    dataset_directory: str
    output_directory: str
    code_revision: str | None = None
    query_prefix: str = "query: "
    document_prefix: str = "passage: "
    same_repository_hard_negatives: int = 7
    temperature: float = 0.02
    learning_rate: float = 2e-5
    epochs: float = 1.0
    batch_size: int = 8
    gradient_accumulation_steps: int = 4
    cached_mini_batch_size: int = 8
    warmup_ratio: float = 0.05
    max_sequence_length: int = 512
    max_train_examples: int | None = None
    max_validation_examples: int | None = None
    seed: int = 0xCCE
    trust_remote_code: bool = False


@dataclass(frozen=True)
class TrainingSummary:
    output_directory: str
    train_examples: int
    validation_examples: int
    train_repositories: list[str]
    validation_repositories: list[str]
    dataset_sha256: dict[str, str]
    model_manifest: str


def validate_recipe(recipe: TrainingRecipe) -> None:
    if not is_immutable_revision(recipe.base_revision):
        raise ValueError("base_revision must be a 40-64 character immutable hexadecimal revision")
    if recipe.code_revision is not None and not is_immutable_revision(recipe.code_revision):
        raise ValueError("code_revision must be an immutable hexadecimal revision when supplied")
    if recipe.same_repository_hard_negatives < 1:
        raise ValueError("at least one same-repository hard negative is required")
    if not 0 < recipe.temperature <= 1:
        raise ValueError("temperature must be in (0, 1]")
    if recipe.learning_rate <= 0:
        raise ValueError("learning_rate must be positive")
    if recipe.epochs <= 0 or recipe.batch_size < 1 or recipe.gradient_accumulation_steps < 1:
        raise ValueError("epochs, batch_size, and gradient_accumulation_steps must be positive")
    if not 32 <= recipe.max_sequence_length <= 8192:
        raise ValueError("max_sequence_length must be in 32..=8192")
    if recipe.max_train_examples is not None and recipe.max_train_examples < 1:
        raise ValueError("max_train_examples must be positive when supplied")
    if recipe.max_validation_examples is not None and recipe.max_validation_examples < 1:
        raise ValueError("max_validation_examples must be positive when supplied")


def train_retriever(recipe: TrainingRecipe) -> TrainingSummary:
    """Fine-tune a dual encoder with explicit and in-batch repository-local negatives.

    Imports are lazy so dataset construction and the Rust runtime do not require PyTorch.
    """

    validate_recipe(recipe)
    datasets: Any = require_module("datasets", "models")
    sentence_transformers: Any = require_module("sentence_transformers", "models,training")
    torch: Any = require_module("torch", "models")

    dataset_directory = Path(recipe.dataset_directory)
    train_path = dataset_directory / "train.jsonl"
    validation_path = dataset_directory / "validation.jsonl"
    train_examples = list(load_jsonl(train_path, TrainingExample))
    validation_examples = list(load_jsonl(validation_path, TrainingExample))
    if recipe.max_train_examples is not None:
        train_examples = train_examples[: recipe.max_train_examples]
    if recipe.max_validation_examples is not None:
        validation_examples = validation_examples[: recipe.max_validation_examples]
    validate_splits(train_examples, validation_examples, recipe.same_repository_hard_negatives)

    seed_everything(recipe.seed, torch)
    model = sentence_transformers.SentenceTransformer(
        recipe.base_model,
        revision=recipe.base_revision,
        trust_remote_code=recipe.trust_remote_code,
        model_kwargs={"code_revision": recipe.code_revision} if recipe.code_revision else None,
    )
    model.max_seq_length = recipe.max_sequence_length
    train_dataset = datasets.Dataset.from_list(
        [training_row(example, recipe) for example in train_examples]
    )
    validation_dataset = datasets.Dataset.from_list(
        [training_row(example, recipe) for example in validation_examples]
    )
    losses: Any = require_module(
        "sentence_transformers.sentence_transformer.losses", "models,training"
    )
    loss = losses.CachedMultipleNegativesRankingLoss(
        model,
        scale=1.0 / recipe.temperature,
        mini_batch_size=min(recipe.cached_mini_batch_size, recipe.batch_size),
    )
    output_directory = Path(recipe.output_directory)
    output_directory.mkdir(parents=True, exist_ok=True)
    arguments = sentence_transformers.SentenceTransformerTrainingArguments(
        output_dir=str(output_directory),
        num_train_epochs=recipe.epochs,
        per_device_train_batch_size=recipe.batch_size,
        per_device_eval_batch_size=recipe.batch_size,
        gradient_accumulation_steps=recipe.gradient_accumulation_steps,
        learning_rate=recipe.learning_rate,
        warmup_ratio=recipe.warmup_ratio,
        fp16=bool(torch.cuda.is_available()),
        eval_strategy="steps" if validation_examples else "no",
        eval_steps=250,
        save_strategy="steps",
        save_steps=250,
        save_total_limit=2,
        logging_steps=25,
        seed=recipe.seed,
        data_seed=recipe.seed,
        batch_sampler="no_duplicates",
        report_to="none",
    )
    trainer = sentence_transformers.SentenceTransformerTrainer(
        model=model,
        args=arguments,
        train_dataset=train_dataset,
        eval_dataset=validation_dataset if validation_examples else None,
        loss=loss,
    )
    trainer.train()
    model.save_pretrained(str(output_directory))

    train_repositories = sorted({item.positive.repository for item in train_examples})
    validation_repositories = sorted({item.positive.repository for item in validation_examples})
    manifest_path = output_directory / "cce-model-manifest.json"
    manifest_path.write_text(
        json.dumps(
            {
                "schemaVersion": 1,
                "recipe": asdict(recipe),
                "baseModel": recipe.base_model,
                "baseRevision": recipe.base_revision,
                "codeRevision": recipe.code_revision,
                "queryPrefix": recipe.query_prefix,
                "documentPrefix": recipe.document_prefix,
                "trainExamples": len(train_examples),
                "validationExamples": len(validation_examples),
                "trainRepositories": train_repositories,
                "validationRepositories": validation_repositories,
                "datasetSha256": {
                    "train.jsonl": sha256_file(train_path),
                    "validation.jsonl": sha256_file(validation_path),
                },
                "python": platform.python_version(),
                "platform": platform.platform(),
                "cudaAvailable": bool(torch.cuda.is_available()),
                "seed": recipe.seed,
            },
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    return TrainingSummary(
        output_directory=str(output_directory.resolve()),
        train_examples=len(train_examples),
        validation_examples=len(validation_examples),
        train_repositories=train_repositories,
        validation_repositories=validation_repositories,
        dataset_sha256={
            "train.jsonl": sha256_file(train_path),
            "validation.jsonl": sha256_file(validation_path),
        },
        model_manifest=str(manifest_path.resolve()),
    )


def export_onnx(
    model_directory: Path,
    output_directory: Path,
    optimization: str = "O3",
    quantization: str | None = None,
) -> Path:
    """Export a trained local model for CCE's ONNX runtime, optionally CPU-quantized."""

    sentence_transformers: Any = require_module("sentence_transformers", "onnx")
    model = sentence_transformers.SentenceTransformer(
        str(model_directory.resolve()),
        backend="onnx",
        model_kwargs={"export": True, "provider": "CPUExecutionProvider"},
    )
    output_directory.mkdir(parents=True, exist_ok=True)
    model.save_pretrained(str(output_directory))
    if quantization:
        sentence_transformers.export_dynamic_quantized_onnx_model(
            model=model,
            quantization_config=quantization,
            model_name_or_path=str(output_directory),
        )
    else:
        sentence_transformers.export_optimized_onnx_model(
            model=model,
            optimization_config=optimization,
            model_name_or_path=str(output_directory),
        )
    onnx_directory = output_directory / "onnx"
    candidates = sorted(
        path for path in onnx_directory.glob("*.onnx") if path.name != "model.onnx"
    )
    if candidates:
        shutil.copy2(candidates[-1], onnx_directory / "model.onnx")
    source_manifest = model_directory / "cce-model-manifest.json"
    if source_manifest.is_file():
        payload = json.loads(source_manifest.read_text(encoding="utf-8"))
        payload["onnxOptimization"] = optimization
        payload["onnxQuantization"] = quantization
        payload["runtimeModel"] = runtime_model_name(str(payload["baseModel"]))
        required_files = [
            "config.json",
            "special_tokens_map.json",
            "tokenizer.json",
            "tokenizer_config.json",
            "onnx/model.onnx",
        ]
        payload["files"] = {
            name: sha256_file(output_directory / name) for name in required_files
        }
        bundle_hasher = hashlib.sha256()
        for name, digest in sorted(payload["files"].items()):
            bundle_hasher.update(name.encode())
            bundle_hasher.update(b"\0")
            bundle_hasher.update(str(digest).encode())
            bundle_hasher.update(b"\0")
        payload["bundleRevision"] = bundle_hasher.hexdigest()
        (output_directory / "cce-model-manifest.json").write_text(
            json.dumps(payload, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
    return output_directory.resolve()


def runtime_model_name(base_model: str) -> str:
    normalized = base_model.lower()
    if "multilingual-e5-small" in normalized:
        return "MultilingualE5Small"
    if "jina-embeddings-v2-base-code" in normalized:
        return "JinaEmbeddingsV2BaseCode"
    if "bge-small-en-v1.5" in normalized:
        return "BGESmallENV15"
    raise ValueError(f"no CCE fastembed runtime mapping for trained base model {base_model}")


def training_row(example: TrainingExample, recipe: TrainingRecipe) -> dict[str, str]:
    row = {
        "query": recipe.query_prefix + example.query,
        "positive": recipe.document_prefix + example.positive.text,
    }
    for index, negative in enumerate(
        example.negatives[: recipe.same_repository_hard_negatives], start=1
    ):
        row[f"negative_{index}"] = recipe.document_prefix + negative.text
    return row


def validate_splits(
    train_examples: list[TrainingExample],
    validation_examples: list[TrainingExample],
    hard_negatives: int,
) -> None:
    if not train_examples:
        raise ValueError("training split is empty")
    train_repositories = {item.positive.repository for item in train_examples}
    validation_repositories = {item.positive.repository for item in validation_examples}
    overlap = train_repositories & validation_repositories
    if overlap:
        raise ValueError(f"repository leakage between train and validation: {sorted(overlap)}")
    all_examples = [*train_examples, *validation_examples]
    revisions: dict[str, str] = {}
    for example in all_examples:
        previous = revisions.setdefault(example.positive.repository, example.positive.revision)
        if previous != example.positive.revision:
            raise ValueError(f"multiple revisions for repository {example.positive.repository}")
        if len(example.negatives) < hard_negatives:
            raise ValueError(
                f"{example.example_id} has {len(example.negatives)} negatives, "
                f"needs {hard_negatives}"
            )


def seed_everything(seed: int, torch: Any) -> None:
    os.environ.setdefault("PYTHONHASHSEED", str(seed))
    random.seed(seed)
    torch.manual_seed(seed)
    if torch.cuda.is_available():
        torch.cuda.manual_seed_all(seed)


def is_immutable_revision(value: str) -> bool:
    return 40 <= len(value) <= 64 and all(
        character in "0123456789abcdefABCDEF" for character in value
    )


def require_module(name: str, extra: str) -> Any:
    try:
        return import_module(name)
    except ImportError as error:
        raise RuntimeError(f"install cce-research[{extra}] to use this operation") from error


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()
