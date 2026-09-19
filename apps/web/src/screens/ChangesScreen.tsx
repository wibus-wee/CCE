import * as stylex from '@stylexjs/stylex'
import dagre from 'dagre'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useNavigate, useSearchParams } from 'react-router'
import {
  Background,
  BackgroundVariant,
  Controls,
  type Edge,
  Handle,
  MarkerType,
  MiniMap,
  type Node,
  Position,
  ReactFlow,
  ReactFlowProvider,
  useEdgesState,
  useNodesInitialized,
  useNodesState,
  useReactFlow,
  useStore,
} from '@xyflow/react'
import '@xyflow/react/dist/style.css'
import {
  api,
  describeError,
  type ArchitectureDiff,
  type ChangedRelation,
  type DiffEntityRef,
  type DiffRelation,
  type ImpactReport,
} from '../api'
import { fileUrl } from '../lib/navigation'
import { ActionIconButton } from '../ui/ActionIconButton'
import { ActionToggleGroup } from '../ui/ActionToggleGroup'
import { DisplayBadge } from '../ui/DisplayBadge'
import { DisplayFilePath } from '../ui/DisplayFilePath'
import { FeedbackEmptyState, FeedbackSkeleton } from '../ui/FeedbackStates'
import { FormSearchField } from '../ui/FormSearchField'
import { IconArrowUpRight, IconCaretDown, IconDiff, IconGraph, IconX } from '../ui/icons'
import { font, vars } from '../ui/tokens.stylex'
import { useColorScheme } from '../ui/useDark'

/**
 * Changes — the post-session review surface. A snapshot delta is not a
 * list, it is a graph operation: files appear, edges get wired, edges
 * get cut. So the primary view IS the graph — a file-level delta
 * rendered by React Flow + dagre: new files in green, modified in
 * amber, deletions in red, and files the change merely *reached into*
 * as grey context nodes. Solid green edges are new cross-file wiring;
 * dashed red edges are wiring that was dropped. Kind chips on the
 * canvas gate which relation kinds draw — `contains`/`changed_with`
 * bookkeeping starts off — and hovering or picking a node isolates its
 * neighborhood so dense deltas stay readable.
 *
 * The right rail holds the enumeration — one expandable row per file
 * with its symbol and relation deltas, removals first because that is
 * what review is for. Clicking a graph node turns the rail into that
 * file's detail pane (master-detail); the pane's ↗ jumps to the file.
 *
 * `?base=`/`?head=` override the default (previous committed, current)
 * pair for shareable diffs.
 */
export function ChangesScreen() {
  const [searchParams] = useSearchParams()
  const [diff, setDiff] = useState<ArchitectureDiff | null>(null)
  const [err, setErr] = useState<string>()
  const [busy, setBusy] = useState(true)
  const [text, setText] = useState('')
  const [selected, setSelected] = useState<string>()
  const scheme = useColorScheme()

  const base = searchParams.get('base') ?? undefined
  const head = searchParams.get('head') ?? undefined

  useEffect(() => {
    let dead = false
    setBusy(true)
    api
      .archDiff(base, head)
      .then((d) => !dead && setDiff(d))
      .catch((e) => !dead && setErr(describeError(e)))
      .finally(() => !dead && setBusy(false))
    return () => {
      dead = true
    }
  }, [base, head])

  const needle = text.trim().toLowerCase()
  const cards = useMemo(() => buildCards(diff), [diff])
  const visibleCards = useMemo(() => {
    if (!needle) return cards
    return cards.map((c) => filterCard(c, needle)).filter((c): c is FileDelta => c != null)
  }, [cards, needle])
  const story = useMemo(() => (diff ? buildStory(cards) : []), [cards, diff])
  const kinds = useMemo(() => graphKindCounts(diff), [diff])
  const [hiddenKinds, setHiddenKinds] = useState<ReadonlySet<string>>(() => new Set(NOISE_KINDS))
  const enabledKinds = useMemo(
    () => new Set(kinds.map(([k]) => k).filter((k) => !hiddenKinds.has(k))),
    [kinds, hiddenKinds],
  )
  const graph = useMemo(() => buildGraph(diff, cards, enabledKinds), [diff, cards, enabledKinds])
  const selectedNode = useMemo(
    () => graph.nodes.find((n) => n.path === selected),
    [graph, selected],
  )

  const c = diff?.counts
  const quiet =
    c != null &&
    c.addedEntities === 0 &&
    c.removedEntities === 0 &&
    c.addedRelations === 0 &&
    c.removedRelations === 0 &&
    c.changedRelations === 0

  const visiblePaths = new Set(visibleCards.map((c) => c.path))

  return (
    <div {...stylex.props(styles.root)}>
      <header {...stylex.props(styles.head)}>
        <h2 {...stylex.props(styles.title)}>Changes</h2>
        <DisplayBadge severity="low">live</DisplayBadge>
        {diff && (
          <code {...stylex.props(styles.pair)} title={`${diff.baseSnapshotId} → ${diff.headSnapshotId}`}>
            {diff.baseSnapshotId.slice(0, 8)}…{diff.headSnapshotId.slice(0, 8)}
          </code>
        )}
        <span {...stylex.props(styles.spacer)} />
        <div {...stylex.props(styles.search)}>
          <FormSearchField
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder="filter by name or path…"
            onClear={() => setText('')}
            aria-label="Filter changes"
          />
        </div>
      </header>

      {diff && (diff.truncated || base || head) && (
        <div {...stylex.props(styles.meta)}>
          {diff.truncated && <span>lists truncated server-side — export for the full set</span>}
          {(base || head) && <span>{diff.truncated ? ' · ' : ''}custom pair via ?base=/?head=</span>}
        </div>
      )}

      {busy && <FeedbackSkeleton height={380} />}
      {err && (
        <FeedbackEmptyState
          icon={<IconDiff size={20} />}
          title="no snapshot pair to diff"
          description={`/v1/diff/architecture — ${err}`}
        />
      )}
      {diff && quiet && (
        <FeedbackEmptyState
          icon={<IconDiff size={20} />}
          title="no structural change"
          description="the two snapshots are architecturally identical — nothing added, removed, or re-wired"
        />
      )}

      {diff && !quiet && (
        <>
          <div {...stylex.props(styles.story)}>
            {story.map((line, i) => (
              <div key={i} {...stylex.props(styles.storyLine)}>
                <span
                  {...stylex.props(
                    styles.storyGlyph,
                    line.tone === 'add' ? styles.cAdd : line.tone === 'del' ? styles.cDel : undefined,
                  )}
                >
                  {line.tone === 'add' ? '+' : line.tone === 'del' ? '−' : '•'}
                </span>
                <span>{line.text}</span>
              </div>
            ))}
          </div>

          <div {...stylex.props(styles.split)}>
            <div {...stylex.props(styles.graphPane)}>
              {kinds.length > 1 && (
                <div {...stylex.props(styles.kindBar)}>
                  <ActionToggleGroup
                    multiple
                    aria-label="Relation kinds drawn in the graph"
                    options={kinds.map(([k, n]) => ({ value: k, label: `${k} ${n}` }))}
                    value={kinds.map(([k]) => k).filter((k) => !hiddenKinds.has(k))}
                    onValueChange={(v) => {
                      const on = new Set(typeof v === 'string' ? [v] : v)
                      setHiddenKinds(new Set(kinds.map(([k]) => k).filter((k) => !on.has(k))))
                    }}
                  />
                </div>
              )}
              {graph.nodes.length > 0 ? (
                <ReactFlowProvider>
                  <DeltaCanvas
                    graph={graph}
                    scheme={scheme}
                    needle={needle}
                    visiblePaths={visiblePaths}
                    selected={selected}
                    onPick={setSelected}
                  />
                </ReactFlowProvider>
              ) : (
                <div {...stylex.props(styles.graphEmpty)}>
                  repository-level delta — no file nodes to draw
                </div>
              )}
              {graph.nodes.length > 0 && graph.edges.length === 0 && (
                <div {...stylex.props(styles.graphNote)}>
                  no cross-file rewiring — every change is internal to its file
                </div>
              )}
            </div>

            <div {...stylex.props(styles.rail)}>
              {selectedNode ? (
                <NodeDetail node={selectedNode} onClose={() => setSelected(undefined)} />
              ) : (
                <>
                  {visibleCards.length === 0 ? (
                    <div {...stylex.props(styles.railEmpty)}>
                      no file in this delta mentions “{text.trim()}”
                    </div>
                  ) : (
                    visibleCards.map((card) => (
                      <FileRow key={card.path} card={card} />
                    ))
                  )}
                  {diff.changedRelations.length > 0 && <ChangedRow rows={diff.changedRelations} />}
                </>
              )}
            </div>
          </div>

          <div {...stylex.props(styles.footnote)}>
            {diff.provenance} · byte-range duplicate edges collapse into one row · graph draws
            cross-file wiring — the canvas chips gate relation kinds (contains/changed_with
            start off); intra-file edges live inside each row
          </div>
        </>
      )}
    </div>
  )
}

