from __future__ import annotations

import atexit
import json
import os
import shlex
import shutil
import socket
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Literal, TextIO

import httpx
import yaml

from .schema import (
    BenchmarkCase,
    CaseResult,
    LineRange,
    RetrievedItem,
    RetrievedRange,
)

SESSION_MODES = ("subprocess", "daemon")
_DAEMON_ENDPOINTS = {"search": "/v1/search", "context": "/v1/context"}


class DaemonRequestUnsupported(RuntimeError):
    """A case property the daemon HTTP API cannot express — currently only
    pinned routes on /v1/search, whose handler hardcodes `routes: []`.
    Callers fall back to the subprocess path for that case."""


class DaemonSession:
    """Handle to a warm `cce-daemon` process bound to a loopback port (or,
    in tests, to any HTTP endpoint speaking the same API)."""

    def __init__(
        self,
        base_url: str,
        timeout_seconds: int,
        process: subprocess.Popen[bytes] | None = None,
        log_handle: TextIO | None = None,
    ) -> None:
        self.base_url = base_url
        self.process = process
        self._log_handle = log_handle
        # trust_env=False: the daemon is loopback-only; ambient proxy
        # settings (env vars or OS-level) must never see these requests.
        self._client = httpx.Client(base_url=base_url, timeout=timeout_seconds, trust_env=False)

    def alive(self) -> bool:
        return self.process is None or self.process.poll() is None

    def post(self, endpoint: str, body: dict[str, Any]) -> dict[str, Any]:
        try:
            response = self._client.post(endpoint, json=body)
        except httpx.HTTPError as error:
            raise RuntimeError(f"daemon POST {endpoint} failed: {error}") from error
        if response.status_code != 200:
            raise RuntimeError(
                f"daemon POST {endpoint} returned {response.status_code}: {response.text[-2000:]}"
            )
        payload = response.json()
        if not isinstance(payload, dict):
            raise RuntimeError(f"daemon POST {endpoint} returned non-object JSON")
        return payload

    def close(self) -> None:
        """Idempotent: safe to call from both `Adapter.shutdown` and atexit."""
        self._client.close()
        if self.process is not None and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=10)
        if self._log_handle is not None:
            self._log_handle.close()
            self._log_handle = None


def _free_port() -> int:
    """Allocate an ephemeral loopback port. There is an inherent race
    between close and the daemon's bind; acceptable for a local harness."""
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


def _daemon_binary(executable: str) -> str:
    """The daemon binary sits next to the CLI: `target/release/cce` maps to
    `target/release/cce-daemon`; a PATH-resolved `cce` maps to `cce-daemon`."""
    path = Path(executable)
    return str(path.with_name(f"cce-daemon{path.suffix}"))


def _daemon_log_path(adapter_name: str) -> Path:
    """Daemon stdout/stderr (JSON tracing) lands in
    `research/output/<adapter>.daemon.log` — one file per run, truncated."""
    output_dir = Path(__file__).resolve().parents[1] / "output"
    output_dir.mkdir(parents=True, exist_ok=True)
    return output_dir / f"{adapter_name}.daemon.log"


def _wait_ready(session: DaemonSession, adapter: Adapter, log_path: Path) -> None:
    """Poll /healthz until the daemon accepts requests or the budget runs
    out. /healthz is the right probe — not /v1/status — because the daemon
    binds its listener only after `CceEngine::open` returns (dense model
    load included), while /v1/status can legitimately 409 on an unindexed
    repository with the engine already warm."""
    deadline = time.monotonic() + adapter.timeout_seconds
    while time.monotonic() < deadline:
        if session.process is not None and session.process.poll() is not None:
            tail = log_path.read_text(encoding="utf-8")[-2000:] if log_path.is_file() else ""
            raise RuntimeError(
                f"adapter {adapter.name}: daemon exited during startup (log {log_path}): {tail}"
            )
        try:
            response = session._client.get("/healthz", timeout=2.0)
            if response.status_code == 200:
                return
        except httpx.HTTPError:
            pass
        time.sleep(0.1)
    raise RuntimeError(
        f"adapter {adapter.name}: daemon did not become ready within "
        f"{adapter.timeout_seconds}s (log {log_path})"
    )


