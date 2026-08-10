from __future__ import annotations

import json
import os
import shlex
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import yaml

from .schema import BenchmarkCase, CaseResult, RetrievedRange


@dataclass(frozen=True)
class Adapter:
    name: str
    command: list[str]
    timeout_seconds: int
    environment: dict[str, str]
    model_identity: str
    model_revision: str

    @classmethod
    def load(cls, path: Path) -> Adapter:
        raw = yaml.safe_load(path.read_text(encoding="utf-8"))
        if not isinstance(raw, dict):
            raise ValueError(f"{path}: adapter must be an object")
        command = [str(item) for item in raw["command"]]
        executable = Path(command[0])
        if not executable.is_absolute():
            workspace_executable = (Path.cwd() / executable).resolve()
            if workspace_executable.is_file():
                command[0] = str(workspace_executable)
        return cls(
            name=str(raw["name"]),
            command=command,
            timeout_seconds=int(raw.get("timeout_seconds", 120)),
            environment={str(key): str(value) for key, value in raw.get("environment", {}).items()},
            model_identity=str(raw.get("model_identity", "none")),
            model_revision=str(raw.get("model_revision", "none")),
        )

    def run(self, case: BenchmarkCase, repository_root: Path, system_revision: str) -> CaseResult:
        variables = {
            "repository": str(repository_root),
            "query": case.query,
            "intent": case.intent,
            "budget": str(case.budget_tokens),
        }
        command = [part.format_map(variables) for part in self.command]
        environment = os.environ.copy()
        for key, value in self.environment.items():
            environment.setdefault(key, value)
        started = time.perf_counter()
        completed = subprocess.run(
            command,
            cwd=repository_root,
            env=environment,
            check=False,
            capture_output=True,
            text=True,
            timeout=self.timeout_seconds,
        )
        elapsed_ms = (time.perf_counter() - started) * 1000
        if completed.returncode != 0:
            rendered = shlex.join(command)
            raise RuntimeError(
                f"adapter {self.name} failed ({rendered}): {completed.stderr[-2000:]}"
            )
        payload = json.loads(completed.stdout)
        return CaseResult(
            case_id=case.case_id,
            system=self.name,
            system_revision=system_revision,
            dataset_revision=case.provenance.dataset_revision,
            retrieved=normalize_context_pack(payload),
            abstained=not any(
                item.get("kind") != "orientation"
                and (
                    item.get("kind") == "history"
                    or item.get("provenance", {}).get("sourceAddress")
                    or item.get("provenance", {}).get("evidenceAddresses")
                )
                for item in payload.get("items", [])
            ),
            query_ms=elapsed_ms,
            metadata={"command": shlex.join(command)},
        )


def normalize_context_pack(payload: dict[str, Any]) -> list[RetrievedRange]:
    output: list[RetrievedRange] = []
    for item in payload.get("items", []):
        provenance = item.get("provenance", {})
        addresses = list(provenance.get("evidenceAddresses", []))
        if source := provenance.get("sourceAddress"):
            addresses.insert(0, source)
        for address in addresses:
            output.append(
                RetrievedRange(
                    path=address["path"],
                    start_line=address["startLine"],
                    end_line=address["endLine"],
                    symbol=provenance.get("symbolName") or address.get("symbolId"),
                    route=provenance.get("route", "unknown"),
                    rank=max(1, provenance.get("rank", len(output) + 1)),
                    score=float(provenance.get("score", 0.0)),
                    estimated_tokens=int(item.get("estimatedTokens", 0)),
                    citation_verified=bool(provenance.get("verifiedCurrent", False)),
                )
            )
    return output