// --- delta → file rows ---------------------------------------------------------

const NO_FILE = '(repository-level)'

interface FileDelta {
  path: string
  isNew: boolean
  isGone: boolean
  addedSymbols: DiffEntityRef[]
  removedSymbols: DiffEntityRef[]
  addedRels: DiffRelation[]
  removedRels: DiffRelation[]
}

const STRUCTURAL = new Set(['file', 'directory'])
const relKey = (r: { kind: string; source: DiffEntityRef; target: DiffEntityRef }) =>
  `${r.kind}|${r.source.qualifiedName ?? r.source.name}|${r.target.qualifiedName ?? r.target.name}`

/** Group the whole delta by file — the unit a person reviews in. `file`
 *  and `directory` entities mark the container itself as new/gone rather
 *  than listing as symbols. Relations attribute to their source file;
 *  byte-range duplicates of the same semantic edge collapse to one row. */
function buildCards(diff: ArchitectureDiff | null): FileDelta[] {
  if (!diff) return []
  const m = new Map<string, FileDelta>()
  const card = (p: string) => {
    let c = m.get(p)
    if (!c) {
      c = { path: p, isNew: false, isGone: false, addedSymbols: [], removedSymbols: [], addedRels: [], removedRels: [] }
      m.set(p, c)
    }
    return c
  }
  for (const e of diff.addedEntities) {
    const c = card(e.path ?? NO_FILE)
    if (STRUCTURAL.has(e.kind)) c.isNew = true
    else c.addedSymbols.push(e)
  }
  for (const e of diff.removedEntities) {
    const c = card(e.path ?? NO_FILE)
    if (STRUCTURAL.has(e.kind)) c.isGone = true
    else c.removedSymbols.push(e)
  }
  const seen = new Set<string>()
  for (const r of diff.addedRelations) {
    const k = `+${relKey(r)}`
    if (seen.has(k)) continue
    seen.add(k)
    card(r.source.path ?? r.target.path ?? NO_FILE).addedRels.push(r)
  }
  for (const r of diff.removedRelations) {
    const k = `-${relKey(r)}`
    if (seen.has(k)) continue
    seen.add(k)
    card(r.source.path ?? r.target.path ?? NO_FILE).removedRels.push(r)
  }
  const weight = (c: FileDelta) =>
    (c.isGone ? 1e6 : c.isNew ? 5e5 : 0) +
    c.addedSymbols.length +
    c.removedSymbols.length +
    c.addedRels.length +
    c.removedRels.length
  return [...m.values()].sort((a, b) => weight(b) - weight(a) || a.path.localeCompare(b.path))
}

function filterCard(c: FileDelta, needle: string): FileDelta | null {
  const hit = (e: DiffEntityRef) =>
    `${e.name} ${e.qualifiedName ?? ''} ${e.path ?? ''}`.toLowerCase().includes(needle)
  const rHit = (r: DiffRelation) =>
    hit(r.source) || hit(r.target) || r.kind.toLowerCase().includes(needle)
  const f: FileDelta = {
    ...c,
    addedSymbols: c.addedSymbols.filter(hit),
    removedSymbols: c.removedSymbols.filter(hit),
    addedRels: c.addedRels.filter(rHit),
    removedRels: c.removedRels.filter(rHit),
  }
  const empty =
    !f.addedSymbols.length && !f.removedSymbols.length && !f.addedRels.length && !f.removedRels.length
  if (empty && !c.path.toLowerCase().includes(needle)) return null
  return f
}

// --- generated story -------------------------------------------------------------

const baseName = (p: string) => p.split('/').pop() ?? p