def start_daemon(adapter: Adapter, repository_root: Path) -> DaemonSession:
    """Spawn `cce-daemon` for this adapter and block until it is warm.

    The daemon inherits the adapter `environment` (CCE_DATA_DIR etc.) and
    receives the dense flags as startup arguments — the model is loaded
    once here instead of once per case."""
    binary = _daemon_binary(adapter.command[0])
    if os.sep in binary or (os.altsep and os.altsep in binary):
        if not Path(binary).is_file():
            raise RuntimeError(
                f"adapter {adapter.name}: session 'daemon' needs {binary} — "
                "build it with `cargo build --release -p cce-daemon`"
            )
    elif shutil.which(binary) is None:
        raise RuntimeError(f"adapter {adapter.name}: {binary} not found on PATH")
    port = _free_port()
    command = [binary, str(repository_root), "--bind", f"127.0.0.1:{port}"]
    if adapter.dense:
        command += ["--dense", adapter.dense]
    if adapter.embedding_model:
        command += ["--embedding-model", adapter.embedding_model]
    if adapter.embedding_dimensions is not None:
        command += ["--embedding-dimensions", str(adapter.embedding_dimensions)]
    if adapter.reranker:
        command.append(f"--reranker={adapter.reranker}")
    environment = os.environ.copy()
    environment.update(adapter.environment)
    log_path = _daemon_log_path(adapter.name)
    log_handle = log_path.open("w", encoding="utf-8")
    process = subprocess.Popen(
        command,
        cwd=repository_root,
        env=environment,
        stdout=log_handle,
        stderr=subprocess.STDOUT,
    )
    session = DaemonSession(
        base_url=f"http://127.0.0.1:{port}",
        timeout_seconds=adapter.timeout_seconds,
        process=process,
        log_handle=log_handle,
    )
    try:
        _wait_ready(session, adapter, log_path)
    except BaseException:
        session.close()
        raise
    return session


def _flag_value(argv: list[str], flag: str) -> str | None:
    """First `--flag value` or `--flag=value` occurrence in expanded argv."""
    for index, part in enumerate(argv):
        if part == flag and index + 1 < len(argv):
            return argv[index + 1]
        if part.startswith(f"{flag}="):
            return part.split("=", 1)[1]
    return None


