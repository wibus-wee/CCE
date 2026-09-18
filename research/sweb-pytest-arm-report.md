# External benchmark arm: sweb-pytest

First non-self corpus arm for the retrieval kernel. Real GitHub issue text
(SWE-bench `problem_statement`) is the query; the fix-patch's touched files
are file-level golds — the standard issue-localization labeling, no
hand-labeling required.

## What was built

| Artifact | Path |
|---|---|
| Dataset (29 cases, `sweb-pytest-v2`) | `benchmarks/datasets/sweb-pytest.jsonl` |
| Sparse arm adapter | `benchmarks/adapters/cce-external-search-sparse.yaml` |
| Dense-daemon arm adapter | `benchmarks/adapters/cce-external-search-daemon-dense-jina-code.yaml` |
| Dataset builder (re-runnable) | `research/build_sweb_pytest_dataset.py` |
| Source rows (SWE-bench → pytest 7.x subset) | `research/external/swebench-pytest-v7-rows.json` |
| Indexed repo (NOT committed) | `research/external/pytest` — pytest-dev/pytest @ 7.4.4 (`33f694f4`) |
| Result bundles + manifests | `research/output/sweb-pytest-{sparse,dense-daemon}.jsonl[.manifest.json]` |
| Metric dumps | `research/output/sweb-pytest-{sparse,dense-daemon}-metrics.json` |

## Corpus and labeling

- **Repo**: pytest-dev/pytest @ tag 7.4.4 (~250 `.py` files incl. `testing/`).
  Mid-size, real-world Python codebase with a `src/_pytest/` core.
- **Cases**: 26 SWE-bench `test`-split instances (fetched via the HF
  datasets-server, filtered to `repo == pytest-dev/pytest`, `version 7.x`
  so issue text is contemporaneous with the 7.4.4 checkout) + 1 negative
  control + 2 maintainer-authored adversarial abstention probes added by
  the main thread (`sweb-adv-*`, with pre-judged decoy files).
- **Golds**: `gold_files` = fix-patch touched paths (verified present at the
  checkout); `gold_symbols`/`gold_facts` = patch hunk-context symbols,
  verified *defined at the checkout* via `ast` (24/29 cases carry
  claim-level facts, so `claim_support` is exercised deterministically).
- **Intent**: `issue_localization` on all SWE-bench cases; 6 withhold
  `supply_intent` to score the classifier.

## Headline metrics (n=29)

| Metric | sparse | dense-daemon (jina-code) |
|---|---|---|
| recall@5 | 0.438 | 0.438 |
| recall@20 | 0.583 | 0.583 |
| file_recall@20 | 0.583 | 0.583 |
| file_recall@50 | 0.732 | 0.686 |
| file_success@20 | 0.517 | 0.517 |
| mrr | 0.276 | 0.274 |
| ndcg@10 | 0.293 | 0.302 |
| claim_support (n=24) | 0.542 | 0.583 |
| symbol_recall@20 | 0.307 | 0.307 |
| abstention_accuracy | 0.897 | 0.897 |
| no_context_precision | 1.000 | 1.000 |
| decoy_hit_rate@20 | 0.007 | 0.007 |
| intent_accuracy (n=6) | 0.333 | 0.333 |
| unjudged_rate@20 | 0.935 | 0.935 |
| query_ms (mean) | 137 830 | 178 357 |

Guardrails (file_success@20, file_recall@20, abstention_accuracy,
no_context_precision) all report; the two arms are within noise of each
other — on long issue text, dense contributes mostly tail candidates.

## Follow-up: pair-floor bounding (commit `726fb40`)

The caveat-3 latency pathology was root-caused to the lexical fallback:
`fts_match_queries` ORed *every* pairwise AND — a 32-term issue query
emitted 496 FTS5 clauses, ~15.5 s for one MATCH, and the planner runs
`lexical_search` 3–4× per query (lexical/knowledge/history routes plus
PRF), compounding to the observed ~138 s mean / ~810 s worst.

The fix (`floor_evidence` in `metadata.rs`): per-term document-frequency
measurement (one indexed count each), an ubiquity cutoff (df > docs/8,
absolute floor 200 — ~zero-IDF terms like `test`/`def`/`pytest` carry no
evidence), then the cheapest surviving pairs up to `PAIR_CLAUSES_MAX` =
150, plus a live-term rescue so the floor never silently drops.

### Six-arm comparison (n=29, current evaluator)