function kindCounts<T extends { kind: string }>(items: T[]): [string, number][] {
  const m = new Map<string, number>()
  for (const i of items) m.set(i.kind, (m.get(i.kind) ?? 0) + 1)
  return [...m.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
}

const REL_VERB: Record<string, string> = {
  calls: 'calls',
  references: 'references',
  contains: 'contains',
  imports: 'imports',
  extends: 'extends',
  implements: 'implements',
}
const relVerb = (k: string) => REL_VERB[k] ?? 'relates to'

function plural(kind: string, n: number) {
  return n === 1 ? kind : kind.endsWith('s') ? kind : `${kind}s`
}

/** Two-to-five plain sentences that carry the headline before any row
 *  is opened — new files first, deletions, then dropped wiring. */
function buildStory(cards: FileDelta[]): { text: string; tone?: 'add' | 'del' }[] {
  const lines: { text: string; tone?: 'add' | 'del' }[] = []
  const newFiles = cards.filter((c) => c.isNew)
  const gone = cards.filter((c) => c.isGone)
  const modified = cards.filter((c) => !c.isNew && !c.isGone)

  if (newFiles.length === 1) {
    const f = newFiles[0]!
    const relBits = kindCounts(f.addedRels)
      .map(([k, n]) => `${n} ${k}`)
      .join(' · ')
    lines.push({
      text: `${baseName(f.path)} is a new file — ${f.addedSymbols.length} symbols${f.addedRels.length ? `, ${f.addedRels.length} edges wired${relBits ? ` (${relBits})` : ''}` : ''}`,
      tone: 'add',
    })
  } else if (newFiles.length > 1) {
    lines.push({
      text: `${newFiles.length} new files — ${newFiles
        .map((f) => baseName(f.path))
        .slice(0, 4)
        .join(', ')}${newFiles.length > 4 ? '…' : ''}`,
      tone: 'add',
    })
  }
  for (const f of gone.slice(0, 3)) {
    lines.push({ text: `${baseName(f.path)} was deleted — ${f.removedSymbols.length} symbols gone`, tone: 'del' })
  }
  for (const f of modified) {
    const bits: string[] = []
    if (f.removedSymbols.length) {
      const ks = kindCounts(f.removedSymbols)
        .map(([k, n]) => `${n} ${plural(k, n)}`)
        .join(' · ')
      bits.push(`${f.removedSymbols.length} symbols removed (${ks})`)
    }
    const top = f.removedRels[0]
    if (f.removedRels.length && top) {
      const kcs = kindCounts(f.removedRels)
      const what =
        kcs.length === 1
          ? `${f.removedRels.length} ${kcs[0]![0]}`
          : `${f.removedRels.length} edges (${kcs.map(([k, n]) => `${n} ${k}`).join(' · ')})`
      bits.push(
        `${what} dropped — e.g. ${top.source.name} no longer ${relVerb(top.kind)} ${top.target.name}`,
      )
    }
    if (bits.length) lines.push({ text: `${baseName(f.path)} — ${bits.join(' ; ')}`, tone: 'del' })
    if (lines.length >= 4) break
  }
  if (lines.length < 2) {
    for (const f of modified) {
      if (f.addedSymbols.length || f.addedRels.length) {
        const bits: string[] = []
        if (f.addedSymbols.length) bits.push(`${f.addedSymbols.length} new symbols`)
        if (f.addedRels.length) bits.push(`${f.addedRels.length} new edges`)
        lines.push({ text: `${baseName(f.path)} gained ${bits.join(' and ')}`, tone: 'add' })
      }
      if (lines.length >= 3) break
    }
  }
  if (!lines.length) {
    lines.push({ text: `structural delta across ${cards.length} files — see the list below` })
  }
  return lines.slice(0, 5)
}

// --- delta graph ------------------------------------------------------------------

type NodeStatus = 'new' | 'deleted' | 'modified' | 'context'

interface GraphModel {
  nodes: GraphNode[]
  edges: GraphEdge[]
}

interface GraphNode {
  path: string
  label?: string
  status: NodeStatus
  card?: FileDelta
  inbound: number
  outbound: number
  /** Cross-file relations touching this file — powers the detail pane's
   *  inbound/outbound groups (a file's card only carries source-side
   *  attribution, so "who wired into me" needs the full touch set). */
  touching: { r: DiffRelation; added: boolean }[]
}

interface GraphEdge {
  src: string
  tgt: string
  added: [string, number][]
  removed: [string, number][]
}

/** Kinds that are bookkeeping rather than wiring — `contains` re-draws
 *  the file tree and `changed_with` draws git co-change cliques; both
 *  bury the structural delta under hundreds of edges. They start
 *  toggled off in the canvas but stay enumerated in the rail rows. */
const NOISE_KINDS: ReadonlySet<string> = new Set(['contains', 'changed_with'])

/** Cross-file relation counts per kind — powers the canvas kind
 *  filter. Byte-range duplicates collapse like `buildCards` does. */
function graphKindCounts(diff: ArchitectureDiff | null): [string, number][] {
  if (!diff) return []
  const m = new Map<string, number>()
  const tally = (rows: DiffRelation[], sign: string) => {
    const seen = new Set<string>()
    for (const r of rows) {
      const s = r.source.path
      const t = r.target.path
      if (!s || !t || s === t) continue
      const key = `${sign}${relKey(r)}`
      if (seen.has(key)) continue
      seen.add(key)
      m.set(r.kind, (m.get(r.kind) ?? 0) + 1)
    }
  }
  tally(diff.addedRelations, '+')
  tally(diff.removedRelations, '-')
  return [...m.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
}

/** File-level delta graph: a node per changed file plus grey context
 *  nodes for files the change reached into but did not alter. Edges are
 *  cross-file relation deltas aggregated by (source file, target file);
 *  intra-file wiring stays inside the file rows. `enabled` gates which
 *  relation kinds draw — hidden kinds still list in the rail. */
function buildGraph(
  diff: ArchitectureDiff | null,
  cards: FileDelta[],
  enabled: ReadonlySet<string>,
): GraphModel {
  if (!diff) return { nodes: [], edges: [] }
  const byPath = new Map(cards.map((c) => [c.path, c]))
  const nodes = new Map<string, GraphNode>()
  const node = (p: string): GraphNode => {
    let n = nodes.get(p)
    if (!n) {
      const card = byPath.get(p)
      n = {
        path: p,
        status: card ? (card.isGone ? 'deleted' : card.isNew ? 'new' : 'modified') : 'context',
        card,
        inbound: 0,
        outbound: 0,
        touching: [],
      }
      nodes.set(p, n)
    }
    return n
  }
  const edgeMap = new Map<string, GraphEdge>()
  const edge = (src: string, tgt: string): GraphEdge => {
    const k = `${src}|${tgt}`
    let e = edgeMap.get(k)
    if (!e) {
      e = { src, tgt, added: [], removed: [] }
      edgeMap.set(k, e)
    }
    return e
  }
  const seen = new Set<string>()
  const bump = (list: [string, number][], kind: string) => {
    const e = list.find(([k]) => k === kind)
    if (e) e[1]++
    else list.push([kind, 1])
  }
  for (const r of diff.addedRelations) {
    if (!enabled.has(r.kind)) continue
    const k = `+${relKey(r)}`
    if (seen.has(k)) continue
    seen.add(k)
    const s = r.source.path
    const t = r.target.path
    if (!s || !t || s === t) continue
    bump(edge(s, t).added, r.kind)
    node(s).outbound++
    node(s).touching.push({ r, added: true })
    node(t).inbound++
    node(t).touching.push({ r, added: true })
  }
  for (const r of diff.removedRelations) {
    if (!enabled.has(r.kind)) continue
    const k = `-${relKey(r)}`
    if (seen.has(k)) continue
    seen.add(k)
    const s = r.source.path
    const t = r.target.path
    if (!s || !t || s === t) continue
    bump(edge(s, t).removed, r.kind)
    node(s).outbound++
    node(s).touching.push({ r, added: false })
    node(t).inbound++
    node(t).touching.push({ r, added: false })
  }

  // Context-node cap: a new file can legitimately reach dozens of
  // untouched files — the shape is "fans out widely", not 25 legible
  // nodes. Keep the busiest few; collapse the rest into one "+N more"
  // node that still carries their aggregate edges.
  const CTX_CAP = 4
  const contexts = [...nodes.values()]
    .filter((n) => n.status === 'context')
    .sort((a, b) => b.inbound + b.outbound - (a.inbound + a.outbound))
  if (contexts.length > CTX_CAP) {
    const rest = contexts.slice(CTX_CAP)
    const REST = '…rest'
    const restNode: GraphNode = {
      path: REST,
      label: `+${rest.length} more files`,
      status: 'context',
      inbound: rest.reduce((a, n) => a + n.inbound, 0),
      outbound: rest.reduce((a, n) => a + n.outbound, 0),
      touching: [],
    }
    const restPaths = new Set(rest.map((n) => n.path))
    const merged = new Map<string, GraphEdge>()
    for (const e of edgeMap.values()) {
      const src = restPaths.has(e.src) ? REST : e.src
      const tgt = restPaths.has(e.tgt) ? REST : e.tgt
      const k = `${src}|${tgt}`
      const m = merged.get(k) ?? { src, tgt, added: [], removed: [] }
      for (const [kind, n] of e.added) bump(m.added, kind)
      for (const [kind, n] of e.removed) bump(m.removed, kind)
      merged.set(k, m)
    }
    for (const p of restPaths) nodes.delete(p)
    nodes.set(REST, restNode)
    return { nodes: [...nodes.values()], edges: [...merged.values()] }
  }
  return { nodes: [...nodes.values()], edges: [...edgeMap.values()] }
}

// --- React Flow canvas --------------------------------------------------------------

type ChangeData = { node: GraphNode; selected: boolean; dim: boolean } & Record<string, unknown>

const nodeTypes = { change: ChangeNode }
const NODE_W = 208
const NODE_H = 56

function DeltaCanvas({
  graph,
  scheme,
  needle,
  visiblePaths,
  selected,
  onPick,
}: {
  graph: GraphModel
  scheme: 'light' | 'dark'
  needle: string
  visiblePaths: Set<string>
  selected?: string
  onPick: (path?: string) => void
}) {
  const laid = useMemo(() => {
    const g = new dagre.graphlib.Graph()
    g.setGraph({ rankdir: 'LR', nodesep: 36, ranksep: 110, marginx: 20, marginy: 20 })
    g.setDefaultEdgeLabel(() => ({}))
    for (const n of graph.nodes) g.setNode(n.path, { width: NODE_W, height: NODE_H })
    for (const e of graph.edges) g.setEdge(e.src, e.tgt)
    dagre.layout(g)
    return g
  }, [graph])

  // Neighborhood focus: the hovered (else selected) node keeps its
  // incident edges and neighbors lit while the rest of the graph
  // recedes — the only way a dense delta stays readable.
  const [hover, setHover] = useState<string>()
  const focus = hover ?? selected
  const incident = useMemo(() => {
    if (!focus) return null
    const s = new Set<string>([focus])
    for (const e of graph.edges) {
      if (e.src === focus) s.add(e.tgt)
      if (e.tgt === focus) s.add(e.src)
    }
    return s
  }, [focus, graph])

  const rfNodes: Node<ChangeData>[] = useMemo(
    () =>
      graph.nodes.map((n) => {
        const pos = laid.node(n.path)
        const dim =
          (Boolean(needle) && !visiblePaths.has(n.path) && n.status !== 'context') ||
          (incident != null && !incident.has(n.path))
        return {
          id: n.path,
          type: 'change',
          position: { x: pos.x - NODE_W / 2, y: pos.y - NODE_H / 2 },
          data: { node: n, selected: n.path === selected, dim },
        }
      }),
    [graph, laid, needle, visiblePaths, selected, incident],
  )

  const rfEdges: Edge[] = useMemo(
    () => {
      // Labels only when the graph is sparse — on a fan-out they stack
      // into noise; width + color carry the information instead.
      const showLabels = graph.edges.length <= 8
      return graph.edges.map((e) => {
        const addN = e.added.reduce((a, [, n]) => a + n, 0)
        const delN = e.removed.reduce((a, [, n]) => a + n, 0)
        const mixed = addN > 0 && delN > 0
        const color = mixed ? vars.scaleMedium : delN > 0 ? vars.scaleHigh : vars.scaleLow
        const bits: string[] = []
        if (addN) bits.push(`+${addN} ${e.added.length === 1 ? e.added[0]![0] : 'edges'}`)
        if (delN) bits.push(`−${delN} ${e.removed.length === 1 ? e.removed[0]![0] : 'edges'}`)
        const offFocus = incident != null && e.src !== focus && e.tgt !== focus
        return {
          id: `${e.src}|${e.tgt}`,
          source: e.src,
          target: e.tgt,
          sourceHandle: 's',
          targetHandle: 't',
          label: showLabels && !offFocus ? bits.join('  ') : undefined,
          labelStyle: { fontSize: 10, fontFamily: font.mono, fill: color },
          labelBgStyle: { fill: vars.bgBase, fillOpacity: 0.85 },
          style: {
            stroke: color,
            strokeWidth: Math.min(3, 1 + Math.max(addN, delN) / 4),
            strokeDasharray: delN > 0 && addN === 0 ? '5 4' : undefined,
            opacity: offFocus
              ? 0.07
              : needle && !visiblePaths.has(e.src) && !visiblePaths.has(e.tgt)
                ? 0.25
                : 1,
          },
          // Edge style opacity doesn't reach marker defs — drop the
          // arrowhead on faded edges instead of leaving it floating.
          markerEnd: offFocus
            ? undefined
            : { type: MarkerType.ArrowClosed, width: 14, height: 14, color },
        }
      })
    },
    [graph, needle, visiblePaths, incident, focus],
  )

  const [flowNodes, setFlowNodes, onNodesChange] = useNodesState<Node<ChangeData>>(rfNodes)
  // Edges seed empty and populate via effect — feeding them at mount races
  // handle registration and spams RF error #008 in dev.
  const [flowEdges, setFlowEdges, onEdgesChange] = useEdgesState<Edge>([])
  const laidRef = useRef(laid)
  useEffect(() => {
    // Data-only updates (search/filter/focus) must not clobber positions
    // the user dragged — but a fresh dagre layout (new delta pair, kind
    // toggles) has to win, otherwise nodes keep stale coordinates.
    const relayout = laidRef.current !== laid
    laidRef.current = laid
    setFlowNodes((ns) => {
      const byId = new Map(ns.map((n) => [n.id, n]))
      return rfNodes.map((d) => {
        const live = byId.get(d.id)
        return live && !relayout ? { ...d, position: live.position, measured: live.measured } : d
      })
    })
  }, [rfNodes, laid, setFlowNodes])
  useEffect(() => setFlowEdges(rfEdges), [rfEdges, setFlowEdges])

  const sig = `${graph.nodes.length}:${graph.edges.length}`
  const rf = useReactFlow()

  // Bounds from the dagre layout itself — deterministic and independent
  // of node measurement, so framing never races position updates.
  const bounds = useMemo(() => {
    let minX = Infinity
    let minY = Infinity
    let maxX = -Infinity
    let maxY = -Infinity
    for (const n of graph.nodes) {
      const p = laid.node(n.path)
      minX = Math.min(minX, p.x - NODE_W / 2)
      minY = Math.min(minY, p.y - NODE_H / 2)
      maxX = Math.max(maxX, p.x + NODE_W / 2)
      maxY = Math.max(maxY, p.y + NODE_H / 2)
    }
    return { x: minX, y: minY, width: Math.max(1, maxX - minX), height: Math.max(1, maxY - minY) }
  }, [graph, laid])

  return (
    <ReactFlow
      nodes={flowNodes}
      edges={flowEdges}
      nodeTypes={nodeTypes}
      onNodesChange={onNodesChange}
      onEdgesChange={onEdgesChange}
      onNodeClick={(_, n) => {
        if (n.id === '…rest') return
        if (n.id !== selected) {
          // Big deltas fit at far-out zooms — bring the picked node to a
          // readable zoom instead of leaving it a speck in the hairball.
          void rf.setCenter(n.position.x + NODE_W / 2, n.position.y + NODE_H / 2, {
            zoom: Math.max(rf.getZoom(), 1),
            duration: 200,
          })
        }
        onPick(n.id === selected ? undefined : n.id)
      }}
      onNodeMouseEnter={(_, n) => setHover(n.id)}
      onNodeMouseLeave={() => setHover(undefined)}
      onPaneClick={() => onPick(undefined)}
      nodesConnectable={false}
      deleteKeyCode={null}
      minZoom={0.05}
      maxZoom={2}
      colorMode={scheme}
      aria-label="Change delta graph"
      proOptions={{ hideAttribution: true }}
    >
      <Background variant={BackgroundVariant.Dots} gap={24} size={1} />
      <Controls position="bottom-right" showInteractive={false} />
      <MiniMap
        position="bottom-left"
        pannable
        zoomable
        nodeColor={(n: Node<ChangeData>) =>
          n.data.node.status === 'new'
            ? vars.scaleLow
            : n.data.node.status === 'deleted'
              ? vars.scaleHigh
              : n.data.node.status === 'context'
                ? vars.colorFaint
                : vars.scaleMedium
        }
        maskColor={vars.bgSunken}
        bgColor={vars.bgRaised}
      />
      <Refit sig={sig} bounds={bounds} />
    </ReactFlow>
  )
}

/** Re-frames the dagre bounds whenever the node/edge set changes (initial
 *  mount counts). `fitView` reads measured node positions — which lag a
 *  commit behind a re-layout — so this computes the viewport transform
 *  straight from the layout bounds instead. */
function Refit({
  sig,
  bounds,
}: {
  sig: string
  bounds: { x: number; y: number; width: number; height: number }
}) {
  const rf = useReactFlow()
  const inited = useNodesInitialized()
  const paneW = useStore((s) => s.width)
  const paneH = useStore((s) => s.height)
  const last = useRef('')
  useEffect(() => {
    if (!inited || last.current === sig || paneW === 0) return
    last.current = sig
    const pad = 0.08
    const zoom = Math.min(
      1.15,
      Math.min((paneW * (1 - 2 * pad)) / bounds.width, (paneH * (1 - 2 * pad)) / bounds.height),
    )
    void rf.setViewport(
      {
        x: paneW / 2 - (bounds.x + bounds.width / 2) * zoom,
        y: paneH / 2 - (bounds.y + bounds.height / 2) * zoom,
        zoom,
      },
      { duration: 220 },
    )
  }, [inited, sig, bounds, paneW, paneH, rf])
  return null
}

/** File node — status dot, basename, one-line delta summary. Context
 *  nodes render hollow: the change reached them but did not alter them. */
function ChangeNode({ data }: { data: ChangeData }) {
  const { node, dim, selected } = data
  const c = node.card
  const bits: string[] = []
  if (node.status === 'context') {
    if (node.inbound) bits.push(`${node.inbound} edge${node.inbound === 1 ? '' : 's'} now reach in`)
    if (!bits.length) bits.push('unchanged')
  } else {
    if (c?.addedSymbols.length) bits.push(`+${c.addedSymbols.length} symbols`)
    if (c?.removedSymbols.length) bits.push(`−${c.removedSymbols.length} symbols`)
    if (c?.addedRels.length) bits.push(`+${c.addedRels.length} edges`)
    if (c?.removedRels.length) bits.push(`−${c.removedRels.length} edges`)
    if (!bits.length) bits.push(node.status === 'new' ? 'appeared' : 'touched')
  }
  return (
    <div
      {...stylex.props(
        styles.gNode,
        node.status === 'new' && styles.gNodeNew,
        node.status === 'deleted' && styles.gNodeDel,
        node.status === 'context' && styles.gNodeCtx,
        selected && styles.gNodeSel,
        dim && styles.gNodeDim,
      )}
    >
      <Handle id="t" type="target" position={Position.Left} {...stylex.props(styles.handle)} />
      <Handle id="s" type="source" position={Position.Right} {...stylex.props(styles.handle)} />
      <div {...stylex.props(styles.gNodeTop)}>
        <span
          {...stylex.props(
            styles.dot,
            node.status === 'new'
              ? styles.dotAdd
              : node.status === 'deleted'
                ? styles.dotDel
                : node.status === 'context'
                  ? styles.dotCtx
                  : styles.dotMod,
          )}
        />
        <span {...stylex.props(styles.gNodeName)}>{node.label ?? baseName(node.path)}</span>
      </div>
      <div {...stylex.props(styles.gNodeSub)}>{bits.join(' · ')}</div>
    </div>
  )
}

// --- file row ------------------------------------------------------------------------

const SYM_CAP = 12
const REL_CAP = 8

function FileRow({ card }: { card: FileDelta }) {
  const navigate = useNavigate()
  const total =
    card.addedSymbols.length + card.removedSymbols.length + card.addedRels.length + card.removedRels.length
  const [open, setOpen] = useState(total > 0 && total <= 10)
  const [hov, setHov] = useState(false)
  const hasFile = card.path !== NO_FILE

  const nums: [string, 'add' | 'del'][] = [
    card.addedSymbols.length > 0 && [`+${card.addedSymbols.length} symbols`, 'add'],
    card.removedSymbols.length > 0 && [`−${card.removedSymbols.length} symbols`, 'del'],
    card.addedRels.length > 0 && [`+${card.addedRels.length} edges`, 'add'],
    card.removedRels.length > 0 && [`−${card.removedRels.length} edges`, 'del'],
  ].filter((x): x is [string, 'add' | 'del'] => Boolean(x))

  return (
    <div>
      <div
        role="button"
        tabIndex={0}
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault()
            setOpen((o) => !o)
          }
        }}
        onMouseEnter={() => setHov(true)}
        onMouseLeave={() => setHov(false)}
        {...stylex.props(styles.fileRow)}
      >
        <span {...stylex.props(styles.caret, open && styles.caretOpen)}>
          <IconCaretDown size={9} />
        </span>
        <span
          {...stylex.props(
            styles.dot,
            card.isGone ? styles.dotDel : card.isNew ? styles.dotAdd : styles.dotMod,
          )}
          title={card.isNew ? 'new file' : card.isGone ? 'deleted' : 'modified'}
        />
        <span {...stylex.props(styles.fileName)}>
          {hasFile ? <DisplayFilePath path={card.path} /> : 'repository-level entities'}
        </span>
        {(card.isNew || card.isGone) && (
          <span {...stylex.props(styles.fileTag)}>{card.isNew ? 'new' : 'deleted'}</span>
        )}
        <span {...stylex.props(styles.fileNums)}>
          {nums.map(([label, tone], i) => (
            <span key={label}>
              {i > 0 && ' · '}
              <span {...stylex.props(tone === 'add' ? styles.cAdd : styles.cDel)}>{label}</span>
            </span>
          ))}
        </span>
        <span {...stylex.props(styles.rowAction)} onClick={(e) => e.stopPropagation()}>
          {hasFile && hov && (
            <ActionIconButton
              compact
              icon={<IconArrowUpRight size={10} />}
              tooltip={`open ${card.path}`}
              label={`Open ${card.path}`}
              onClick={() => navigate(fileUrl(card.path))}
            />
          )}
        </span>
      </div>

      {open && (
        <div {...stylex.props(styles.detail)}>
          {total === 0 && (
            <div {...stylex.props(styles.groupEmpty)}>
              {card.isNew ? 'the file itself is new — no symbols recorded yet' : 'nothing itemized'}
            </div>
          )}
          <SymbolGroup tone="del" items={card.removedSymbols} />
          <SymbolGroup tone="add" items={card.addedSymbols} showImpact />
          <RelGroup tone="del" items={card.removedRels} card={card} />
          <RelGroup tone="add" items={card.addedRels} card={card} />
        </div>
      )}
    </div>
  )
}

