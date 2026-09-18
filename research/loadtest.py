"""Repeatable HTTP load-test driver for gateway/daemon performance checks.

Fires requests at a target endpoint under a concurrency sweep and reports,
per level: wall time, achieved QPS, latency p50/p95/p99 (ms), error count
(transport errors + non-2xx, with a status-code distribution), and — when
the header is present — the share of responses carrying ``x-cce-cache: hit``.

Example:
    python research/loadtest.py http://127.0.0.1:7820/v1/search/all --method POST --body '{"query":"x","limit":10}' --concurrency 1,8,32 --requests 200

Python standard library only. Lives in research/ per the repo's
Python-confinement rule: this is infra tooling, not an algorithm experiment.
"""

from __future__ import annotations

import argparse
import itertools
import json
import sys
import threading
import time
import urllib.error
import urllib.request
from collections import Counter
from concurrent.futures import ThreadPoolExecutor

CACHE_HEADER = "x-cce-cache"


def parse_args(argv=None):
    p = argparse.ArgumentParser(
        prog="loadtest.py",
        description="Concurrency-sweep HTTP load test (stdlib only).",
    )
    p.add_argument("url", help="target endpoint URL")
    p.add_argument(
        "--method",
        type=str.upper,
        choices=["GET", "POST"],
        default=None,
        help="HTTP method (default: POST when --body is set, else GET)",
    )
    p.add_argument(
        "--body",
        default=None,
        help="request body as a JSON string, or @path/to/file.json",
    )
    p.add_argument(
        "--header",
        action="append",
        default=[],
        metavar='"Name: value"',
        help="extra request header; repeatable",
    )
    p.add_argument(
        "--concurrency",
        default="1,4,8,16",
        help="comma-separated concurrency levels (default: 1,4,8,16)",
    )
    mode = p.add_mutually_exclusive_group()
    mode.add_argument(
        "--requests",
        type=int,
        default=100,
        help="requests per concurrency level (default: 100)",
    )
    mode.add_argument(
        "--duration",
        type=float,
        default=None,
        help="seconds to run per level (alternative to --requests)",
    )
    p.add_argument(
        "--warmup",
        type=int,
        default=5,
        help="discarded warmup requests at the first level (default: 5)",
    )
    p.add_argument(
        "--timeout",
        type=float,
        default=30.0,
        help="per-request timeout in seconds (default: 30)",
    )
    args = p.parse_args(argv)

    headers = {}
    for h in args.header:
        if ":" not in h:
            p.error(f"--header must be 'Name: value', got: {h!r}")
        name, value = h.split(":", 1)
        name = name.strip()
        if not name:
            p.error(f"--header has an empty name: {h!r}")
        headers[name] = value.strip()

    body = None
    if args.body is not None:
        if args.body.startswith("@"):
            path = args.body[1:]
            try:
                with open(path, "rb") as fh:
                    body = fh.read()
            except OSError as exc:
                p.error(f"cannot read --body file {path!r}: {exc}")
            text = body.decode("utf-8", "replace")
        else:
            body = args.body.encode("utf-8")
            text = args.body
        try:
            json.loads(text)
        except ValueError:
            print("warning: --body is not valid JSON; sending raw bytes", file=sys.stderr)
        else:
            if not any(k.lower() == "content-type" for k in headers):
                headers["Content-Type"] = "application/json"

    levels = []
    for tok in args.concurrency.split(","):
        tok = tok.strip()
        if not tok:
            continue
        try:
            level = int(tok)
        except ValueError:
            p.error(f"bad --concurrency value: {tok!r}")
        if level < 1:
            p.error(f"--concurrency levels must be >= 1, got {level}")
        if level not in levels:
            levels.append(level)
    if not levels:
        p.error("--concurrency produced no levels")

    if args.requests < 0:
        p.error("--requests must be >= 0")
    if args.duration is not None and args.duration <= 0:
        p.error("--duration must be > 0")
    if args.warmup < 0:
        p.error("--warmup must be >= 0")
    if args.timeout <= 0:
        p.error("--timeout must be > 0")

    args.method = args.method or ("POST" if body is not None else "GET")
    args.body_bytes = body
    args.parsed_headers = headers
    args.levels = levels
    return args


def send_once(opener, url, method, headers, body, timeout):
    """One request -> (latency_ms, status|None, cache_header|None).

    status None marks a transport-level error (URLError, timeout, reset).
    Non-2xx responses still record their status code via HTTPError.
    """
    req = urllib.request.Request(url, data=body, headers=headers, method=method)
    t0 = time.perf_counter()
    try:
        with opener.open(req, timeout=timeout) as resp:
            resp.read()
            status = resp.status
            cache = resp.headers.get(CACHE_HEADER)
    except urllib.error.HTTPError as exc:
        try:
            exc.read()
        except Exception:
            pass
        status = exc.code
        cache = exc.headers.get(CACHE_HEADER) if exc.headers else None
    except Exception:
        status = None
        cache = None
    latency_ms = (time.perf_counter() - t0) * 1000.0
    return latency_ms, status, cache