@dataclass(frozen=True)
class Adapter:
    name: str
    command: list[str]
    timeout_seconds: int
    environment: dict[str, str]
    model_identity: str
    model_revision: str
    dense: str | None = None
    embedding_model: str | None = None
    embedding_dimensions: int | None = None
    reranker: str | None = None
    session: str = "subprocess"
    # Live daemon sessions keyed by resolved repository path. Mutable
    # process state on an otherwise frozen value object — excluded from
    # equality/hash so adapter identity stays purely declarative.
    _daemons: dict[str, DaemonSession] = field(
        default_factory=dict, init=False, repr=False, compare=False
    )

    @classmethod
    def load(cls, path: Path) -> Adapter:
        raw = yaml.safe_load(path.read_text(encoding="utf-8"))
        if not isinstance(raw, dict):
            raise ValueError(f"{path}: adapter must be an object")
        session = str(raw.get("session", "subprocess"))
        if session not in SESSION_MODES:
            raise ValueError(f"{path}: session must be one of {SESSION_MODES}, got {session!r}")
        dense = raw.get("dense")
        if dense is not None and dense not in ("baseline", "local", "disabled"):
            raise ValueError(
                f"{path}: dense must be 'baseline', 'local', or 'disabled', got {dense!r}"
            )
        embedding_model = raw.get("embedding_model")
        embedding_dimensions = (
            int(raw["embedding_dimensions"]) if "embedding_dimensions" in raw else None
        )
        if dense is None and (embedding_model or embedding_dimensions is not None):
            raise ValueError(
                f"{path}: embedding_model/embedding_dimensions require a dense backend"
            )
        reranker = raw.get("reranker")
        model_identity = str(raw.get("model_identity", "none"))
        if model_identity == "none" and embedding_model:
            model_identity = f"local:{embedding_model}"
        if reranker:
            suffix = f"rerank:{reranker}"
            model_identity = (
                suffix if model_identity == "none" else f"{model_identity}+{suffix}"
            )
        return cls(
            name=str(raw["name"]),
            command=[str(item) for item in raw["command"]],
            timeout_seconds=int(raw.get("timeout_seconds", 120)),
            environment={str(key): str(value) for key, value in raw.get("environment", {}).items()},
            model_identity=model_identity,
            model_revision=str(raw.get("model_revision", "none")),
            dense=str(dense) if dense else None,
            embedding_model=str(embedding_model) if embedding_model else None,
            embedding_dimensions=embedding_dimensions,
            reranker=str(reranker) if reranker else None,
            session=session,
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
        # All three flags are clap-global, so appending at the end is valid
        # and cannot collide with template-expanded {intent_args}/{route_args}.
        if self.dense:
            command.extend(["--dense", self.dense])
        if self.embedding_model:
            command.extend(["--embedding-model", self.embedding_model])
        if self.embedding_dimensions is not None:
            command.extend(["--embedding-dimensions", str(self.embedding_dimensions)])
        if self.reranker:
            # `=` form: an optional-value flag must not consume a trailing
            # token as its value.
            command.append(f"--reranker={self.reranker}")
        return command

    def start_session(self, repository_root: Path) -> None:
        """Pre-warm the session before the case loop. No-op for subprocess
        adapters; for daemon adapters this spawns `cce-daemon` and blocks
        until it answers /healthz, so startup failures surface before any
        result is written rather than mid-run."""
        if self.session == "daemon":
            self._daemon_session(repository_root)

    def shutdown(self) -> None:
        """Terminate every daemon session this adapter started."""
        for session in self._daemons.values():
            session.close()
        self._daemons.clear()

    def run(
        self,
        case: BenchmarkCase,
        repository_root: Path,
        system_revision: str,
        raw_sink: TextIO | None = None,
    ) -> CaseResult:
        """Run one case. `raw_sink`, when given, receives a JSONL line with
        the case id and the verbatim payload — the raw record metrics are
        recomputed from after accounting changes."""
        if self.session == "daemon":
            try:
                endpoint, body = self.daemon_request(case, repository_root)
            except DaemonRequestUnsupported as error:
                # The daemon cannot express this case (e.g. pinned routes on
                # /v1/search). One cold subprocess keeps the result correct;
                # metadata marks it so latency outliers are attributable.
                print(
                    f"[daemon] {case.case_id}: {error} — falling back to subprocess",
                    file=sys.stderr,
                )
                result = self._run_subprocess(
                    case, repository_root, system_revision, raw_sink
                )
                result.metadata["session"] = "subprocess-fallback"
                return result
            return self._run_daemon(
                case, repository_root, system_revision, endpoint, body, raw_sink
            )
        return self._run_subprocess(case, repository_root, system_revision, raw_sink)

    def daemon_request(
        self, case: BenchmarkCase, repository_root: Path
    ) -> tuple[str, dict[str, Any]]:
        """Translate one case into a daemon (endpoint, JSON body) pair.

        The command template is expanded exactly as in subprocess mode, so
        `{intent_args}`/`{route_args}`/`{budget}` and flag values
        (--limit/--budget/--candidates) behave identically; only the
        transport changes. Intent goes out only when the case supplies it.
        """
        argv = self.build_command(case, repository_root)
        subcommand = next((part for part in argv if part in _DAEMON_ENDPOINTS), None)
        if subcommand is None:
            raise ValueError(
                f"adapter {self.name}: session 'daemon' requires a 'search' or "
                "'context' subcommand in the command template"
            )
        if subcommand == "search":
            if case.routes:
                raise DaemonRequestUnsupported(
                    f"daemon /v1/search cannot pin routes ({', '.join(case.routes)}); "
                    "only /v1/context accepts a routes array"
                )
            body: dict[str, Any] = {
                "query": case.query,
                "limit": int(_flag_value(argv, "--limit") or "20"),
            }
            if case.supply_intent:
                body["intent"] = case.intent
            return "/v1/search", body
        body = {
            "query": case.query,
            "budgetTokens": int(_flag_value(argv, "--budget") or case.budget_tokens),
            "maxCandidates": int(_flag_value(argv, "--candidates") or "50"),
            "requireFresh": True,
        }
        if case.routes:
            body["routes"] = list(case.routes)
        if case.supply_intent:
            body["intent"] = case.intent
        return "/v1/context", body

    def _daemon_session(self, repository_root: Path) -> DaemonSession:
        """Get-or-create the warm daemon for this repository. Lazy so
        call sites that skip `start_session` (lint-gold, probes) still work;
        atexit guarantees the process is reaped if shutdown() is missed."""
        key = str(repository_root.resolve())
        session = self._daemons.get(key)
        if session is not None and session.alive():
            return session
        if session is not None:
            session.close()
        session = start_daemon(self, repository_root)
        atexit.register(session.close)
        self._daemons[key] = session
        return session

    def _run_daemon(
        self,
        case: BenchmarkCase,
        repository_root: Path,
        system_revision: str,
        endpoint: str,
        body: dict[str, Any],
        raw_sink: TextIO | None = None,
    ) -> CaseResult:
        session = self._daemon_session(repository_root)
        started = time.perf_counter()
        payload = session.post(endpoint, body)
        elapsed_ms = (time.perf_counter() - started) * 1000
        return self._result(
            case,
            system_revision,
            payload,
            elapsed_ms,
            metadata={
                "command": (
                    f"POST {session.base_url}{endpoint} {json.dumps(body, sort_keys=True)}"
                ),
                "engine_latency_ms": payload.get("latencyMs"),
                "session": "daemon",
            },
            raw_sink=raw_sink,
        )

    def _run_subprocess(
        self,
        case: BenchmarkCase,
        repository_root: Path,
        system_revision: str,
        raw_sink: TextIO | None = None,
    ) -> CaseResult:
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
        return self._result(
            case,
            system_revision,
            payload,
            elapsed_ms,
            metadata={
                "command": shlex.join(command),
                "engine_latency_ms": payload.get("latencyMs"),
            },
            raw_sink=raw_sink,
        )

    def _result(
        self,
        case: BenchmarkCase,
        system_revision: str,
        payload: dict[str, Any],
        elapsed_ms: float,
        metadata: dict[str, str | int | float | bool | None],
        raw_sink: TextIO | None = None,
    ) -> CaseResult:
        if raw_sink is not None:
            raw_sink.write(
                json.dumps({"case_id": case.case_id, "payload": payload}) + "\n"
            )
        normalized = normalize_payload(payload)
        return CaseResult(
            case_id=case.case_id,
            system=self.name,
            system_revision=system_revision,
            dataset_revision=case.provenance.dataset_revision,
            retrieved=normalized.retrieved,
            items=normalized.items,
            result_kind=normalized.result_kind,
            used_tokens=normalized.used_tokens,
            verdict_state=normalized.verdict_state,
            metrics_version=METRICS_VERSION,
            abstained=not normalized.retrieved,
            predicted_intent=predicted_intent(payload),
            plan_routes=plan_routes(payload),
            graph_policy=graph_policy(payload),
            missing_capabilities=list(payload.get("missingCapabilities", [])),
            query_ms=elapsed_ms,
            metadata=metadata,
        )

    def component_map(self, repository_root: Path) -> dict[str, str]:
        """Package name → rootDir from the system's architecture map, fetched
        once per run. Any failure (no map support, nonzero exit, bad JSON)
        yields an empty map — component metrics are skipped, never fatal."""
        command = [self.command[0], "--json", "map", str(repository_root)]
        environment = os.environ.copy()
        environment.update(self.environment)
        try:
            completed = subprocess.run(
                command,
                cwd=repository_root,
                env=environment,
                check=False,
                capture_output=True,
                text=True,
                timeout=self.timeout_seconds,
            )
            if completed.returncode != 0:
                return {}
            payload = json.loads(completed.stdout)
        except (OSError, subprocess.TimeoutExpired, json.JSONDecodeError):
            return {}
        packages = payload.get("packages")
        if not isinstance(packages, list):
            return {}
        return {
            str(package["name"]): str(package.get("rootDir", ""))
            for package in packages
            if isinstance(package, dict) and package.get("name")
        }


# Metric-accounting semantics emitted by this adapter version.
# 1 = legacy flat rows (pre-item accounting); 2 = item-aware accounting.
METRICS_VERSION = 2


@dataclass(frozen=True)
class NormalizedPayload:
    """Both result layers derived from one payload. `retrieved` is the
    flat compat view expanded from `items` via `RetrievedItem.to_ranges`
    — there is exactly one address-expansion path, so the layers cannot
    disagree."""

    items: list[RetrievedItem]
    retrieved: list[RetrievedRange]
    result_kind: Literal["search", "context"]
    verdict_state: str | None
    used_tokens: int | None


def normalize_payload(payload: dict[str, Any]) -> NormalizedPayload:
    """Normalize either output shape: a context pack (`items`) or a raw
    search result (`hits`). Both carry source-linked provenance."""
    if "hits" in payload:
        return normalize_search_result(payload)
    return normalize_context_pack(payload)


def _citation(address: dict[str, Any], symbol: str | None) -> LineRange:
    return LineRange(
        path=address["path"],
        start_line=address["startLine"],
        end_line=address["endLine"],
        symbol=symbol or address.get("symbolId"),
    )


def _verdict_state(payload: dict[str, Any]) -> str | None:
    # Search results carry `verdict`; context packs carry `searchVerdict`
    # once the delivery layer exports it. Both use `state`.
    for key in ("verdict", "searchVerdict"):
        verdict = payload.get(key)
        if isinstance(verdict, dict) and verdict.get("state"):
            return str(verdict["state"])
    return None


def normalize_search_result(payload: dict[str, Any]) -> NormalizedPayload:
    items: list[RetrievedItem] = []
    request = payload.get("request") or {}
    for hit in payload.get("hits", []):
        symbol = hit.get("symbolName")
        primary = (
            _citation(hit["address"], symbol) if hit.get("address") else None
        )
        supporting = [
            _citation(address, None) for address in hit.get("evidence", [])
        ]
        items.append(
            RetrievedItem(
                item_id=hit.get("documentId") or hit.get("entityId"),
                rank=max(1, int(hit.get("rank", len(items) + 1))),
                score=float(hit.get("score", 0.0)),
                route=str(hit.get("route", "unknown")),
                symbol=symbol,
                estimated_tokens=0,
                snapshot_id=request.get("snapshotId"),
                region_id=hit.get("regionId"),
                citation_verified=bool(hit.get("verifiedCurrent", False)),
                primary=primary,
                supporting=supporting,
            )
        )
    return NormalizedPayload(
        items=items,
        retrieved=[row for item in items for row in item.to_ranges()],
        result_kind="search",
        verdict_state=_verdict_state(payload),
        used_tokens=None,
    )


def normalize_context_pack(payload: dict[str, Any]) -> NormalizedPayload:
    items: list[RetrievedItem] = []
    for item in payload.get("items", []):
        provenance = item.get("provenance", {})
        symbol = provenance.get("symbolName")
        source = provenance.get("sourceAddress")
        primary = _citation(source, symbol) if source else None
        supporting = [
            _citation(address, None)
            for address in provenance.get("evidenceAddresses", [])
        ]
        items.append(
            RetrievedItem(
                item_id=item.get("id"),
                rank=max(1, int(provenance.get("rank", len(items) + 1))),
                score=float(provenance.get("score", 0.0)),
                route=str(provenance.get("route", "unknown")),
                symbol=symbol,
                estimated_tokens=int(item.get("estimatedTokens", 0)),
                snapshot_id=provenance.get("snapshotId") or payload.get("snapshotId"),
                citation_verified=bool(provenance.get("verifiedCurrent", False)),
                kind=item.get("kind"),
                primary=primary,
                supporting=supporting,
            )
        )
    return NormalizedPayload(
        items=items,
        retrieved=[row for item in items for row in item.to_ranges()],
        result_kind="context",
        verdict_state=_verdict_state(payload),
        used_tokens=(
            int(payload["usedTokens"]) if payload.get("usedTokens") is not None else None
        ),
    )


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