// --- node detail pane --------------------------------------------------------------

/** The rail's detail mode — what a graph click opens. A changed file
 *  gets its symbol and outbound edge groups plus inbound edges the card
 *  misses (cards attribute relations to their source file only, so
 *  "who wired into me" comes from the node's touch set). A context file
 *  shows only the edges that reached it. `…rest` is never selectable. */
function NodeDetail({ node, onClose }: { node: GraphNode; onClose: () => void }) {
  const navigate = useNavigate()
  const c = node.card
  const isCtx = node.status === 'context'

  const listed = new Set([...(c?.addedRels ?? []), ...(c?.removedRels ?? [])].map(relKey))
  const inbound = node.touching.filter(
    (t) => t.r.target.path === node.path && t.r.source.path !== node.path && !listed.has(relKey(t.r)),
  )
  const inboundAdd = inbound.filter((t) => t.added).map((t) => t.r)
  const inboundDel = inbound.filter((t) => !t.added).map((t) => t.r)
  const outboundAdd = isCtx
    ? node.touching.filter((t) => t.added && t.r.source.path === node.path).map((t) => t.r)
    : (c?.addedRels ?? [])
  const outboundDel = isCtx
    ? node.touching.filter((t) => !t.added && t.r.source.path === node.path).map((t) => t.r)
    : (c?.removedRels ?? [])
  const pseudo: FileDelta = c ?? {
    path: node.path,
    isNew: false,
    isGone: false,
    addedSymbols: [],
    removedSymbols: [],
    addedRels: [],
    removedRels: [],
  }
  const addE = outboundAdd.length + inboundAdd.length
  const delE = outboundDel.length + inboundDel.length
  const nums: { label: string; tone: 'add' | 'del' }[] = []
  if (c && c.addedSymbols.length > 0) nums.push({ label: `+${c.addedSymbols.length} symbols`, tone: 'add' })
  if (c && c.removedSymbols.length > 0) nums.push({ label: `−${c.removedSymbols.length} symbols`, tone: 'del' })
  if (addE > 0) nums.push({ label: `+${addE} edges`, tone: 'add' })
  if (delE > 0) nums.push({ label: `−${delE} edges`, tone: 'del' })

  return (
    <div>
      <div {...stylex.props(styles.detailHead)}>
        <span
          {...stylex.props(
            styles.dot,
            node.status === 'new'
              ? styles.dotAdd
              : node.status === 'deleted'
                ? styles.dotDel
                : node.status === 'context'
                  ? styles.dotCtx
                  : styles.dotMod,
          )}
        />
        <span {...stylex.props(styles.fileName)}>
          <DisplayFilePath path={node.path} />
        </span>
        <span {...stylex.props(styles.fileTag)}>
          {node.status === 'new'
            ? 'new file'
            : node.status === 'deleted'
              ? 'deleted'
              : node.status === 'context'
                ? 'unchanged'
                : 'modified'}
        </span>
        <span {...stylex.props(styles.fileNums)}>
          {nums.map(({ label, tone }, i) => (
            <span key={label}>
              {i > 0 && ' · '}
              <span {...stylex.props(tone === 'add' ? styles.cAdd : styles.cDel)}>{label}</span>
            </span>
          ))}
        </span>
        <ActionIconButton
          compact
          icon={<IconArrowUpRight size={10} />}
          tooltip={`open ${node.path}`}
          label={`Open ${node.path}`}
          onClick={() => navigate(fileUrl(node.path))}
        />
        <ActionIconButton
          compact
          icon={<IconX size={10} />}
          tooltip="back to all files"
          label="Close detail"
          onClick={onClose}
        />
      </div>
      <div {...stylex.props(styles.detailBody)}>
        {isCtx && (
          <div {...stylex.props(styles.groupNote)}>
            unchanged itself — the delta reached it through these edges
          </div>
        )}
        <SymbolGroup tone="del" items={c?.removedSymbols ?? []} />
        <SymbolGroup tone="add" items={c?.addedSymbols ?? []} showImpact />
        <RelGroup tone="del" items={outboundDel} card={pseudo} />
        <RelGroup tone="add" items={outboundAdd} card={pseudo} />
        <RelGroup tone="del" items={inboundDel} card={pseudo} suffix="no longer reach in" />
        <RelGroup tone="add" items={inboundAdd} card={pseudo} suffix="now reach in" />
        {nums.length === 0 && <div {...stylex.props(styles.groupNote)}>nothing itemized</div>}
      </div>
    </div>
  )
}

