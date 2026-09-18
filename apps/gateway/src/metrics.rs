//! In-process metrics for the gateway: every counter is an atomic, so
//! instrumentation costs a few nanoseconds and never locks. Two views
//! over the same series — `render_prometheus` for scrapers
//! (`GET /metrics`, text exposition format) and `snapshot` for
//! `GET /v1/metrics` JSON consumers.
//!
//! Series are fixed at compile time: no label-keyed maps, no dynamic
//! registration — the gateway's metric surface is small and stable.

use std::fmt::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Upstream-call latency and push-duration bucket edges, milliseconds.
/// Fixed boundaries keep the histogram allocation-free.
const HIST_BOUNDS_MS: [u64; 10] = [50, 100, 250, 500, 1000, 2500, 5000, 15000, 30000, 120_000];

/// Fixed-boundary histogram: bucket counts plus sum and count — enough
/// for a Prometheus `histogram` series and pXX estimates from buckets.
#[derive(Debug)]
struct Histogram {
    /// `buckets[i]` counts observations <= `HIST_BOUNDS_MS[i]`; the last
    /// slot is the implicit +Inf bucket.
    buckets: [AtomicU64; HIST_BOUNDS_MS.len() + 1],
    sum: AtomicU64,
    count: AtomicU64,
}

impl Histogram {
    fn new() -> Self {
        Self {
            buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            sum: AtomicU64::new(0),
            count: AtomicU64::new(0),
        }
    }

