import * as stylex from '@stylexjs/stylex'
import dagre from 'dagre'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useNavigate } from 'react-router'
import {
  Background,
  BackgroundVariant,
  Controls,
  Handle,
  MarkerType,
  MiniMap,
  Position,
  ReactFlow,
  ReactFlowProvider,
  useEdgesState,
  useNodesInitialized,
  useNodesState,
  useReactFlow,
  type Edge,
  type Node,
  type NodeProps,
} from '@xyflow/react'
import '@xyflow/react/dist/style.css'
import {
  api,
  describeError,
  type CodebaseMap,
  type ComponentExplanation,
  type PackageNode,
} from '../api'
import { FacetRail } from '../components/FacetRail'
import { emptyFilters, toggleFilter, type ActiveFilters, type FacetGroup } from '../lib/facets'
import { fileUrl } from '../lib/navigation'
import { ActionIconButton } from '../ui/ActionIconButton'
import { CopyButton } from '../ui/CopyButton'
import { DisplayBadge } from '../ui/DisplayBadge'
import { DisplayFilePath } from '../ui/DisplayFilePath'
import { FeedbackEmptyState, FeedbackSkeleton } from '../ui/FeedbackStates'
import { FormSearchField } from '../ui/FormSearchField'
import { IconArrowUpRight, IconPackage } from '../ui/icons'
import { font, vars } from '../ui/tokens.stylex'
import { useColorScheme } from '../ui/useDark'

/**
 * Map — the package dependency atlas as a real node graph (React Flow +
 * dagre layered layout, not a table). `/v1/map` ships manifest-derived
 * edges, so the canvas is a TOP-DOWN DAG: consumers sit on the surface
 * rank, foundations sink to bedrock, and every arrow points down at the
 * thing a package depends on — the whole structure is build-system truth
 * (`provenance` names the deterministic source; `live` marks it).
 *
 * Reading model: hover a package and its neighborhood isolates — blue
 * edges/cards are what it DEPENDS on, amber are its DEPENDENTS, the rest
 * dims. Click pins a detail rail (`/v1/explain` — member files, tests,
 * declared edges). Packages with no manifest edges aren't part of the
 * DAG at all; they render in a detached strip, not faked into a rank.
 * Pan/zoom/drag are React Flow's; the layout re-fits on facet changes.
 */