// --- groups inside a row ----------------------------------------------------------

function SymbolGroup({
  tone,
  items,
  showImpact,
}: {
  tone: 'add' | 'del'
  items: DiffEntityRef[]
  showImpact?: boolean
}) {
  if (!items.length) return null
  return (
    <>
      {kindCounts(items).map(([kind]) => {
        const group = items.filter((i) => i.kind === kind)
        return (
          <div key={kind} {...stylex.props(styles.group)}>
            <div {...stylex.props(styles.groupLabel)}>
              <span {...stylex.props(tone === 'add' ? styles.cAdd : styles.cDel)}>
                {tone === 'add' ? '+' : '−'}
              </span>{' '}
              {group.length} {plural(kind, group.length)} {tone === 'add' ? 'landed' : 'gone'}
            </div>
            {group.slice(0, SYM_CAP).map((e) => (
              <SymRow key={e.entityId} e={e} showImpact={showImpact} />
            ))}
            {group.length > SYM_CAP && (
              <div {...stylex.props(styles.groupEmpty)}>+{group.length - SYM_CAP} more</div>
            )}
          </div>
        )
      })}
    </>
  )
}

function SymRow({ e, showImpact }: { e: DiffEntityRef; showImpact?: boolean }) {
  const [impactOpen, setImpactOpen] = useState(false)
  const [hov, setHov] = useState(false)
  return (
    <div
      onMouseEnter={() => setHov(true)}
      onMouseLeave={() => setHov(false)}
      {...stylex.props(styles.symRow)}
    >
      <span {...stylex.props(styles.symKind)}>{e.kind}</span>
      <span {...stylex.props(styles.symName)} title={e.qualifiedName ?? e.name}>
        {e.name}
      </span>
      <span {...stylex.props(styles.rowAction)}>
        {showImpact && (hov || impactOpen) && (
          <ActionIconButton
            compact
            icon={<IconGraph size={11} />}
            tooltip="blast radius — what this touches within 2 hops"
            label={`Impact of ${e.name}`}
            onClick={() => setImpactOpen((o) => !o)}
          />
        )}
      </span>
      {impactOpen && <ImpactPeek name={e.qualifiedName ?? e.name} />}
    </div>
  )
}

