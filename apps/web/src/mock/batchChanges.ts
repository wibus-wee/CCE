/**
 * MOCK — fixture batch changes for the Batch Changes surface (the
 * Sourcegraph Batch Changes analog). No `/v1/batch-changes` endpoint
 * exists and nothing reconciles changesets against a code host, so every
 * consumer must label the surface `preview` — spec excerpts, changeset
 * states, checks, reviews, unified diffs, burndown series and execution
 * history are fixtures, never live reconciliation truth.
 *
 * When the endpoint lands, delete this file and switch the screen to the
 * API; the shapes mirror the intended response.
 */

/** Changeset lifecycle — one reviewable diff per affected location. */
export type MockChangesetState = 'open' | 'merged' | 'closed' | 'draft' | 'failed'

/** CI rollup for the changeset's head commit. */
export type MockCheckState = 'passed' | 'failed' | 'pending' | 'none'

/** Code-host review rollup. */
export type MockReviewState = 'approved' | 'changes_requested' | 'pending' | 'none'

export interface MockChangeset {
  /** Canonical address of the affected location (one changeset each). */
  path: string
  state: MockChangesetState
  checkState: MockCheckState
  reviewState: MockReviewState
  /** Diffstat of the published diff. */
  added: number
  removed: number
  /**
   * Unified-diff excerpt — `@@` hunk headers plus ` `/`+`/`-`-prefixed
   * lines, like the code host's patch endpoint returns. Representative
   * hunks only; line counts need not equal the diffstat.
   */
  diff: string
  /** Last sync of the code-host state. */
  updatedAt: string
}

/** One line of the batch change's apply/preview/sync log. */
export interface MockExecution {
  at: string
  /** Verb, e.g. 'apply', 'execute', 'publish', 'sync', 'check', 'review'. */
  action: string
  detail: string
}

/** Cumulative merged-changeset count at a sync point — the burndown series. */
export interface MockMergePoint {
  at: string
  count: number
}

export interface MockBatchChange {
  name: string
  state: 'open' | 'merged' | 'closed' | 'draft'
  description: string
  /** YAML excerpt of the batch spec that produced the changesets. */
  spec: string
  changesets: MockChangeset[]
  /** Execution history, newest first. */
  executions: MockExecution[]
  /** Cumulative merges per sync, oldest first — drives the burndown chart. */
  mergedOverTime: MockMergePoint[]
  updatedAt: string
}

const hoursAgo = (h: number) => new Date(Date.now() - h * 3600_000).toISOString()
const daysAgo = (d: number) => hoursAgo(d * 24)

/**
 * The working set a fleet-edit surface would carry: two live batch changes
 * mid-review (mixed open/merged/draft/failed changesets), one fully landed
 * sweep, and one unpublished draft. Spec excerpts keep the real batch-spec
 * shape — `on`, `steps`, `changesetTemplate` — so the detail view reads
 * like Sourcegraph's.
 */
