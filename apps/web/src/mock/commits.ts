/**
 * MOCK — fixture git history for the Commits surface (Sourcegraph's repo
 * commit list). No `/v1/commits` endpoint exists and `git log` is not wired
 * to the index yet, so every consumer must label the surface `preview` —
 * fixture commits are never presented as live repo truth.
 *
 * When commit history lands, delete this file and switch the screen to the
 * API; the shape mirrors the intended response. Shas/messages/timestamps
 * deliberately overlap `MOCK_TIMELINE` (src/mock/index.ts) and
 * `MOCK_BRANCHES` (src/mock/branches.ts) so the mocked surfaces tell one
 * coherent story: `main`'s tip here is the same `f0a317c0` push the
 * dashboard timeline and the branch list already claim.
 */

/**
 * One `@@`-delimited unified-diff hunk — the same bytes `git diff` emits.
 */
export interface MockDiffHunk {
  /** First line number on the old (left) side — 0 for new files. */
  oldStart: number
  /** First line number on the new (right) side. */
  newStart: number
  /** Trailing `@@ …` context — the enclosing block a real diff prints. */
  section?: string
  /**
   * Hunk body in unified-diff wire format: every entry begins with the
   * one-char marker — `' '` context, `'+'` added, `'-'` removed — and the
   * rendered text is `line.slice(1)`. Counts in the `@@` header are derived
   * from these markers (ctx+del on the old side, ctx+add on the new).
   */
  lines: string[]
}

/** One file touched by a commit — enough for per-file diffstat rows. */
export interface MockCommitFile {
  /** Repo-relative path — deep-links into `/browse/<path>`. */
  path: string
  added: number
  removed: number
  /**
   * Mock unified diff for the expanded file row — 1–2 hunks of plausible
   * code in the file's own language (.rs for crates, .tsx/.ts for apps/web,
   * .py for research). Fabricated line text, never real `git diff` output.
   */
  hunks: MockDiffHunk[]
}

export interface MockCommit {
  /** Full 40-char sha — rows render the first 8. */
  sha: string
  /** Subject line. */
  message: string
  /**
   * Optional message body — the paragraphs after the subject, rendered on
   * the commit page. Most fixture commits are subject-only, like real
   * history.
   */
  body?: string
  /** Author handle (fixture identities, not real users). */
  author: string
  /** ISO timestamp. */
  at: string
  /** Parent shas — linear history carries one, merges carry two. */
  parents: string[]
  /** Files touched, each with its own +/− counts. */
  filesChanged: MockCommitFile[]
  /** Signed commit — renders the verified badge. */
  verified?: boolean
}

/**
 * `main` history, newest first — 32 commits spanning Sep 7–18. The screen
 * opens on the Sep 12–18 working week; the Sep 7–11 tail pages in through
 * the 'Load older commits' button. `parents` chain to the next-older
 * fixture commit so expansion metadata reads like real `git log`; the tail
 * commit points at a fictional earlier parent — the honest "history
 * continues past this page" case.
 */