| Metric | old | rare-12 | rare+anchor | budget-v1 | budget-v2 | **ubiquity** |
|---|---|---|---|---|---|---|
| query_ms | 137 830 | 2 069 | 6 740 | 3 348 | 3 448 | **2 822** |
| recall@10 | .472 | .524 | .576 | .472 | .472 | .472 |
| recall@20 | .583 | .576 | .593 | .559 | .559 | .559 |
| recall@50 | .732 | .607 | .624 | .583 | .583 | **.617** |
| claim_support | .542 | .375 | .479 | .500 | .500 | .500 |
| symbol_recall@20 | .307 | .252 | .252 | .403 | .403 | **.403** |
| decoy_hit_rate@20 | .103 | .103 | .103 | .103 | .103 | .103 |
| abstention_accuracy | .897 | .897 | .897 | .897 | .897 | .897 |

**Stale-metrics lesson.** An earlier comparison reported `decoy_hit_rate@20`
.007 → .103 as a budget-arm regression. Re-evaluating the *old* result file
with the current evaluator shows .103 there too — the stored metrics JSONs
predate adjudication filling `judged_files`, so the "regression" was a
stale-metric artifact, not a retrieval difference. All six arms are
decoy-identical; always re-evaluate baselines rather than trusting stored
metric dumps across adjudication rounds.

**Negative result — scan-cost budget.** Replacing the clause cap with a
cumulative `Σ(df_a+df_b)` cost budget (1M doc-scans, ≤300 clauses) made
things *worse*: query_ms 6 053 and recall@50 .593 vs ubiquity's 2 822/.617.
The tight clause bound is load-bearing twice — it caps posting-merge work
*and* concentrates the floor on the rarest evidence; extra mid-frequency
clauses changed stage-internal `bm25` ranking and pushed rare-evidence
documents out of the top-`limit` window. Reverted.

**Residual honest gaps vs old** (the price of bounded clauses): recall@50
−.115, claim_support −.042, recall@5 −.069. Gold documents whose query-term
coverage concentrates in mid×mid pairs (`parametrize`×`mark`, ~12–16 k df)
lose those clauses to the budget. Gains: symbol_recall@20 +.096, mrr/ndcg
flat-to-up, and 49× latency — the floor pathology is closed.

## Caveats

1. **Gold = fix patch, not adjudicated relevance.** Files touched by the
   real fix are gold; anything else retrieved is "unjudged" — hence
   `unjudged_rate@20` ≈ 0.93. `bpref@20` ≈ 0.58 is the
   incompleteness-tolerant read. A pooled-adjudication round (same
   `adjudicate`/`apply-judgments` flow as cce-self) would tighten this.
2. **Version skew**: checkout is 7.4.4; cases span 7.0–7.4. Gold *paths*
   all verified present, but issue semantics drift slightly for the 7.0
   cases (a fix may already be applied, or code moved). Symbols/facts were
   re-verified at checkout to compensate.
3. **Query latency was the headline finding — now resolved**: mean
   query_ms ≈ 138–178 s, heavy-tailed (min ~1 s, max ~14 min). Long issue
   text with corpus-common terms ("test", "summary", "fixture") exploded
   the pairwise evidence floor into C(n,2) FTS5 clauses —
   `sweb-pytest-10482` took ~810 s and initially crashed the run at the
   360 s adapter default (both adapters now use `timeout_seconds: 1800`).
   Fixed by `floor_evidence` (df-ranked bounded pairs, commit `726fb40`);
   see the follow-up section — sparse query_ms is now ~2.8 s.
4. **Abstention**: all 3 no_context probes retrieved full result sets
   (search never abstains) → `abstention_accuracy` 0.897 reflects
   engine behavior, not harness error. `decoy_hit_rate` ≈ 0.7–1.4% shows
   the pre-judged decoy files are rarely surfaced, though.
5. **Component metrics skipped**: pytest is a single package; `cce map`
   returns no usable component map.
6. **Manifest note**: sparse results were produced in two batches
   (run died on the 360 s timeout at case 12); they were merged in dataset
   order with `dataset_revision` re-labeled v1→v2 (case contents identical
   — verified by diffing the regenerated builder output; only the
   revision label and the two `sweb-adv` additions changed). The manifest's
   adapter sha covers the post-bump (1800 s) adapter.
7. **Ops gotchas encoded in the adapters**: the cce binary must be an
   absolute path (subprocess cwd = external repo), and the data dir
   (`.cce-bench-external`, inside the checkout) must be excluded via
   `.git/info/exclude` — otherwise every search writes index files into
   the snapshot, re-snapshotting itself (~2.5 min/query of pure churn).
   The dense arm's first-case `query_ms` is polluted by one-time view
   repair after the previous daemon was terminated mid-session.