export function MapScreen() {
  const [map, setMap] = useState<CodebaseMap | null>(null)
  const [err, setErr] = useState<string>()
  const [busy, setBusy] = useState(true)
  const [text, setText] = useState('')
  const [filters, setFilters] = useState<ActiveFilters>(emptyFilters())
  const [hover, setHover] = useState<string>()
  const [selected, setSelected] = useState<string>()

  useEffect(() => {
    let dead = false
    api
      .map()
      .then((m) => !dead && setMap(m))
      .catch((e) => !dead && setErr(describeError(e)))
      .finally(() => !dead && setBusy(false))
    return () => {
      dead = true
    }
  }, [])

  const groups: FacetGroup[] = useMemo(() => {
    if (!map) return []
    const m = new Map<string, number>()
    for (const p of map.packages) m.set(p.ecosystem, (m.get(p.ecosystem) ?? 0) + 1)
    return [
      {
        id: 'ecosystem',
        label: 'Ecosystem',
        options: [...m.entries()]
          .map(([value, n]) => ({ value, label: value, count: n }))
          .sort((a, b) => b.count - a.count || a.value.localeCompare(b.value)),
      },
    ]
  }, [map])

  /** Facet-filtered package set — the graph is rebuilt from these. */
  const packages = useMemo(() => {
    const eco = filters.ecosystem
    return (map?.packages ?? []).filter((p) => !eco || eco.size === 0 || eco.has(p.ecosystem))
  }, [map, filters])

  const base = useMemo(() => layoutDagre(packages), [packages])

  // Facet changes can filter the pinned package out — don't leave a rail
  // open for a node that's no longer on the canvas.
  useEffect(() => {
    if (selected && !packages.some((p) => p.name === selected)) setSelected(undefined)
    if (hover && !packages.some((p) => p.name === hover)) setHover(undefined)
  }, [packages, selected, hover])

  /** Search dims non-matches instead of removing them — the DAG keeps its
   *  shape so a hit reads in context. */
  const needle = text.trim().toLowerCase()
  const matches = useMemo(() => {
    if (!needle) return null
    const s = new Set<string>()
    for (const p of packages) {
      if (`${p.name} ${p.rootDir} ${p.manifestPath}`.toLowerCase().includes(needle)) s.add(p.name)
    }
    return s
  }, [packages, needle])

  /** Neighborhood of the hovered (or pinned) node — the highlight set. */
  const focus = hover ?? selected
  const neighborhood = useMemo(() => {
    if (!focus) return null
    return {
      deps: new Set(base.depsOf.get(focus) ?? []),
      users: new Set(base.usersOf.get(focus) ?? []),
    }
  }, [focus, base])

  /** Violated package pair → edge ids. Violations carry the endpoint
   *  package names, so matching is exact — but the crossed boundary may
   *  not exist as a declared edge, in which case nothing is highlighted. */
  const violated = useMemo(() => {
    const s = new Set<string>()
    for (const v of map?.violations ?? []) {
      s.add(`${v.sourcePackage}->${v.targetPackage}`)
    }
    return s
  }, [map])

  const nodeRelation = (name: string): Relation => {
    if (matches && !matches.has(name)) return 'dim'
    if (!neighborhood) return 'none'
    if (name === focus) return 'focus'
    if (neighborhood.deps.has(name)) return 'dep'
    if (neighborhood.users.has(name)) return 'user'
    return 'far'
  }

  const nodes: Node<PackageData>[] = useMemo(
    () =>
      base.nodes.map((n) => ({
        ...n,
        data: { ...n.data, relation: nodeRelation(n.id) },
      })),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [base, matches, neighborhood, focus],
  )

  const edges: Edge[] = useMemo(
    () =>
      base.edges.map((e) => {
        const vio = violated.has(e.id)
        let variant: EdgeVariant = 'base'
        if (neighborhood) {
          if (e.source === focus && neighborhood.deps.has(e.target)) variant = 'dep'
          else if (e.target === focus && neighborhood.users.has(e.source)) variant = 'user'
        }
        const dim = vio
          ? false
          : neighborhood
            ? variant === 'base'
            : matches != null && !(matches.has(e.source) && matches.has(e.target))
        const color = vio
          ? vars.scaleCritical
          : variant === 'dep'
            ? vars.accentInfo
            : variant === 'user'
              ? vars.scaleMedium
              : vars.colorMuted
        return {
          ...e,
          style: {
            stroke: color,
            strokeWidth: variant === 'base' && !vio ? 1.2 : 1.9,
            strokeDasharray: vio ? '5 3' : undefined,
            opacity: dim ? 0.16 : 1,
          },
          markerEnd: {
            type: MarkerType.ArrowClosed,
            color,
            width: 16,
            height: 16,
          },
        }
      }),
    [base, neighborhood, focus, matches, violated],
  )

  const total = map?.packages.length ?? 0
  const bedrock = useMemo(() => {
    let best: PackageNode | undefined
    for (const p of packages) {
      if (!best || p.dependents.length > best.dependents.length) best = p
    }
    return best && best.dependents.length > 0 ? best : undefined
  }, [packages])

  const scheme = useColorScheme()
  const sig = `${map?.snapshotId}:${packages.map((p) => p.name).join(',')}`

  return (
    <div {...stylex.props(styles.root)}>
      <header {...stylex.props(styles.head)}>
        <h2 {...stylex.props(styles.title)}>Map</h2>
        <DisplayBadge severity="low">live</DisplayBadge>
        {map && (
          <span {...stylex.props(styles.count)}>
            {packages.length === total
              ? `${total} packages · ${base.edges.length} edges`
              : `${packages.length} of ${total} packages · ${base.edges.length} edges`}
            {base.layers > 0 && ` · ${base.layers + 1} layers`}
            {base.detached.length > 0 && ` · ${base.detached.length} detached`}
          </span>
        )}
        {map && (
          <code {...stylex.props(styles.snap)} title={map.snapshotId}>
            {map.snapshotId.slice(0, 13)}
          </code>
        )}
        <span {...stylex.props(styles.spacer)} />
        <div {...stylex.props(styles.search)}>
          <FormSearchField
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder="dim all but…"
            onClear={() => setText('')}
            aria-label="Highlight packages"
          />
        </div>
      </header>

      {map && (
        <div {...stylex.props(styles.legend)}>
          <span>
            <span {...stylex.props(styles.legendArrow)}>↓</span> depends on
          </span>
          <span>
            <i {...stylex.props(styles.legendChip, styles.legendDep)} /> its dependencies
          </span>
          <span>
            <i {...stylex.props(styles.legendChip, styles.legendUser)} /> its dependents
          </span>
          <span {...stylex.props(styles.legendDim)}>hover isolates a neighborhood · drag to rearrange</span>
          <span {...stylex.props(styles.legendProv)} title={map.provenance}>
            {bedrock && `bedrock ${bedrock.name} (dep-by ${bedrock.dependents.length}) · `}
            {map.provenance}
          </span>
        </div>
      )}

      {map && map.violationCount > 0 && (
        <div {...stylex.props(styles.violations)}>
          <DisplayBadge severity="medium">{map.violationCount} violations</DisplayBadge>
          <div {...stylex.props(styles.violationList)}>
            {map.violations.slice(0, 4).map((v, i) => (
              <div key={i} {...stylex.props(styles.violationRow)}>
                {v.sourcePackage} → {v.targetPackage}: {v.kind} {v.sourceEntity} → {v.targetEntity}
                <span {...stylex.props(styles.violationPath)}>{v.evidencePath}</span>
              </div>
            ))}
            {map.violationCount > map.violations.length && (
              <div {...stylex.props(styles.violationRow)}>
                +{map.violationCount - map.violations.length} more
              </div>
            )}
          </div>
        </div>
      )}

      <div {...stylex.props(styles.body)}>
        <FacetRail
          groups={groups}
          active={filters}
          onToggle={(g, v) => setFilters((f) => toggleFilter(f, g, v))}
        />
        <div {...stylex.props(styles.canvasCol)}>
          {busy && <FeedbackSkeleton height={420} />}
          {err && <FeedbackEmptyState title="map failed to load" description={`/v1/map — ${err}`} />}
          {map && packages.length === 0 && (
            <FeedbackEmptyState
              icon={<IconPackage size={20} />}
              title={total === 0 ? 'no packages in this snapshot' : 'no packages match'}
              description={
                total === 0 ? 'the resolver found no manifests' : 'adjust the ecosystem facet'
              }
            />
          )}
          {map && packages.length > 0 && (
            <div {...stylex.props(styles.canvasCard)}>
              <ReactFlowProvider>
                <MapCanvas
                  nodes={nodes}
                  edges={edges}
                  sig={sig}
                  scheme={scheme}
                  onHover={setHover}
                  onSelect={(id) => setSelected((s) => (s === id ? undefined : id))}
                  onDeselect={() => setSelected(undefined)}
                />
              </ReactFlowProvider>
              {base.nodes.length === 0 && (
                <div {...stylex.props(styles.canvasEmpty)}>
                  no manifest edges among these packages — see the detached strip below
                </div>
              )}
            </div>
          )}
          {map && base.detached.length > 0 && (
            <div {...stylex.props(styles.detached)}>
              <div {...stylex.props(styles.detachedLabel)}>
                detached — no manifest edges ({base.detached.length})
              </div>
              <div {...stylex.props(styles.detachedRow)}>
                {base.detached.map((p) => (
                  <button
                    key={p.name}
                    type="button"
                    title={`${p.name} — ${p.rootDir}`}
                    onClick={() => setSelected((s) => (s === p.name ? undefined : p.name))}
                    onMouseEnter={() => setHover(p.name)}
                    onMouseLeave={() => setHover(undefined)}
                    {...stylex.props(
                      styles.detachedCard,
                      selected === p.name && styles.detachedCardSel,
                      matches && !matches.has(p.name) && styles.detachedDim,
                    )}
                  >
                    <span {...stylex.props(styles.detachedName)}>{p.name}</span>
                    <span {...stylex.props(styles.detachedMeta)}>
                      {p.ecosystem} · {p.memberFiles} files
                    </span>
                  </button>
                ))}
              </div>
            </div>
          )}
        </div>
        {selected && (
          <DetailRail
            name={selected}
            pkg={packages.find((p) => p.name === selected)}
            onClose={() => setSelected(undefined)}
          />
        )}
      </div>
    </div>
  )
}

