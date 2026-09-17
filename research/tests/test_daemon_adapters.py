import json
import subprocess
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

import pytest
import yaml

from cce_research import adapters
from cce_research.adapters import Adapter, DaemonRequestUnsupported, DaemonSession
from cce_research.schema import BenchmarkCase, Provenance

SEARCH_PAYLOAD: dict[str, Any] = {
    "hits": [
        {
            "address": {
                "path": "src/a.rs",
                "startLine": 1,
                "endLine": 9,
                "symbolId": "a::f",
            },
            "evidence": [],
            "route": "lexical",
            "rank": 1,
            "score": 0.5,
            "verifiedCurrent": True,
        }
    ],
    "latencyMs": 7,
    "plan": {"intent": "impact", "routes": ["lexical"], "graphPolicy": "none"},
}

CONTEXT_PAYLOAD: dict[str, Any] = {
    "items": [
        {
            "estimatedTokens": 12,
            "provenance": {
                "sourceAddress": {"path": "src/a.rs", "startLine": 1, "endLine": 9},
                "evidenceAddresses": [],
                "route": "lexical",
                "rank": 1,
                "score": 0.5,
                "verifiedCurrent": True,
            },
        }
    ],
    "latencyMs": 9,
    "intent": "impact",
    "planRoutes": ["lexical"],
    "graphPolicy": "none",
}


def _case(**overrides: object) -> BenchmarkCase:
    fields: dict[str, object] = {
        "case_id": "c1",
        "repository": "example/repo",
        "revision": "WORKTREE",
        "query": "q",
        "intent": "impact",
        "gold_files": ["src/a.rs"],
        "provenance": Provenance(
            source_url="https://example.com/repo",
            dataset_revision="test-v1",
            license_spdx="Apache-2.0",
            redistribution="allowed",
            construction_method="test fixture",
        ),
    }
    fields.update(overrides)
    return BenchmarkCase.model_validate(fields)


def _write_adapter(tmp_path: Path, **extra: object) -> Path:
    raw: dict[str, object] = {
        "name": "test-daemon",
        "command": [
            "cce",
            "--json",
            "search",
            "{repository}",
            "{query}",
            "{intent_args}",
            "{route_args}",
            "--limit",
            "50",
        ],
        "environment": {},
    }
    raw.update(extra)
    path = tmp_path / "adapter.yaml"
    path.write_text(yaml.safe_dump(raw), encoding="utf-8")
    return path


class _FakeDaemon(BaseHTTPRequestHandler):
    """Minimal stand-in for cce-daemon: canned payloads, records bodies."""

    def _reply(self, payload: dict[str, Any], status: int = 200) -> None:
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:
        if self.path == "/healthz":
            self._reply({"status": "ok", "version": "test"})
        else:
            self._reply({"error": "not found"}, status=404)

    def do_POST(self) -> None:
        length = int(self.headers.get("Content-Length", "0"))
        self.server.last_body = json.loads(self.rfile.read(length))  # type: ignore[attr-defined]
        self.server.last_path = self.path  # type: ignore[attr-defined]
        if self.path == "/v1/search":
            self._reply(SEARCH_PAYLOAD)
        elif self.path == "/v1/context":
            self._reply(CONTEXT_PAYLOAD)
        else:
            self._reply({"error": "not found"}, status=404)

    def log_message(self, *args: object) -> None:
        pass


@pytest.fixture
def fake_daemon() -> Any:
    server = ThreadingHTTPServer(("127.0.0.1", 0), _FakeDaemon)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    yield f"http://127.0.0.1:{server.server_address[1]}", server
    server.shutdown()
    thread.join(timeout=5)


def _attach_fake(adapter: Adapter, base_url: str, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        adapters,
        "start_daemon",
        lambda a, repo: DaemonSession(base_url, a.timeout_seconds),
    )


def test_session_defaults_to_subprocess(tmp_path: Path) -> None:
    adapter = Adapter.load(_write_adapter(tmp_path))
    assert adapter.session == "subprocess"


def test_invalid_session_is_rejected(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="session must be one of"):
        Adapter.load(_write_adapter(tmp_path, session="fork-server"))


