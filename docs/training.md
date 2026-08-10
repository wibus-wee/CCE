# Local retriever and reranker training

CCE trains and serves embeddings without an inference service. Python under `research/` builds and trains models; the production Rust runtime consumes a verified ONNX bundle.

## Dataset construction

`research/corpora/production-repositories.yaml` pins every source repository to an immutable commit and assigns the entire repository to exactly one of train, validation, or test. This prevents project conventions or duplicated code from leaking across splits. The manifest stratifies JavaScript, TypeScript, Rust, Python, C, and mixed-language repositories by small, medium, large, and enterprise scale. Training, frozen benchmark, and scale-stress roles are explicit.

The static repository holdout is combined with a temporal rule: feedback after the manifest's
observation cutoff first enters a rolling future-evaluation window. It must not be used to train
the challenger evaluated on that window. Only a later dataset revision may advance the cutoff.
This avoids a superficially improving benchmark that merely memorizes recent user queries.

External evaluation repositories registered in `benchmarks/external-datasets.yaml` are
evaluation-only. In particular, Agent Retrieval Bench and SWE-Explore queries, gold files, and
trajectory regions must never be exported into feedback training examples.

The builder clones only pinned revisions, indexes each checkout through the production CCE binary, and uses exactly the runtime document representation:

```text
path: <repository-relative path>
representation: RawCode
<source-aligned entity text>
```

Queries come from source documentation, symbol navigation, and recent commit subjects. Every example contains explicit same-repository hard negatives selected by lexical overlap, language, and directory proximity. Source text, revision, byte range, and SHA-256 remain in every record.

```bash
cargo build --release --locked -p cce-cli
uv run --project research cce-research build-training-data \
  research/corpora/production-repositories.yaml \
  research/workspace \
  research/output/corpus \
  --cce-binary target/release/cce \
  --hard-negatives 7
```

## Contrastive fine-tuning

The trainer uses cached multiple-negatives ranking loss. Explicit negatives are combined with in-batch negatives; query and document prefixes are part of the recipe and later copied into the model manifest. Base model and optional remote-code revisions must be immutable hashes.

```bash
uv run --project research --extra models --extra training cce-research train-retriever \
  research/output/corpus \
  research/output/models/cce-e5-enhanced \
  intfloat/multilingual-e5-small \
  614241f622f53c4eeff9890bdc4f31cfecc418b3 \
  --hard-negatives 7 \
  --epochs 1 \
  --batch-size 8 \
  --gradient-accumulation-steps 4 \
  --max-sequence-length 512
```

`--max-train-examples` and `--max-validation-examples` are smoke/debug limits, not production defaults. Do not tune against the test split.

## Local continuous-learning loop

Long-running CLI, daemon, and MCP deployments can retain query trajectories and explicit
feedback in the same local CCE data root. No query, source text, trace, dataset, model, or
benchmark result is sent to an inference or telemetry service. Keep the data root on encrypted
storage and apply the same backup policy as the indexed repositories.

Raw `shown_to_model` and `opened_by_agent` events are retained for funnel analysis only. They
are position- and exposure-biased, so the exporter never turns them into relevance labels.
`cited_or_used` and `edited_or_affected` are positive labels; the default high-confidence
export requires an explicit document-level `rejected` event from the same trajectory for every
hard negative. A trajectory without both sides remains raw telemetry and is reported as skipped
in the manifest.

Export one or more local data roots with a fixed observation cutoff:

```bash
uv run --project research cce-research export-feedback \
  --data-root /srv/cce/project-a \
  --data-root /srv/cce/project-b \
  --output research/output/feedback/train.jsonl \
  --cutoff 2026-08-10T00:00:00Z \
  --max-negatives 7
```

The adjacent manifest records every input database SHA-256, content-addressed trace digest,
cutoff, event counts, label policy, skipped trajectory count, dataset revision, and final JSONL
SHA-256. Feedback examples are always assigned to `train`; frozen validation/test repositories
and future-time evaluation windows must remain separate. Re-exporting with a later cutoff creates
a new auditable dataset revision rather than mutating an old one.

Train a challenger against the combined pinned corpus and feedback train split, then benchmark
the champion and challenger on the identical frozen held-out file and hardware. Promotion is a
separate local operation:

```bash
uv run --project research cce-research promote-model \
  --champion-benchmark research/output/champion.json \
  --challenger-benchmark research/output/challenger.json \
  --candidate research/output/models/challenger-onnx \
  --registry /srv/cce/model-registry
```

The gate rejects aggregate Recall/MRR/nDCG regressions, missing or excessive per-query-kind
quality, p95 latency or peak-RSS increases above the configured ratios, and throughput below its
floor. Comparable results must have the same dataset hash and hardware record. Passing bundles
are copied without symlinks into an immutable content-hashed release and `current.json` is
replaced atomically; failed decisions leave the current champion untouched. Every pass or failure
has an audit manifest under `registry/audit/`. Training loss alone can never promote a model.

## ONNX promotion bundle

```bash
uv run --project research --extra models --extra onnx cce-research export-onnx \
  research/output/models/cce-e5-enhanced \
  research/output/models/cce-e5-enhanced-onnx \
  --optimization O3
```

The exporter writes the tokenizer, config, `onnx/model.onnx`, training provenance, per-file SHA-256, runtime model family, and a deterministic `bundleRevision`. Rust refuses a mismatched revision, family, escaped or symlinked path, missing file, or hash mismatch.

## Cross-encoder fine-tuning

The same repository-isolated examples train the second-stage cross-encoder. Each query forms one positive pair and up to seven repository-local hard-negative pairs. Binary cross entropy uses a positive-class weight equal to the negative count. Custom model code is accepted only when both model and code revisions are immutable; if a checkpoint declares `auto_map` without `--trust-remote-code`, training fails instead of silently initializing a generic architecture.

```bash
uv run --project research --extra models --extra training cce-research train-reranker \
  research/output/corpus \
  research/output/models/cce-reranker-enhanced \
  jinaai/jina-reranker-v1-turbo-en \
  5bcd26bbe1913aff3cc1983d91f2c3ac9ee91cbf \
  --code-revision 5bcd26bbe1913aff3cc1983d91f2c3ac9ee91cbf \
  --trust-remote-code \
  --hard-negatives 7

uv run --project research --extra models --extra onnx cce-research export-reranker-onnx \
  research/output/models/cce-reranker-enhanced \
  research/output/models/cce-reranker-enhanced-onnx \
  jinaai/jina-reranker-v1-turbo-en \
  5bcd26bbe1913aff3cc1983d91f2c3ac9ee91cbf
```

`promote-reranker-bundle` performs the same manifest generation for an already exported pinned model. Its manifest records the source revision, runtime model identity, every required SHA-256, and a content-derived `bundleRevision`.

## Promotion gates

A candidate is promotable only after at least three seeds on repository-held-out data and a paired base-versus-tuned comparison. Report overall and intent/query-kind metrics, cold and warm latency, indexing throughput, peak RSS, disk footprint, and confidence intervals. Exact symbols should still be routed to exact/lexical search; dense improvement must be evaluated primarily on natural-language behavior, issue localization, and architecture queries. Rerankers must be compared on the identical candidate pools, including repository-local hard negatives; training loss is not a ranking result.

The initial local smoke experiment trained multilingual E5 on 128 SQLite examples and evaluated 120 untouched ripgrep queries. It improved Recall@1 from 0.1167 to 0.2750, Recall@10 from 0.3667 to 0.5417, MRR from 0.2193 to 0.3639, and nDCG@10 from 0.2355 to 0.3888. This validates the pipeline and cross-repository signal, but it is not a production promotion: it used one seed, a small sample, and did not improve the sparse change-localization bucket.

The first cross-encoder smoke experiment trained the pinned Jina turbo reranker on 256 SQLite queries (2048 positive/negative pairs) and evaluated 120 untouched ripgrep queries with identical eight-document candidate pools. Recall@1 improved from 0.2500 to 0.3917, MRR from 0.4691 to 0.6008, nDCG@10 from 0.5983 to 0.7005, and Recall@5 from 0.9500 to 0.9917. PyTorch p95 increased from 0.94s to 1.12s. This is also not a promotion: it is one seed, change-localization remains sparse and weak, and Rust ONNX end-to-end latency needs a broader measurement.
