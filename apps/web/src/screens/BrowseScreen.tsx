import * as stylex from '@stylexjs/stylex'
import { useHotkey } from '@tanstack/react-hotkeys'
// No IconSymbols/IconTree in ../ui/icons — missing glyphs come from Phosphor
// at the call site (the same set the icon wrappers use, cf. BranchesScreen).
import { TreeStructure, UserList } from '@phosphor-icons/react'
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { useLocation, useNavigate, useParams, useSearchParams } from 'react-router'
import { api, describeError, type DefinitionHit, type FileContent, type FileListEntry } from '../api'
import { RepoOverview } from '../components/RepoOverview'
import { prewarmHighlight } from '../lib/highlight'
import { fileUrl, queryUrl } from '../lib/navigation'
import { CodePane } from '../lib/snippet'
import { useAppStore } from '../lib/store'
import { MOCK_SYMBOLS, type MockSymbolKind } from '../mock/symbols'
import { MOCK_BRANCHES, MOCK_TAGS } from '../mock/branches'
import { blameFor, type BlameLine } from '../mock/blame'
import { MOCK_COMMITS } from '../mock/commits'
import { ActionIconButton } from '../ui/ActionIconButton'
import { CopyButton } from '../ui/CopyButton'
import { DisplayBadge } from '../ui/DisplayBadge'
import { DisplayBytes, DisplayTimeAgo } from '../ui/DisplayNumber'
import { DisplayTree } from '../ui/DisplayTree'
import type { TreeNode } from '../ui/tree'
import { FeedbackEmptyState, FeedbackLoading } from '../ui/FeedbackStates'
import { FeedbackTip } from '../ui/FeedbackTip'
import { FormSearchField } from '../ui/FormSearchField'
import {
  IconCaretDown,
  IconCheckSmall,
  IconTag,
  IconDownload,
  IconGitBranch,
  IconFile,
  IconFolderOpen,
  IconHistoryClock,
  IconWrap,
} from '../ui/icons'
import { LayoutBreadcrumb } from '../ui/LayoutStructure'
import { popup } from '../ui/recipes.stylex'
import { OverlayDropdown, OverlayDropdownItem, OverlayDropdownSeparator } from '../ui/OverlayMenu'
import { font, vars } from '../ui/tokens.stylex'

/**
 * Browse — the repository file browser. Both the tree and the file view
 * are real endpoints (`/v1/files`, `/v1/file`) over the committed
 * snapshot. The file view carries Sourcegraph's blob right rail — a
 * collapsible Symbols outline + file History toggled from the header.
 * Both panels are fixture-derived (`src/mock/`, no `/v1/symbols` or
 * `/v1/commits` yet) so each section header carries a `preview` tag —
 * fixture rows are never presented as live index or git truth.
 */
