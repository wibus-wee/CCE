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
        return cls(
            name=str(raw["name"]),
            command=[str(item) for item in raw["command"]],
            timeout_seconds=int(raw.get("timeout_seconds", 120)),
            environment={str(key): str(value) for key, value in raw.get("environment", {}).items()},
            model_identity=str(raw.get("model_identity", "none")),
            model_revision=str(raw.get("model_revision", "none")),
        )

    def build_command(self, case: BenchmarkCase, repository_root: Path) -> list[str]:
        """Expand the command template for one case.

        Scalar placeholders format inline. `{intent_args}` expands to
        `--intent <value>` or to nothing when the case withholds intent
        (`supply_intent: false`); `{route_args}` expands to repeated
        `--route <name>` pairs when the case pins an ablation route set.
        """
        scalars = {
            "repository": str(repository_root),
            "query": case.query,
            "intent": case.intent,
            "budget": str(case.budget_tokens),
        }
        command: list[str] = []
        for part in self.command:
            if part == "{intent_args}":
                if case.supply_intent:
                    command.extend(["--intent", case.intent])
                continue
            if part == "{route_args}":
                for route in case.routes:
                    command.extend(["--route", route])
                continue
            command.append(part.format_map(scalars))
        return command

    def run(self, case: BenchmarkCase, repository_root: Path, system_revision: str) -> CaseResult:
        command = self.build_command(case, repository_root)
        environment = os.environ.copy()
        environment.update(self.environment)
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
        retrieved = normalize_payload(payload)
        return CaseResult(
            case_id=case.case_id,
            system=self.name,
            system_revision=system_revision,
            dataset_revision=case.provenance.dataset_revision,
            retrieved=retrieved,
            abstained=not retrieved,
            predicted_intent=predicted_intent(payload),
            plan_routes=plan_routes(payload),
            graph_policy=graph_policy(payload),
            missing_capabilities=list(payload.get("missingCapabilities", [])),
            query_ms=elapsed_ms,
            metadata={"command": shlex.join(command)},
        )


def normalize_payload(payload: dict[str, Any]) -> list[RetrievedRange]:
    """Normalize either output shape: a context pack (`items`) or a raw
    search result (`hits`). Both carry source-linked provenance."""
    if "hits" in payload:
        return normalize_search_result(payload)
    return normalize_context_pack(payload)


def normalize_search_result(payload: dict[str, Any]) -> list[RetrievedRange]:
    output: list[RetrievedRange] = []
    for hit in payload.get("hits", []):
        addresses = list(hit.get("evidence", []))
        if address := hit.get("address"):
            addresses.insert(0, address)
        for address in addresses:
            output.append(
                RetrievedRange(
                    path=address["path"],
                    start_line=address["startLine"],
                    end_line=address["endLine"],
                    symbol=hit.get("symbolName") or address.get("symbolId"),
                    route=str(hit.get("route", "unknown")),
                    rank=max(1, int(hit.get("rank", len(output) + 1))),
                    score=float(hit.get("score", 0.0)),
                    estimated_tokens=0,
                    citation_verified=bool(hit.get("verifiedCurrent", False)),
                )
            )
    return output


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


def predicted_intent(payload: dict[str, Any]) -> str | None:
    """Resolved intent: `plan.intent` on search results, top-level `intent`
    on context packs (both are post-classification)."""
    plan = payload.get("plan")
    if isinstance(plan, dict) and plan.get("intent"):
        return str(plan["intent"])
    intent = payload.get("intent")
    return str(intent) if intent else None


def plan_routes(payload: dict[str, Any]) -> list[str]:
    plan = payload.get("plan")
    if isinstance(plan, dict):
        return [str(route) for route in plan.get("routes", [])]
    # Context packs carry the executed routes at the top level.
    return [str(route) for route in payload.get("planRoutes", [])]


def graph_policy(payload: dict[str, Any]) -> str | None:
    plan = payload.get("plan")
    if isinstance(plan, dict) and plan.get("graphPolicy"):
        return str(plan["graphPolicy"])
    policy = payload.get("graphPolicy")
    return str(policy) if policy else None