function RelGroup({
  tone,
  items,
  card,
  suffix,
}: {
  tone: 'add' | 'del'
  items: DiffRelation[]
  card: FileDelta
  suffix?: string
}) {
  if (!items.length) return null
  return (
    <>
      {kindCounts(items).map(([kind]) => {
        const group = items.filter((i) => i.kind === kind)
        return (
          <div key={kind} {...stylex.props(styles.group)}>
            <div {...stylex.props(styles.groupLabel)}>
              <span {...stylex.props(tone === 'add' ? styles.cAdd : styles.cDel)}>
                {tone === 'add' ? '+' : '−'}
              </span>{' '}
              {group.length} {kind} {suffix ?? (tone === 'add' ? 'wired in' : 'dropped')}
            </div>
            {group.slice(0, REL_CAP).map((r) => (
              <RelRow key={r.relationId} r={r} card={card} dropped={tone === 'del'} />
            ))}
            {group.length > REL_CAP && (
              <div {...stylex.props(styles.groupEmpty)}>+{group.length - REL_CAP} more</div>
            )}
          </div>
        )
      })}
    </>
  )
}

const shortExtractor = (x: string) => x.replace(/^scip:/, '').split(/[\s(]/)[0]

/** `a → b` with file hints only where an endpoint lives outside this file. */
function RelRow({ r, card, dropped }: { r: DiffRelation; card: FileDelta; dropped?: boolean }) {
  const crossSrc = r.source.path && r.source.path !== card.path
  const crossTgt = r.target.path && r.target.path !== card.path
  return (
    <div {...stylex.props(styles.relRow)}>
      <span {...stylex.props(styles.relKind)}>{r.kind}</span>
      <span {...stylex.props(styles.relEnds)}>
        {crossSrc && <span {...stylex.props(styles.relFile)}>{baseName(r.source.path!)}:</span>}
        {r.source.name}
        <span {...stylex.props(styles.relArrow)}> {dropped ? '⇢' : '→'} </span>
        {r.target.name}
        {crossTgt && <span {...stylex.props(styles.relFile)}> @{baseName(r.target.path!)}</span>}
      </span>
      <span {...stylex.props(styles.relMeta)}>
        {dropped ? 'was ' : ''}via {shortExtractor(r.extractor)}
        {r.confidence < 1 && ` · ${(r.confidence * 100).toFixed(0)}%`}
      </span>
    </div>
  )
}

// --- re-wired (changed) relations -----------------------------------------------------

function ChangedRow({ rows }: { rows: ChangedRelation[] }) {
  const [open, setOpen] = useState(false)
  return (
    <div>
      <div
        role="button"
        tabIndex={0}
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault()
            setOpen((o) => !o)
          }
        }}
        {...stylex.props(styles.fileRow)}
      >
        <span {...stylex.props(styles.caret, open && styles.caretOpen)}>
          <IconCaretDown size={9} />
        </span>
        <span {...stylex.props(styles.changedLabel)}>re-wired edges</span>
        <span {...stylex.props(styles.changedNote)}>
          {rows.length} — confidence or extractor moved; usually re-index noise
        </span>
      </div>
      {open && (
        <div {...stylex.props(styles.detail)}>
          {rows.slice(0, 25).map((r) => {
            const confDelta = r.headConfidence - r.baseConfidence
            return (
              <div key={`${r.baseRelationId}->${r.headRelationId}`} {...stylex.props(styles.relRow)}>
                <span {...stylex.props(styles.relKind)}>{r.kind}</span>
                <span {...stylex.props(styles.relEnds)}>
                  {r.source.name}
                  <span {...stylex.props(styles.relArrow)}> → </span>
                  {r.target.name}
                </span>
                <span {...stylex.props(styles.relMeta)}>
                  conf {(r.baseConfidence * 100).toFixed(0)}→{(r.headConfidence * 100).toFixed(0)}%
                  <span {...stylex.props(confDelta >= 0 ? styles.cAdd : styles.cDel)}>
                    {' '}
                    ({confDelta >= 0 ? '+' : ''}
                    {(confDelta * 100).toFixed(0)})
                  </span>
                  {r.baseOrigin !== r.headOrigin && ` · ${r.baseOrigin}→${r.headOrigin}`}
                </span>
              </div>
            )
          })}
          {rows.length > 25 && (
            <div {...stylex.props(styles.groupEmpty)}>+{rows.length - 25} more — export for the full set</div>
          )}
        </div>
      )}
    </div>
  )
}

