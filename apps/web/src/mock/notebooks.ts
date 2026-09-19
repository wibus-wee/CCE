/**
 * MOCK — fixtures for the Notebooks surface (`/v1/notebooks` does not
 * exist). The shapes mirror the intended API: a notebook is a document of
 * typed blocks — markdown | query | file | symbol — with an owner, a
 * namespace/visibility pair, stars, and timestamps. Query hits, file
 * contents, and symbol signatures are stored excerpts from real repo
 * paths; a live notebook would re-run its query blocks against the current
 * snapshot and read file blocks fresh. Every consumer must label the
 * surface `preview` — fixtures are never index truth.
 */

const hoursAgo = (h: number) => new Date(Date.now() - h * 3600_000).toISOString()
const daysAgo = (d: number) => new Date(Date.now() - d * 86400_000).toISOString()

export type MockNotebookBlockType = 'markdown' | 'query' | 'file' | 'symbol'

/** A stored search hit — the canned result set a query block renders. */
export interface MockNotebookHit {
  path: string
  line: number
  /** The matched source line, as a hit row would show it. */
  preview: string
  /** Retrieval route that produced the hit. */
  route: string
}

export interface MockNotebookMarkdownBlock {
  id: string
  type: 'markdown'
  /** Raw markdown source — the consumer renders it, never sanitizes it away. */
  markdown: string
}

export interface MockNotebookQueryBlock {
  id: string
  type: 'query'
  /** Sourcegraph-style query in the app's own query language. */
  query: string
  /** Stored result set — a live block would re-run against the snapshot. */
  hits: MockNotebookHit[]
}

export interface MockNotebookFileBlock {
  id: string
  type: 'file'
  path: string
  /** Inclusive [start, end] line range; `null` embeds the entire file. */
  lineRange: readonly [number, number] | null
  /** Stored excerpt — file blocks freeze content at save time. */
  content: string
}

export interface MockNotebookSymbolBlock {
  id: string
  type: 'symbol'
  name: string
  kind: 'function' | 'struct' | 'trait' | 'module' | 'constant' | 'component' | 'type'
  path: string
  line: number
  /** Declaration text rendered as the signature card. */
  signature: string
  /** One-line doc note, like the symbol sidebar's hover blurb. */
  doc?: string
}

export type MockNotebookBlock =
  | MockNotebookMarkdownBlock
  | MockNotebookQueryBlock
  | MockNotebookFileBlock
  | MockNotebookSymbolBlock

export interface MockNotebook {
  id: string
  title: string
  description: string
  blocks: MockNotebookBlock[]
  /** Creator/owner handle. */
  owner: string
  /** Namespace the notebook publishes under (`@user` / `@org`). */
  namespace: string
  /** Sourcegraph's two-state visibility. */
  visibility: 'public' | 'private'
  stars: number
  /** Whether the current viewer starred it — drives the star button. */
  starred: boolean
  createdAt: string
  updatedAt: string
}

