//! Cross-repo search fan-out: `POST /v1/search/all` scatters one query to
//! every selected repo's worker and interleaves the per-repo result lists.
//!
//! Merge policy is deliberately rank-based round-robin: each worker's
//! fused score is corpus-local (RRF sums are not comparable across
//! repositories — established fact from the benchmark harness), so the
//! only honest merge is positional interleave. Per-repo failures degrade
//! into the `degraded` list rather than failing the request: a partial
//! answer with visible holes beats no answer, and callers can see which
//! repos did not contribute.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::task::JoinSet;
use utoipa::ToSchema;

use crate::GatewayState;

/// Per-repo fan-out timeout: bounds a slow or hung worker so one bad repo
/// cannot stall the whole merged response.
const FANOUT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FanoutRequest {
    query: String,
    /// Merged hit cap across all repos.
    #[serde(default = "default_limit")]
    limit: usize,
    /// Hits requested per repo before merging — defaults to `limit` so a
    /// single-repo answer is not truncated by the interleave.
    #[serde(default)]
    per_repo_limit: Option<usize>,
    /// Repo ids, slugs, or names; absent/empty fans to every registered repo.
    #[serde(default)]
    repos: Vec<String>,
}

const fn default_limit() -> usize {
    20
}

/// One repo that could not answer — the reason is surfaced, never hidden.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DegradedRepo {
    repo_id: String,
    repo_slug: String,
    error: String,
}

/// Per-repo outcome accounting: how much each repo contributed.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepoOutcome {
    repo_id: String,
    repo_slug: String,
    hit_count: usize,
    latency_ms: u64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FanoutResponse {
    query: String,
    limit: usize,
    /// Repos that returned a usable response.
    searched: usize,
    /// Repos that failed — unreachable, timeout, worker error, malformed.
    degraded: Vec<DegradedRepo>,
    per_repo: Vec<RepoOutcome>,
    /// Round-robin-by-rank interleave; each hit carries `"repo": slug`.
    hits: Vec<Value>,
    /// Union of per-repo `missingCapabilities`, each prefixed `<slug>:`.
    missing_capabilities: Vec<String>,
    /// Per-repo verdicts pass through verbatim — like scores, verdict
    /// semantics are corpus-local and are not merged.
    verdicts: std::collections::HashMap<String, Value>,
}

/// What one worker returned, distilled for the merge.
struct RepoReply {
    entry: crate::RepoEntry,
    hits: Vec<Value>,
    missing: Vec<String>,
    verdict: Option<Value>,
    latency_ms: u64,
}

/// Round-robin interleave by rank position: position 0 of every repo,
/// then position 1, and so on — the only merge that does not pretend
/// corpus-local scores are comparable. Each hit gains `"repo": slug`.
fn merge_hits(per_repo: Vec<(String, Vec<Value>)>, limit: usize) -> Vec<Value> {
    let mut lanes: Vec<(String, std::vec::IntoIter<Value>)> = per_repo
        .into_iter()
        .map(|(slug, hits)| (slug, hits.into_iter()))
        .collect();
    let mut merged = Vec::new();
    loop {
        let mut progressed = false;
        for (slug, lane) in &mut lanes {
            if let Some(mut hit) = lane.next() {
                progressed = true;
                if let Value::Object(ref mut map) = hit {
                    map.insert("repo".to_owned(), Value::String(slug.clone()));
                }
                merged.push(hit);
                if merged.len() >= limit {
                    return merged;
                }
            }
        }
        if !progressed {
            return merged;
        }
    }
}

#[utoipa::path(post, path = "/v1/search/all", tag = "search",
    summary = "fan one query out to every selected repo's worker and merge by rank",
    description = "Scatters `POST {worker}/v1/search` concurrently across repos (10s per-repo \
                   timeout) and interleaves hits round-robin by rank — scores are corpus-local \
                   and never compared across repos. Per-repo failures land in `degraded` without \
                   failing the request.",
    request_body = FanoutRequest,
    responses(
        (status = 200, body = FanoutResponse),
        (status = "4XX", description = "client error", body = crate::ErrorBody)
    ))]