export function BrowseScreen() {
  // The URL is the selection: `/browse/<path>?L=<line>` — tree clicks,
  // deep links, refresh, and back/forward all resolve to the same state.
  const params = useParams()
  const [searchParams] = useSearchParams()
  const location = useLocation()
  const navigate = useNavigate()
  const selected = params['*'] || undefined
  const lineParam = searchParams.get('L')
  // `?rev=<branch>` — Sourcegraph's rev picker. The index holds HEAD only, so
  // the picker switches the label and the banner says the content is HEAD;
  // `rev` is dropped for the default branch (bare URL = HEAD = main).
  const revParam = searchParams.get('rev')
  const rev = revParam ?? 'main'
  // `?L=7` → [7,7]; `?L=4-9` → [4,9] — shift-click ranges, Sourcegraph parity.
  const anchor = useMemo<readonly [number, number] | undefined>(() => {
    const m = /^(\d+)(?:-(\d+))?$/.exec(lineParam ?? '')
    if (!m) return undefined
    const a = Number(m[1])
    const b = m[2] ? Number(m[2]) : a
    return [Math.min(a, b), Math.max(a, b)] as const
  }, [lineParam])
  const targetLine = anchor?.[0]

  const [filter, setFilter] = useState('')
  const [file, setFile] = useState<FileContent>()
  const [fileError, setFileError] = useState<string>()
  const [fileLoading, setFileLoading] = useState(false)
  const [pendingLine, setPendingLine] = useState<number>()
  const [flashRange, setFlashRange] = useState<readonly [number, number]>()
  const [wrap, setWrap] = useState(false)
  // Blame gutter — opt-in fixture annotations (mock/blame.ts), never live
  // git truth; the note under the header says so while it's on.
  const [blame, setBlame] = useState(false)
  // The right rail is closed by default — the header's Symbols/History
  // toggles open it; each enabled section keeps its own collapse state.
  const [rail, setRail] = useState({ symbols: false, history: false })
  const pushRecentFile = useAppStore((s) => s.pushRecentFile)
  const files = useAppStore((s) => s.files)
  const setFiles = useAppStore((s) => s.setFiles)
  const [listError, setListError] = useState<string>()

  const select = useCallback(
    (path: string) => navigate(fileUrl(path)),
    [navigate],
  )

  const pickRev = useCallback(
    (name: string) => {
      const q = new URLSearchParams(searchParams)
      if (name === 'main') q.delete('rev')
      else q.set('rev', name)
      navigate({ pathname: location.pathname, search: q.size ? `?${q}` : '' })
    },
    [navigate, searchParams, location.pathname],
  )

  // Deterministic fixture blame for the open file — memoized per path so
  // re-renders never reshuffle authorship (mock/blame.ts seeds by path).
  const blameLines = useMemo(() => {
    if (!blame || !selected || !file || file.binary) return undefined
    return blameFor(selected, file.content.split('\n').length)
  }, [blame, selected, file])

  // ── Code-intelligence hover — REAL /v1/def + /v1/refs (Sourcegraph's
  // signature feature). The word under the cursor is extracted via
  // caretRangeFromPoint, definitions resolve through the engine call graph
  // (session-cached per word), and the tooltip only appears when the graph
  // actually knows the symbol — hover stays silent otherwise.
  const [ci, setCi] = useState<{
    word: string
    x: number
    y: number
    defs: DefinitionHit[]
    refs: number | null
  } | null>(null)
  const ciCache = useRef(new Map<string, DefinitionHit[]>())
  const ciTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const ciClose = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)

  const dismissCi = useCallback((delay = 0) => {
    clearTimeout(ciClose.current)
    ciClose.current = setTimeout(() => setCi(null), delay)
  }, [])

  const onCodeHover = useCallback(
    (e: React.MouseEvent) => {
      const hit = wordAt(e.clientX, e.clientY)
      if (!hit || hit.word === ci?.word) {
        if (!hit) dismissCi(120)
        return
      }
      clearTimeout(ciTimer.current)
      clearTimeout(ciClose.current)
      ciTimer.current = setTimeout(() => {
        const { word, rect } = hit
        const cached = ciCache.current.get(word)
        const show = (defs: DefinitionHit[]) => {
          if (!defs.length) return
          setCi({ word, x: rect.left, y: rect.bottom + 6, defs, refs: null })
          api
            .references(word)
            .then((r) =>
              setCi((c) =>
                c != null && c.word === word ? { ...c, refs: r.references.length } : c,
              ),
            )
            .catch(() => {})
        }
        if (cached !== undefined) return show(cached)
        api
          .definitions(word)
          .then((r) => {
            ciCache.current.set(word, r.definitions)
            show(r.definitions)
          })
          .catch(() => ciCache.current.set(word, []))
      }, 140)
    },
    [ci?.word, dismissCi],
  )

  // Escape + navigation close the tooltip.
  useHotkey('Escape', () => setCi(null), {
    enabled: ci != null,
    conflictBehavior: 'allow',
  })
  useEffect(() => setCi(null), [selected])
  // Both timers die with the screen — a pending fire after unmount would
  // set state on a dead component.
  useEffect(
    () => () => {
      clearTimeout(ciTimer.current)
      clearTimeout(ciClose.current)
    },
    [],
  )

  // Recently-viewed — feeds Home's "jump back in" row.
  useEffect(() => {
    if (selected) pushRecentFile(selected)
  }, [selected, pushRecentFile])

  // Every navigation — including a repeated link to the same file:line —
  // re-arms the line flash (location.key changes on each history entry).
  useEffect(() => {
    setFilter('')
    setFlashRange(undefined)
    setPendingLine(targetLine)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [location.key])

  // Line targeting — the span only exists once CodePane mounts, so scroll here.
  useEffect(() => {
    if (!file || pendingLine == null) return
    const line = pendingLine
    setPendingLine(undefined)
    document.querySelector(`[data-line="${line}"]`)?.scrollIntoView({ block: 'center' })
    setFlashRange(anchor)
    const t = setTimeout(() => setFlashRange(undefined), 2800)
    return () => clearTimeout(t)
  }, [file, pendingLine, anchor])

  useEffect(() => {
    if (files) return // shared list already warm — palette fetched it earlier
    let cancelled = false
    api
      .files()
      .then((report) => {
        if (!cancelled) setFiles(report.files)
      })
      .catch((value) => {
        if (!cancelled) setListError(describeError(value))
      })
    return () => {
      cancelled = true
    }
  }, [files, setFiles])

  useEffect(() => {
    if (!selected) return
    let cancelled = false
    setFileLoading(true)
    setFileError(undefined)
    api
      .file(selected)
      .then(async (content) => {
        // Highlight before mount — the pane's first paint is already colored.
        await prewarmHighlight(content.content, content.language)
        if (!cancelled) setFile(content)
      })
      .catch((value) => {
        if (!cancelled) {
          setFile(undefined)
          setFileError(describeError(value))
        }
      })
      .finally(() => {
        if (!cancelled) setFileLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [selected])

  const filtered = useMemo(() => {
    if (!files) return []
    const needle = filter.trim().toLowerCase()
    return needle ? files.filter((f) => f.path.toLowerCase().includes(needle)) : files
  }, [files, filter])

  // Rail data — fixture lookups keyed to the open path. Most files have
  // neither symbols nor commits; the sections say so honestly instead of
  // fabricating rows. Symbols sort by position like a real outline.
  const fileSymbols = useMemo(
    () =>
      selected
        ? MOCK_SYMBOLS.filter((s) => s.path === selected).sort((a, b) => a.line - b.line)
        : [],
    [selected],
  )
  const fileCommits = useMemo(
    () =>
      selected
        ? MOCK_COMMITS.filter((c) => c.filesChanged.some((f) => f.path === selected))
        : [],
    [selected],
  )

  // The permalink — published to the store so the global `y` hotkey copies
  // this canonical URL (path + ?L= anchor, no session params).
  const setPermalink = useAppStore((s) => s.setPermalink)
  const permalink = selected
    ? `${window.location.origin}${fileUrl(selected, anchor)}`
    : ''
  useEffect(() => {
    setPermalink(permalink || undefined)
    return () => setPermalink(undefined)
  }, [permalink, setPermalink])

  return (
    <div {...stylex.props(styles.root)}>
      <aside {...stylex.props(styles.rail)}>
        <div {...stylex.props(styles.railRev)}>
          <RevPicker rev={rev} onPick={pickRev} />
        </div>
        {rev !== 'main' && (
          <p {...stylex.props(styles.revNote)}>
            viewing '{rev}' — content is HEAD; per-rev browsing pending
          </p>
        )}
        <div {...stylex.props(styles.railSearch)}>
          <FormSearchField
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="filter paths"
            onClear={() => setFilter('')}
            aria-label="Filter paths"
          />
        </div>
        <div {...stylex.props(styles.tree)}>
          {listError && <FeedbackTip variant="error">{listError}</FeedbackTip>}
          {!files && !listError && <FeedbackLoading label="Listing snapshot files" />}
          {files && filtered.length === 0 && (
            <p {...stylex.props(styles.empty)}>no paths match the filter</p>
          )}
          {filtered.length > 0 && (
            <DisplayTree
              items={filtered}
              getPath={(f) => f.path}
              fileIcons
              leaf={(node) => (
                <FileLeaf node={node} selected={selected} onSelect={select} />
              )}
            />
          )}
        </div>
      </aside>

      <section {...stylex.props(styles.viewer)}>
        {!selected &&
          (files && files.length > 0 ? (
            <RepoOverview files={files} />
          ) : (
            <FeedbackEmptyState
              icon={<IconFolderOpen size={20} />}
              title="Pick a file"
              description={`${files?.length ?? '…'} files in the committed snapshot`}
            />
          ))}
        {selected && (
          <>
            <header {...stylex.props(styles.fileHead)}>
              <LayoutBreadcrumb
                items={selected.split('/').map((segment, i, all) => ({
                  label: segment,
                  onClick:
                    i < all.length - 1
                      ? () => setFilter(`${all.slice(0, i + 1).join('/')}/`)
                      : undefined,
                }))}
              />
              {file && (
                <span {...stylex.props(styles.fileMeta)}>
                  {file.language && <DisplayBadge color={false}>{file.language}</DisplayBadge>}
                  <DisplayBytes value={file.content.length} />
                  <CopyButton text={selected} title="Copy path" label="Copy path" size={13} />
                  <ActionIconButton
                    tooltip="Download raw file"
                    label="Download raw file"
                    onClick={() => {
                      const blob = new Blob([file.content], { type: 'text/plain' })
                      const a = document.createElement('a')
                      a.href = URL.createObjectURL(blob)
                      a.download = selected.split('/').pop() ?? 'file'
                      a.click()
                      URL.revokeObjectURL(a.href)
                    }}
                    icon={<IconDownload size={13} />}
                  />
                  <ActionIconButton
                    tooltip={wrap ? 'Disable line wrap' : 'Wrap long lines'}
                    label="Toggle line wrap"
                    active={wrap}
                    onClick={() => setWrap((w) => !w)}
                    icon={<IconWrap size={13} />}
                  />
                  <ActionIconButton
                    tooltip={blame ? 'Hide blame annotations' : 'Show blame annotations'}
                    label="Toggle blame annotations"
                    active={blame}
                    onClick={() => setBlame((b) => !b)}
                    icon={<UserList size={13} />}
                  />
                  <ActionIconButton
                    tooltip={rail.symbols ? 'Hide symbols panel' : 'Show symbols panel'}
                    label="Toggle symbols panel"
                    active={rail.symbols}
                    onClick={() => setRail((r) => ({ ...r, symbols: !r.symbols }))}
                    icon={<TreeStructure size={13} />}
                  />
                  <ActionIconButton
                    tooltip={rail.history ? 'Hide file history panel' : 'Show file history panel'}
                    label="Toggle file history panel"
                    active={rail.history}
                    onClick={() => setRail((r) => ({ ...r, history: !r.history }))}
                    icon={<IconHistoryClock size={13} />}
                  />
                  <CopyButton
                    text={permalink}
                    title="Copy permalink (y)"
                    label="Copy permalink"
                    size={13}
                  />
                </span>
              )}
            </header>
            {fileLoading && <FeedbackLoading label={`Reading ${selected}`} />}
            {fileError && <FeedbackTip variant="error">{fileError}</FeedbackTip>}
            {file && !fileLoading && (
              <div {...stylex.props(styles.fileRow)}>
                <div
                  {...stylex.props(styles.codeWrap)}
                  onMouseMove={onCodeHover}
                  onMouseLeave={() => dismissCi(200)}
                >
                  {blame && (
                    <div {...stylex.props(styles.blameNote)}>
                      blame annotations are fixture — /v1/blame pending
                    </div>
                  )}
                  {file.truncated && (
                    <FeedbackTip variant="warning" title="Truncated">
                      the daemon capped this file's content
                    </FeedbackTip>
                  )}
                  {file.binary ? (
                    <FeedbackEmptyState
                      icon={<IconFile size={20} />}
                      title="Binary file"
                      description="The daemon flagged this as binary — no text view."
                    />
                  ) : (
                    <CodePane
                      content={file.content}
                      language={file.language}
                      highlightRange={flashRange}
                      selectedRange={anchor}
                      wrap={wrap}
                      lineGutter={
                        blameLines
                          ? (n) => <BlameCell line={blameLines[n - 1]} prev={blameLines[n - 2]} />
                          : undefined
                      }
                      onLineClick={(n, shift) =>
                        navigate(
                          shift && anchor
                            ? fileUrl(selected!, [anchor[0], n])
                            : fileUrl(selected!, n),
                        )
                      }
                    />
                  )}
                </div>
                {(rail.symbols || rail.history) && (
                  <aside {...stylex.props(styles.sideRail)} aria-label="File panels">
                    {rail.symbols && (
                      <RailSection
                        icon={<TreeStructure size={11} />}
                        title="Symbols"
                        count={fileSymbols.length}
                        badge={
                          <DisplayBadge
                            severity="medium"
                            title="fixture data — the /v1/symbols endpoint is pending"
                          >
                            preview
                          </DisplayBadge>
                        }
                      >
                        {fileSymbols.length === 0 ? (
                          <p {...stylex.props(styles.railEmpty)}>
                            no symbols indexed for this file
                          </p>
                        ) : (
                          fileSymbols.map((s) => (
                            <button
                              key={s.qualifiedName}
                              type="button"
                              title={`${s.qualifiedName}:${s.line}`}
                              onClick={() => navigate(fileUrl(selected, s.line))}
                              {...stylex.props(styles.symRow)}
                            >
                              <span {...stylex.props(styles.symKind)} title={s.kind}>
                                {KIND_TAG[s.kind]}
                              </span>
                              <span {...stylex.props(styles.symName)}>{s.name}</span>
                              <span {...stylex.props(styles.symLine)}>:{s.line}</span>
                            </button>
                          ))
                        )}
                      </RailSection>
                    )}
                    {rail.history && (
                      <RailSection
                        icon={<IconHistoryClock size={11} />}
                        title="History"
                        count={fileCommits.length}
                        badge={
                          <DisplayBadge
                            severity="medium"
                            title="fixture commits — git history is not wired to the index yet"
                          >
                            preview
                          </DisplayBadge>
                        }
                      >
                        {fileCommits.length === 0 ? (
                          <p {...stylex.props(styles.railEmpty)}>
                            no fixture commits touch this file
                          </p>
                        ) : (
                          fileCommits.map((c) => (
                            <button
                              key={c.sha}
                              type="button"
                              title={`${c.message} — opens the commits preview`}
                              onClick={() => navigate('/commits')}
                              {...stylex.props(styles.histRow)}
                            >
                              <span {...stylex.props(styles.histTop)}>
                                <code {...stylex.props(styles.sha)} title={c.sha}>
                                  {c.sha.slice(0, 8)}
                                </code>
                                <span {...stylex.props(styles.histMsg)}>{c.message}</span>
                              </span>
                              <span {...stylex.props(styles.histMeta)}>
                                <span>{c.author}</span>
                                <DisplayTimeAgo value={c.at} />
                              </span>
                            </button>
                          ))
                        )}
                      </RailSection>
                    )}
                  </aside>
                )}
              </div>
            )}
          </>
        )}
      </section>
      {ci && (
        <div
          {...stylex.props(popup.surface, styles.ciTip)}
          style={{ left: Math.min(ci.x, window.innerWidth - 330), top: ci.y }}
          onMouseEnter={() => clearTimeout(ciClose.current)}
          onMouseLeave={() => dismissCi(0)}
          role="tooltip"
        >
          <div {...stylex.props(styles.ciHead)}>
            <code {...stylex.props(styles.ciWord)}>{ci.word}</code>
            {ci.defs[0]?.kind && <DisplayBadge color={false}>{ci.defs[0].kind}</DisplayBadge>}
            <DisplayBadge severity="low" title="live — engine call graph">
              live
            </DisplayBadge>
          </div>
          {ci.defs.slice(0, 2).map((d, i) => (
            <button
              key={`${d.address?.path ?? ''}:${i}`}
              type="button"
              {...stylex.props(styles.ciRow)}
              onClick={() => {
                if (d.address) navigate(fileUrl(d.address.path, d.address.startLine))
                setCi(null)
              }}
            >
              <span {...stylex.props(styles.ciDef)}>
                {d.qualifiedName ?? d.name}
              </span>
              {d.address && (
                <span {...stylex.props(styles.ciSite)}>
                  {d.address.path}:{d.address.startLine}
                </span>
              )}
            </button>
          ))}
          <div {...stylex.props(styles.ciFoot)}>
            <button
              type="button"
              {...stylex.props(styles.ciLink)}
              onClick={() => {
                navigate(queryUrl(`type:symbol ${ci.word}`))
                setCi(null)
              }}
            >
              find references{ci.refs != null ? ` (${ci.refs})` : ''}
            </button>
            <span {...stylex.props(styles.ciHint)}>esc closes</span>
          </div>
        </div>
      )}
    </div>
  )
}

function FileLeaf({
  node,
  selected,
  onSelect,
}: {
  node: TreeNode<FileListEntry>
  selected?: string
  onSelect: (path: string) => void
}) {
  const on = node.path === selected
  const ref = useRef<HTMLButtonElement>(null)
  useEffect(() => {
    if (on) ref.current?.scrollIntoView({ block: 'nearest' })
  }, [on])
  return (
    <button
      ref={ref}
      type="button"
      aria-current={on || undefined}
      onClick={() => onSelect(node.path)}
      {...stylex.props(styles.leaf, on && styles.leafOn)}
    >
      {node.name}
    </button>
  )
}

/** Compact kind tag for outline rows — `title` on the chip gives the full kind. */
const KIND_TAG: Record<MockSymbolKind, string> = {
  function: 'fn',
  struct: 'st',
  trait: 'tr',
  module: 'mod',
  constant: 'cn',
  component: 'cp',
  type: 'ty',
}

/**
 * One collapsible rail section — a caret header (label + row count +
 * `preview` fixture tag) over the rows. `open` is local: toggling the
 * section off and on from the file header re-mounts it expanded.
 */
function RailSection({
  icon,
  title,
  count,
  badge,
  children,
}: {
  icon: ReactNode
  title: string
  count: number
  badge?: ReactNode
  children: ReactNode
}) {
  const [open, setOpen] = useState(true)
  return (
    <section {...stylex.props(styles.railSection)}>
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
        {...stylex.props(styles.railHead)}
      >
        <span {...stylex.props(styles.railCaret, open && styles.railCaretOpen)}>
          <IconCaretDown size={10} />
        </span>
        {icon}
        <span>{title}</span>
        <span {...stylex.props(styles.railCount)}>{count}</span>
        <span {...stylex.props(styles.railSpacer)} />
        {badge}
      </button>
      {open && <div {...stylex.props(styles.railBody)}>{children}</div>}
    </section>
  )
}

/**
 * Identifiers the hover skips — language keywords + anything under 3 chars
 * would spam the engine for nothing.
 */
const CI_KEYWORDS = new Set(
  'as async await break case catch class const continue do else enum export finally fn for from function if impl import in interface let match mod new of pub return self static struct switch this throw try type undefined use var while yield true false null'.split(
    ' ',
  ),
)

/**
 * The word under a viewport point — `caretRangeFromPoint` finds the text
 * node, then the identifier expands around the caret offset. Returns the
 * word plus its bounding rect for tooltip placement, or null when the
 * point isn't on a meaningful identifier.
 */
function wordAt(x: number, y: number): { word: string; rect: DOMRect } | null {
  const caret = (
    document as Document & { caretRangeFromPoint?: (x: number, y: number) => Range }
  ).caretRangeFromPoint?.(x, y)
  const node = caret?.startContainer
  if (!caret || !node || node.nodeType !== Node.TEXT_NODE) return null
  const text = (node as Text).data
  const ident = (c: string) => /[A-Za-z0-9_$]/.test(c)
  let s = caret.startOffset
  let e = caret.startOffset
  while (s > 0 && ident(text[s - 1]!)) s--
  while (e < text.length && ident(text[e]!)) e++
  const word = text.slice(s, e)
  if (word.length < 3 || CI_KEYWORDS.has(word)) return null
  const range = document.createRange()
  range.setStart(node, s)
  range.setEnd(node, e)
  return { word, rect: range.getBoundingClientRect() }
}

/**
 * One blame-gutter cell — the block's first line shows `sha8 author`;
 * continuation lines get a bare tick. `tone` fades with commit age so
 * recent work reads brighter than history (GitHub's age gradient).
 */
function BlameCell({ line, prev }: { line?: BlameLine; prev?: BlameLine }) {
  if (!line) return <span {...stylex.props(styles.blameCell)} />
  const first = !prev || prev.sha !== line.sha || prev.author !== line.author
  const weeks = (Date.now() - new Date(line.at).getTime()) / (7 * 24 * 3600 * 1000)
  const tone = weeks < 1 ? styles.blameT0 : weeks < 6 ? styles.blameT1 : styles.blameT2
  return (
    <span
      {...stylex.props(styles.blameCell, tone)}
      title={`${line.sha.slice(0, 8)} ${line.author} — ${line.at.slice(0, 10)}`}
    >
      {first ? `${line.sha.slice(0, 7)} ${line.author}` : '┊'}
    </span>
  )
}

/**
 * Rev picker — Sourcegraph's `@ branch` selector over the tree. Picking a
 * ref rewrites `?rev=`; the index holds HEAD only, so non-default picks
 * show the honest banner instead of pretending to re-resolve content.
 */
function RevPicker({ rev, onPick }: { rev: string; onPick: (name: string) => void }) {
  const [open, setOpen] = useState(false)
  return (
    <OverlayDropdown
      open={open}
      onOpenChange={setOpen}
      align="start"
      trigger={
        <button
          type="button"
          aria-label={`Revision: ${rev}`}
          title="Revision — the index holds HEAD only"
          {...stylex.props(styles.revTrigger)}
        >
          {MOCK_TAGS.some((t) => t.name === rev) ? (
            <IconTag size={11} />
          ) : (
            <IconGitBranch size={11} />
          )}
          <span {...stylex.props(styles.revName)}>{rev}</span>
          <span {...stylex.props(styles.revCaret, open && styles.revCaretOpen)}>
            <IconCaretDown size={10} />
          </span>
        </button>
      }
    >
      {MOCK_BRANCHES.map((b) => (
        <OverlayDropdownItem
          key={b.name}
          icon={<IconGitBranch size={12} />}
          onClick={() => onPick(b.name)}
        >
          <span {...stylex.props(styles.revRow)}>
            <span {...stylex.props(styles.revItemName)}>{b.name}</span>
            {b.isDefault && <DisplayBadge color={false}>default</DisplayBadge>}
            <code {...stylex.props(styles.revSha)}>{b.headSha.slice(0, 8)}</code>
            <span {...stylex.props(styles.revCheck)}>
              {rev === b.name && <IconCheckSmall size={11} />}
            </span>
          </span>
        </OverlayDropdownItem>
      ))}
      {MOCK_TAGS.length > 0 && (
        <>
          <OverlayDropdownSeparator />
          {MOCK_TAGS.map((t) => (
            <OverlayDropdownItem
              key={t.name}
              icon={<IconTag size={12} />}
              onClick={() => onPick(t.name)}
            >
              <span {...stylex.props(styles.revRow)}>
                <span {...stylex.props(styles.revItemName)}>{t.name}</span>
                {t.release && <DisplayBadge severity="low">latest</DisplayBadge>}
                <code {...stylex.props(styles.revSha)}>{t.sha}</code>
                <span {...stylex.props(styles.revCheck)}>
                  {rev === t.name && <IconCheckSmall size={11} />}
                </span>
              </span>
            </OverlayDropdownItem>
          ))}
        </>
      )}
      <OverlayDropdownSeparator />
      <div {...stylex.props(styles.revMenuNote)}>
        rev switching pending — the index holds HEAD only
      </div>
    </OverlayDropdown>
  )
}

const styles = stylex.create({
  root: {
    display: 'flex',
    gap: 14,
    alignItems: 'flex-start',
  },
  rail: {
    width: 230,
    flexShrink: 0,
    position: 'sticky',
    top: 0,
    display: 'flex',
    flexDirection: 'column',
    gap: 8,
  },
  railRev: {
    display: 'flex',
  },
  revTrigger: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 6,
    width: '100%',
    paddingTop: 5,
    paddingBottom: 5,
    paddingLeft: 8,
    paddingRight: 8,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderMute,
    borderRadius: 4,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    color: vars.colorMuted,
    fontFamily: 'inherit',
    fontSize: 11,
    cursor: 'pointer',
  },
  revName: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    color: vars.colorBase,
    flex: 1,
    minWidth: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
    textAlign: 'left',
  },
  revCaret: {
    display: 'inline-flex',
    color: vars.colorFaint,
    transform: 'rotate(-90deg)',
    transitionProperty: 'transform',
    transitionDuration: '120ms',
  },
  revCaretOpen: {
    transform: 'rotate(0deg)',
  },
  revNote: {
    margin: 0,
    fontSize: 10,
    lineHeight: 1.5,
    color: vars.scaleHigh,
    paddingTop: 5,
    paddingBottom: 5,
    paddingLeft: 8,
    paddingRight: 8,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderMute,
    borderRadius: 4,
    backgroundColor: vars.bgSecondary,
  },
  revRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 7,
    flex: 1,
    minWidth: 0,
  },
  revItemName: {
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorBase,
    whiteSpace: 'nowrap',
  },
  revSha: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
    marginLeft: 'auto',
  },
  revCheck: {
    display: 'inline-flex',
    width: 12,
    color: vars.colorActive,
    flexShrink: 0,
  },
  revMenuNote: {
    paddingTop: 5,
    paddingBottom: 4,
    paddingLeft: 10,
    paddingRight: 10,
    fontSize: 10,
    color: vars.colorFaint,
  },
  railSearch: {
    flexShrink: 0,
  },
  // CI tooltip — fixed-position card anchored under the hovered word.
  ciTip: {
    position: 'fixed',
    zIndex: 60,
    display: 'flex',
    flexDirection: 'column',
    gap: 2,
    minWidth: 220,
    maxWidth: 320,
    paddingTop: 6,
    paddingBottom: 6,
    paddingLeft: 8,
    paddingRight: 8,
  },
  ciHead: {
    display: 'flex',
    alignItems: 'center',
    gap: 6,
    paddingBottom: 4,
  },
  ciWord: {
    fontFamily: font.mono,
    fontSize: 12,
    fontWeight: 600,
    color: vars.colorBase,
  },
  ciRow: {
    display: 'flex',
    flexDirection: 'column',
    gap: 1,
    width: '100%',
    paddingTop: 4,
    paddingBottom: 4,
    paddingLeft: 6,
    paddingRight: 6,
    borderWidth: 0,
    borderRadius: 4,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    fontFamily: 'inherit',
    textAlign: 'left',
    cursor: 'pointer',
  },
  ciDef: {
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorBase,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  ciSite: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorActive,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  ciFoot: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    marginTop: 4,
    paddingTop: 5,
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
  },
  ciLink: {
    fontSize: 11,
    color: vars.colorActive,
    backgroundColor: 'transparent',
    borderWidth: 0,
    padding: 0,
    cursor: 'pointer',
    fontFamily: 'inherit',
  },
  ciHint: {
    marginLeft: 'auto',
    fontSize: 10,
    color: vars.colorFaint,
  },
  blameNote: {
    fontSize: 10,
    color: vars.colorFaint,
    paddingTop: 4,
    paddingBottom: 4,
    paddingLeft: 8,
    paddingRight: 8,
    borderBottomWidth: 1,
    borderBottomStyle: 'solid',
    borderBottomColor: vars.borderMute,
  },
  blameCell: {
    display: 'inline-block',
    width: 96,
    overflow: 'hidden',
    whiteSpace: 'nowrap',
    textOverflow: 'ellipsis',
    fontFamily: font.mono,
    fontSize: 9,
    lineHeight: 1.6,
    color: vars.colorFaint,
    textAlign: 'left',
    paddingRight: 6,
  },
  blameT0: {
    color: vars.colorMuted,
  },
  blameT1: {
    color: vars.colorFaint,
  },
  blameT2: {
    color: vars.colorFaint,
    opacity: vars.opFade,
  },
  tree: {
    maxHeight: 'calc(100vh - 170px)',
    overflowY: 'auto',
    overflowX: 'hidden',
  },
  empty: {
    margin: 0,
    padding: '6px 8px',
    fontSize: 12,
    color: vars.colorFaint,
  },
  leaf: {
    flex: 1,
    minWidth: 0,
    borderWidth: 0,
    backgroundColor: 'transparent',
    fontFamily: 'inherit',
    fontSize: 'inherit',
    color: 'inherit',
    textAlign: 'left',
    cursor: 'pointer',
    padding: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
    borderRadius: 3,
  },
  leafOn: {
    color: vars.colorActive,
    fontWeight: 600,
  },
  viewer: {
    flex: 1,
    minWidth: 0,
    display: 'flex',
    flexDirection: 'column',
    gap: 10,
  },
  fileHead: {
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: 10,
    flexWrap: 'wrap',
  },
  fileMeta: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 8,
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorMuted,
  },
  // Code card + optional right rail — the card keeps flex:1 so the rail's
  // fixed 220px never squeezes it below readable width.
  fileRow: {
    display: 'flex',
    gap: 10,
    alignItems: 'flex-start',
  },
  codeWrap: {
    flex: 1,
    minWidth: 0,
    borderRadius: 8,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgCode,
    overflow: 'hidden',
    display: 'flex',
    flexDirection: 'column',
  },
  // ── Blob right rail (Symbols / History) ──────────────────────────────
  sideRail: {
    width: 220,
    flexShrink: 0,
    position: 'sticky',
    top: 0,
    maxHeight: 'calc(100vh - 170px)',
    overflowY: 'auto',
    display: 'flex',
    flexDirection: 'column',
    gap: 8,
    borderLeftWidth: 1,
    borderLeftStyle: 'solid',
    borderLeftColor: vars.borderMute,
    paddingLeft: 10,
  },
  railSection: {
    display: 'flex',
    flexDirection: 'column',
  },
  railHead: {
    display: 'flex',
    alignItems: 'center',
    gap: 6,
    borderWidth: 0,
    backgroundColor: 'transparent',
    fontFamily: 'inherit',
    fontSize: 11,
    fontWeight: 600,
    color: vars.colorMuted,
    paddingTop: 4,
    paddingBottom: 4,
    paddingLeft: 2,
    paddingRight: 2,
    cursor: 'pointer',
    textAlign: 'left',
    borderRadius: 4,
  },
  railCaret: {
    display: 'inline-flex',
    color: vars.colorFaint,
    flexShrink: 0,
    transform: 'rotate(-90deg)',
    transitionProperty: 'transform',
    transitionDuration: '120ms',
  },
  railCaretOpen: {
    transform: 'rotate(0deg)',
  },
  railCount: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    fontWeight: 400,
    color: vars.colorFaint,
  },
  railSpacer: {
    flex: 1,
  },
  railBody: {
    display: 'flex',
    flexDirection: 'column',
    paddingBottom: 6,
  },
  railEmpty: {
    margin: 0,
    paddingTop: 2,
    paddingBottom: 4,
    paddingLeft: 4,
    paddingRight: 2,
    fontSize: 11,
    color: vars.colorFaint,
  },
  symRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 6,
    borderWidth: 0,
    fontFamily: 'inherit',
    fontSize: 12,
    color: vars.colorBase,
    paddingTop: 4,
    paddingBottom: 4,
    paddingLeft: 4,
    paddingRight: 2,
    cursor: 'pointer',
    textAlign: 'left',
    borderRadius: 4,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
  },
  symKind: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 9,
    fontWeight: 600,
    lineHeight: '1.55',
    color: vars.colorFaint,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderMute,
    borderRadius: 3,
    paddingTop: 0,
    paddingBottom: 0,
    paddingLeft: 3,
    paddingRight: 3,
    minWidth: 16,
    textAlign: 'center',
    flexShrink: 0,
  },
  symName: {
    flex: 1,
    minWidth: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  symLine: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    color: vars.colorFaint,
    flexShrink: 0,
  },
  histRow: {
    display: 'flex',
    flexDirection: 'column',
    gap: 2,
    borderWidth: 0,
    fontFamily: 'inherit',
    color: vars.colorBase,
    paddingTop: 5,
    paddingBottom: 5,
    paddingLeft: 4,
    paddingRight: 2,
    cursor: 'pointer',
    textAlign: 'left',
    borderRadius: 4,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
  },
  histTop: {
    display: 'flex',
    alignItems: 'center',
    gap: 6,
    minWidth: 0,
  },
  // Same sha chip CommitsScreen uses — one idiom for commit identifiers.
  sha: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    color: vars.colorActive,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    borderRadius: 4,
    backgroundColor: vars.bgSunken,
    paddingTop: 1,
    paddingBottom: 1,
    paddingLeft: 5,
    paddingRight: 5,
    flexShrink: 0,
  },
  histMsg: {
    flex: 1,
    minWidth: 0,
    fontSize: 12,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  histMeta: {
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: 6,
    paddingLeft: 2,
    fontSize: 10,
    color: vars.colorFaint,
  },
})
