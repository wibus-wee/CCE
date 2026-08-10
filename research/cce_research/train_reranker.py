"""Local cross-encoder training, ONNX export, and hash-bound bundle promotion."""

from __future__ import annotations

import hashlib
import json
import os
import platform
import random
import shutil
import tempfile
from dataclasses import asdict, dataclass
from importlib import import_module
from pathlib import Path
from typing import Any

from .schema import TrainingExample, load_jsonl
from .train_retriever import is_immutable_revision, sha256_file, validate_splits


@dataclass(frozen=True)
class RerankerTrainingRecipe:
    base_model: str
    base_revision: str
    dataset_directory: str
    output_directory: str
    code_revision: str | None = None
    hard_negatives: int = 7
    learning_rate: float = 2e-5
    epochs: float = 1.0
    batch_size: int = 8
    gradient_accumulation_steps: int = 4
    warmup_ratio: float = 0.05
    max_sequence_length: int = 512
    max_train_examples: int | None = None
    max_validation_examples: int | None = None
    seed: int = 0xCCE
    trust_remote_code: bool = False


@dataclass(frozen=True)
class RerankerTrainingSummary:
    output_directory: str
    train_queries: int
    validation_queries: int
    train_pairs: int
    validation_pairs: int
    train_repositories: list[str]
    validation_repositories: list[str]
    model_manifest: str


@dataclass(frozen=True)
class RerankerBundleSummary:
    output_directory: str
    runtime_model: str
    source_revision: str
    bundle_revision: str
    files: dict[str, str]


def validate_reranker_recipe(recipe: RerankerTrainingRecipe) -> None:
    if not is_immutable_revision(recipe.base_revision):
        raise ValueError("base_revision must be a 40-64 character immutable hexadecimal revision")
    if recipe.code_revision is not None and not is_immutable_revision(recipe.code_revision):
        raise ValueError("code_revision must be an immutable hexadecimal revision when supplied")
    if recipe.hard_negatives < 1:
        raise ValueError("at least one hard negative is required")
    if recipe.learning_rate <= 0:
        raise ValueError("learning_rate must be positive")
    if recipe.epochs <= 0 or recipe.batch_size < 1 or recipe.gradient_accumulation_steps < 1:
        raise ValueError("epochs, batch_size, and gradient_accumulation_steps must be positive")
    if not 64 <= recipe.max_sequence_length <= 8192:
        raise ValueError("max_sequence_length must be in 64..=8192")
    if recipe.max_train_examples is not None and recipe.max_train_examples < 1:
        raise ValueError("max_train_examples must be positive when supplied")
    if recipe.max_validation_examples is not None and recipe.max_validation_examples < 1:
        raise ValueError("max_validation_examples must be positive when supplied")