def test_daemon_search_request_maps_template_flags(tmp_path: Path) -> None:
    adapter = Adapter.load(_write_adapter(tmp_path, session="daemon"))
    endpoint, body = adapter.daemon_request(_case(), Path("/repo"))
    assert endpoint == "/v1/search"
    assert body == {"query": "q", "limit": 50, "intent": "impact"}


def test_daemon_request_omits_intent_when_withheld(tmp_path: Path) -> None:
    adapter = Adapter.load(_write_adapter(tmp_path, session="daemon"))
    _, body = adapter.daemon_request(_case(supply_intent=False), Path("/repo"))
    assert "intent" not in body


def test_daemon_search_request_rejects_pinned_routes(tmp_path: Path) -> None:
    adapter = Adapter.load(_write_adapter(tmp_path, session="daemon"))
    with pytest.raises(DaemonRequestUnsupported, match="cannot pin routes"):
        adapter.daemon_request(_case(routes=["lexical"]), Path("/repo"))


def test_daemon_context_request_maps_budget_candidates_routes(tmp_path: Path) -> None:
    adapter = Adapter.load(
        _write_adapter(
            tmp_path,
            session="daemon",
            command=[
                "cce",
                "--json",
                "context",
                "{repository}",
                "{query}",
                "{intent_args}",
                "{route_args}",
                "--budget",
                "{budget}",
                "--candidates",
                "80",
            ],
        )
    )
    case = _case(routes=["lexical"], budget_tokens=4096)
    endpoint, body = adapter.daemon_request(case, Path("/repo"))
    assert endpoint == "/v1/context"
    assert body == {
        "query": "q",
        "intent": "impact",
        "budgetTokens": 4096,
        "maxCandidates": 80,
        "requireFresh": True,
        "routes": ["lexical"],
    }


def test_daemon_run_posts_case_and_normalizes(
    tmp_path: Path, fake_daemon: Any, monkeypatch: pytest.MonkeyPatch
) -> None:
    base_url, server = fake_daemon
    adapter = Adapter.load(_write_adapter(tmp_path, session="daemon"))
    _attach_fake(adapter, base_url, monkeypatch)
    result = adapter.run(_case(), Path("/repo"), "rev")
    adapter.shutdown()
    assert server.last_path == "/v1/search"
    assert server.last_body == {"query": "q", "limit": 50, "intent": "impact"}
    assert [item.path for item in result.retrieved] == ["src/a.rs"]
    assert result.metadata["session"] == "daemon"
    assert result.metadata["engine_latency_ms"] == 7
    assert str(result.metadata["command"]).startswith(f"POST {base_url}/v1/search")
    assert result.query_ms >= 0


def test_daemon_run_falls_back_for_routed_search_case(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    adapter = Adapter.load(_write_adapter(tmp_path, session="daemon"))
    commands: list[list[str]] = []

    def fake_run(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        commands.append(command)
        return subprocess.CompletedProcess(command, 0, stdout=json.dumps(SEARCH_PAYLOAD), stderr="")

    monkeypatch.setattr(subprocess, "run", fake_run)
    result = adapter.run(_case(routes=["lexical"]), Path("/repo"), "rev")
    assert result.metadata["session"] == "subprocess-fallback"
    assert "--route" in commands[0]
    assert [item.path for item in result.retrieved] == ["src/a.rs"]


def test_subprocess_run_is_unchanged(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    adapter = Adapter.load(_write_adapter(tmp_path))
    commands: list[list[str]] = []

    def fake_run(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        commands.append(command)
        return subprocess.CompletedProcess(command, 0, stdout=json.dumps(SEARCH_PAYLOAD), stderr="")

    monkeypatch.setattr(subprocess, "run", fake_run)
    result = adapter.run(_case(), Path("/repo"), "rev")
    assert commands[0][:4] == ["cce", "--json", "search", "/repo"]
    assert "session" not in result.metadata
    assert result.metadata["command"] == "cce --json search /repo q --intent impact --limit 50"
    assert [item.path for item in result.retrieved] == ["src/a.rs"]
