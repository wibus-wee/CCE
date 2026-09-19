"""Gateway end-to-end smoke: real cce-gateway binary + stub workers.

Spawns the gateway binary and two in-process stub workers, then asserts
the production mechanics that unit tests cannot: fan-out merge across
repos, degraded-repo tolerance, response caching, circuit-breaker
fast-fail, and both metrics surfaces.

    python research/e2e_gateway.py --gateway ./target/debug/cce-gateway

Exits 0 on pass, 1 on first failure (or after listing all failures).
Python standard library only — infra tooling, lives in research/ per the
repo's Python-confinement rule (same justification as loadtest.py).
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

GATEWAY_PORT = 7890
WORKER_A_PORT = 7891
WORKER_B_PORT = 7892


def http(method: str, url: str, body: dict | None = None, timeout: float = 10.0):
    """One request → (status, headers, parsed-json-or-None)."""
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(
        url,
        data=data,
        method=method,
        headers={"content-type": "application/json"} if data else {},
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read()
            return resp.status, dict(resp.headers), _maybe_json(raw)
    except urllib.error.HTTPError as err:
        raw = err.read()
        return err.code, dict(err.headers), _maybe_json(raw)


def _maybe_json(raw: bytes):
    try:
        return json.loads(raw)
    except (ValueError, UnicodeDecodeError):
        return None


class StubHandler(BaseHTTPRequestHandler):
    """Canned daemon responses; `mode` set on the server picks healthy
    (200 + hits) or sick (500) behavior."""

    def do_POST(self):  # noqa: N802 - stdlib naming
        length = int(self.headers.get("content-length", 0))
        self.rfile.read(length)
        self.server.calls += 1
        if self.server.mode == "sick":
            self.send_response(500)
            self.send_header("content-length", "2")
            self.end_headers()
            self.wfile.write(b"{}")
            return
        body = json.dumps(
            {
                "hits": [
                    {
                        "entityId": f"e-{self.server.tag}",
                        "score": 1.0,
                        "documentId": f"doc-{self.server.tag}",
                    }
                ],
                "missingCapabilities": [],
                "verdict": {"state": "answered"},
            }
        ).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):  # noqa: N802
        if self.path == "/healthz":
            body = b'{"status":"ok"}'
            self.send_response(200)
        else:
            body = b"{}"
            self.send_response(404)
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args):
        pass


def start_stub(port: int, tag: str, mode: str = "healthy") -> ThreadingHTTPServer:
    server = ThreadingHTTPServer(("127.0.0.1", port), StubHandler)
    server.tag = tag
    server.mode = mode
    server.calls = 0
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


class Check:
    """Collects assertion failures so the report lists them all."""

    def __init__(self):
        self.failures: list[str] = []

    def expect(self, condition: bool, label: str, detail: str = ""):
        mark = "ok " if condition else "FAIL"
        print(f"  [{mark}] {label}" + (f" — {detail}" if detail and not condition else ""))
        if not condition:
            self.failures.append(label)


def main() -> int:
    parser = argparse.ArgumentParser(prog="e2e_gateway.py")
    parser.add_argument(
        "--gateway",
        default="./target/debug/cce-gateway",
        help="path to the cce-gateway binary",
    )
    args = parser.parse_args()

    check = Check()
    base = f"http://127.0.0.1:{GATEWAY_PORT}"
    stub_a = start_stub(WORKER_A_PORT, "alpha")
    stub_b = start_stub(WORKER_B_PORT, "beta")

    with tempfile.TemporaryDirectory() as data_dir:
        gateway = subprocess.Popen(
            [
                args.gateway,
                "--data-dir",
                data_dir,
                "--bind",
                f"127.0.0.1:{GATEWAY_PORT}",
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        try:
            deadline = time.time() + 15
            while time.time() < deadline:
                try:
                    status, _, _ = http("GET", f"{base}/healthz", timeout=1)
                    if status == 200:
                        break
                except (urllib.error.URLError, TimeoutError, ConnectionError):
                    time.sleep(0.2)
            else:
                print("gateway did not become healthy")
                return 1

            print("register repos:")
            status, _, body = http(
                "POST",
                f"{base}/v1/repos",
                {"id": "alpha", "name": "alpha", "workerUrl": f"http://127.0.0.1:{WORKER_A_PORT}"},
            )
            check.expect(status == 200, "register alpha", f"status {status}")
            status, _, body = http(
                "POST",
                f"{base}/v1/repos",
                {"id": "beta", "name": "beta", "workerUrl": f"http://127.0.0.1:{WORKER_B_PORT}"},
            )
            check.expect(status == 200, "register beta", f"status {status}")

            print("proxy + cache:")
            search = {"query": "alpha-symbol", "limit": 5}
            status, _, body = http("POST", f"{base}/alpha/v1/search", search)
            check.expect(status == 200, "proxy search 200", f"status {status}")
            status, headers, _ = http("POST", f"{base}/alpha/v1/search", search)
            check.expect(
                headers.get("x-cce-cache") == "hit",
                "repeat search replays from cache",
                f"header {headers.get('x-cce-cache')}",
            )
            check.expect(
                stub_a.calls == 1,
                "cache hit skipped the worker",
                f"worker calls {stub_a.calls}",
            )

            print("fan-out:")
            status, _, body = http(
                "POST", f"{base}/v1/search/all", {"query": "q", "limit": 5}
            )
            check.expect(status == 200, "fanout 200", f"status {status}")
            hits = (body or {}).get("hits", [])
            repos = {hit.get("repo") for hit in hits}
            check.expect(
                repos == {"alpha", "beta"},
                "fanout merged hits tagged per repo",
                f"repos {repos}",
            )
            per_repo = {r["repoSlug"]: r["hitCount"] for r in (body or {}).get("perRepo", [])}
            check.expect(
                per_repo == {"alpha": 1, "beta": 1},
                "perRepo accounting",
                f"perRepo {per_repo}",
            )

            print("degraded tolerance:")
            # server_close releases the listen socket too — without it the
            # kernel still accepts connections that nobody answers, and the
            # gateway's 10s fanout timeout would be the only tell.
            stub_b.shutdown()
            stub_b.server_close()
            status, _, body = http(
                "POST", f"{base}/v1/search/all", {"query": "q", "limit": 5}
            )
            check.expect(status == 200, "fanout survives a dead repo", f"status {status}")
            degraded = {d["repoSlug"] for d in (body or {}).get("degraded", [])}
            check.expect(
                "beta" in degraded,
                "dead repo reported degraded",
                f"degraded {degraded}",
            )
            check.expect(
                {h.get("repo") for h in (body or {}).get("hits", [])} == {"alpha"},
                "live repo still merged",
            )

            print("circuit breaker:")
            for _ in range(3):
                http("POST", f"{base}/beta/v1/search", {"query": "x", "limit": 1})
            started = time.time()
            status, headers, _ = http(
                "POST", f"{base}/beta/v1/search", {"query": "x2", "limit": 1}
            )
            elapsed = time.time() - started
            check.expect(
                status == 503 and headers.get("x-cce-breaker") == "open",
                "open circuit fast-fails",
                f"status {status} breaker {headers.get('x-cce-breaker')}",
            )
            check.expect(elapsed < 0.5, "fast-fail is fast", f"{elapsed:.3f}s")

            print("mcp gated the same way:")
            started = time.time()
            status, _, body = http(
                "POST",
                f"{base}/beta/mcp",
                {
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {"name": "cce_status", "arguments": {}},
                },
            )
            elapsed = time.time() - started
            # The MCP surface degrades to tool isError content, not a hung
            # request or a bare 5xx — the breaker is shared with the proxy.
            content = json.dumps(body or {})
            check.expect(
                status == 200 and ("isError" in content or "error" in content),
                "mcp call fast-fails as tool error on open circuit",
                f"status {status} body {str(body)[:120]}",
            )
            check.expect(elapsed < 0.5, "mcp fast-fail is fast", f"{elapsed:.3f}s")

            print("metrics:")
            status, headers, raw_metrics = (
                *http("GET", f"{base}/metrics")[:2],
                urllib.request.urlopen(f"{base}/metrics").read().decode(),
            )
            check.expect(status == 200, "GET /metrics 200")
            check.expect(
                "cce_gateway_proxy_ok_total" in raw_metrics
                and "cce_gateway_cache_hit_total" in raw_metrics,
                "prometheus series present",
            )
            status, _, body = http("GET", f"{base}/v1/metrics")
            check.expect(status == 200, "GET /v1/metrics 200")
            check.expect(
                (body or {}).get("cache", {}).get("hit", 0) >= 1,
                "cache hit counted",
                f"body {body}",
            )
            breakers = (body or {}).get("breakers", {})
            check.expect(
                breakers.get("beta") in {"open", "half_open"} or any(
                    v in {"open", "half_open"} for v in breakers.values()
                ),
                "breaker state exported",
                f"breakers {breakers}",
            )
        finally:
            gateway.terminate()
            try:
                gateway.wait(timeout=5)
            except subprocess.TimeoutExpired:
                gateway.kill()

    stub_a.shutdown()
    stub_a.server_close()
    print()
    if check.failures:
        print(f"E2E FAIL — {len(check.failures)} check(s): {check.failures}")
        return 1
    print("E2E PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