// --- React Flow canvas ----------------------------------------------------------

type Relation = 'none' | 'focus' | 'dep' | 'user' | 'far' | 'dim'
type EdgeVariant = 'base' | 'dep' | 'user'
type PackageData = { pkg: PackageNode; relation: Relation } & Record<string, unknown>

const nodeTypes = { pkg: PackageCard }

function MapCanvas({
  nodes,
  edges,
  sig,
  scheme,
  onHover,
  onSelect,
  onDeselect,
}: {
  nodes: Node<PackageData>[]
  edges: Edge[]
  sig: string
  scheme: 'light' | 'dark'
  onHover: (id?: string) => void
  onSelect: (id: string) => void
  onDeselect: () => void
}) {
  const [flowNodes, setFlowNodes, onNodesChange] = useNodesState<Node<PackageData>>(nodes)
  // Edges seed empty and populate via effect — feeding them at mount races
  // handle registration and spams RF error #008 in dev.
  const [flowEdges, setFlowEdges, onEdgesChange] = useEdgesState<Edge>([])

  // Data-only updates (hover/search/facet) must not clobber positions the
  // user dragged — merge fresh data onto live positions.
  useEffect(() => {
    setFlowNodes((ns) => {
      const byId = new Map(ns.map((n) => [n.id, n]))
      return nodes.map((d) => {
        const live = byId.get(d.id)
        return live ? { ...d, position: live.position, measured: live.measured } : d
      })
    })
  }, [nodes, setFlowNodes])
  useEffect(() => setFlowEdges(edges), [edges, setFlowEdges])

  return (
    <ReactFlow
      nodes={flowNodes}
      edges={flowEdges}
      nodeTypes={nodeTypes}
      onNodesChange={onNodesChange}
      onEdgesChange={onEdgesChange}
      onNodeMouseEnter={(_, n) => onHover(n.id)}
      onNodeMouseLeave={() => onHover(undefined)}
      onNodeClick={(_, n) => onSelect(n.id)}
      onPaneClick={onDeselect}
      nodesConnectable={false}
      deleteKeyCode={null}
      minZoom={0.05}
      maxZoom={2}
      colorMode={scheme}
      aria-label="Package dependency graph"
    >
      <Background variant={BackgroundVariant.Dots} gap={24} size={1} />
      <Controls position="bottom-right" showInteractive={false} />
      <MiniMap
        position="bottom-left"
        pannable
        zoomable
        nodeColor={vars.colorMuted}
        maskColor={vars.bgSunken}
        bgColor={vars.bgRaised}
      />
      <Refit sig={sig} />
    </ReactFlow>
  )
}