export const MOCK_BATCH_CHANGES: MockBatchChange[] = [
  {
    name: 'StyleX port — replace remaining CSS modules',
    state: 'open',
    description: 'Migrate legacy style sheets to atomic StyleX across apps/web.',
    spec: `name: stylex-port
description: >-
  Migrate remaining *.module.css files in apps/web to atomic
  stylex.create blocks on the token layer.
on:
  - repositoriesMatchingQuery: file:\\.module\\.css$ apps/web
steps:
  - run: scripts/stylex-migrate.ts --write
    container: node:22-alpine
changesetTemplate:
  title: 'web: port styles to StyleX'
  branch: batch/stylex-port
  commit:
    message: 'Migrate CSS module to stylex.create'
  published: true`,
    updatedAt: hoursAgo(5),
    mergedOverTime: [
      { at: daysAgo(9), count: 0 },
      { at: daysAgo(7), count: 1 },
      { at: daysAgo(4), count: 2 },
      { at: daysAgo(1), count: 3 },
      { at: hoursAgo(5), count: 3 },
    ],
    changesets: [
      {
        path: 'apps/web/src/screens/BrowseScreen.tsx',
        state: 'merged', checkState: 'passed', reviewState: 'approved',
        added: 148, removed: 162, updatedAt: hoursAgo(8),
        diff: `@@ -1,10 +1,10 @@
 import * as stylex from '@stylexjs/stylex'
-import styles from './BrowseScreen.module.css'
 import { useMemo, useState } from 'react'
 import { LayoutDataTable } from '../ui/LayoutStructure'
+import { font, vars } from '../ui/tokens.stylex'
 
 export function BrowseScreen() {
   return (
-    <div className={styles.root}>
+    <div {...stylex.props(styles.root)}>
@@ -96,6 +96,15 @@ export function BrowseScreen() {
   )
 }
 
+const styles = stylex.create({
+  root: {
+    display: 'flex',
+    flexDirection: 'column',
+    gap: 14,
+  },
+})`,
      },
      {
        path: 'apps/web/src/screens/QueryScreen.tsx',
        state: 'merged', checkState: 'passed', reviewState: 'approved',
        added: 214, removed: 238, updatedAt: hoursAgo(9),
        diff: `@@ -1,9 +1,9 @@
-import styles from './QueryScreen.module.css'
+import * as stylex from '@stylexjs/stylex'
 import { useCallback, useMemo, useRef, useState } from 'react'
 import { FormSearchField } from '../ui/FormSearchField'
+import { font, vars } from '../ui/tokens.stylex'
@@ -347,8 +347,8 @@ function HitRow({ hit, expanded }: HitRowProps) {
     <button
       type="button"
-      className={\`\${styles.row} \${expanded ? styles.open : ''}\`}
+      {...stylex.props(styles.row, expanded && styles.open)}
       aria-expanded={expanded}
       onClick={onToggle}
     >`,
      },
      {
        path: 'apps/web/src/screens/SymbolsScreen.tsx',
        state: 'merged', checkState: 'passed', reviewState: 'approved',
        added: 96, removed: 104, updatedAt: hoursAgo(11),
        diff: `@@ -1,7 +1,8 @@
-import styles from './SymbolsScreen.module.css'
+import * as stylex from '@stylexjs/stylex'
 import { LayoutVirtualList } from '../ui/LayoutStructure'
+import { vars } from '../ui/tokens.stylex'
@@ -41,7 +42,7 @@ function SymbolRow({ sym }: { sym: Symbol }) {
-        <span className={styles.kind}>{sym.kind}</span>
+        <span {...stylex.props(styles.kind)}>{sym.kind}</span>
         <DisplayFilePath path={sym.path} />
@@ -66,0 +67,10 @@ function SymbolRow({ sym }: { sym: Symbol }) {
+const styles = stylex.create({
+  kind: {
+    fontSize: 10,
+    textTransform: 'uppercase',
+    letterSpacing: '0.05em',
+    color: vars.colorFaint,
+  },
+})`,
      },
      {
        path: 'apps/web/src/components/FacetRail.tsx',
        state: 'open', checkState: 'passed', reviewState: 'approved',
        added: 88, removed: 92, updatedAt: hoursAgo(3),
        diff: `@@ -1,8 +1,8 @@
-import styles from './FacetRail.module.css'
+import * as stylex from '@stylexjs/stylex'
 import { IconCheckSmall } from '../ui/icons'
+import { vars } from '../ui/tokens.stylex'
@@ -30,9 +30,9 @@ function FacetOption({ opt, on }: FacetOptionProps) {
           <button
             type="button"
-            className={\`\${styles.opt} \${on ? styles.on : ''}\`}
+            {...stylex.props(styles.opt, on && styles.on)}
             aria-pressed={on}
           >
@@ -95,0 +96,10 @@ function FacetOption({ opt, on }: FacetOptionProps) {
+const styles = stylex.create({
+  opt: {
+    display: 'flex',
+    alignItems: 'center',
+    gap: 6,
+    color: vars.colorMuted,
+  },
+})`,
      },
      {
        path: 'apps/web/src/components/CommandPalette.tsx',
        state: 'open', checkState: 'pending', reviewState: 'pending',
        added: 132, removed: 140, updatedAt: hoursAgo(2),
        diff: `@@ -1,9 +1,9 @@
-import styles from './CommandPalette.module.css'
+import * as stylex from '@stylexjs/stylex'
 import { Dialog } from '@base-ui-components/react'
+import { vars } from '../ui/tokens.stylex'
@@ -55,7 +55,7 @@ export function CommandPalette() {
-        <kbd className={styles.kbd}>esc</kbd>
+        <kbd {...stylex.props(styles.kbd)}>esc</kbd>
         <span>to close</span>`,
      },
      {
        path: 'apps/web/src/screens/DashboardScreen.tsx',
        state: 'open', checkState: 'failed', reviewState: 'changes_requested',
        added: 176, removed: 158, updatedAt: hoursAgo(1.5),
        diff: `@@ -1,9 +1,9 @@
-import styles from './DashboardScreen.module.css'
+import * as stylex from '@stylexjs/stylex'
+import { vars } from '../ui/tokens.stylex'
 import { DisplayBadge } from '../ui/DisplayBadge'
@@ -74,7 +74,7 @@ function StatCard({ stat }: { stat: Stat }) {
-      <div className={styles.card}>
+      <div {...stylex.props(styles.card)}>
         <span>{stat.label}</span>
@@ -118,0 +119,10 @@ function StatCard({ stat }: { stat: Stat }) {
+const styles = stylex.create({
+  card: {
+    borderWidth: 1,
+    borderStyle: 'solid',
+    borderColor: vars.borderBase,
+    borderRadius: 8,
+  },
+})`,
      },
      {
        path: 'apps/web/src/screens/MonitoringScreen.tsx',
        state: 'draft', checkState: 'none', reviewState: 'none',
        added: 61, removed: 58, updatedAt: hoursAgo(12),
        diff: `@@ -1,8 +1,8 @@
-import styles from './MonitoringScreen.module.css'
+import * as stylex from '@stylexjs/stylex'
 import { ActionButton } from '../ui/ActionButton'
+import { vars } from '../ui/tokens.stylex'
@@ -149,7 +149,7 @@ function MonitorRow({ m }: { m: Monitor }) {
-          <span className={styles.cron}>{m.schedule}</span>
+          <span {...stylex.props(styles.cron)}>{m.schedule}</span>`,
      },
      {
        path: 'apps/web/src/screens/IndexScreen.tsx',
        state: 'failed', checkState: 'failed', reviewState: 'changes_requested',
        added: 202, removed: 187, updatedAt: hoursAgo(0.8),
        diff: `@@ -1,9 +1,9 @@
-import styles from './IndexScreen.module.css'
+import * as stylex from '@stylexjs/stylex'
+import { vars } from '../ui/tokens.stylex'
 import { DisplayProgressBar } from '../ui/DisplayProgressBar'
@@ -203,7 +203,7 @@ function IndexLane({ lane }: { lane: Lane }) {
-        <div className={styles.lane}>
+        <div {...stylex.props(styles.lane)}>
@@ -240,0 +241,9 @@ function IndexLane({ lane }: { lane: Lane }) {
+const styles = stylex.create({
+  lane: {
+    display: 'grid',
+    gridTemplateColumns: '1fr 1fr',
+    gap: 8,
+  },
+})`,
      },
      {
        path: 'apps/web/src/screens/InsightsScreen.tsx',
        state: 'closed', checkState: 'none', reviewState: 'none',
        added: 74, removed: 70, updatedAt: hoursAgo(30),
        diff: `@@ -1,9 +1,9 @@
-import styles from './InsightsScreen.module.css'
+import * as stylex from '@stylexjs/stylex'
 import { ActionToggleGroup } from '../ui/ActionToggleGroup'
+import { vars } from '../ui/tokens.stylex'
@@ -180,7 +180,7 @@ function InsightRow({ insight }: InsightRowProps) {
-      <button className={styles.row} onClick={onToggle}>
+      <button {...stylex.props(styles.row)} onClick={onToggle}>`,
      },
    ],
    executions: [
      { at: hoursAgo(0.8), action: 'sync', detail: 'IndexScreen.tsx marked failed — rebase conflict on main' },
      { at: hoursAgo(1.5), action: 'check', detail: 'DashboardScreen.tsx checks failed (eslint stylex rules)' },
      { at: hoursAgo(3), action: 'review', detail: 'FacetRail.tsx approved — ready to merge' },
      { at: hoursAgo(9), action: 'publish', detail: '8 of 9 changesets published · 1 held back as draft' },
      { at: hoursAgo(14), action: 'apply', detail: 'spec applied — resolved 9 workspaces in apps/web' },
    ],
  },
  {
    name: 'SCIP emitters — reference edges for trait impls',
    state: 'open',
    description: 'Graph completeness for the structural route.',
    spec: `name: scip-trait-edges
description: >-
  Emit reference edges for trait impls so the structural route
  resolves impl blocks, not just definitions.
on:
  - repositoriesMatchingQuery: lang:rust file:indexer|scip|graph
steps:
  - run: cargo run -p cce-scip -- --emit-refs
    container: rust:1.82-slim
changesetTemplate:
  title: 'scip: emit trait-impl reference edges'
  branch: batch/scip-trait-edges
  commit:
    message: 'Emit reference edges for trait impls'
  published: true`,
    updatedAt: hoursAgo(31),
    mergedOverTime: [
      { at: daysAgo(5), count: 0 },
      { at: daysAgo(2), count: 0 },
      { at: daysAgo(1.7), count: 1 },
      { at: hoursAgo(31), count: 1 },
    ],
    changesets: [
      {
        path: 'crates/cce-engine/src/indexer.rs',
        state: 'merged', checkState: 'passed', reviewState: 'approved',
        added: 86, removed: 41, updatedAt: hoursAgo(40),
        diff: `@@ -214,6 +214,14 @@ impl Indexer {
     for sym in doc.symbols() {
       self.graph.define(sym.id, sym.kind, span)?;
+      // Emit reference edges for trait impls so the structural
+      // route resolves impl blocks, not just definitions.
+      for tr in sym.impl_traits() {
+        self.graph.relate(sym.id, tr, EdgeKind::Reference)?;
+      }
     }
     Ok(())
   }`,
      },
      {
        path: 'crates/cce-engine/src/graph.rs',
        state: 'open', checkState: 'passed', reviewState: 'pending',
        added: 64, removed: 22, updatedAt: hoursAgo(6),
        diff: `@@ -88,6 +88,13 @@ impl Graph {
   }
+
+  /// Record a typed edge between two symbols.
+  pub fn relate(&mut self, from: SymId, to: SymId, kind: EdgeKind) -> Result<()> {
+    self.edges.push(Edge { from, to, kind });
+    self.adjacency.entry(from).or_default().push(to);
+    Ok(())
+  }
 
   pub fn degree(&self, id: SymId) -> usize {
     self.adjacency.get(&id).map_or(0, |v| v.len())`,
      },
      {
        path: 'crates/cce-engine/src/providers.rs',
        state: 'open', checkState: 'pending', reviewState: 'pending',
        added: 48, removed: 19, updatedAt: hoursAgo(6),
        diff: `@@ -34,7 +34,8 @@ fn register(registry: &mut Registry, opts: &ProviderOpts) {
-    registry.register(ScipProvider::new(false));
+    // --emit-refs turns on trait-impl reference edges.
+    registry.register(ScipProvider::new(opts.emit_refs));
     registry.register(CtagsProvider::default());
   }`,
      },
      {
        path: 'crates/cce-engine/src/scip.rs',
        state: 'open', checkState: 'failed', reviewState: 'changes_requested',
        added: 121, removed: 56, updatedAt: hoursAgo(4),
        diff: `@@ -140,6 +140,15 @@ impl ScipProvider {
       let occ = Occurrence::new(sym.id, role, span);
       self.sink.push(occ);
+      if self.emit_refs {
+        for tr in &sym.impl_traits {
+          self.sink.push(Occurrence::reference(
+            sym.id,
+            tr.id,
+          ));
+        }
+      }
     }
   }`,
      },
      {
        path: 'crates/cce-engine/src/snapshot.rs',
        state: 'draft', checkState: 'none', reviewState: 'none',
        added: 12, removed: 4, updatedAt: hoursAgo(33),
        diff: `@@ -22,6 +22,9 @@ pub struct Snapshot {
   pub files: Vec<FileEntry>,
+  /// Reference edges emitted by scip providers (v2+).
+  #[serde(default)]
+  pub ref_edges: Vec<Edge>,
 }`,
      },
    ],
    executions: [
      { at: hoursAgo(4), action: 'check', detail: 'scip.rs checks failed (clippy::pedantic)' },
      { at: hoursAgo(6), action: 'review', detail: 'graph.rs review requested from engine owners' },
      { at: hoursAgo(30), action: 'publish', detail: '4 of 5 changesets published' },
      { at: hoursAgo(34), action: 'apply', detail: 'spec applied — resolved 5 workspaces in crates/cce-engine' },
    ],
  },
  {
    name: 'Remove deprecated /v0 endpoints',
    state: 'merged',
    description: 'Cleanup sweep before the gateway split.',
    spec: `name: drop-v0-endpoints
description: >-
  Remove the deprecated /v0 surface handlers ahead of the gateway
  split — /v1 covers every route.
on:
  - repositoriesMatchingQuery: '/v0/' file:routes|api|gateway
steps:
  - run: scripts/drop-v0.sh --delete
    container: alpine:3.20
changesetTemplate:
  title: 'api: remove deprecated /v0 handlers'
  branch: batch/drop-v0
  commit:
    message: 'Remove deprecated /v0 endpoints'
  published: true`,
    updatedAt: hoursAgo(120),
    mergedOverTime: [
      { at: daysAgo(6), count: 0 },
      { at: daysAgo(5), count: 1 },
      { at: daysAgo(4.5), count: 2 },
      { at: daysAgo(4.2), count: 4 },
      { at: daysAgo(4), count: 5 },
      { at: hoursAgo(120), count: 5 },
    ],
    changesets: [
      {
        path: 'apps/daemon/src/routes/v0.ts',
        state: 'merged', checkState: 'passed', reviewState: 'approved',
        added: 0, removed: 214, updatedAt: hoursAgo(96),
        diff: `@@ -1,214 +0,0 @@
-import { Router } from 'express'
-import { legacyIndex } from '../handlers/index'
-import { legacyQuery } from '../handlers/query'
-import { legacySymbols } from '../handlers/symbols'
-
-export const v0 = Router()
-
-// Deprecated: every route below is covered by /v1.
-v0.get('/search', legacyQuery)
-v0.get('/symbols', legacySymbols)
-v0.post('/index', legacyIndex)`,
      },
      {
        path: 'apps/daemon/src/routes/legacy.ts',
        state: 'merged', checkState: 'passed', reviewState: 'approved',
        added: 12, removed: 168, updatedAt: hoursAgo(98),
        diff: `@@ -1,14 +1,6 @@
-import { Router } from 'express'
-import { v0 } from './v0'
-
-export const legacy = Router()
-legacy.use('/v0', v0)
+// /v0 handlers removed ahead of the gateway split —
+// /v1 covers every route. See routes/v1.ts.
+export {}
@@ -48,12 +40,4 @@
-export function mountLegacy(app: Express) {
-  app.use(legacy)
-}`,
      },
      {
        path: 'apps/cli/src/commands/query.ts',
        state: 'merged', checkState: 'passed', reviewState: 'approved',
        added: 31, removed: 58, updatedAt: hoursAgo(100),
        diff: `@@ -18,7 +18,7 @@
-    const res = await fetch(\`\${base}/v0/query\`, {
+    const res = await fetch(\`\${base}/v1/query\`, {
       method: 'POST',
@@ -30,9 +30,6 @@
-  // /v0 returns {results}; /v1 wraps hits in {data:{hits}}
-  const hits = body.results ?? body.data.hits
+  const hits = body.data.hits
   return hits`,
      },
      {
        path: 'crates/cce-engine/src/api.rs',
        state: 'merged', checkState: 'passed', reviewState: 'approved',
        added: 18, removed: 92, updatedAt: hoursAgo(102),
        diff: `@@ -55,11 +55,6 @@
-pub fn mount_v0(routes: &mut Router) {
-    routes.add("/v0/search", v0_search);
-    routes.add("/v0/symbols", v0_symbols);
-}
-
 pub fn mount_v1(routes: &mut Router) {
     routes.add("/v1/search", v1_search);
     routes.add("/v1/query", v1_query);
   }`,
      },
      {
        path: 'apps/web/src/api/client.ts',
        state: 'merged', checkState: 'passed', reviewState: 'approved',
        added: 22, removed: 47, updatedAt: hoursAgo(104),
        diff: `@@ -12,7 +12,7 @@
-const API = '/v0'
+const API = '/v1'
 
 export async function search(q: string) {
   return fetchJson(\`\${API}/search?q=\${q}\`)
@@ -44,8 +44,4 @@
-export async function legacySearch(q: string) {
-  return fetchJson(\`/v0/search?q=\${q}\`)
-}`,
      },
      {
        path: 'crates/cce-engine/src/gateway.rs',
        state: 'closed', checkState: 'none', reviewState: 'none',
        added: 4, removed: 36, updatedAt: hoursAgo(118),
        diff: `@@ -30,9 +30,6 @@ async fn route(req: Request) -> Response {
-    // /v0 shim — remove once batch/drop-v0 lands
-    if path.starts_with("/v0/") {
-        return proxy_legacy(req).await;
-    }
     route_v1(req).await
 }`,
      },
    ],
    executions: [
      { at: hoursAgo(96), action: 'sync', detail: '5 merged · 1 closed — all changesets resolved' },
      { at: hoursAgo(118), action: 'sync', detail: 'gateway.rs closed — superseded by feature/gateway-split' },
      { at: hoursAgo(122), action: 'publish', detail: '6 of 6 changesets published' },
      { at: hoursAgo(140), action: 'apply', detail: 'spec applied — resolved 6 workspaces' },
    ],
  },
  {
    name: 'Benchmark corpus refresh — gold v3',
    state: 'draft',
    description: 'Re-label query sets against the new snapshot format.',
    spec: `name: gold-corpus-v3
description: >-
  Re-label the gold query corpus against snapshot v3 — routes,
  hit sets and freshness assertions.
on:
  - repositoriesMatchingQuery: file:research/gold
steps:
  - run: python research/relabel.py --snapshot v3
    container: python:3.13-slim
changesetTemplate:
  title: 'research: re-label gold corpus v3'
  branch: batch/gold-v3
  commit:
    message: 'Re-label gold corpus against snapshot v3'
  published: false`,
    updatedAt: hoursAgo(200),
    mergedOverTime: [
      { at: daysAgo(8), count: 0 },
      { at: daysAgo(4), count: 0 },
      { at: hoursAgo(200), count: 0 },
    ],
    changesets: [
      {
        path: 'research/gold/queries.jsonl',
        state: 'draft', checkState: 'none', reviewState: 'none',
        added: 412, removed: 388, updatedAt: hoursAgo(200),
        diff: `@@ -38,5 +38,5 @@
-{"q":"auth middleware","route":"lexical","hits":["src/auth/mw.ts#L22","src/auth/jwt.ts#L8"],"snapshot":"v2"}
-{"q":"trait impl edges","route":"structural","hits":["crates/cce-engine/src/graph.rs#L91"],"snapshot":"v2"}
+{"q":"auth middleware","route":"lexical","hits":["src/auth/mw.ts#L22","src/auth/jwt.ts#L8"],"snapshot":"v3","freshness":"indexed"}
+{"q":"trait impl edges","route":"structural","hits":["crates/cce-engine/src/graph.rs#L91"],"snapshot":"v3","freshness":"indexed"}
 {"q":"dense fallback","route":"semantic","hits":["crates/cce-embed/src/lib.rs#L40"],"snapshot":"v2"}`,
      },
      {
        path: 'research/gold/routes.json',
        state: 'draft', checkState: 'none', reviewState: 'none',
        added: 96, removed: 90, updatedAt: hoursAgo(200),
        diff: `@@ -5,9 +5,10 @@
   "meta": {
-    "snapshot": "v2",
-    "assertions": ["hits", "latency"]
+    "snapshot": "v3",
+    "assertions": ["hits", "latency", "freshness"],
+    "freshness_window_s": 300
   }`,
      },
      {
        path: 'research/gold/freshness.yaml',
        state: 'draft', checkState: 'none', reviewState: 'none',
        added: 44, removed: 31, updatedAt: hoursAgo(200),
        diff: `@@ -1,7 +1,9 @@
-snapshot: v2
+snapshot: v3
 views:
   dense:
-    max_age_s: 600
+    max_age_s: 300
+    required: true
   lexical:
     max_age_s: 60`,
      },
      {
        path: 'research/relabel.py',
        state: 'draft', checkState: 'none', reviewState: 'none',
        added: 28, removed: 11, updatedAt: hoursAgo(200),
        diff: `@@ -14,8 +14,10 @@
-def relabel(row):
-    row["snapshot"] = "v2"
-    return row
+def relabel(row, snapshot="v3"):
+    row["snapshot"] = snapshot
+    row["freshness"] = "indexed"
+    return row
 
 for line in sys.stdin:
     print(json.dumps(relabel(json.loads(line))))`,
      },
    ],
    executions: [
      { at: hoursAgo(200), action: 'preview', detail: 'dry run — 0 published (spec unpublished)' },
      { at: hoursAgo(202), action: 'execute', detail: '4 workspaces resolved in research/ · all cached' },
      { at: hoursAgo(210), action: 'draft', detail: 'spec saved as draft — not applied' },
    ],
  },
]