    fn observe(&self, ms: u64) {
        let index = HIST_BOUNDS_MS
            .iter()
            .position(|bound| ms <= *bound)
            .unwrap_or(HIST_BOUNDS_MS.len());
        if let Some(bucket) = self.buckets.get(index) {
            bucket.fetch_add(1, Ordering::Relaxed);
        }
        self.sum.fetch_add(ms, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    /// Cumulative `le` counts as Prometheus expects them — each line is
    /// "observations <= bound", so the last entry equals `count`.
    fn cumulative(&self) -> Vec<u64> {
        let mut running = 0_u64;
        self.buckets
            .iter()
            .map(|bucket| {
                running += bucket.load(Ordering::Relaxed);
                running
            })
            .collect()
    }
}

/// Gateway counters. `record_*` methods are the only mutation surface;
/// render/snapshot read Relaxed — counters are monotonic approximations,
/// never transactional state.
#[derive(Debug)]
pub(crate) struct Metrics {
    started: Instant,
    // Proxy outcomes — every proxied request lands in exactly one.
    proxy_ok: AtomicU64,
    proxy_timeout: AtomicU64,
    proxy_connect: AtomicU64,
    proxy_other: AtomicU64,
    /// Shed by the upstream concurrency bound before dispatch.
    proxy_shed: AtomicU64,
    /// Fast-failed while the worker's circuit was open.
    proxy_breaker: AtomicU64,
    cache_hit: AtomicU64,
    cache_miss: AtomicU64,
    /// Coalesced onto an in-flight singleflight leader.
    cache_follower: AtomicU64,
    upstream_ms: Histogram,
    // Fan-out.
    fanout_requests: AtomicU64,
    fanout_repos_searched: AtomicU64,
    fanout_repos_degraded: AtomicU64,
    // Push lifecycle.
    push_ok: AtomicU64,
    push_failed: AtomicU64,
    push_files: AtomicU64,
    push_ms: Histogram,
}

impl Metrics {
    pub(crate) fn new() -> Self {
        Self {
            started: Instant::now(),
            proxy_ok: AtomicU64::new(0),
            proxy_timeout: AtomicU64::new(0),
            proxy_connect: AtomicU64::new(0),
            proxy_other: AtomicU64::new(0),
            proxy_shed: AtomicU64::new(0),
            proxy_breaker: AtomicU64::new(0),
            cache_hit: AtomicU64::new(0),
            cache_miss: AtomicU64::new(0),
            cache_follower: AtomicU64::new(0),
            upstream_ms: Histogram::new(),
            fanout_requests: AtomicU64::new(0),
            fanout_repos_searched: AtomicU64::new(0),
            fanout_repos_degraded: AtomicU64::new(0),
            push_ok: AtomicU64::new(0),
            push_failed: AtomicU64::new(0),
            push_files: AtomicU64::new(0),
            push_ms: Histogram::new(),
        }
    }

    pub(crate) fn uptime_seconds(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    /// One proxied request by terminal outcome: `"ok"` (worker answered,
    /// any status), `"timeout"`, `"connect"`, `"other"` transport,
    /// `"shed"`, `"breaker"`, `"cache_hit"`, `"cache_miss"`,
    /// `"singleflight"`. Unknown kinds fold into `"other"`.
    pub(crate) fn record_proxy(&self, kind: &'static str) {
        let counter = match kind {
            "ok" => &self.proxy_ok,
            "timeout" => &self.proxy_timeout,
            "connect" => &self.proxy_connect,
            "shed" => &self.proxy_shed,
            "breaker" => &self.proxy_breaker,
            "cache_hit" => &self.cache_hit,
            "cache_miss" => &self.cache_miss,
            "singleflight" => &self.cache_follower,
            _ => &self.proxy_other,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// Wall-clock latency of one upstream worker call, milliseconds.
    pub(crate) fn record_upstream_ms(&self, ms: u64) {
        self.upstream_ms.observe(ms);
    }

    /// One `/v1/search/all` request: how many repos answered vs degraded.
    pub(crate) fn record_fanout(&self, searched: usize, degraded: usize) {
        self.fanout_requests.fetch_add(1, Ordering::Relaxed);
        self.fanout_repos_searched
            .fetch_add(searched as u64, Ordering::Relaxed);
        self.fanout_repos_degraded
            .fetch_add(degraded as u64, Ordering::Relaxed);
    }

    /// One push commit: outcome, materialized files, wall time.
    pub(crate) fn record_push(&self, ok: bool, files: u64, elapsed_ms: u64) {
        if ok {
            self.push_ok.fetch_add(1, Ordering::Relaxed);
        } else {
            self.push_failed.fetch_add(1, Ordering::Relaxed);
        }
        self.push_files.fetch_add(files, Ordering::Relaxed);
        self.push_ms.observe(elapsed_ms);
    }

    /// Prometheus text exposition (0.0.4): counters as `*_total`,
    /// histograms with cumulative `le` buckets plus `_sum`/`_count`.
    pub(crate) fn render_prometheus(&self) -> String {
        let mut out = String::with_capacity(4096);
        let counter = |out: &mut String, name: &str, help: &str, value: u64| {
            let _ = writeln!(
                out,
                "# HELP {name} {help}\n# TYPE {name} counter\n{name} {value}"
            );
        };
        let histogram = |out: &mut String, name: &str, help: &str, hist: &Histogram| {
            let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} histogram");
            let cumulative = hist.cumulative();
            for (index, bound) in HIST_BOUNDS_MS.iter().enumerate() {
                let le_count = cumulative.get(index).copied().unwrap_or_default();
                let _ = writeln!(out, "{name}_bucket{{le=\"{bound}\"}} {le_count}");
            }
            let _ = writeln!(
                out,
                "{name}_bucket{{le=\"+Inf\"}} {}\n{name}_sum {}\n{name}_count {}",
                cumulative.last().copied().unwrap_or_default(),
                hist.sum.load(Ordering::Relaxed),
                hist.count.load(Ordering::Relaxed),
            );
        };
        counter(
            &mut out,
            "cce_gateway_uptime_seconds",
            "Seconds since the gateway process started.",
            self.uptime_seconds(),
        );
        counter(
            &mut out,
            "cce_gateway_proxy_ok_total",
            "Proxied requests the worker answered (any status).",
            self.proxy_ok.load(Ordering::Relaxed),
        );
        counter(
            &mut out,
            "cce_gateway_proxy_timeout_total",
            "Proxied requests that timed out against the worker.",
            self.proxy_timeout.load(Ordering::Relaxed),
        );
        counter(
            &mut out,
            "cce_gateway_proxy_connect_total",
            "Proxied requests that failed to connect to the worker.",
            self.proxy_connect.load(Ordering::Relaxed),
        );
        counter(
            &mut out,
            "cce_gateway_proxy_other_total",
            "Proxied requests failed by other transport errors.",
            self.proxy_other.load(Ordering::Relaxed),
        );
        counter(
            &mut out,
            "cce_gateway_proxy_shed_total",
            "Requests shed by the upstream concurrency bound.",
            self.proxy_shed.load(Ordering::Relaxed),
        );
        counter(
            &mut out,
            "cce_gateway_proxy_breaker_total",
            "Requests fast-failed on an open worker circuit.",
            self.proxy_breaker.load(Ordering::Relaxed),
        );
        counter(
            &mut out,
            "cce_gateway_cache_hit_total",
            "Cacheable search requests replayed from cache.",
            self.cache_hit.load(Ordering::Relaxed),
        );
        counter(
            &mut out,
            "cce_gateway_cache_miss_total",
            "Cacheable search requests that fetched upstream.",
            self.cache_miss.load(Ordering::Relaxed),
        );
        counter(
            &mut out,
            "cce_gateway_cache_singleflight_total",
            "Search requests coalesced onto an in-flight leader.",
            self.cache_follower.load(Ordering::Relaxed),
        );
        counter(
            &mut out,
            "cce_gateway_fanout_requests_total",
            "Fan-out search requests served.",
            self.fanout_requests.load(Ordering::Relaxed),
        );
        counter(
            &mut out,
            "cce_gateway_fanout_repos_searched_total",
            "Repos that answered a fan-out branch.",
            self.fanout_repos_searched.load(Ordering::Relaxed),
        );
        counter(
            &mut out,
            "cce_gateway_fanout_repos_degraded_total",
            "Repos degraded inside a fan-out request.",
            self.fanout_repos_degraded.load(Ordering::Relaxed),
        );
        counter(
            &mut out,
            "cce_gateway_push_ok_total",
            "Pushes materialized successfully.",
            self.push_ok.load(Ordering::Relaxed),
        );
        counter(
            &mut out,
            "cce_gateway_push_failed_total",
            "Pushes that failed before materializing.",
            self.push_failed.load(Ordering::Relaxed),
        );
        counter(
            &mut out,
            "cce_gateway_push_files_total",
            "Files materialized across successful pushes.",
            self.push_files.load(Ordering::Relaxed),
        );
        histogram(
            &mut out,
            "cce_gateway_upstream_milliseconds",
            "Upstream worker call latency in milliseconds.",
            &self.upstream_ms,
        );
        histogram(
            &mut out,
            "cce_gateway_push_milliseconds",
            "Push materialization wall time in milliseconds.",
            &self.push_ms,
        );
        out
    }

    /// JSON snapshot mirroring the Prometheus series — same counters,
    /// histograms as boundary→count maps plus sum/count.
    pub(crate) fn snapshot(&self) -> serde_json::Value {
        let histogram = |hist: &Histogram| {
            let cumulative = hist.cumulative();
            let mut buckets = serde_json::Map::new();
            for (index, bound) in HIST_BOUNDS_MS.iter().enumerate() {
                buckets.insert(
                    bound.to_string(),
                    serde_json::Value::from(cumulative.get(index).copied().unwrap_or_default()),
                );
            }
            buckets.insert(
                "+Inf".to_owned(),
                serde_json::Value::from(cumulative.last().copied().unwrap_or_default()),
            );
            serde_json::json!({
                "buckets": buckets,
                "sum": hist.sum.load(Ordering::Relaxed),
                "count": hist.count.load(Ordering::Relaxed),
            })
        };
        serde_json::json!({
            "uptimeSeconds": self.uptime_seconds(),
            "proxy": {
                "ok": self.proxy_ok.load(Ordering::Relaxed),
                "timeout": self.proxy_timeout.load(Ordering::Relaxed),
                "connect": self.proxy_connect.load(Ordering::Relaxed),
                "other": self.proxy_other.load(Ordering::Relaxed),
                "shed": self.proxy_shed.load(Ordering::Relaxed),
                "breaker": self.proxy_breaker.load(Ordering::Relaxed),
            },
            "cache": {
                "hit": self.cache_hit.load(Ordering::Relaxed),
                "miss": self.cache_miss.load(Ordering::Relaxed),
                "singleflight": self.cache_follower.load(Ordering::Relaxed),
            },
            "upstreamMs": histogram(&self.upstream_ms),
            "fanout": {
                "requests": self.fanout_requests.load(Ordering::Relaxed),
                "reposSearched": self.fanout_repos_searched.load(Ordering::Relaxed),
                "reposDegraded": self.fanout_repos_degraded.load(Ordering::Relaxed),
            },
            "push": {
                "ok": self.push_ok.load(Ordering::Relaxed),
                "failed": self.push_failed.load(Ordering::Relaxed),
                "files": self.push_files.load(Ordering::Relaxed),
                "durationMs": histogram(&self.push_ms),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_proxy_folds_unknown_kinds_into_other() {
        let metrics = Metrics::new();
        metrics.record_proxy("ok");
        metrics.record_proxy("timeout");
        metrics.record_proxy("made-up-kind");
        let snap = metrics.snapshot();
        assert_eq!(snap["proxy"]["ok"], 1);
        assert_eq!(snap["proxy"]["timeout"], 1);
        assert_eq!(snap["proxy"]["other"], 1);
    }

    #[test]
    fn histogram_boundary_is_inclusive() {
        let metrics = Metrics::new();
        metrics.record_upstream_ms(100);
        metrics.record_upstream_ms(120_001);
        let snap = metrics.snapshot();
        assert_eq!(snap["upstreamMs"]["buckets"]["100"], 1);
        assert_eq!(snap["upstreamMs"]["buckets"]["+Inf"], 2);
        assert_eq!(snap["upstreamMs"]["count"], 2);
    }

    #[test]
    fn prometheus_renders_typed_series() {
        let metrics = Metrics::new();
        metrics.record_proxy("shed");
        metrics.record_fanout(3, 1);
        metrics.record_push(true, 42, 7);
        let text = metrics.render_prometheus();
        assert!(text.contains("# TYPE cce_gateway_proxy_shed_total counter"));
        assert!(text.contains("cce_gateway_proxy_shed_total 1"));
        assert!(text.contains("cce_gateway_fanout_repos_degraded_total 1"));
        assert!(text.contains("cce_gateway_push_files_total 42"));
        assert!(text.contains("cce_gateway_push_milliseconds_bucket{le=\"+Inf\"} 1"));
        assert!(text.contains("cce_gateway_uptime_seconds"));
    }
}