/** Re-fits the view whenever the package set or snapshot changes — the
 *  initial mount counts as a change, so first load lands fitted too. */
function Refit({ sig }: { sig: string }) {
  const rf = useReactFlow()
  const inited = useNodesInitialized()
  const last = useRef('')
  useEffect(() => {
    if (!inited || last.current === sig) return
    last.current = sig
    void rf.fitView({ padding: 0.14, maxZoom: 1.2, duration: 220 })
  }, [inited, sig, rf])
  return null
}

// --- dagre layout -----------------------------------------------------------------

const NODE_W = 216
const NODE_H = 62

/**
 * dagre layered layout, rankdir TB: consumers on the surface rank,
 * bedrock at the bottom. Deterministic — same snapshot, same picture.
 * Detached nodes (no manifest edges) are excluded from the DAG and
 * rendered by the caller in their own strip.
 */
function layoutDagre(packages: PackageNode[]) {
  const names = new Set(packages.map((p) => p.name))
  const depsOf = new Map<string, string[]>()
  const usersOf = new Map<string, string[]>()
  const edgePairs: [string, string][] = []
  for (const p of packages) {
    const deps = p.dependencies.filter((d) => names.has(d))
    depsOf.set(p.name, deps)
    for (const d of deps) {
      edgePairs.push([p.name, d])
      const u = usersOf.get(d)
      if (u) u.push(p.name)
      else usersOf.set(d, [p.name])
    }
  }

  const linked = packages.filter(
    (p) => (depsOf.get(p.name)?.length ?? 0) > 0 || (usersOf.get(p.name)?.length ?? 0) > 0,
  )
  const linkedSet = new Set(linked.map((p) => p.name))
  const detached = packages.filter((p) => !linkedSet.has(p.name))
  // Dedupe — a manifest may declare the same dep twice across scopes.
  const pairs = [...new Map(edgePairs.map(([f, t]) => [`${f}->${t}`, [f, t] as const])).values()]

  const g = new dagre.graphlib.Graph()
  g.setGraph({ rankdir: 'TB', nodesep: 44, ranksep: 84, marginx: 24, marginy: 24 })
  g.setDefaultEdgeLabel(() => ({}))
  for (const p of linked) g.setNode(p.name, { width: NODE_W, height: NODE_H })
  for (const [f, t] of pairs) g.setEdge(f, t)
  dagre.layout(g)

  const nodes: Node<PackageData>[] = linked.map((p) => {
    const pos = g.node(p.name)
    return {
      id: p.name,
      type: 'pkg',
      position: { x: pos.x - NODE_W / 2, y: pos.y - NODE_H / 2 },
      data: { pkg: p, relation: 'none' as Relation },
    }
  })
  const edges: Edge[] = pairs.map(([f, t]) => ({
    id: `${f}->${t}`,
    source: f,
    target: t,
    sourceHandle: 's',
    targetHandle: 't',
  }))
  // Rank count for the header stat — distinct y centers = distinct ranks.
  const ranks = new Set(linked.map((p) => g.node(p.name)?.y)).size
  return { nodes, edges, depsOf, usersOf, detached, layers: Math.max(0, ranks - 1) }
}

