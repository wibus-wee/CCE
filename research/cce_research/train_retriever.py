"""Conservative SFT entry point for issue-to-context retrievers.

Training is intentionally not executed by the production runtime. Install the `models` and
`training` extras, review dataset licensing, and pin a base model revision before calling this
module.
"""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class TrainingRecipe:
    base_model: str
    base_revision: str
    output_directory: str
    same_repository_hard_negatives: int = 7
    temperature: float = 0.02
    learning_rate: float = 2e-5


def validate_recipe(recipe: TrainingRecipe) -> None:
    if not recipe.base_revision or recipe.base_revision in {"main", "latest"}:
        raise ValueError("base_revision must be an immutable model revision")
    if recipe.same_repository_hard_negatives < 1:
        raise ValueError("at least one same-repository hard negative is required")
    if not 0 < recipe.temperature <= 1:
        raise ValueError("temperature must be in (0, 1]")