pub(crate) async fn search_all(
    State(state): State<Arc<GatewayState>>,
    Json(input): Json<FanoutRequest>,
) -> Json<FanoutResponse> {
    // Registry snapshot is taken once and the lock released before any
    // network work — never hold a sync mutex across .await.
    let entries: Vec<crate::RepoEntry> = { state.registry.lock().values().cloned().collect() };
    let per_repo_limit = input.per_repo_limit.unwrap_or(input.limit).max(1);

    // Resolve the requested set. Unknown selectors degrade rather than
    // reject — a typo'd repo name must not hide the repos that can answer.
    let mut selected: Vec<crate::RepoEntry> = Vec::new();
    let mut degraded: Vec<DegradedRepo> = Vec::new();
    if input.repos.is_empty() {
        selected = entries;
    } else {
        for selector in &input.repos {
            match entries.iter().find(|entry| {
                entry.id == *selector || entry.slug == *selector || entry.name == *selector
            }) {
                Some(entry) => selected.push(entry.clone()),
                None => degraded.push(DegradedRepo {
                    repo_id: selector.clone(),
                    repo_slug: selector.clone(),
                    error: "unknown repo".to_owned(),
                }),
            }
        }
    }
    // Deterministic interleave order independent of registry iteration.
    selected.sort_by(|a, b| a.slug.cmp(&b.slug));

    let mut tasks = JoinSet::new();
    for entry in selected {
        let state = Arc::clone(&state);
        let query = input.query.clone();
        tasks.spawn(async move {
            let started = Instant::now();
            // Fan-out shares the gateway-wide upstream bound, and the
            // permit comes before the circuit admit — a local shed or an
            // open circuit carries no verdict about the worker, while
            // every admitted call reports exactly one outcome below.
            let (reply, worker_ok): (_, Option<bool>) = match crate::acquire_upstream(&state).await
            {
                Err(_) => (Err("saturated".to_owned()), None),
                Ok(_permit) if !state.breakers.admit(&entry.id) => {
                    (Err("circuit open".to_owned()), None)
                }
                Ok(_permit) => {
                    let outcome = tokio::time::timeout(FANOUT_TIMEOUT, async {
                        state
                            .client
                            .post(format!("{}/v1/search", entry.worker_url))
                            .json(&serde_json::json!({"query": query, "limit": per_repo_limit}))
                            .send()
                            .await
                    })
                    .await;
                    match outcome {
                        Err(_) => (Err("timeout".to_owned()), Some(false)),
                        Ok(Err(error)) => (Err(format!("unreachable: {error}")), Some(false)),
                        Ok(Ok(response)) => {
                            let status = response.status();
                            if status.is_server_error() {
                                (Err(format!("worker error {status}")), Some(false))
                            } else {
                                match response.json::<Value>().await {
                                    Err(error) => (
                                        Err(format!("invalid response ({status}): {error}")),
                                        Some(false),
                                    ),
                                    Ok(_) if !status.is_success() => {
                                        (Err(format!("worker error {status}")), Some(true))
                                    }
                                    Ok(body) => {
                                        let hits = body
                                            .get("hits")
                                            .and_then(Value::as_array)
                                            .cloned()
                                            .unwrap_or_default();
                                        let missing = body
                                            .get("missingCapabilities")
                                            .and_then(Value::as_array)
                                            .map(|items| {
                                                items
                                                    .iter()
                                                    .filter_map(Value::as_str)
                                                    .map(str::to_owned)
                                                    .collect()
                                            })
                                            .unwrap_or_default();
                                        (
                                            Ok((hits, missing, body.get("verdict").cloned())),
                                            Some(true),
                                        )
                                    }
                                }
                            }
                        }
                    }
                }
            };
            match worker_ok {
                Some(true) => state.breakers.on_success(&entry.id),
                Some(false) => state.breakers.on_failure(&entry.id),
                None => {}
            }
            state.metrics.record_upstream_ms(
                u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            );
            (entry, started.elapsed(), reply)
        });
    }

    let mut replies: Vec<RepoReply> = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        let Ok((entry, elapsed, reply)) = joined else {
            continue; // join error — spawn panicked; treat as lost repo
        };
        match reply {
            Ok((hits, missing, verdict)) => replies.push(RepoReply {
                entry,
                hits,
                missing,
                verdict,
                latency_ms: elapsed.as_millis().try_into().unwrap_or(u64::MAX),
            }),
            Err(error) => degraded.push(DegradedRepo {
                repo_id: entry.id,
                repo_slug: entry.slug,
                error,
            }),
        }
    }
    // Deterministic output order: sort replies by slug like `selected`.
    replies.sort_by(|a, b| a.entry.slug.cmp(&b.entry.slug));
    degraded.sort_by(|a, b| a.repo_slug.cmp(&b.repo_slug));

    let searched = replies.len();
    state.metrics.record_fanout(searched, degraded.len());
    let per_repo: Vec<RepoOutcome> = replies
        .iter()
        .map(|reply| RepoOutcome {
            repo_id: reply.entry.id.clone(),
            repo_slug: reply.entry.slug.clone(),
            hit_count: reply.hits.len(),
            latency_ms: reply.latency_ms,
        })
        .collect();
    let missing_capabilities: Vec<String> = replies
        .iter()
        .flat_map(|reply| {
            reply
                .missing
                .iter()
                .map(move |cap| format!("{}: {cap}", reply.entry.slug))
        })
        .collect();
    let verdicts: std::collections::HashMap<String, Value> = replies
        .iter()
        .filter_map(|reply| {
            reply
                .verdict
                .clone()
                .map(|verdict| (reply.entry.slug.clone(), verdict))
        })
        .collect();
    let lanes: Vec<(String, Vec<Value>)> = replies
        .into_iter()
        .map(|reply| (reply.entry.slug, reply.hits))
        .collect();
    let hits = merge_hits(lanes, input.limit.max(1));

    Json(FanoutResponse {
        query: input.query,
        limit: input.limit,
        searched,
        degraded,
        per_repo,
        hits,
        missing_capabilities,
        verdicts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hits(prefix: &str, count: usize) -> Vec<Value> {
        (0..count)
            .map(|index| json!({"id": format!("{prefix}{index}"), "rank": index}))
            .collect()
    }

    #[test]
    fn merge_interleaves_by_rank_and_injects_repo() {
        let merged = merge_hits(
            vec![
                ("a".to_owned(), hits("a", 3)),
                ("b".to_owned(), hits("b", 1)),
                ("c".to_owned(), hits("c", 2)),
            ],
            20,
        );
        let order: Vec<String> = merged
            .iter()
            .map(|hit| hit["id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(order, ["a0", "b0", "c0", "a1", "c1", "a2"]);
        assert!(merged.iter().all(|hit| hit["repo"].is_string()));
        assert_eq!(merged[0]["repo"], "a");
        assert_eq!(merged[2]["repo"], "c");
    }

    #[test]
    fn merge_truncates_at_limit() {
        let merged = merge_hits(
            vec![
                ("a".to_owned(), hits("a", 5)),
                ("b".to_owned(), hits("b", 5)),
            ],
            3,
        );
        assert_eq!(merged.len(), 3);
    }

    #[test]
    fn merge_empty_and_missing_lanes() {
        assert!(merge_hits(Vec::new(), 10).is_empty());
        let merged = merge_hits(
            vec![("a".to_owned(), Vec::new()), ("b".to_owned(), hits("b", 2))],
            10,
        );
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0]["repo"], "b");
    }
}