def train_reranker(recipe: RerankerTrainingRecipe) -> RerankerTrainingSummary:
    """Train a binary cross-encoder against repository-local mined hard negatives."""

    validate_reranker_recipe(recipe)
    datasets: Any = require_module("datasets", "models")
    sentence_transformers: Any = require_module("sentence_transformers", "models,training")
    torch: Any = require_module("torch", "models")
    losses: Any = require_module("sentence_transformers.cross_encoder.losses", "models,training")
    transformers: Any = require_module("transformers", "models")

    dataset_directory = Path(recipe.dataset_directory)
    train_path = dataset_directory / "train.jsonl"
    validation_path = dataset_directory / "validation.jsonl"
    train_examples = list(load_jsonl(train_path, TrainingExample))
    validation_examples = list(load_jsonl(validation_path, TrainingExample))
    if recipe.max_train_examples is not None:
        train_examples = train_examples[: recipe.max_train_examples]
    if recipe.max_validation_examples is not None:
        validation_examples = validation_examples[: recipe.max_validation_examples]
    validate_splits(train_examples, validation_examples, recipe.hard_negatives)

    seed_everything(recipe.seed, torch)
    config_payload, _ = transformers.PretrainedConfig.get_config_dict(
        recipe.base_model,
        revision=recipe.base_revision,
        code_revision=recipe.code_revision,
    )
    requires_remote_code = bool(config_payload.get("auto_map"))
    if requires_remote_code and not recipe.trust_remote_code:
        raise ValueError(
            "base reranker declares custom model code; pass --trust-remote-code with immutable "
            "base/code revisions instead of silently initializing a generic architecture"
        )
    model = sentence_transformers.CrossEncoder(
        recipe.base_model,
        revision=recipe.base_revision,
        trust_remote_code=recipe.trust_remote_code,
        config_kwargs={"code_revision": recipe.code_revision} if recipe.code_revision else None,
        num_labels=1,
        max_length=recipe.max_sequence_length,
    )
    if requires_remote_code and model.model.__class__.__module__.startswith("transformers.models."):
        raise RuntimeError("custom reranker architecture was not loaded; refusing to train")
    train_rows = reranker_rows(train_examples, recipe.hard_negatives)
    validation_rows = reranker_rows(validation_examples, recipe.hard_negatives)
    train_dataset = datasets.Dataset.from_list(train_rows)
    validation_dataset = datasets.Dataset.from_list(validation_rows)
    output_directory = Path(recipe.output_directory)
    output_directory.mkdir(parents=True, exist_ok=True)
    arguments = sentence_transformers.CrossEncoderTrainingArguments(
        output_dir=str(output_directory),
        num_train_epochs=recipe.epochs,
        per_device_train_batch_size=recipe.batch_size,
        per_device_eval_batch_size=recipe.batch_size,
        gradient_accumulation_steps=recipe.gradient_accumulation_steps,
        learning_rate=recipe.learning_rate,
        warmup_ratio=recipe.warmup_ratio,
        fp16=bool(torch.cuda.is_available()),
        eval_strategy="steps" if validation_rows else "no",
        eval_steps=250,
        save_strategy="steps",
        save_steps=250,
        save_total_limit=2,
        logging_steps=25,
        seed=recipe.seed,
        data_seed=recipe.seed,
        report_to="none",
    )
    loss = losses.BinaryCrossEntropyLoss(
        model,
        pos_weight=torch.tensor([float(recipe.hard_negatives)], device=model.device),
    )
    trainer = sentence_transformers.CrossEncoderTrainer(
        model=model,
        args=arguments,
        train_dataset=train_dataset,
        eval_dataset=validation_dataset if validation_rows else None,
        loss=loss,
    )
    trainer.train()
    model.save_pretrained(str(output_directory))

    manifest_path = output_directory / "cce-reranker-training-manifest.json"
    train_repositories = sorted({item.positive.repository for item in train_examples})
    validation_repositories = sorted({item.positive.repository for item in validation_examples})
    manifest_path.write_text(
        json.dumps(
            {
                "schemaVersion": 1,
                "recipe": asdict(recipe),
                "baseModel": recipe.base_model,
                "baseRevision": recipe.base_revision,
                "trainQueries": len(train_examples),
                "validationQueries": len(validation_examples),
                "trainPairs": len(train_rows),
                "validationPairs": len(validation_rows),
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
    return RerankerTrainingSummary(
        output_directory=str(output_directory.resolve()),
        train_queries=len(train_examples),
        validation_queries=len(validation_examples),
        train_pairs=len(train_rows),
        validation_pairs=len(validation_rows),
        train_repositories=train_repositories,
        validation_repositories=validation_repositories,
        model_manifest=str(manifest_path.resolve()),
    )


def reranker_rows(
    examples: list[TrainingExample], hard_negatives: int
) -> list[dict[str, str | float]]:
    rows: list[dict[str, str | float]] = []
    for example in examples:
        rows.append(
            {
                "query": example.query,
                "document": example.positive.text,
                "label": 1.0,
            }
        )
        rows.extend(
            {
                "query": example.query,
                "document": negative.text,
                "label": 0.0,
            }
            for negative in example.negatives[:hard_negatives]
        )
    return rows


def export_reranker_onnx(
    model_directory: Path,
    output_directory: Path,
    runtime_model: str,
    source_revision: str,
    max_sequence_length: int = 512,
) -> RerankerBundleSummary:
    """Export a trained sequence classifier to the layout consumed by fastembed."""

    if not is_immutable_revision(source_revision):
        raise ValueError("source_revision must be an immutable hexadecimal revision")
    if not 64 <= max_sequence_length <= 8192:
        raise ValueError("max_sequence_length must be in 64..=8192")
    optimum: Any = require_module("optimum.onnxruntime", "onnx")
    transformers: Any = require_module("transformers", "models")
    output_directory.mkdir(parents=True, exist_ok=True)
    onnx_directory = output_directory / "onnx"
    onnx_directory.mkdir(parents=True, exist_ok=True)
    export_config = transformers.AutoConfig.from_pretrained(
        str(model_directory.resolve()),
        local_files_only=True,
        trust_remote_code=True,
    )
    export_config.max_position_embeddings = max_sequence_length
    with tempfile.TemporaryDirectory(prefix="cce-reranker-export-") as temporary:
        staging = Path(temporary)
        for source in model_directory.iterdir():
            if source.is_file():
                shutil.copy2(source, staging / source.name)
        export_config.save_pretrained(staging)
        model = optimum.ORTModelForSequenceClassification.from_pretrained(
            str(staging),
            config=export_config,
            export=True,
            local_files_only=True,
            trust_remote_code=True,
            provider="CPUExecutionProvider",
        )
        model.save_pretrained(onnx_directory)
    tokenizer = transformers.AutoTokenizer.from_pretrained(
        str(model_directory.resolve()), local_files_only=True
    )
    tokenizer.save_pretrained(output_directory)
    if (onnx_directory / "config.json").is_file():
        shutil.copy2(onnx_directory / "config.json", output_directory / "config.json")
    else:
        export_config.save_pretrained(output_directory)
    return write_reranker_bundle_manifest(output_directory, runtime_model, source_revision)


def promote_reranker_bundle(
    source_directory: Path,
    output_directory: Path,
    runtime_model: str,
    source_revision: str,
) -> RerankerBundleSummary:
    """Copy an existing pinned ONNX reranker into a portable verified local bundle."""

    if not is_immutable_revision(source_revision):
        raise ValueError("source_revision must be an immutable hexadecimal revision")
    if source_directory.resolve() == output_directory.resolve():
        raise ValueError("source and output directories must differ")
    required = required_reranker_files(source_directory)
    for relative in required:
        source = source_directory / relative
        destination = output_directory / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
    return write_reranker_bundle_manifest(output_directory, runtime_model, source_revision)


def required_reranker_files(directory: Path) -> list[Path]:
    required = [
        Path("config.json"),
        Path("special_tokens_map.json"),
        Path("tokenizer.json"),
        Path("tokenizer_config.json"),
        Path("onnx/model.onnx"),
    ]
    missing = [str(path) for path in required if not (directory / path).is_file()]
    if missing:
        raise ValueError(f"reranker bundle is missing required files: {missing}")
    data_file = Path("onnx/model.onnx.data")
    if (directory / data_file).is_file():
        required.append(data_file)
    return required


def write_reranker_bundle_manifest(
    directory: Path, runtime_model: str, source_revision: str
) -> RerankerBundleSummary:
    files = {
        path.as_posix(): sha256_file(directory / path)
        for path in required_reranker_files(directory)
    }
    digest = hashlib.sha256()
    for name, file_sha256 in sorted(files.items()):
        digest.update(name.encode())
        digest.update(b"\0")
        digest.update(file_sha256.encode())
        digest.update(b"\0")
    bundle_revision = digest.hexdigest()
    manifest = {
        "schemaVersion": 1,
        "runtimeModel": runtime_model,
        "sourceRevision": source_revision,
        "bundleRevision": bundle_revision,
        "files": files,
    }
    (directory / "cce-reranker-manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return RerankerBundleSummary(
        output_directory=str(directory.resolve()),
        runtime_model=runtime_model,
        source_revision=source_revision,
        bundle_revision=bundle_revision,
        files=files,
    )


def seed_everything(seed: int, torch: Any) -> None:
    os.environ.setdefault("PYTHONHASHSEED", str(seed))
    random.seed(seed)
    torch.manual_seed(seed)
    if torch.cuda.is_available():
        torch.cuda.manual_seed_all(seed)


def require_module(name: str, extra: str) -> Any:
    try:
        return import_module(name)
    except ImportError as error:
        raise RuntimeError(f"install cce-research[{extra}] to use this operation") from error