def run_level(level, limit, duration_s, req_args):
    """Fire requests at `level` concurrency.

    Stops after `limit` total requests (atomic ticket counter) or, when
    `duration_s` is set, once the deadline passes. Returns (results, wall_s).
    """
    url, method, headers, body, timeout = req_args
    results = []
    tickets = itertools.count()
    lock = threading.Lock()
    deadline = time.monotonic() + duration_s if duration_s is not None else None

    def worker():
        # One opener per worker; env proxies are bypassed so loopback
        # measurements are not silently routed through a proxy.
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        while True:
            if deadline is not None and time.monotonic() >= deadline:
                return
            with lock:
                n = next(tickets)
            if limit is not None and n >= limit:
                return
            results.append(send_once(opener, url, method, headers, body, timeout))

    t0 = time.perf_counter()
    with ThreadPoolExecutor(max_workers=level) as pool:
        futures = [pool.submit(worker) for _ in range(level)]
        for fut in futures:
            fut.result()
    return results, time.perf_counter() - t0


def percentile(sorted_vals, pct):
    """Linear-interpolated percentile of a sorted list; None when empty."""
    n = len(sorted_vals)
    if n == 0:
        return None
    if n == 1:
        return sorted_vals[0]
    rank = (pct / 100.0) * (n - 1)
    lo = int(rank)
    hi = min(lo + 1, n - 1)
    frac = rank - lo
    return sorted_vals[lo] + (sorted_vals[hi] - sorted_vals[lo]) * frac


def is_ok(status):
    return status is not None and 200 <= status < 300


def fmt_ms(value):
    return f"{value:9.2f}" if value is not None else f"{'-':>9}"


def main(argv=None):
    args = parse_args(argv)
    req_args = (args.url, args.method, args.parsed_headers, args.body_bytes, args.timeout)

    if args.duration is not None:
        mode = f"duration={args.duration:g}s/level"
    else:
        mode = f"requests={args.requests}/level"
    print(f"target: {args.url}  method={args.method}  timeout={args.timeout:g}s  {mode}")

    if args.warmup > 0:
        wres, wwall = run_level(args.levels[0], args.warmup, None, req_args)
        print(
            f"warmup: {len(wres)} requests @ conc={args.levels[0]} "
            f"in {wwall:.2f}s (discarded)"
        )

    header = (
        f"{'conc':>5} {'reqs':>6} {'wall_s':>8} {'qps':>9} {'p50_ms':>9} "
        f"{'p95_ms':>9} {'p99_ms':>9} {'errs':>6} {'cache_hit%':>11}"
    )
    print()
    print(header)
    print("-" * len(header))

    rows = []
    for level in args.levels:
        limit = None if args.duration is not None else args.requests
        results, wall = run_level(level, limit, args.duration, req_args)
        rows.append((level, results, wall))

        lat = sorted(r[0] for r in results)
        n = len(results)
        ok = sum(1 for r in results if is_ok(r[1]))
        transport = sum(1 for r in results if r[1] is None)
        errors = n - ok
        n_resp = n - transport
        hits = sum(1 for r in results if r[2] == "hit")
        header_seen = any(r[2] is not None for r in results)
        hit_str = f"{100.0 * hits / n_resp:.1f}%" if header_seen and n_resp else "-"
        qps = n / wall if wall > 0 else 0.0

        print(
            f"{level:>5} {n:>6} {wall:>8.2f} {qps:>9.1f} "
            f"{fmt_ms(percentile(lat, 50))} {fmt_ms(percentile(lat, 95))} "
            f"{fmt_ms(percentile(lat, 99))} {errors:>6} {hit_str:>11}"
        )

    for level, results, _wall in rows:
        transport = sum(1 for r in results if r[1] is None)
        dist = Counter(r[1] for r in results if r[1] is not None and not is_ok(r[1]))
        if transport or dist:
            parts = ", ".join(f"{code}x{cnt}" for code, cnt in sorted(dist.items()))
            line = f"conc={level}: transport errors={transport}"
            if parts:
                line += f", non-2xx: {parts}"
            print(line)

    total = sum(len(r) for _lv, r, _w in rows)
    total_ok = sum(1 for _lv, r, _w in rows for x in r if is_ok(x[1]))
    total_wall = sum(w for _lv, _r, w in rows)
    overall_qps = total / total_wall if total_wall > 0 else 0.0
    best_level, best_results, best_wall = max(
        rows, key=lambda row: (len(row[1]) / row[2] if row[2] > 0 else 0.0)
    )
    best_qps = len(best_results) / best_wall if best_wall > 0 else 0.0

    print()
    print(
        f"summary: {total} reqs | {total_ok} ok | {total - total_ok} err | "
        f"wall {total_wall:.2f}s | overall qps {overall_qps:.1f} | "
        f"best conc={best_level} ({best_qps:.1f} qps)"
    )
    return 0 if total_ok > 0 else 1


if __name__ == "__main__":
    sys.exit(main())