export const MOCK_COMMITS: MockCommit[] = [
  // ── Sep 18 ────────────────────────────────────────────────────────────
  {
    sha: 'f0a317c0d92e4a1b8c5f73e6d4a2b9c18f3e5d7a',
    message: 'web: atomic components — Base UI + StyleX port',
    body: 'Ports the @antfu/design component vocabulary onto Base UI + StyleX.\n\n- Action*/Display*/Form* primitives under src/ui/\n- semantic tokens only — no hard-coded colors, light/dark parity\n- hover-revealed row actions reserve width so columns never shift',
    author: 'wibus',
    at: '2026-09-18T09:12:00Z',
    parents: ['9f2d4b71e8a3c6d0f5b2e9a7c4d8f1b3a6e0c5d9'],
    verified: true,
    filesChanged: [
      {
        path: 'apps/web/src/ui/ActionButton.tsx',
        added: 148,
        removed: 96,
        hunks: [
          {
            oldStart: 15,
            newStart: 15,
            section: 'export function ActionButton({',
            lines: [
              "  variant = 'action',",
              '-  size,',
              "+  size = 'md',",
              '   icon,',
              '   loading,',
              '-  className,',
              '   ...rest',
              ' }: Props) {',
              "-  const cls = cx(btn(variant), size === 'sm' && text.sm, className)",
              "+  const cls = stylex.props(buttons[variant], size === 'sm' && styles.sm).className",
              '   const content = (',
            ],
          },
          {
            oldStart: 58,
            newStart: 59,
            section: 'return (',
            lines: [
              '   return (',
              '-    <button type="button" className={cls} disabled={disabled || loading}>',
              '+    <button',
              '+      type="button"',
              '+      className={cls}',
              '+      disabled={disabled || loading}',
              '+      {...rest}',
              '+    >',
              '       {content}',
              '     </button>',
              '   )',
            ],
          },
        ],
      },
      {
        path: 'apps/web/src/ui/DisplayBadge.tsx',
        added: 72,
        removed: 58,
        hunks: [
          {
            oldStart: 30,
            newStart: 30,
            section: 'export function DisplayBadge({',
            lines: [
              '   text,',
              '   color = true,',
              '   severity,',
              '-  icon,',
              "+  variant = 'subtle',",
              '+  icon,',
              "+  rounded = 'md',",
              '   children,',
              '   title,',
              ' }: {',
            ],
          },
        ],
      },
      {
        path: 'apps/web/src/ui/recipes.stylex.ts',
        added: 211,
        removed: 140,
        hunks: [
          {
            oldStart: 10,
            newStart: 10,
            section: 'const focusRing = {',
            lines: [
              ' } as const',
              ' ',
              '-export const buttons = stylex.create({',
              '-  action: {',
              '-    display: \'inline-flex\',',
              '+const btnBase = {',
              '+  display: \'inline-flex\',',
              '+  alignItems: \'center\',',
              '+  gap: 8,',
              '+  paddingTop: 5,',
              '+  paddingBottom: 5,',
              '+} as const',
            ],
          },
        ],
      },
    ],
  },
  {
    sha: '9f2d4b71e8a3c6d0f5b2e9a7c4d8f1b3a6e0c5d9',
    message: 'web: Commits screen — day-grouped fixture timeline',
    author: 'devin',
    at: '2026-09-18T07:48:00Z',
    parents: ['1b8e6c3af7d2e4b9a0c5f8e3d6b1a4c7e9f2d5b8'],
    filesChanged: [
      {
        path: 'apps/web/src/screens/CommitsScreen.tsx',
        added: 312,
        removed: 0,
        hunks: [
          {
            oldStart: 0,
            newStart: 1,
            lines: [
              "+import * as stylex from '@stylexjs/stylex'",
              "+import { useMemo, useState } from 'react'",
              "+import { useNavigate } from 'react-router'",
              "+import { FacetRail } from '../components/FacetRail'",
              "+import { MOCK_COMMITS, type MockCommit } from '../mock/commits'",
              "+import { font, vars } from '../ui/tokens.stylex'",
              '+',
              '+export function CommitsScreen() {',
            ],
          },
        ],
      },
      {
        path: 'apps/web/src/mock/commits.ts',
        added: 96,
        removed: 0,
        hunks: [
          {
            oldStart: 0,
            newStart: 1,
            lines: [
              '+/**',
              '+ * MOCK — fixture git history for the Commits surface.',
              '+ */',
              '+export interface MockCommitFile {',
              '+  path: string',
              '+  added: number',
              '+  removed: number',
              '+}',
            ],
          },
        ],
      },
    ],
  },
  {
    sha: '1b8e6c3af7d2e4b9a0c5f8e3d6b1a4c7e9f2d5b8',
    message: 'cli: cce query --json envelope for agent adapters',
    body: 'Agent adapters consume stdout verbatim, so --json emits a single\nenvelope { hits, route } instead of the human-readable path:line list.',
    author: 'kpack',
    at: '2026-09-18T06:20:00Z',
    parents: ['d5a91f06c3e7b4d2a8f5e1c9b6d3a0f4e7c2b5d9'],
    filesChanged: [
      {
        path: 'apps/cli/src/main.rs',
        added: 118,
        removed: 34,
        hunks: [
          {
            oldStart: 142,
            newStart: 142,
            section: 'fn render_query(',
            lines: [
              '     let hits = engine.query(&q)?;',
              '-    for hit in &hits {',
              '-        println!("{}:{}: {}", hit.path, hit.line, hit.snippet);',
              '-    }',
              '+    if args.json {',
              '+        let env = QueryEnvelope { hits: &hits, route: route.as_str() };',
              '+        println!("{}", serde_json::to_string(&env)?);',
              '+    } else {',
              '+        for hit in &hits {',
              '+            println!("{}:{}: {}", hit.path, hit.line, hit.snippet);',
              '+        }',
              '+    }',
              '     Ok(())',
            ],
          },
        ],
      },
    ],
  },
  {
    sha: 'd5a91f06c3e7b4d2a8f5e1c9b6d3a0f4e7c2b5d9',
    message: 'research: gold corpus — snapshot-v3 relabel pass',
    author: 'june',
    at: '2026-09-18T02:15:00Z',
    parents: ['c5e99258b1d4f7a0e3c6b9d2f5a8e1c4b7d0f3a6'],
    filesChanged: [
      {
        path: 'research/cce_research/schema.py',
        added: 74,
        removed: 52,
        hunks: [
          {
            oldStart: 31,
            newStart: 31,
            section: 'class GoldRow(BaseModel):',
            lines: [
              '     query_id: str',
              '-    relevant: list[str]',
              '+    relevant: list[RelevantSpan]',
              '+    snapshot: str = "v3"',
              '     route: Literal["lexical", "dense", "structural"]',
              ' ',
              '-    def as_dict(self) -> dict:',
              '-        return {"query_id": self.query_id, "relevant": self.relevant}',
            ],
          },
        ],
      },
      {
        path: 'benchmarks/datasets/cce-self.jsonl',
        added: 412,
        removed: 388,
        hunks: [
          {
            oldStart: 204,
            newStart: 204,
            lines: [
              ' {"query_id": "q-0183", "route": "dense", "relevant": ["crates/cce-engine/src/dense.rs"]}',
              '-{"query_id": "q-0184", "route": "lexical", "relevant": ["crates/cce-engine/src/grep.rs:44"]}',
              '+{"query_id": "q-0184", "route": "lexical", "relevant": [{"path": "crates/cce-engine/src/grep.rs", "span": [44, 61]}]}',
              '-{"query_id": "q-0185", "route": "structural", "relevant": ["crates/cce-engine/src/scip.rs:112"]}',
              '+{"query_id": "q-0185", "route": "structural", "relevant": [{"path": "crates/cce-engine/src/scip.rs", "span": [112, 140]}]}',
              ' {"query_id": "q-0186", "route": "dense", "relevant": ["crates/cce-engine/src/rerank.rs"]}',
            ],
          },
        ],
      },
    ],
  },
  // ── Sep 17 ────────────────────────────────────────────────────────────
  {
    sha: 'c5e99258b1d4f7a0e3c6b9d2f5a8e1c4b7d0f3a6',
    message: 'engine: late-fusion rerank over dense+lexical',
    body: 'Dense and lexical scores now fuse through FusionWeights instead of a\nbare tuple, and candidates present in both lists get an overlap bonus\nbefore sorting. Warmup moved off the Retrieval trait — the daemon owns it.',
    author: 'wibus',
    at: '2026-09-17T22:41:00Z',
    parents: ['4c7b2e9df5a8c1e4b7d0a3f6c9e2b5d8a1f4c7e0'],
    verified: true,
    filesChanged: [
      {
        path: 'crates/cce-engine/src/rerank.rs',
        added: 184,
        removed: 32,
        hunks: [
          {
            oldStart: 1,
            newStart: 1,
            section: 'pub fn fuse(',
            lines: [
              ' pub fn fuse(',
              '     dense: &[ScoredHit],',
              '     lexical: &[ScoredHit],',
              '-    weights: (f32, f32),',
              '+    weights: FusionWeights,',
              ' ) -> Vec<ScoredHit> {',
              '     let mut acc: HashMap<DocId, f32> = HashMap::new();',
              '-    accumulate(&mut acc, dense, weights.0);',
              '-    accumulate(&mut acc, lexical, weights.1);',
              '+    accumulate(&mut acc, dense, weights.dense);',
              '+    accumulate(&mut acc, lexical, weights.lexical);',
              '+    rerank_pairs(&mut acc, dense, lexical);',
              '     into_sorted(acc)',
            ],
          },
          {
            oldStart: 88,
            newStart: 91,
            section: 'fn rerank_pairs(',
            lines: [
              ' fn rerank_pairs(acc: &mut HashMap<DocId, f32>, a: &[ScoredHit], b: &[ScoredHit]) {',
              '+    let in_b: HashSet<DocId> = b.iter().map(|h| h.doc).collect();',
              '+    for hit in a {',
              '+        if in_b.contains(&hit.doc) {',
              '+            *acc.entry(hit.doc).or_default() += OVERLAP_BONUS;',
              '+        }',
              '+    }',
              ' }',
            ],
          },
        ],
      },
      {
        path: 'crates/cce-engine/src/retrieval.rs',
        added: 96,
        removed: 71,
        hunks: [
          {
            oldStart: 88,
            newStart: 88,
            section: 'impl Retrieval for Engine {',
            lines: [
              '         let lexical = self.lexical.search(&query)?;',
              '-        let fused = rerank::fuse(&dense, &lexical, (0.6, 0.4));',
              '+        let fused = rerank::fuse(&dense, &lexical, self.weights);',
              '         Ok(self.pack(fused, &query)?)',
              '     }',
              ' ',
              '-    fn warmup(&self) {',
              '-        self.lexical.warmup();',
              '-    }',
            ],
          },
        ],
      },
      {
        path: 'crates/cce-engine/src/dense.rs',
        added: 44,
        removed: 12,
        hunks: [
          {
            oldStart: 57,
            newStart: 57,
            section: 'impl DenseIndex {',
            lines: [
              '     pub fn search(&self, q: &[f32], k: usize) -> Vec<ScoredHit> {',
              '-        self.flat.scan(q, k)',
              '+        match &self.backend {',
              '+            DenseBackend::Local(m) => m.scan(q, k),',
              '+            DenseBackend::Disabled => Vec::new(),',
              '+        }',
              '     }',
              ' }',
            ],
          },
        ],
      },
    ],
  },
  {
    sha: '4c7b2e9df5a8c1e4b7d0a3f6c9e2b5d8a1f4c7e0',
    message: 'web: Query screen — route-aware filter rail',
    author: 'devin',
    at: '2026-09-17T19:26:00Z',
    parents: ['9d1b42ea7c0f3e6b9d2a5c8f1e4b7d0a3f6c9e2b'],
    filesChanged: [
      {
        path: 'apps/web/src/screens/QueryScreen.tsx',
        added: 143,
        removed: 96,
        hunks: [
          {
            oldStart: 120,
            newStart: 120,
            section: 'const groups: FacetGroup[]',
            lines: [
              '   const groups: FacetGroup[] = useMemo(() => {',
              '-    return routeFacets(route)',
              '+    const base = routeFacets(route)',
              '+    return [...base, ...(dynamic ? dynamicFacets(dynamic) : [])]',
              '   }, [route, dynamic])',
              ' ',
              '-  const [hits, setHits] = useState<Hit[]>([])',
            ],
          },
        ],
      },
      {
        path: 'apps/web/src/components/FacetRail.tsx',
        added: 38,
        removed: 12,
        hunks: [
          {
            oldStart: 92,
            newStart: 92,
            section: 'const LIMIT = 6',
            lines: [
              '   const [open, setOpen] = useState(true)',
              '+  const [expanded, setExpanded] = useState(false)',
              '   const LIMIT = 6',
              '-  const options = group.options',
              '+  const options = expanded ? group.options : group.options.slice(0, LIMIT)',
              ' ',
              '   return (',
            ],
          },
        ],
      },
    ],
  },
  {
    sha: '9d1b42ea7c0f3e6b9d2a5c8f1e4b7d0a3f6c9e2b',
    message: 'daemon: lease-guard concurrent index writes',
    body: 'Serializes concurrent index writes behind a ttl lease — two daemons\ncan no longer interleave snapshot commits. The guard aborts renewal on\ndrop so a crashed writer\u2019s lease expires on its own.',
    author: 'wibus',
    at: '2026-09-17T18:03:00Z',
    parents: [
      '8e3f1a54d9b2c5e8f1a4d7c0b3e6f9a2c5d8e1b4',
      'a4c19e27f3b6d9c0e5a8f1b4d7c2e6a9f0b3d5c8e1',
    ],
    verified: true,
    filesChanged: [
      {
        path: 'apps/daemon/src/main.rs',
        added: 132,
        removed: 48,
        hunks: [
          {
            oldStart: 64,
            newStart: 64,
            section: 'async fn index_write(',
            lines: [
              '     let state = self.state.clone();',
              '-    tokio::spawn(async move { index::run(&state).await })',
              '+    let lease = state.locks.acquire("index", LEASE_TTL).await?;',
              '+    tokio::spawn(async move {',
              '+        let _guard = lease;',
              '+        index::run(&state).await',
              '+    })',
              ' }',
            ],
          },
        ],
      },
      {
        path: 'crates/cce-engine/src/lock.rs',
        added: 88,
        removed: 17,
        hunks: [
          {
            oldStart: 12,
            newStart: 12,
            section: 'pub struct LeaseGuard {',
            lines: [
              ' pub struct LeaseGuard {',
              '     path: PathBuf,',
              '+    ttl: Duration,',
              '+    renew: JoinHandle<()>,',
              ' }',
              ' ',
              ' impl Drop for LeaseGuard {',
              '     fn drop(&mut self) {',
              '-        let _ = fs::remove_file(&self.path);',
              '+        self.renew.abort();',
              '+        let _ = fs::remove_file(&self.path);',
              '     }',
            ],
          },
        ],
      },
    ],
  },
  {
    sha: '8e3f1a54d9b2c5e8f1a4d7c0b3e6f9a2c5d8e1b4',
    message: 'engine: snapshot_diff — per-view invalidation edges',
    author: 'june',
    at: '2026-09-17T14:37:00Z',
    parents: ['2d9c8b47e1f4a7d0c3b6e9f2a5d8c1e4b7f0a3d6'],
    filesChanged: [
      {
        path: 'crates/cce-engine/src/snapshot_diff.rs',
        added: 167,
        removed: 23,
        hunks: [
          {
            oldStart: 40,
            newStart: 40,
            section: 'pub fn diff_views(',
            lines: [
              ' pub fn diff_views(old: &Snapshot, new: &Snapshot) -> ViewDelta {',
              '     let mut delta = ViewDelta::default();',
              '     for (id, view) in &new.views {',
              '-        if old.views.get(id) != Some(view) {',
              '-            delta.dirty.insert(*id);',
              '+        match old.views.get(id) {',
              '+            Some(v) if v == view => continue,',
              '+            _ => delta.dirty.insert(*id),',
              '         }',
              '     }',
              '+    for id in old.views.keys() {',
              '+        if !new.views.contains_key(id) {',
              '+            delta.dropped.insert(*id);',
              '+        }',
              '+    }',
              '     delta',
            ],
          },
        ],
      },
      {
        path: 'crates/cce-engine/src/history.rs',
        added: 59,
        removed: 31,
        hunks: [
          {
            oldStart: 74,
            newStart: 74,
            section: 'pub fn record(',
            lines: [
              '     pub fn record(&mut self, delta: ViewDelta) {',
              '-        self.entries.push((self.tick, delta.dirty_count()));',
              '+        self.entries.push(HistoryEntry {',
              '+            tick: self.tick,',
              '+            dirty: delta.dirty.iter().copied().collect(),',
              '+            dropped: delta.dropped.iter().copied().collect(),',
              '+        });',
              '         self.tick += 1;',
              '     }',
            ],
          },
        ],
      },
    ],
  },
  {
    sha: '2d9c8b47e1f4a7d0c3b6e9f2a5d8c1e4b7f0a3d6',
    message: 'web: Monitoring screen — diff watcher fixtures',
    author: 'devin',
    at: '2026-09-17T11:05:00Z',
    parents: ['77ac0d21f8e4b7a0d3c6f9e2b5a8d1c4f7b0e3a6'],
    filesChanged: [
      {
        path: 'apps/web/src/screens/MonitoringScreen.tsx',
        added: 154,
        removed: 0,
        hunks: [
          {
            oldStart: 0,
            newStart: 1,
            lines: [
              "+import * as stylex from '@stylexjs/stylex'",
              "+import { useMemo, useState } from 'react'",
              "+import { MOCK_MONITORS } from '../mock/monitors'",
              "+import { DisplayBadge } from '../ui/DisplayBadge'",
              "+import { font, vars } from '../ui/tokens.stylex'",
              '+',
              '+export function MonitoringScreen() {',
            ],
          },
        ],
      },
      {
        path: 'apps/web/src/mock/index.ts',
        added: 18,
        removed: 2,
        hunks: [
          {
            oldStart: 96,
            newStart: 96,
            section: 'export const MOCK_TIMELINE',
            lines: [
              '   {',
              "     id: 'tl-monitor',",
              "-    kind: 'push',",
              "+    kind: 'monitor',",
              "+    label: 'diff watcher fired — 2 paths',",
              "     at: '2026-09-17T10:58:00Z',",
              '   },',
            ],
          },
        ],
      },
    ],
  },
  // ── Sep 16 ────────────────────────────────────────────────────────────
  {
    sha: '77ac0d21f8e4b7a0d3c6f9e2b5a8d1c4f7b0e3a6',
    message: 'packing: deterministic budget split for context packs',
    author: 'wibus',
    at: '2026-09-16T15:29:00Z',
    parents: ['6a4e0d83c7b1f4e7a0d3c6b9f2e5a8d1c4b7f0e3'],
    verified: true,
    filesChanged: [
      {
        path: 'crates/cce-engine/src/context.rs',
        added: 157,
        removed: 89,
        hunks: [
          {
            oldStart: 102,
            newStart: 102,
            section: 'pub fn pack(',
            lines: [
              ' pub fn pack(hits: Vec<ScoredHit>, budget: Budget) -> ContextPack {',
              '-    let mut pack = ContextPack::with_budget(budget.tokens);',
              '-    for hit in hits {',
              '-        pack.try_push(hit);',
              '-    }',
              '+    let split = budget.split(STAGE_WEIGHTS);',
              '+    let mut pack = ContextPack::with_budget(split.total);',
              '+    for (stage, stage_hits) in group_by_stage(hits) {',
              '+        pack.try_push_stage(stage, stage_hits, split.for_stage(stage));',
              '+    }',
              '     pack',
              ' }',
            ],
          },
        ],
      },
      {
        path: 'crates/cce-engine/src/planner.rs',
        added: 63,
        removed: 24,
        hunks: [
          {
            oldStart: 33,
            newStart: 33,
            section: 'fn plan_query(',
            lines: [
              '     let route = router::classify(&q);',
              '-    Plan { route, budget: Budget::default() }',
              '+    let budget = Budget::default().with_stage_caps(route.stage_caps());',
              '+    Plan { route, budget }',
              ' }',
            ],
          },
        ],
      },
    ],
  },
  {
    sha: '6a4e0d83c7b1f4e7a0d3c6b9f2e5a8d1c4b7f0e3',
    message: 'daemon: watch mode — debounce index triggers',
    author: 'kpack',
    at: '2026-09-16T13:52:00Z',
    parents: ['b2c5f798e0d3a6c9f2e5b8d1c4a7f0e3b6d9c2f5'],
    filesChanged: [
      {
        path: 'apps/daemon/src/watch.rs',
        added: 121,
        removed: 63,
        hunks: [
          {
            oldStart: 45,
            newStart: 45,
            section: 'async fn drain_events(',
            lines: [
              '     while let Ok(ev) = rx.try_recv() {',
              '-        pending.insert(ev.path);',
              '+        pending.entry(ev.path).or_insert(ev.kind);',
              '     }',
              '-    if !pending.is_empty() {',
              '-        trigger_index(pending);',
              '-    }',
              '+    if pending.is_empty() {',
              '+        return;',
              '+    }',
              '+    sleep(DEBOUNCE_MS).await;',
              '+    trigger_index(mem::take(&mut pending));',
              ' }',
            ],
          },
        ],
      },
      {
        path: 'apps/cli/src/main.rs',
        added: 96,
        removed: 41,
        hunks: [
          {
            oldStart: 88,
            newStart: 88,
            section: 'Commands::Watch',
            lines: [
              '     Commands::Watch { path } => {',
              '-        daemon::watch(path)?;',
              '+        let rt = tokio::runtime::Runtime::new()?;',
              '+        rt.block_on(daemon::watch(path, &args))?;',
              '     }',
              '     Commands::Query { q } => {',
              '         let hits = engine.query(&q)?;',
            ],
          },
        ],
      },
    ],
  },
  {
    sha: 'b2c5f798e0d3a6c9f2e5b8d1c4a7f0e3b6d9c2f5',
    message: 'web: Notebooks screen — block fixture surface',
    author: 'june',
    at: '2026-09-16T10:18:00Z',
    parents: ['7f1a9c62b5d8e1f4a7c0b3e6d9f2a5c8e1b4d7f0'],
    filesChanged: [
      {
        path: 'apps/web/src/screens/NotebooksScreen.tsx',
        added: 168,
        removed: 0,
        hunks: [
          {
            oldStart: 0,
            newStart: 1,
            lines: [
              "+import * as stylex from '@stylexjs/stylex'",
              "+import { useMemo, useState } from 'react'",
              "+import { MOCK_NOTEBOOKS } from '../mock/notebooks'",
              "+import { DisplayBadge } from '../ui/DisplayBadge'",
              "+import { font, vars } from '../ui/tokens.stylex'",
              '+',
              '+export function NotebooksScreen() {',
            ],
          },
        ],
      },
      {
        path: 'apps/web/src/mock/index.ts',
        added: 12,
        removed: 3,
        hunks: [
          {
            oldStart: 120,
            newStart: 120,
            section: 'MOCK_TIMELINE',
            lines: [
              '   {',
              "-    kind: 'file',",
              "+    kind: 'notebook',",
              "+    label: 'nb: retrieval-eval.ipynb',",
              "     at: '2026-09-16T09:40:00Z',",
              '   },',
            ],
          },
        ],
      },
    ],
  },
  // ── Sep 15 ────────────────────────────────────────────────────────────
  {
    sha: '7f1a9c62b5d8e1f4a7c0b3e6d9f2a5c8e1b4d7f0',
    message: 'research: trajectory export for SFT runs',
    author: 'june',
    at: '2026-09-15T16:44:00Z',
    parents: ['e3f08b90a2c5d8e1f4b7a0c3e6d9f2b5a8e1c4d7'],
    filesChanged: [
      {
        path: 'research/cce_research/trajectory.py',
        added: 143,
        removed: 27,
        hunks: [
          {
            oldStart: 52,
            newStart: 52,
            section: 'def export_trajectory(',
            lines: [
              ' def export_trajectory(run: Run, out: Path) -> None:',
              '     rows = [step.as_row() for step in run.steps]',
              '-    pd.DataFrame(rows).to_csv(out)',
              '+    df = pd.DataFrame(rows)',
              '+    df["reward"] = df["score"].clip(0.0, 1.0)',
              '+    df.to_json(out, orient="records", lines=True)',
              '     log.info("wrote %d steps -> %s", len(rows), out)',
            ],
          },
        ],
      },
      {
        path: 'research/cce_research/stats.py',
        added: 58,
        removed: 19,
        hunks: [
          {
            oldStart: 18,
            newStart: 18,
            section: 'def bootstrap_ci(',
            lines: [
              ' def bootstrap_ci(vals: list[float], n: int = 1000) -> tuple[float, float]:',
              '-    means = [np.mean(rng.choice(vals, len(vals))) for _ in range(n)]',
              '+    samples = rng.choice(vals, (n, len(vals)), replace=True)',
              '+    means = samples.mean(axis=1)',
              '     return (np.percentile(means, 2.5), np.percentile(means, 97.5))',
            ],
          },
        ],
      },
    ],
  },
  {
    sha: 'e3f08b90a2c5d8e1f4b7a0c3e6d9f2b5a8e1c4d7',
    message: 'scip: emit reference edges for trait impls',
    author: 'wibus',
    at: '2026-09-15T11:52:00Z',
    parents: ['e6d3b815f9c2e5a8d1b4c7f0e3a6d9c2b5f8e1a4'],
    filesChanged: [
      {
        path: 'crates/cce-engine/src/scip.rs',
        added: 142,
        removed: 38,
        hunks: [
          {
            oldStart: 96,
            newStart: 96,
            section: 'fn emit_relationships(',
            lines: [
              '     for rel in &sym.relationships {',
              '-        if rel.is_reference {',
              '+        if rel.is_reference || rel.is_implementation {',
              '             edges.push(Edge {',
              '                 from: sym.id,',
              '                 to: rel.symbol.clone(),',
              '-                kind: EdgeKind::Reference,',
              '+                kind: if rel.is_implementation {',
              '+                    EdgeKind::Implements',
              '+                } else {',
              '+                    EdgeKind::Reference',
              '+                },',
              '             });',
              '         }',
            ],
          },
        ],
      },
      {
        path: 'crates/cce-engine/src/relations.rs',
        added: 57,
        removed: 9,
        hunks: [
          {
            oldStart: 8,
            newStart: 8,
            section: 'pub enum EdgeKind {',
            lines: [
              ' pub enum EdgeKind {',
              '     Reference,',
              '     Definition,',
              '+    Implements,',
              '+    TypeDefinition,',
              ' }',
            ],
          },
        ],
      },
    ],
  },
  {
    sha: 'e6d3b815f9c2e5a8d1b4c7f0e3a6d9c2b5f8e1a4',
    message: 'engine: grep route — literal fast path',
    author: 'wibus',
    at: '2026-09-15T09:31:00Z',
    parents: ['3a8d5f09c6e2b5d8a1f4c7e0b3a6d9c2f5b8e1d4'],
    filesChanged: [
      {
        path: 'crates/cce-engine/src/grep.rs',
        added: 104,
        removed: 36,
        hunks: [
          {
            oldStart: 60,
            newStart: 60,
            section: 'pub fn search(',
            lines: [
              '     let plan = match LiteralPlan::try_from(pattern) {',
              '-        Ok(_) => regex::search(pattern, corpus),',
              '+        Ok(lit) => memchr::scan(&lit, corpus),',
              '+        Err(_) => regex::search(pattern, corpus),',
              '     };',
              '     plan.collect()',
              ' }',
            ],
          },
        ],
      },
      {
        path: 'crates/cce-engine/src/retrieval.rs',
        added: 22,
        removed: 18,
        hunks: [
          {
            oldStart: 141,
            newStart: 141,
            section: 'Route::Grep',
            lines: [
              '         Route::Grep(pattern) => {',
              '-            let hits = self.grep.search(&pattern)?;',
              '+            let hits = self.grep.search(&pattern)?.into_scored();',
              '             Ok(self.pack(hits, &query)?)',
              '         }',
            ],
          },
        ],
      },
    ],
  },
  {
    sha: '3a8d5f09c6e2b5d8a1f4c7e0b3a6d9c2f5b8e1d4',
    message: 'benchmarks: self-v7 flow run manifests',
    author: 'team',
    at: '2026-09-15T08:02:00Z',
    parents: ['42b6e1f7d8a3c6e9f2b5d0a7c4e8f1b3d6a0c5e8'],
    filesChanged: [
      {
        path: 'benchmarks/results/self-v7-flow.jsonl.manifest.json',
        added: 24,
        removed: 0,
        hunks: [
          {
            oldStart: 0,
            newStart: 1,
            lines: [
              '+{',
              '+  "dataset": "cce-self-v7",',
              '+  "flow": "retrieval-eval",',
              '+  "rows": 412,',
              '+  "sha256": "9f2d4b71e8a3c6d0",',
              '+  "created": "2026-09-15T08:02:00Z"',
              '+}',
            ],
          },
        ],
      },
      {
        path: 'benchmarks/results/self-v7-perturb.jsonl.manifest.json',
        added: 24,
        removed: 0,
        hunks: [
          {
            oldStart: 0,
            newStart: 1,
            lines: [
              '+{',
              '+  "dataset": "cce-self-v7",',
              '+  "flow": "perturbation",',
              '+  "rows": 96,',
              '+  "sha256": "1b8e6c3af7d2e4b9",',
              '+  "created": "2026-09-15T08:02:00Z"',
              '+}',
            ],
          },
        ],
      },
    ],
  },
  // ── Sep 14 ────────────────────────────────────────────────────────────
  {
    sha: '42b6e1f7d8a3c6e9f2b5d0a7c4e8f1b3d6a0c5e8',
    message: 'web: redesign — StyleX token layer, dark parity',
    author: 'wibus',
    at: '2026-09-14T20:17:00Z',
    parents: ['5c9e2a76f1b4d7c0e3a6b9d2f5c8e1a4d7f0b3c6'],
    verified: true,
    filesChanged: [
      {
        path: 'apps/web/src/ui/tokens.stylex.ts',
        added: 136,
        removed: 52,
        hunks: [
          {
            oldStart: 40,
            newStart: 40,
            section: 'export const vars',
            lines: [
              "   primary500: '#5b9544',",
              "+  primary600: '#49833e',",
              "+  onPrimary: '#ffffff',",
              ' ',
              "-  scaleLow: '#65a30d',",
              '+  // Severity ramp (gray -> lime -> amber -> orange -> red)',
              "+  scaleLow: '#4d7c0f',",
              "   scaleMedium: '#b45309',",
            ],
          },
        ],
      },
      {
        path: 'apps/web/src/styles.css',
        added: 41,
        removed: 87,
        hunks: [
          {
            oldStart: 10,
            newStart: 10,
            section: ':root {',
            lines: [
              ' :root {',
              '-  --bg: #ffffff;',
              '-  --fg: #262626;',
              '-  --muted: #6b6b6b;',
              '+  color-scheme: light dark;',
              '+  font-family: ui-sans-serif, system-ui, sans-serif;',
              ' }',
              ' ',
              '-body { background: var(--bg); color: var(--fg); }',
            ],
          },
        ],
      },
      {
        path: 'apps/web/src/ui/recipes.stylex.ts',
        added: 98,
        removed: 61,
        hunks: [
          {
            oldStart: 46,
            newStart: 46,
            section: 'export const badges',
            lines: [
              ' export const badges = stylex.create({',
              '   base: {',
              "     display: 'inline-flex',",
              "-    padding: '2px 6px',",
              '+    paddingTop: 2,',
              '+    paddingBottom: 2,',
              '+    paddingLeft: 6,',
              '+    paddingRight: 6,',
              '     borderRadius: 6,',
            ],
          },
        ],
      },
    ],
  },
  {
    sha: '5c9e2a76f1b4d7c0e3a6b9d2f5c8e1a4d7f0b3c6',
    message: 'research: model_benchmark — dense backend matrix',
    author: 'kpack',
    at: '2026-09-14T15:40:00Z',
    parents: ['0d7f4b28a6c9e2f5b8d1a4c7e0b3f6a9d2c5e8b1'],
    filesChanged: [
      {
        path: 'research/cce_research/model_benchmark.py',
        added: 186,
        removed: 74,
        hunks: [
          {
            oldStart: 71,
            newStart: 71,
            section: 'BACKENDS = {',
            lines: [
              ' BACKENDS = {',
              '-    "onnx": OnnxBackend(),',
              '+    "onnx-int8": OnnxBackend(quant="int8"),',
              '+    "onnx-fp16": OnnxBackend(quant="fp16"),',
              '+    "candle": CandleBackend(),',
              '     "disabled": DisabledBackend(),',
              ' }',
            ],
          },
        ],
      },
      {
        path: 'research/cce_research/pooling.py',
        added: 92,
        removed: 55,
        hunks: [
          {
            oldStart: 22,
            newStart: 22,
            section: 'def mean_pool(',
            lines: [
              ' def mean_pool(hidden: np.ndarray, mask: np.ndarray) -> np.ndarray:',
              '-    return hidden.mean(axis=1)',
              '+    masked = hidden * mask[..., None]',
              '+    return masked.sum(axis=1) / mask.sum(axis=1, keepdims=True).clip(min=1)',
              ' ',
              '+def cls_pool(hidden: np.ndarray) -> np.ndarray:',
              '+    return hidden[:, 0]',
            ],
          },
        ],
      },
    ],
  },
  // ── Sep 13 ────────────────────────────────────────────────────────────
  {
    sha: '0d7f4b28a6c9e2f5b8d1a4c7e0b3f6a9d2c5e8b1',
    message: 'daemon: file watcher — ignore artifact-store churn',
    author: 'devin',
    at: '2026-09-13T17:29:00Z',
    parents: ['c1e9a637d4f7b0e3a6c9d2f5b8e1a4d7c0f3b6e9'],
    filesChanged: [
      {
        path: 'apps/daemon/src/watch.rs',
        added: 78,
        removed: 34,
        hunks: [
          {
            oldStart: 70,
            newStart: 70,
            section: 'fn accept(',
            lines: [
              ' fn accept(path: &Path) -> bool {',
              '-    !path.starts_with(&artifact_root())',
              '+    let rel = path.strip_prefix(repo_root()).unwrap_or(path);',
              '+    !ignore::matcher().is_ignored(rel)',
              ' }',
            ],
          },
        ],
      },
      {
        path: 'crates/cce-engine/src/ignore.rs',
        added: 45,
        removed: 12,
        hunks: [
          {
            oldStart: 5,
            newStart: 5,
            section: 'pub fn matcher(',
            lines: [
              " pub fn matcher() -> &'static Gitignore {",
              '     static M: OnceLock<Gitignore> = OnceLock::new();',
              '     M.get_or_init(|| {',
              '-        Gitignore::new(repo_root().join(".gitignore")).0',
              '+        let (mut g, _) = Gitignore::new(repo_root().join(".gitignore"));',
              '+        g.add(".cce/artifacts/**");',
              '+        g',
              '     })',
              ' }',
            ],
          },
        ],
      },
    ],
  },
  {
    sha: 'c1e9a637d4f7b0e3a6c9d2f5b8e1a4d7c0f3b6e9',
    message: 'web: Insights screen — code-health fixture charts',
    author: 'june',
    at: '2026-09-13T11:53:00Z',
    parents: ['94b6d1e5c8f2a5b8d1e4c7f0a3b6d9c2e5f8a1d4'],
    filesChanged: [
      {
        path: 'apps/web/src/screens/InsightsScreen.tsx',
        added: 142,
        removed: 0,
        hunks: [
          {
            oldStart: 0,
            newStart: 1,
            lines: [
              "+import * as stylex from '@stylexjs/stylex'",
              "+import { MOCK_INSIGHTS } from '../mock/insights'",
              "+import { DisplayBar } from '../ui/DisplayCharts'",
              "+import { font, vars } from '../ui/tokens.stylex'",
              '+',
              '+export function InsightsScreen() {',
            ],
          },
        ],
      },
      {
        path: 'apps/web/src/ui/DisplayCharts.tsx',
        added: 87,
        removed: 19,
        hunks: [
          {
            oldStart: 30,
            newStart: 30,
            section: 'export function DisplayBar(',
            lines: [
              '   return (',
              '     <div {...stylex.props(styles.row)}>',
              '-      <span>{label}</span>',
              "+      <span {...stylex.props(styles.label)}>{label}</span>",
              '       <span {...stylex.props(styles.track)}>',
              '+        <span',
              '+          {...stylex.props(styles.fill)}',
              '+          style={{ width: `${pct}%` }}',
              '+        />',
              '       </span>',
            ],
          },
        ],
      },
    ],
  },
  // ── Sep 12 ────────────────────────────────────────────────────────────
  {
    sha: '94b6d1e5c8f2a5b8d1e4c7f0a3b6d9c2e5f8a1d4',
    message: 'engine: history view — commit graph materialization',
    author: 'wibus',
    at: '2026-09-12T19:08:00Z',
    parents: ['f3a8c2d9e6b1f4a7d0c3e6b9f2a5d8c1e4b7f0a3'],
    filesChanged: [
      {
        path: 'crates/cce-engine/src/history.rs',
        added: 203,
        removed: 0,
        hunks: [
          {
            oldStart: 0,
            newStart: 1,
            lines: [
              '+use std::collections::BTreeSet;',
              '+',
              '+/// One recorded view-delta at a snapshot tick.',
              '+#[derive(Default)]',
              '+pub struct History {',
              '+    tick: u64,',
              '+    entries: Vec<HistoryEntry>,',
              '+}',
            ],
          },
        ],
      },
      {
        path: 'crates/cce-engine/src/repository.rs',
        added: 96,
        removed: 41,
        hunks: [
          {
            oldStart: 55,
            newStart: 55,
            section: 'pub fn materialize(',
            lines: [
              '     pub fn materialize(&self) -> Result<CommitGraph> {',
              '-        let commits = self.read_refs()?;',
              '+        let commits = self.walk_refs(WalkOrder::Topo)?;',
              '-        Ok(CommitGraph::default())',
              '+        Ok(CommitGraph::from_commits(commits))',
              '     }',
            ],
          },
        ],
      },
    ],
  },
  {
    sha: 'f3a8c2d9e6b1f4a7d0c3e6b9f2a5d8c1e4b7f0a3',
    message: 'research: ablations — retrieval stage isolation',
    author: 'june',
    at: '2026-09-12T14:22:00Z',
    parents: ['a5f8e2b1c4d7e0f3a6b9c2e5d8f1a4c7e0b3f6a9'],
    filesChanged: [
      {
        path: 'research/cce_research/ablations.py',
        added: 128,
        removed: 46,
        hunks: [
          {
            oldStart: 40,
            newStart: 40,
            section: 'ABLATIONS = [',
            lines: [
              ' ABLATIONS = [',
              '     "no-rerank",',
              '-    "no-dense",',
              '+    "no-dense",    # lexical only',
              '+    "no-lexical",  # dense only',
              '+    "no-pack",     # raw hits',
              ' ]',
              ' ',
              ' def run_ablation(name: str, corpus: Corpus) -> Result:',
            ],
          },
        ],
      },
      {
        path: 'research/cce_research/metrics.py',
        added: 71,
        removed: 33,
        hunks: [
          {
            oldStart: 14,
            newStart: 14,
            section: 'def recall_at_k(',
            lines: [
              ' def recall_at_k(ranked: list[str], gold: set[str], k: int) -> float:',
              '     if not gold:',
              '         return 0.0',
              '-    return len(set(ranked[:k]) & gold) / len(gold)',
              '+    hits = len(set(ranked[:k]) & gold)',
              '+    return hits / len(gold)',
              ' ',
              '+def mrr(ranked: list[str], gold: set[str]) -> float:',
            ],
          },
        ],
      },
    ],
  },
  // ── Sep 11 ─────────── older page — revealed by 'Load older' ───────────
  {
    sha: 'a5f8e2b1c4d7e0f3a6b9c2e5d8f1a4c7e0b3f6a9',
    message: 'engine: artifact store — CAS write path',
    author: 'wibus',
    at: '2026-09-11T16:02:00Z',
    parents: ['b7d2e9a4c6f1e8b0d3a5c7e9f2b4d6a8c0e2f4a6'],
    verified: true,
    filesChanged: [
      {
        path: 'crates/cce-engine/src/artifact.rs',
        added: 156,
        removed: 0,
        hunks: [
          {
            oldStart: 0,
            newStart: 1,
            lines: [
              '+use sha2::{Digest, Sha256};',
              '+',
              '+/// Content-addressed key — the hex digest of the stored payload.',
              '+#[derive(Clone, Copy, PartialEq, Eq, Hash)]',
              '+pub struct ArtifactKey(pub [u8; 32]);',
              '+',
              '+impl ArtifactKey {',
              '+    pub fn of(bytes: &[u8]) -> Self {',
            ],
          },
        ],
      },
      {
        path: 'crates/cce-engine/src/store.rs',
        added: 64,
        removed: 18,
        hunks: [
          {
            oldStart: 20,
            newStart: 20,
            section: 'pub fn put(',
            lines: [
              '     pub fn put(&self, bytes: &[u8]) -> Result<ArtifactKey> {',
              '-        let key = ArtifactKey::random();',
              '+        let key = ArtifactKey::of(bytes);',
              '+        if self.has(&key) {',
              '+            return Ok(key); // already stored — CAS dedup',
              '+        }',
              '         fs::write(self.path(&key), bytes)?;',
              '         Ok(key)',
              '     }',
            ],
          },
        ],
      },
    ],
  },
  {
    sha: 'b7d2e9a4c6f1e8b0d3a5c7e9f2b4d6a8c0e2f4a6',
    message: 'web: Symbols screen — documentSymbol fixtures',
    author: 'devin',
    at: '2026-09-11T11:24:00Z',
    parents: ['c9e1b4d7a2f5c8e0b3d6a9c1e4f7b0d2a5c8e1f4'],
    filesChanged: [
      {
        path: 'apps/web/src/screens/SymbolsScreen.tsx',
        added: 118,
        removed: 0,
        hunks: [
          {
            oldStart: 0,
            newStart: 1,
            lines: [
              "+import * as stylex from '@stylexjs/stylex'",
              "+import { MOCK_SYMBOLS } from '../mock/index'",
              "+import { DisplayBadge } from '../ui/DisplayBadge'",
              "+import { font, vars } from '../ui/tokens.stylex'",
              '+',
              '+export function SymbolsScreen() {',
            ],
          },
        ],
      },
      {
        path: 'apps/web/src/mock/index.ts',
        added: 34,
        removed: 6,
        hunks: [
          {
            oldStart: 60,
            newStart: 60,
            section: 'MOCK_SYMBOLS',
            lines: [
              '   symbols: [',
              "-    { name: 'fuse', kind: 'fn' },",
              "+    { name: 'fuse', kind: 'fn', path: 'crates/cce-engine/src/rerank.rs' },",
              "+    { name: 'pack', kind: 'fn', path: 'crates/cce-engine/src/context.rs' },",
              '   ],',
            ],
          },
        ],
      },
    ],
  },
  // ── Sep 10 ────────────────────────────────────────────────────────────
  {
    sha: 'c9e1b4d7a2f5c8e0b3d6a9c1e4f7b0d2a5c8e1f4',
    message: 'cli: cce index — progress reporting',
    author: 'kpack',
    at: '2026-09-10T15:47:00Z',
    parents: ['d2a6c8e0f4b7d9a1c3e5b8d0f2a4c6e8b0d2a4c6'],
    filesChanged: [
      {
        path: 'apps/cli/src/main.rs',
        added: 76,
        removed: 22,
        hunks: [
          {
            oldStart: 61,
            newStart: 61,
            section: 'Commands::Index',
            lines: [
              '     Commands::Index { path } => {',
              '-        index::run(path)?;',
              '+        let pb = Progress::new(file_count(path)?);',
              '+        index::run_with(path, |done| pb.set(done))?;',
              '+        pb.finish();',
              '     }',
              '     Commands::Query { q } => {',
            ],
          },
        ],
      },
      {
        path: 'apps/cli/src/progress.rs',
        added: 52,
        removed: 0,
        hunks: [
          {
            oldStart: 0,
            newStart: 1,
            lines: [
              '+/// Terminal progress reporter — stderr ticks, no deps.',
              '+pub struct Progress {',
              '+    total: u64,',
              '+    done: u64,',
              '+}',
              '+',
              '+impl Progress {',
              '+    pub fn new(total: u64) -> Self {',
            ],
          },
        ],
      },
    ],
  },
  {
    sha: 'd2a6c8e0f4b7d9a1c3e5b8d0f2a4c6e8b0d2a4c6',
    message: 'engine: lexical — bm25 scorer tuning',
    author: 'wibus',
    at: '2026-09-10T10:13:00Z',
    parents: ['e4b8d0f2a6c8e0b2d4a6c8e0b2d4a6c8e0b2d4a6'],
    verified: true,
    filesChanged: [
      {
        path: 'crates/cce-engine/src/lexical.rs',
        added: 88,
        removed: 41,
        hunks: [
          {
            oldStart: 9,
            newStart: 9,
            lines: [
              '-const K1: f32 = 1.2;',
              '-const B: f32 = 0.75;',
              '+const K1: f32 = 1.5;',
              '+const B: f32 = 0.6;',
              ' ',
              ' fn bm25(tf: f32, idf: f32, norm: f32) -> f32 {',
              '     idf * (tf * (K1 + 1.0)) / (tf + K1 * norm)',
              ' }',
            ],
          },
        ],
      },
      {
        path: 'crates/cce-engine/src/retrieval.rs',
        added: 31,
        removed: 9,
        hunks: [
          {
            oldStart: 133,
            newStart: 133,
            section: 'Route::Lexical',
            lines: [
              '         Route::Lexical => {',
              '-            self.lexical.search(&query)',
              '+            self.lexical.search_scoped(&query, scope)',
              '         }',
            ],
          },
        ],
      },
    ],
  },
  // ── Sep 9 ─────────────────────────────────────────────────────────────
  {
    sha: 'e4b8d0f2a6c8e0b2d4a6c8e0b2d4a6c8e0b2d4a6',
    message: 'research: eval harness — per-query metrics',
    author: 'june',
    at: '2026-09-09T18:31:00Z',
    parents: ['f6c0e2a4b8d0c2e4a6b8d0c2e4a6b8d0c2e4a6c8'],
    filesChanged: [
      {
        path: 'research/cce_research/eval.py',
        added: 164,
        removed: 0,
        hunks: [
          {
            oldStart: 0,
            newStart: 1,
            lines: [
              '+"""Per-query retrieval metrics — recall@k, MRR, nDCG."""',
              '+from __future__ import annotations',
              '+',
              '+import math',
              '+',
              '+def recall_at_k(ranked: list[str], gold: set[str], k: int) -> float:',
            ],
          },
        ],
      },
      {
        path: 'research/cce_research/metrics.py',
        added: 42,
        removed: 15,
        hunks: [
          {
            oldStart: 8,
            newStart: 8,
            section: 'def precision_at_k(',
            lines: [
              ' def precision_at_k(ranked: list[str], gold: set[str], k: int) -> float:',
              '     if k == 0:',
              '         return 0.0',
              '-    return len(set(ranked[:k]) & gold) / len(ranked[:k])',
              '+    hits = len(set(ranked[:k]) & gold)',
              '+    return hits / k',
            ],
          },
        ],
      },
    ],
  },
  {
    sha: 'f6c0e2a4b8d0c2e4a6b8d0c2e4a6b8d0c2e4a6c8',
    message: 'daemon: /v1/status — index freshness fields',
    author: 'devin',
    at: '2026-09-09T14:05:00Z',
    parents: ['a8d2f4b6c8e0a2c4e6b8d0a2c4e6b8d0a2c4e6a8'],
    filesChanged: [
      {
        path: 'apps/daemon/src/main.rs',
        added: 58,
        removed: 21,
        hunks: [
          {
            oldStart: 96,
            newStart: 96,
            section: 'async fn status(',
            lines: [
              '     async fn status(&self) -> Status {',
              '-        Status { ok: true }',
              '+        Status {',
              '+            ok: true,',
              '+            snapshot: self.index.snapshot_id(),',
              '+            fresh: self.index.is_fresh(),',
              '+        }',
              '     }',
            ],
          },
        ],
      },
      {
        path: 'apps/daemon/src/routes.rs',
        added: 44,
        removed: 12,
        hunks: [
          {
            oldStart: 12,
            newStart: 12,
            section: 'routes! {',
            lines: [
              ' routes! {',
              '+    GET /v1/status => status,',
              '     GET /v1/files => files,',
              '     GET /v1/file => file,',
              ' }',
            ],
          },
        ],
      },
    ],
  },
  // ── Sep 8 ─────────────────────────────────────────────────────────────
  {
    sha: 'a8d2f4b6c8e0a2c4e6b8d0a2c4e6b8d0a2c4e6a8',
    message: 'engine: snapshot — view-level staleness flags',
    author: 'wibus',
    at: '2026-09-08T16:58:00Z',
    parents: ['b0e4a6c8d0f2b4d6a8c0e2b4d6a8c0e2b4d6a8c0'],
    filesChanged: [
      {
        path: 'crates/cce-engine/src/snapshot.rs',
        added: 92,
        removed: 30,
        hunks: [
          {
            oldStart: 18,
            newStart: 18,
            section: 'pub struct View {',
            lines: [
              ' pub struct View {',
              '     pub id: ViewId,',
              '+    pub stale: bool,',
              '     pub manifest: Manifest,',
              ' }',
              ' ',
              '-fn all_dirty(views: &mut Views) {',
              '-    for v in views.values_mut() { v.dirty = true; }',
              '-}',
            ],
          },
        ],
      },
      {
        path: 'crates/cce-engine/src/repository.rs',
        added: 37,
        removed: 11,
        hunks: [
          {
            oldStart: 22,
            newStart: 22,
            section: 'fn mark_stale(',
            lines: [
              '-    fn mark_all_stale(&mut self) {',
              '-        for v in self.views.values_mut() {',
              '-            v.stale = true;',
              '-        }',
              '+    fn mark_stale(&mut self, ids: &BTreeSet<ViewId>) {',
              '+        for id in ids {',
              '+            self.views[id].stale = true;',
              '+        }',
              '     }',
            ],
          },
        ],
      },
    ],
  },
  {
    sha: 'b0e4a6c8d0f2b4d6a8c0e2b4d6a8c0e2b4d6a8c0',
    message: 'web: router — screen registry + deep links',
    author: 'devin',
    at: '2026-09-08T13:36:00Z',
    parents: ['c2e6b8d0a2f4c6e8b0d2a4c6e8b0d2a4c6e8b0d2'],
    filesChanged: [
      {
        path: 'apps/web/src/lib/navigation.ts',
        added: 77,
        removed: 0,
        hunks: [
          {
            oldStart: 0,
            newStart: 1,
            lines: [
              "+import { useCallback } from 'react'",
              "+import { useNavigate } from 'react-router'",
              '+',
              '+export function fileUrl(path: string, line?: number): string {',
              "+  const enc = path.split('/').map(encodeURIComponent).join('/')",
              '+  return `/browse/${enc}${line != null ? `?L=${line}` : ""}`',
              '+}',
            ],
          },
        ],
      },
      {
        path: 'apps/web/src/App.tsx',
        added: 29,
        removed: 14,
        hunks: [
          {
            oldStart: 40,
            newStart: 40,
            section: '<Routes>',
            lines: [
              '       <Routes>',
              '-        <Route path="/browse" element={<Browse/>}/>',
              '+        <Route path="/browse/*" element={<BrowseScreen/>}/>',
              '+        <Route path="/symbols" element={<SymbolsScreen/>}/>',
              '       </Routes>',
            ],
          },
        ],
      },
    ],
  },
  {
    sha: 'c2e6b8d0a2f4c6e8b0d2a4c6e8b0d2a4c6e8b0d2',
    message: 'research: dataset card — self-index corpus',
    author: 'team',
    at: '2026-09-08T09:52:00Z',
    parents: ['d4f8c0e2a4b6d8f0c2e4a6c8e0b2d4a6c8e0b2d4'],
    filesChanged: [
      {
        path: 'research/datasets/cce-self.md',
        added: 48,
        removed: 0,
        hunks: [
          {
            oldStart: 0,
            newStart: 1,
            lines: [
              '+# cce-self',
              '+',
              '+Gold retrieval corpus over this repository.',
              '+',
              '+- rows: 412',
              '+- snapshot: v3',
              '+- labels: path + span',
            ],
          },
        ],
      },
      {
        path: 'benchmarks/datasets/cce-self.jsonl',
        added: 412,
        removed: 0,
        hunks: [
          {
            oldStart: 0,
            newStart: 1,
            lines: [
              '+{"query_id": "q-0001", "route": "lexical", "relevant": ["crates/cce-engine/src/grep.rs"]}',
              '+{"query_id": "q-0002", "route": "dense", "relevant": ["crates/cce-engine/src/dense.rs"]}',
              '+{"query_id": "q-0003", "route": "structural", "relevant": ["crates/cce-engine/src/scip.rs"]}',
              '+{"query_id": "q-0004", "route": "lexical", "relevant": ["crates/cce-engine/src/lexical.rs"]}',
            ],
          },
        ],
      },
    ],
  },
  // ── Sep 7 ─────────────────────────────────────────────────────────────
  {
    sha: 'd4f8c0e2a4b6d8f0c2e4a6c8e0b2d4a6c8e0b2d4',
    message: 'engine: blob store — content addressing',
    body: 'Content addressing lands as a standalone Store so snapshots, packs\nand blobs share one dedup story. `internal` folds into `store`.',
    author: 'wibus',
    at: '2026-09-07T15:11:00Z',
    // Fixture window's floor — history continues past this page.
    parents: ['e8a1d3f5b7c9e1a3d5f7b9c1e3a5f7b9d1e3f5a7'],
    verified: true,
    filesChanged: [
      {
        path: 'crates/cce-engine/src/store.rs',
        added: 71,
        removed: 0,
        hunks: [
          {
            oldStart: 0,
            newStart: 1,
            lines: [
              '+use std::fs;',
              '+use std::path::{Path, PathBuf};',
              '+',
              '+/// On-disk blob store keyed by content digest.',
              '+pub struct Store {',
              '+    root: PathBuf,',
              '+}',
            ],
          },
        ],
      },
      {
        path: 'crates/cce-engine/src/lib.rs',
        added: 12,
        removed: 3,
        hunks: [
          {
            oldStart: 4,
            newStart: 4,
            lines: [
              ' pub mod engine;',
              '+pub mod store;',
              ' pub mod snapshot;',
              '-mod internal;',
            ],
          },
        ],
      },
    ],
  },
]