// --- node card ---------------------------------------------------------------------

function PackageCard({ data }: NodeProps<Node<PackageData>>) {
  const { pkg, relation } = data
  const label = pkg.name.length > 24 ? `${pkg.name.slice(0, 23)}…` : pkg.name
  return (
    <div
      title={`${pkg.name}\n${pkg.rootDir} — ${pkg.memberFiles} files\ndepends on ${pkg.dependencies.length} · depended on by ${pkg.dependents.length}`}
      {...stylex.props(
        styles.nodeCard,
        relation === 'dep' && styles.nodeDep,
        relation === 'user' && styles.nodeUser,
        relation === 'focus' && styles.nodeFocus,
        (relation === 'far' || relation === 'dim') && styles.nodeDim,
      )}
    >
      {/* Edges anchor on invisible handles — consumers emit downward. */}
      <Handle id="t" type="target" position={Position.Top} {...stylex.props(styles.handle)} />
      <div {...stylex.props(styles.nodeName)}>{label}</div>
      <div {...stylex.props(styles.nodeMeta)}>
        {`${pkg.ecosystem} · ${pkg.memberFiles} files · dep:${pkg.dependencies.length} by:${pkg.dependents.length}`}
      </div>
      <Handle id="s" type="source" position={Position.Bottom} {...stylex.props(styles.handle)} />
    </div>
  )
}

// --- detail rail ---------------------------------------------------------------------

const RAIL_ROW_CAP = 12

/**
 * Detail rail — the pinned package's explain card via `/v1/explain`:
 * member files and tests deep-link into Browse, declared edges render
 * immediately from the map row and refine when explain lands. An explain
 * failure keeps the declared edges with an honest note.
 */