// --- impact peek ----------------------------------------------------------------------

const PEEK_CAP = 8

/** Inline two-hop blast radius for a new entity — lazy `/v1/impact`. */
function ImpactPeek({ name }: { name: string }) {
  const [report, setReport] = useState<ImpactReport | null>(null)
  const [err, setErr] = useState<string>()

  useEffect(() => {
    let dead = false
    const timeout = new Promise<never>((_, rej) =>
      setTimeout(() => rej(new Error('timed out — daemon busy')), 20_000),
    )
    Promise.race([api.impact(name), timeout])
      .then((r) => !dead && setReport(r))
      .catch((e: unknown) => !dead && setErr(e instanceof Error ? e.message : 'request failed'))
    return () => {
      dead = true
    }
  }, [name])

  return (
    <div {...stylex.props(styles.peek)}>
      {!report && !err && <span {...stylex.props(styles.peekNote)}>computing blast radius…</span>}
      {err && <span {...stylex.props(styles.peekNote)}>impact unavailable — {err}</span>}
      {report && (
        <>
          <span {...stylex.props(styles.peekCount)}>
            {report.impacted.length === 0
              ? 'touches nothing within 2 hops'
              : `${report.impacted.length} entit${report.impacted.length === 1 ? 'y' : 'ies'} within 2 hops`}
          </span>
          {report.impacted.slice(0, PEEK_CAP).map((e) => (
            <span
              key={e.name}
              {...stylex.props(styles.peekHit)}
              title={`${e.kind} · via ${e.via} · ${(e.confidence * 100).toFixed(0)}%`}
            >
              {e.name}
              <span {...stylex.props(styles.peekHops)}>{e.hops}h</span>
            </span>
          ))}
          {report.impacted.length > PEEK_CAP && (
            <span {...stylex.props(styles.peekNote)}>+{report.impacted.length - PEEK_CAP} more</span>
          )}
          {report.caveats.slice(0, 2).map((c) => (
            <span key={c} {...stylex.props(styles.peekNote)}>
              {c}
            </span>
          ))}
        </>
      )}
    </div>
  )
}

// --- styles ----------------------------------------------------------------------------