export const MOCK_NOTEBOOKS: MockNotebook[] = [
  {
    id: 'nb-freshness',
    title: 'Snapshot freshness — how staleness propagates',
    description:
      'Walkthrough of view invalidation from push to reindex, with live queries.',
    owner: 'wibus',
    namespace: '@wibus',
    visibility: 'public',
    stars: 4,
    starred: false,
    createdAt: daysAgo(12),
    updatedAt: hoursAgo(26),
    blocks: [
      {
        id: 'nb-freshness-b1',
        type: 'markdown',
        markdown: `## Why views go stale

A push doesn't reindex anything — it **invalidates** the views that read
the worktree. The next search runs against the last committed snapshot and
reports its freshness honestly.

- \`lexical\` and \`symbols\` rebuild per snapshot
- \`dense\` only rebuilds once the embedder is warm
- a stale view is a state, never a silent fallback`,
      },
      {
        id: 'nb-freshness-b2',
        type: 'query',
        query: 'snapshot stale',
        hits: [
          { path: 'apps/daemon/src/watch.rs', line: 28, preview: 'fn is_stale(&self, snapshot_id: &str) -> bool {', route: 'structural' },
          { path: 'crates/cce-engine/src/engine.rs', line: 514, preview: 'pub fn status(&self) -> Result<ViewManifest> {', route: 'structural' },
          { path: 'crates/cce-engine/src/snapshot_diff.rs', line: 1, preview: '//! Architecture diff: the entity/relation delta between two committed', route: 'lexical' },
        ],
      },
      {
        id: 'nb-freshness-b3',
        type: 'file',
        path: 'apps/daemon/src/watch.rs',
        lineRange: [54, 63],
        content: `async fn tick(engine: &CceEngine, state: &mut WatchState) {
    // The scan is synchronous file IO on the executor — the same pattern
    // \`CceEngine::index()\` uses when invoked from a request handler.
    let scanned = match RepositoryScanner::new(engine.config().clone()).scan(Some(engine.store())) {
        Ok(scanned) => scanned,
        Err(error) => {
            tracing::warn!(%error, "watch: scan failed");
            return;
        }
    };`,
      },
      {
        id: 'nb-freshness-b4',
        type: 'symbol',
        name: 'WatchState',
        kind: 'struct',
        path: 'apps/daemon/src/watch.rs',
        line: 21,
        signature: 'struct WatchState {',
        doc: 'The last committed snapshot id — watch ticks reindex only when it moves.',
      },
      {
        id: 'nb-freshness-b5',
        type: 'query',
        query: 'ViewManifest',
        hits: [
          { path: 'crates/cce-engine/src/engine.rs', line: 514, preview: 'pub fn status(&self) -> Result<ViewManifest> {', route: 'exact_symbol' },
          { path: 'apps/web/src/screens/IndexScreen.tsx', line: 96, preview: 'const views = manifest ? Object.entries(manifest.views) : []', route: 'lexical' },
          { path: 'apps/daemon/src/main.rs', line: 298, preview: '#[utoipa::path(get, path = "/v1/status", tag = "worker",', route: 'lexical' },
        ],
      },
      {
        id: 'nb-freshness-b6',
        type: 'markdown',
        markdown: `The \`--watch\` tick is the whole loop: **scan → compare → index on
change**. Everything else — badges, the capability matrix — is reporting.`,
      },
    ],
  },
  {
    id: 'nb-routes',
    title: 'Dense vs lexical — when each route wins',
    description:
      'Benchmarks over the gold corpus; rerank fusion explained.',
    owner: 'wibus',
    namespace: '@wibus',
    visibility: 'public',
    stars: 7,
    starred: true,
    createdAt: daysAgo(20),
    updatedAt: hoursAgo(74),
    blocks: [
      {
        id: 'nb-routes-b1',
        type: 'markdown',
        markdown: `## Route selection

The planner reads the query shape and fans out to the routes that can
answer it. \`exact_symbol\` wins on identifiers, natural-language questions
go dense, everything else is lexical with structural assist.`,
      },
      {
        id: 'nb-routes-b2',
        type: 'query',
        query: 'lang:rust embedder',
        hits: [
          { path: 'crates/cce-engine/src/dense.rs', line: 26, preview: 'pub trait Embedder: Send + Sync {', route: 'structural' },
          { path: 'crates/cce-engine/src/dense.rs', line: 37, preview: 'pub enum EmbeddingBackend {', route: 'structural' },
          { path: 'crates/cce-engine/src/dense.rs', line: 162, preview: 'pub struct LocalEmbedder {', route: 'lexical' },
        ],
      },
      {
        id: 'nb-routes-b3',
        type: 'symbol',
        name: 'Embedder',
        kind: 'trait',
        path: 'crates/cce-engine/src/dense.rs',
        line: 26,
        signature: 'pub trait Embedder: Send + Sync {',
        doc: 'One vector per input, normalized — the profile persists with the index.',
      },
      {
        id: 'nb-routes-b4',
        type: 'file',
        path: 'crates/cce-engine/src/rerank.rs',
        lineRange: [1, 7],
        content: `//! Local cross-encoder reranking via fastembed/ONNX.
//!
//! The fused candidate order is a coarse relevance prior built from
//! route-level rank fusion; a cross-encoder re-scores (query, document)
//! pairs jointly and reorders the head of the list. Model files download
//! once into the data root's \`models/\` directory on first use — selecting
//! a reranker is the same explicit network opt-in as \`--dense local\`.`,
      },
      {
        id: 'nb-routes-b5',
        type: 'markdown',
        markdown: `### Offline contract

\`--dense disabled\` marks the dense view \`Failed\` with a reason — indexing
and lexical querying keep working. The same opt-in story covers the
reranker: model files download once, then inference is fully offline.`,
      },
      {
        id: 'nb-routes-b6',
        type: 'query',
        query: 'rerank',
        hits: [
          { path: 'crates/cce-engine/src/rerank.rs', line: 15, preview: 'pub struct LocalReranker {', route: 'exact_symbol' },
          { path: 'crates/cce-engine/src/engine.rs', line: 75, preview: 'reranker: tokio::sync::OnceCell<std::result::Result<Option<crate::LocalReranker>, String>>,', route: 'lexical' },
          { path: 'apps/cli/src/main.rs', line: 50, preview: 'enum DenseMode {', route: 'structural' },
        ],
      },
      {
        id: 'nb-routes-b7',
        type: 'file',
        path: 'research/cce_research/metrics.py',
        lineRange: [12, 20],
        content: `def case_clusters(cases: list[BenchmarkCase]) -> dict[str, str]:
    """case_id -> cluster root. Derived cases (\`::var-*\`, \`::b*\` budget
    curves, pinned ablations) replicate their parent — they are not
    independent evidence. Every statistic that treats rows as iid would
    over-weight variant-heavy families and report dishonestly narrow CIs;
    collapsing to cluster means makes a family exactly one observation.
    Transitive lineage (variant of a variant) resolves to the ultimate
    root; a missing parent id still groups siblings correctly."""
    roots = {case.case_id: (case.derived_from or case.case_id) for case in cases}`,
      },
    ],
  },
  {
    id: 'nb-packs',
    title: 'Onboarding: reading a context pack',
    description:
      'Provenance, budgets, and why every block carries a source address.',
    owner: 'team',
    namespace: '@cce',
    visibility: 'public',
    stars: 2,
    starred: false,
    createdAt: daysAgo(30),
    updatedAt: hoursAgo(140),
    blocks: [
      {
        id: 'nb-packs-b1',
        type: 'markdown',
        markdown: `## Context packs

\`ContextPacker\` turns a search result into a token-budgeted bundle for an
agent: **orientation first**, then hits until the budget runs out. Every
item keeps its provenance — route, rank, snapshot, source address.`,
      },
      {
        id: 'nb-packs-b2',
        type: 'symbol',
        name: 'ContextPacker',
        kind: 'struct',
        path: 'crates/cce-engine/src/context.rs',
        line: 48,
        signature: 'pub struct ContextPacker;',
        doc: 'Packs search hits into a ContextPack within a token budget.',
      },
      {
        id: 'nb-packs-b3',
        type: 'file',
        path: 'crates/cce-engine/src/context.rs',
        lineRange: [60, 72],
        content: `    pub fn pack(&self, search: &SearchResult, budget_tokens: usize) -> ContextPack {
        let orientation = orientation(search);
        let orientation_tokens = estimate_tokens(&orientation);
        let mut items = Vec::new();
        let mut used_tokens = 0_usize;
        if orientation_tokens <= budget_tokens {
            items.push(ContextItem {
                id: "orientation".to_owned(),
                kind: ContextItemKind::Orientation,
                title: "Repository context orientation".to_owned(),
                body: orientation,
                estimated_tokens: orientation_tokens,
                provenance: ContextProvenance {`,
      },
      {
        id: 'nb-packs-b4',
        type: 'query',
        query: 'provenance',
        hits: [
          { path: 'crates/cce-engine/src/context.rs', line: 72, preview: 'provenance: ContextProvenance {', route: 'lexical' },
          { path: 'apps/web/src/screens/ContextScreen.tsx', line: 99, preview: 'const src = item.provenance.sourceAddress', route: 'lexical' },
          { path: 'apps/web/src/screens/ContextScreen.tsx', line: 159, preview: "{item.provenance.route.replaceAll('_', ' ')} · rank {item.provenance.rank} ·{' '}", route: 'lexical' },
        ],
      },
      {
        id: 'nb-packs-b5',
        type: 'markdown',
        markdown: `Try it: run a query, then open **Context** — the item list is the
same pack an agent receives, ranked and budget-labeled.`,
      },
    ],
  },
  {
    id: 'nb-degraded',
    title: 'Debugging degraded views',
    description:
      'Field guide to the capability matrix and provider probes.',
    owner: 'wibus',
    namespace: '@wibus',
    visibility: 'private',
    stars: 5,
    starred: false,
    createdAt: daysAgo(8),
    updatedAt: hoursAgo(190),
    blocks: [
      {
        id: 'nb-degraded-b1',
        type: 'markdown',
        markdown: `## Field guide: degraded views

A view is \`ready\`, \`stale\`, \`building\`, or \`failed\` — the manifest says
which, with a reason. Nothing silently falls back: if dense is down the
response says so, in \`missingCapabilities\`.`,
      },
      {
        id: 'nb-degraded-b2',
        type: 'query',
        query: 'provider detect',
        hits: [
          { path: 'crates/cce-engine/src/providers.rs', line: 22, preview: 'pub(crate) enum DetectState {', route: 'structural' },
          { path: 'crates/cce-engine/src/engine.rs', line: 1502, preview: 'pub fn providers(&self) -> Vec<crate::providers::ProviderReport> {', route: 'structural' },
          { path: 'crates/cce-engine/src/providers.rs', line: 23, preview: '/// Resolved executable or artifact path, plus a display string. `note`', route: 'lexical' },
        ],
      },
      {
        id: 'nb-degraded-b3',
        type: 'file',
        path: 'crates/cce-engine/src/providers.rs',
        lineRange: [1, 8],
        content: `//! External code-intelligence providers. A provider is a local toolchain
//! (rust-analyzer, scip-typescript) or a committed artifact (\`index.scip\`)
//! that produces evidence the engine ingests itself — providers never write
//! \`SQLite\` and never see query traffic. Provisioning/downloading tools is a
//! separate opt-in layer; detection only ever looks at what already exists
//! on the machine or in the repository.

use std::collections::HashMap;`,
      },
      {
        id: 'nb-degraded-b4',
        type: 'symbol',
        name: 'doctor',
        kind: 'function',
        path: 'crates/cce-engine/src/engine.rs',
        line: 542,
        signature: 'pub fn doctor(&self) -> Result<StoreHealth> {',
        doc: "Store health probe — the check behind the index screen's repair hints.",
      },
      {
        id: 'nb-degraded-b5',
        type: 'markdown',
        markdown: `### The index lease

A second writer can't hold \`index.lock\` — \`IndexLease::acquire\` fails
with \`IndexBusy\`, which the daemon surfaces instead of corrupting a view.`,
      },
      {
        id: 'nb-degraded-b6',
        type: 'file',
        path: 'crates/cce-engine/src/lock.rs',
        lineRange: null,
        content: `use std::{
    fs::{File, OpenOptions},
    path::Path,
};

use cce_core::{CceError, Result};
use fs4::{FileExt, TryLockError};

#[derive(Debug)]
pub(crate) struct IndexLease {
    _file: File,
}

impl IndexLease {
    pub(crate) fn acquire(data_root: &Path) -> Result<Self> {
        let path = data_root.join("index.lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| CceError::io(&path, error))?;
        match FileExt::try_lock(&file) {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => Err(CceError::IndexBusy(path)),
            Err(TryLockError::Error(error)) => Err(CceError::io(path, error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_second_writer() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let _first = IndexLease::acquire(directory.path()).expect("first lease");
        let second = IndexLease::acquire(directory.path());
        assert!(matches!(second, Err(CceError::IndexBusy(_))));
    }
}`,
      },
      {
        id: 'nb-degraded-b7',
        type: 'query',
        query: 'IndexBusy',
        hits: [
          { path: 'crates/cce-engine/src/lock.rs', line: 26, preview: 'Err(TryLockError::WouldBlock) => Err(CceError::IndexBusy(path)),', route: 'exact_symbol' },
          { path: 'crates/cce-engine/src/lock.rs', line: 41, preview: 'assert!(matches!(second, Err(CceError::IndexBusy(_))));', route: 'lexical' },
          { path: 'apps/daemon/src/main.rs', line: 289, preview: '(status = "4XX", description = "client error — invalid request or unavailable view", body = ApiErrorBody),', route: 'lexical' },
        ],
      },
    ],
  },
]