function DetailRail({
  name,
  pkg,
  onClose,
}: {
  name: string
  pkg: PackageNode | undefined
  onClose: () => void
}) {
  const navigate = useNavigate()
  const [explain, setExplain] = useState<ComponentExplanation | null>(null)
  const [err, setErr] = useState<string>()

  useEffect(() => {
    setExplain(null)
    setErr(undefined)
    let dead = false
    const timeout = new Promise<never>((_, rej) =>
      setTimeout(() => rej(new Error('timed out — daemon busy')), 20_000),
    )
    Promise.race([api.explain(name), timeout])
      .then((r) => {
        if (!dead) setExplain(r)
      })
      .catch((e: unknown) => {
        if (!dead) setErr(e instanceof Error ? e.message : 'request failed')
      })
    return () => {
      dead = true
    }
  }, [name])

  const deps = explain?.dependencies ?? pkg?.dependencies ?? []
  const users = explain?.dependents ?? pkg?.dependents ?? []
  const files = explain?.memberFiles ?? []
  const tests = explain?.tests ?? []

  return (
    <aside {...stylex.props(styles.rail)}>
      <div {...stylex.props(styles.railHead)}>
        <span {...stylex.props(styles.railName)} title={name}>
          {name}
        </span>
        {pkg && <DisplayBadge text={pkg.ecosystem} color={false} />}
        <span {...stylex.props(styles.spacer)} />
        {pkg && (
          <ActionIconButton
            compact
            icon={<IconArrowUpRight size={10} />}
            tooltip={`open ${pkg.manifestPath}`}
            label={`Open ${pkg.manifestPath}`}
            onClick={() => navigate(fileUrl(pkg.manifestPath))}
          />
        )}
        <button
          type="button"
          aria-label="Close detail"
          onClick={onClose}
          {...stylex.props(styles.railClose)}
        >
          ×
        </button>
      </div>
      {explain?.qualifiedName && (
        <div {...stylex.props(styles.railQual)}>
          <span {...stylex.props(styles.railQualText)}>{explain.qualifiedName}</span>
          <CopyButton
            text={explain.qualifiedName}
            title={`copy ${explain.qualifiedName}`}
            label={`Copy ${explain.qualifiedName}`}
          />
        </div>
      )}
      <div {...stylex.props(styles.railBadgeRow)}>
        <DisplayBadge severity="low" title="live — persisted relations, not inferred">
          live explain
        </DisplayBadge>
        {explain == null && err == null && <span {...stylex.props(styles.railNote)}>loading…</span>}
        {err != null && <span {...stylex.props(styles.railNote)}>explain unavailable — {err}</span>}
      </div>
      <div {...stylex.props(styles.railCols)}>
        <RailList title={`depends on (${deps.length})`} items={deps} empty="none declared" />
        <RailList
          title={`depended on by (${users.length})`}
          items={users}
          empty="nothing depends on it"
        />
      </div>
      {explain != null && (
        <>
          <div {...stylex.props(styles.railLabel)}>member files ({files.length})</div>
          {files.length === 0 && <div {...stylex.props(styles.railEmpty)}>none recorded</div>}
          {files.slice(0, RAIL_ROW_CAP).map((f) => (
            <button
              key={f}
              type="button"
              title={`open ${f}`}
              onClick={() => navigate(fileUrl(f))}
              {...stylex.props(styles.railFile)}
            >
              <DisplayFilePath path={f} />
            </button>
          ))}
          {files.length > RAIL_ROW_CAP && (
            <div {...stylex.props(styles.railEmpty)}>+{files.length - RAIL_ROW_CAP} more</div>
          )}
          {tests.length > 0 && (
            <>
              <div {...stylex.props(styles.railLabel)}>tests ({tests.length})</div>
              {tests.slice(0, RAIL_ROW_CAP).map((f) => (
                <button
                  key={f}
                  type="button"
                  title={`open ${f}`}
                  onClick={() => navigate(fileUrl(f))}
                  {...stylex.props(styles.railFile)}
                >
                  <DisplayFilePath path={f} />
                </button>
              ))}
            </>
          )}
          <div {...stylex.props(styles.railFoot)}>{explain.provenance}</div>
        </>
      )}
    </aside>
  )
}