const styles = stylex.create({
  root: {
    display: 'flex',
    flexDirection: 'column',
    gap: 10,
    // Must be height, not minHeight: under an indefinite container the
    // flex column skips free-space distribution and the split below
    // inflates to the rail's full content height instead of filling the
    // viewport. A definite 100% lets flex:1 work; short screens overflow
    // and scroll via the shell.
    height: '100%',
  },
  head: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    flexWrap: 'wrap',
  },
  title: {
    margin: 0,
    fontSize: 15,
    fontWeight: 600,
  },
  pair: {
    fontFamily: font.mono,
    fontSize: 10.5,
    color: vars.colorMuted,
  },
  spacer: {
    flex: 1,
  },
  search: {
    width: 220,
  },
  meta: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
  },
  story: {
    display: 'flex',
    flexDirection: 'column',
    gap: 5,
    paddingTop: 2,
    paddingBottom: 6,
  },
  storyLine: {
    display: 'flex',
    gap: 8,
    fontSize: 12.5,
    color: vars.colorBase,
    alignItems: 'baseline',
  },
  storyGlyph: {
    fontFamily: font.mono,
    fontWeight: 700,
    width: 10,
    flexShrink: 0,
    color: vars.colorFaint,
  },
  split: {
    display: 'flex',
    gap: 0,
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
    flex: 1,
    minHeight: 420,
  },
  graphPane: {
    flex: 1.15,
    position: 'relative',
    minWidth: 0,
    borderRightWidth: 1,
    borderRightStyle: 'solid',
    borderRightColor: vars.borderMute,
  },
  graphEmpty: {
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    height: '100%',
    fontSize: 11,
    color: vars.colorFaint,
  },
  graphNote: {
    position: 'absolute',
    bottom: 10,
    left: 0,
    right: 0,
    textAlign: 'center',
    fontSize: 10,
    color: vars.colorFaint,
    pointerEvents: 'none',
  },
  kindBar: {
    position: 'absolute',
    top: 8,
    left: 8,
    zIndex: 6,
  },
  rail: {
    flex: 1,
    minWidth: 0,
    overflowY: 'auto',
  },
  railEmpty: {
    padding: 18,
    fontSize: 11,
    color: vars.colorFaint,
  },
  fileRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 9,
    width: '100%',
    borderBottomWidth: 1,
    borderBottomStyle: 'solid',
    borderBottomColor: vars.borderMute,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    paddingTop: 8,
    paddingBottom: 8,
    paddingLeft: 8,
    paddingRight: 6,
    cursor: 'pointer',
    textAlign: 'left',
  },

  caret: {
    display: 'inline-flex',
    color: vars.colorFaint,
    transform: 'rotate(-90deg)',
    transition: 'transform 120ms',
    flexShrink: 0,
  },
  caretOpen: {
    transform: 'rotate(0deg)',
  },
  dot: {
    width: 7,
    height: 7,
    borderRadius: '50%',
    flexShrink: 0,
  },
  dotAdd: {
    backgroundColor: vars.scaleLow,
  },
  dotDel: {
    backgroundColor: vars.scaleHigh,
  },
  dotMod: {
    backgroundColor: vars.scaleMedium,
  },
  dotCtx: {
    backgroundColor: vars.colorFaint,
  },
  fileName: {
    fontFamily: font.mono,
    fontSize: 12,
    fontWeight: 600,
    color: vars.colorBase,
    minWidth: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  fileTag: {
    fontSize: 10,
    color: vars.colorFaint,
    flexShrink: 0,
  },
  fileNums: {
    marginLeft: 'auto',
    fontFamily: font.mono,
    fontSize: 10.5,
    color: vars.colorMuted,
    flexShrink: 0,
    fontVariantNumeric: 'tabular-nums',
  },
  cAdd: {
    color: vars.scaleLow,
    fontWeight: 600,
  },
  cDel: {
    color: vars.scaleHigh,
    fontWeight: 600,
  },
  rowAction: {
    width: 20,
    display: 'inline-flex',
    justifyContent: 'flex-end',
    alignItems: 'center',
    flexShrink: 0,
  },
  detail: {
    borderBottomWidth: 1,
    borderBottomStyle: 'solid',
    borderBottomColor: vars.borderMute,
    paddingTop: 6,
    paddingBottom: 10,
    paddingLeft: 28,
    paddingRight: 8,
    display: 'flex',
    flexDirection: 'column',
    gap: 10,
  },
  group: {
    display: 'flex',
    flexDirection: 'column',
    gap: 0,
  },
  groupLabel: {
    fontSize: 10,
    fontWeight: 600,
    letterSpacing: '0.04em',
    color: vars.colorMuted,
    paddingTop: 2,
    paddingBottom: 3,
  },
  groupEmpty: {
    fontSize: 10.5,
    color: vars.colorFaint,
    paddingTop: 3,
    paddingBottom: 3,
    paddingLeft: 70,
  },
  groupNote: {
    fontSize: 10.5,
    color: vars.colorFaint,
  },
  detailHead: {
    display: 'flex',
    alignItems: 'center',
    gap: 9,
    paddingTop: 8,
    paddingBottom: 8,
    paddingLeft: 8,
    paddingRight: 6,
    borderBottomWidth: 1,
    borderBottomStyle: 'solid',
    borderBottomColor: vars.borderMute,
  },
  detailBody: {
    paddingTop: 8,
    paddingBottom: 12,
    paddingLeft: 8,
    paddingRight: 8,
    display: 'flex',
    flexDirection: 'column',
    gap: 10,
  },
  symRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    flexWrap: 'wrap',
    paddingTop: 2,
    paddingBottom: 2,
    paddingRight: 4,
    borderRadius: 4,
  },
  symKind: {
    fontFamily: font.mono,
    fontSize: 9.5,
    color: vars.colorFaint,
    width: 60,
    flexShrink: 0,
    textAlign: 'right',
  },
  symName: {
    fontFamily: font.mono,
    fontSize: 11.5,
    fontWeight: 500,
    color: vars.colorBase,
    minWidth: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  relRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    paddingTop: 2,
    paddingBottom: 2,
    paddingRight: 4,
    borderRadius: 4,
  },
  relKind: {
    fontFamily: font.mono,
    fontSize: 9.5,
    color: vars.colorFaint,
    width: 60,
    flexShrink: 0,
    textAlign: 'right',
  },
  relEnds: {
    flex: 1,
    minWidth: 0,
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorBase,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  relArrow: {
    color: vars.colorFaint,
  },
  relFile: {
    color: vars.colorFaint,
    fontSize: 10,
  },
  relMeta: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
    flexShrink: 0,
  },
  changedLabel: {
    fontSize: 11,
    fontWeight: 600,
    color: vars.colorMuted,
  },
  changedNote: {
    fontSize: 10,
    color: vars.colorFaint,
  },
  footnote: {
    fontFamily: font.mono,
    fontSize: 9.5,
    color: vars.colorFaint,
    paddingTop: 2,
  },
  // --- graph node ---
  gNode: {
    width: NODE_W,
    height: NODE_H,
    boxSizing: 'border-box',
    display: 'flex',
    flexDirection: 'column',
    justifyContent: 'center',
    gap: 3,
    paddingLeft: 12,
    paddingRight: 10,
    borderRadius: 8,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgRaised,
    cursor: 'pointer',
    boxShadow: {
      default: 'none',
      ':hover': `0 0 0 2px ${vars.scaleLow}`,
    },
  },
  gNodeNew: {
    borderColor: vars.scaleLow,
  },
  gNodeDel: {
    borderColor: vars.scaleHigh,
  },
  gNodeCtx: {
    borderStyle: 'dashed',
    opacity: 0.75,
  },
  gNodeSel: {
    boxShadow: `0 0 0 2px ${vars.scaleLow}`,
  },
  gNodeDim: {
    opacity: 0.3,
  },
  gNodeTop: {
    display: 'flex',
    alignItems: 'center',
    gap: 7,
    minWidth: 0,
  },
  gNodeName: {
    fontFamily: font.mono,
    fontSize: 11.5,
    fontWeight: 600,
    color: vars.colorBase,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  gNodeSub: {
    fontFamily: font.mono,
    fontSize: 9.5,
    color: vars.colorFaint,
    paddingLeft: 14,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  handle: {
    opacity: 0,
    width: 1,
    height: 1,
    borderWidth: 0,
    minWidth: 0,
    minHeight: 0,
  },
  peek: {
    flexBasis: '100%',
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    flexWrap: 'wrap',
    marginTop: 4,
    marginLeft: 70,
    paddingTop: 6,
    paddingBottom: 6,
    paddingLeft: 10,
    paddingRight: 10,
    borderRadius: 6,
    backgroundColor: vars.bgSunken,
  },
  peekCount: {
    fontSize: 10.5,
    fontWeight: 600,
    color: vars.colorMuted,
  },
  peekHit: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorBase,
    display: 'inline-flex',
    alignItems: 'center',
    gap: 4,
    paddingTop: 2,
    paddingBottom: 2,
    paddingLeft: 6,
    paddingRight: 6,
    borderRadius: 4,
    backgroundColor: vars.bgRaised,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
  },
  peekHops: {
    color: vars.colorFaint,
    fontSize: 9,
  },
  peekNote: {
    fontSize: 10,
    color: vars.colorFaint,
  },
})