function RailList({ title, items, empty }: { title: string; items: string[]; empty: string }) {
  return (
    <div {...stylex.props(styles.railCol)}>
      <div {...stylex.props(styles.railLabel)}>{title}</div>
      {items.length === 0 && <div {...stylex.props(styles.railEmpty)}>{empty}</div>}
      {items.slice(0, RAIL_ROW_CAP).map((i) => (
        <div key={i} {...stylex.props(styles.railItem)}>
          {i}
        </div>
      ))}
      {items.length > RAIL_ROW_CAP && (
        <div {...stylex.props(styles.railEmpty)}>+{items.length - RAIL_ROW_CAP} more</div>
      )}
    </div>
  )
}

// --- styles -------------------------------------------------------------------------

const styles = stylex.create({
  root: {
    display: 'flex',
    flexDirection: 'column',
    gap: 12,
    // The shell's scroll div is block-level — minHeight:100% is what lets
    // the canvas below actually fill the viewport instead of collapsing
    // to content height.
    minHeight: '100%',
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
  count: {
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorFaint,
  },
  snap: {
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
  legend: {
    display: 'flex',
    alignItems: 'center',
    gap: 14,
    flexWrap: 'wrap',
    fontSize: 10.5,
    color: vars.colorMuted,
  },
  legendArrow: {
    fontFamily: font.mono,
    color: vars.colorBase,
  },
  legendChip: {
    display: 'inline-block',
    width: 14,
    height: 3,
    borderRadius: 2,
    marginRight: 5,
    verticalAlign: 'middle',
  },
  legendDep: {
    backgroundColor: vars.accentInfo,
  },
  legendUser: {
    backgroundColor: vars.scaleMedium,
  },
  legendDim: {
    color: vars.colorFaint,
  },
  legendProv: {
    marginLeft: 'auto',
    fontFamily: font.mono,
    color: vars.colorFaint,
  },
  violations: {
    display: 'flex',
    alignItems: 'flex-start',
    gap: 10,
    borderRadius: 8,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgRaised,
    paddingTop: 9,
    paddingBottom: 9,
    paddingLeft: 12,
    paddingRight: 12,
  },
  violationList: {
    display: 'flex',
    flexDirection: 'column',
    gap: 2,
    minWidth: 0,
  },
  violationRow: {
    fontFamily: font.mono,
    fontSize: 10.5,
    color: vars.scaleMedium,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  violationPath: {
    color: vars.colorFaint,
    marginLeft: 8,
  },
  body: {
    display: 'flex',
    gap: 16,
    alignItems: 'stretch',
    flex: 1,
    minHeight: 0,
  },
  canvasCol: {
    flex: 1,
    minWidth: 0,
    display: 'flex',
    flexDirection: 'column',
    gap: 8,
    minHeight: 0,
  },
  canvasCard: {
    flex: 1,
    minHeight: 420,
    borderRadius: 10,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgSunken,
    overflow: 'hidden',
    position: 'relative',
  },
  canvasEmpty: {
    position: 'absolute',
    inset: 0,
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    fontSize: 11.5,
    color: vars.colorFaint,
    pointerEvents: 'none',
  },
  nodeCard: {
    width: NODE_W,
    height: NODE_H,
    borderRadius: 9,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgRaised,
    paddingTop: 8,
    paddingBottom: 8,
    paddingLeft: 13,
    paddingRight: 10,
    cursor: 'grab',
    overflow: 'hidden',
  },
  nodeDep: {
    borderColor: vars.accentInfo,
    borderWidth: 1.6,
  },
  nodeUser: {
    borderColor: vars.scaleMedium,
    borderWidth: 1.6,
  },
  nodeFocus: {
    borderColor: vars.scaleLow,
    borderWidth: 2,
  },
  nodeDim: {
    opacity: 0.28,
  },
  handle: {
    width: 1,
    height: 1,
    borderWidth: 0,
    backgroundColor: 'transparent',
    opacity: 0,
    pointerEvents: 'none',
  },
  nodeName: {
    fontFamily: font.mono,
    fontSize: 12.5,
    fontWeight: 600,
    color: vars.colorBase,
    paddingBottom: 6,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  nodeMeta: {
    fontFamily: font.mono,
    fontSize: 9.5,
    color: vars.colorFaint,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  detached: {
    flexShrink: 0,
    borderRadius: 10,
    borderWidth: 1,
    borderStyle: 'dashed',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgRaised,
    paddingTop: 9,
    paddingBottom: 10,
    paddingLeft: 14,
    paddingRight: 14,
  },
  detachedLabel: {
    fontSize: 10,
    fontWeight: 600,
    letterSpacing: '0.06em',
    textTransform: 'uppercase',
    color: vars.colorFaint,
    paddingBottom: 8,
  },
  detachedRow: {
    display: 'flex',
    flexWrap: 'wrap',
    gap: 8,
  },
  detachedCard: {
    display: 'flex',
    flexDirection: 'column',
    gap: 3,
    borderRadius: 8,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: {
      default: vars.bgRaised,
      ':hover': vars.bgHover,
    },
    paddingTop: 7,
    paddingBottom: 7,
    paddingLeft: 11,
    paddingRight: 11,
    cursor: 'pointer',
    textAlign: 'left',
  },
  detachedCardSel: {
    borderColor: vars.scaleLow,
    borderWidth: 2,
  },
  detachedDim: {
    opacity: 0.25,
  },
  detachedName: {
    fontFamily: font.mono,
    fontSize: 11.5,
    fontWeight: 600,
    color: vars.colorBase,
  },
  detachedMeta: {
    fontFamily: font.mono,
    fontSize: 9.5,
    color: vars.colorFaint,
  },
  rail: {
    width: 300,
    flexShrink: 0,
    display: 'flex',
    flexDirection: 'column',
    gap: 4,
    borderRadius: 10,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgRaised,
    paddingTop: 10,
    paddingBottom: 10,
    paddingLeft: 12,
    paddingRight: 12,
    maxHeight: 'calc(100vh - 240px)',
    overflowY: 'auto',
  },
  railHead: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    minWidth: 0,
  },
  railName: {
    fontFamily: font.mono,
    fontSize: 13,
    fontWeight: 600,
    color: vars.colorBase,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  railClose: {
    borderWidth: 0,
    backgroundColor: 'transparent',
    color: vars.colorFaint,
    fontSize: 15,
    lineHeight: 1,
    cursor: 'pointer',
    paddingTop: 2,
    paddingBottom: 2,
    paddingLeft: 4,
    paddingRight: 4,
  },
  railQual: {
    display: 'flex',
    alignItems: 'center',
    gap: 6,
  },
  railQualText: {
    flex: 1,
    minWidth: 0,
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorMuted,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  railBadgeRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    paddingTop: 4,
    paddingBottom: 6,
    borderBottomWidth: 1,
    borderBottomStyle: 'solid',
    borderBottomColor: vars.borderMute,
  },
  railNote: {
    fontSize: 10.5,
    color: vars.colorFaint,
  },
  railCols: {
    display: 'flex',
    gap: 14,
    paddingTop: 4,
  },
  railCol: {
    flex: 1,
    minWidth: 0,
    display: 'flex',
    flexDirection: 'column',
  },
  railLabel: {
    fontSize: 9.5,
    fontWeight: 600,
    letterSpacing: '0.06em',
    textTransform: 'uppercase',
    color: vars.colorFaint,
    paddingTop: 6,
    paddingBottom: 4,
  },
  railItem: {
    fontFamily: font.mono,
    fontSize: 10.5,
    color: vars.colorBase,
    paddingTop: 3,
    paddingBottom: 3,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  railEmpty: {
    fontSize: 10.5,
    color: vars.colorFaint,
    paddingTop: 3,
    paddingBottom: 3,
  },
  railFile: {
    display: 'flex',
    alignItems: 'center',
    minWidth: 0,
    borderWidth: 0,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    fontFamily: 'inherit',
    fontSize: 11,
    color: vars.colorBase,
    paddingTop: 4,
    paddingBottom: 4,
    paddingLeft: 4,
    paddingRight: 4,
    cursor: 'pointer',
    textAlign: 'left',
    borderRadius: 4,
  },
  railFoot: {
    paddingTop: 8,
    fontFamily: font.mono,
    fontSize: 9.5,
    color: vars.colorFaint,
  },
})
