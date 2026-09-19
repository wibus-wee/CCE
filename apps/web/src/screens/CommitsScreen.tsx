import { ArrowsLeftRight, SealCheck } from '@phosphor-icons/react'
import * as stylex from '@stylexjs/stylex'
import { useMemo, useState } from 'react'
import { useNavigate } from 'react-router'
import { FacetRail } from '../components/FacetRail'
import { DiffStat, FileDiff } from '../components/FileDiff'
import { api, type SearchHit } from '../api'
import { emptyFilters, toggleFilter, type ActiveFilters, type FacetGroup } from '../lib/facets'
import { fileUrl } from '../lib/navigation'
import {
  MOCK_COMMITS,
  type MockCommit,
  type MockCommitFile,
} from '../mock/commits'
import { CopyButton } from '../ui/CopyButton'
import { DisplayBadge } from '../ui/DisplayBadge'
import { DisplayFilePath } from '../ui/DisplayFilePath'
import { DisplayTimeAgo } from '../ui/DisplayNumber'
import { FormSearchField } from '../ui/FormSearchField'
import {
  IconArrowUpRight,
  IconCaretDown,
  IconDiff,
  IconGitBranch,
  IconGitCommit,
  IconHistoryClock,
  IconX,
} from '../ui/icons'
import { iconButtons } from '../ui/recipes.stylex'
import { font, vars } from '../ui/tokens.stylex'

/** 'apps/web/src/x.tsx' -> 'apps' — the area facet buckets by path root. */
function topDir(path: string): string {
  return path.split('/')[0] ?? path
}

const DAY_FMT = new Intl.DateTimeFormat('en-US', { month: 'short', day: 'numeric' })

function dayLabel(at: string): string {
  return DAY_FMT.format(new Date(at))
}

function totals(c: MockCommit): { added: number; removed: number } {
  return c.filesChanged.reduce(
    (acc, f) => ({ added: acc.added + f.added, removed: acc.removed + f.removed }),
    { added: 0, removed: 0 },
  )
}

/** The working week the fixture opens on; older commits page in below. */
const FIRST_PAGE = 22
/** Commits appended per 'Load older commits' click. */
const PAGE = 8

/** 'Sep 18' -> 'day-sep-18' — anchor ids for the jump-to-date strip. */
function dayId(day: string): string {
  return `day-${day.toLowerCase().replace(/\s+/g, '-')}`
}

/**
 * Commits — Sourcegraph's repo commit list: a chronological `git log`
 * grouped by day. Each row is one commit — sha chip (+ copy button), subject,
 * author, files-changed count, +/− diffstat, relative time — and expands
 * inline to per-file rows whose mock unified diffs deep-link into Browse;
 * a trailing ↗ deep-links to the commit detail page (`/commit/<sha>`).
 * A collapsible contributors strip rolls the loaded slice up per author —
 * commits, +/−, a share-of-churn bar — each row toggling the same author
 * facet the rail drives. A compare bar merges two commits' file lists
 * (fixture union, labeled as such); a date strip jumps between day groups;
 * 'Load older' appends the fixture's older page.
 *
 * MOCK: `/v1/commits` doesn't exist and `git log` is not wired to the index
 * yet, so the data comes from `src/mock/commits.ts` and the screen carries a
 * `preview` badge — never presented as live repo truth.
 */
export function CommitsScreen() {
  const navigate = useNavigate()
  const [text, setText] = useState('')
  const [filters, setFilters] = useState<ActiveFilters>(emptyFilters())
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(() => new Set())
  const [limit, setLimit] = useState(FIRST_PAGE)
  const [baseSha, setBaseSha] = useState<string>()
  const [headSha, setHeadSha] = useState<string>()
  const [contribOpen, setContribOpen] = useState(false)

  // ── Patch search — REAL /v1/diff, not fixture. The commit list below is
  // preview data (no history-enumerate endpoint); this strip is the live
  // half of the screen: a regex over every stored commit patch, the same
  // thing Sourcegraph's `type:diff` search runs.
  const [patchQuery, setPatchQuery] = useState('')
  const [patchHits, setPatchHits] = useState<SearchHit[] | null>(null)
  const [patchBusy, setPatchBusy] = useState(false)
  const [patchError, setPatchError] = useState<string>()

  const runPatch = async () => {
    const pattern = patchQuery.trim()
    if (!pattern || patchBusy) return
    setPatchBusy(true)
    setPatchError(undefined)
    try {
      const res = await api.diff(pattern, 25)
      setPatchHits(res.hits)
    } catch (e) {
      setPatchHits(null)
      setPatchError(e instanceof Error ? e.message : 'diff search failed')
    } finally {
      setPatchBusy(false)
    }
  }

  /** The loaded slice — the working week first, older fixture commits append. */
  const visible = useMemo(() => MOCK_COMMITS.slice(0, limit), [limit])

  const groups: FacetGroup[] = useMemo(() => {
    const count = (pick: (c: MockCommit) => Iterable<string>) => {
      const m = new Map<string, number>()
      for (const c of visible) {
        for (const key of pick(c)) m.set(key, (m.get(key) ?? 0) + 1)
      }
      return [...m.entries()]
        .map(([value, n]) => ({ value, label: value, count: n }))
        .sort((a, b) => b.count - a.count || a.value.localeCompare(b.value))
    }
    return [
      { id: 'author', label: 'Author', options: count((c) => [c.author]) },
      {
        id: 'area',
        label: 'Area',
        // One count per commit per touched path root, not per file.
        options: count((c) => new Set(c.filesChanged.map((f) => topDir(f.path)))),
      },
    ]
  }, [visible])

  const rows = useMemo(() => {
    const needle = text.trim().toLowerCase()
    return visible.filter((c) => {
      if (needle && !`${c.sha} ${c.message} ${c.author}`.toLowerCase().includes(needle)) {
        return false
      }
      return Object.entries(filters).every(([group, values]) => {
        if (group === 'author') return values.has(c.author)
        if (group === 'area') return c.filesChanged.some((f) => values.has(topDir(f.path)))
        return true
      })
    })
  }, [text, filters, visible])

  /** Day groups in first-seen order — the fixture is already newest-first. */
  const days = useMemo(() => {
    const m = new Map<string, MockCommit[]>()
    for (const c of rows) {
      const label = dayLabel(c.at)
      const list = m.get(label)
      if (list) list.push(c)
      else m.set(label, [c])
    }
    return [...m.entries()]
  }, [rows])

  /**
   * Contributors — GitHub-style per-author rollup over the loaded slice
   * (`visible`, the same data the facet counts describe), sorted by commit
   * count. `last` is the newest commit `at` — ISO strings sort correctly —
   * and `churn` (added+removed) is the bar's proportion base.
   */
  const contributors = useMemo(() => {
    const m = new Map<
      string,
      { author: string; commits: number; added: number; removed: number; last: string }
    >()
    for (const c of visible) {
      const a = m.get(c.author) ?? {
        author: c.author,
        commits: 0,
        added: 0,
        removed: 0,
        last: c.at,
      }
      const t = totals(c)
      a.commits += 1
      a.added += t.added
      a.removed += t.removed
      if (c.at > a.last) a.last = c.at
      m.set(c.author, a)
    }
    return [...m.values()]
      .map((a) => ({ ...a, churn: a.added + a.removed }))
      .sort((x, y) => y.commits - x.commits || x.author.localeCompare(y.author))
  }, [visible])

  /** Churn ceiling — the busiest author's bar fills the track. */
  const contribMax = contributors.reduce((m, a) => Math.max(m, a.churn), 0) || 1

  /**
   * Compare pair — union of both commits' filesChanged with +/− summed
   * per path (`commits` marks files touched by both). This is a fixture
   * merge, not a real git diff; the drawer says so.
   */
  const compare = useMemo(() => {
    if (!baseSha || !headSha || baseSha === headSha) return undefined
    const base = MOCK_COMMITS.find((c) => c.sha === baseSha)
    const head = MOCK_COMMITS.find((c) => c.sha === headSha)
    if (!base || !head) return undefined
    const merged = new Map<string, { added: number; removed: number; commits: number }>()
    for (const c of [base, head]) {
      for (const f of c.filesChanged) {
        const m = merged.get(f.path) ?? { added: 0, removed: 0, commits: 0 }
        m.added += f.added
        m.removed += f.removed
        m.commits += 1
        merged.set(f.path, m)
      }
    }
    const files = [...merged.entries()].map(([path, m]) => ({ path, ...m }))
    return {
      base,
      head,
      files,
      added: files.reduce((n, f) => n + f.added, 0),
      removed: files.reduce((n, f) => n + f.removed, 0),
    }
  }, [baseSha, headSha])

  const toggle = (sha: string) =>
    setExpanded((prev) => {
      const next = new Set(prev)
      if (next.has(sha)) next.delete(sha)
      else next.add(sha)
      return next
    })

  return (
    <div {...stylex.props(styles.root)}>
      <header {...stylex.props(styles.head)}>
        <h2 {...stylex.props(styles.title)}>Commits</h2>
        <DisplayBadge severity="medium">preview</DisplayBadge>
        <DisplayBadge color={false} icon={<IconGitBranch size={10} />} title="default branch">
          main
        </DisplayBadge>
        <span {...stylex.props(styles.count)}>{rows.length === visible.length ? `${rows.length} commits` : `${rows.length} of ${visible.length} commits`}</span>
        <span {...stylex.props(styles.spacer)} />
        <div {...stylex.props(styles.search)}>
          <FormSearchField
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder="message, sha, or author"
            onClear={() => setText('')}
            aria-label="Filter commits"
          />
        </div>
      </header>

      <div {...stylex.props(styles.body)}>
        <FacetRail
          groups={groups}
          active={filters}
          onToggle={(g, v) => setFilters((f) => toggleFilter(f, g, v))}
        />
        <div {...stylex.props(styles.list)}>
          {/* ── Patch search — live /v1/diff over every stored commit patch.
              The list below stays fixture (no history-enumerate endpoint);
              the badge split keeps that honest. */}
          <div {...stylex.props(styles.patch)}>
            <div {...stylex.props(styles.patchBar)}>
              <IconDiff size={12} />
              <span {...stylex.props(styles.patchLabel)}>patch search</span>
              <DisplayBadge severity="low" title="live — queries the daemon, not the fixture">
                live
              </DisplayBadge>
              <span {...stylex.props(styles.patchNote)}>
                regex over stored commit patches — /v1/diff
              </span>
              <span {...stylex.props(styles.spacer)} />
              {patchHits && (
                <span {...stylex.props(styles.patchCount)}>
                  {patchHits.length} {patchHits.length === 1 ? 'hit' : 'hits'}
                </span>
              )}
            </div>
            <FormSearchField
              value={patchQuery}
              onChange={(e) => setPatchQuery(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') runPatch()
              }}
              placeholder="regex over patch lines — e.g. unsafe|TODO"
              onClear={() => {
                setPatchQuery('')
                setPatchHits(null)
              }}
              aria-label="Search stored commit patches"
            />
            {patchError && (
              <div {...stylex.props(styles.patchErr)}>patch search failed — {patchError}</div>
            )}
            {patchHits && patchHits.length === 0 && (
              <div {...stylex.props(styles.patchEmpty)}>
                no patch lines match — try a broader regex
              </div>
            )}
            {patchHits && patchHits.length > 0 && (
              <ul {...stylex.props(styles.patchList)}>
                {patchHits.map((h, i) => (
                  <PatchHit
                    key={`${h.documentId}-${i}`}
                    hit={h}
                    onOpen={(path, line) => navigate(fileUrl(path, line))}
                  />
                ))}
              </ul>
            )}
          </div>
          {/* ── Contributors — GitHub-style rollup of the loaded slice.
              A row toggles the same author facet the rail drives (aria-pressed
              mirrors filters.author); collapsed by default, fixture-labeled. */}
          <section {...stylex.props(styles.contrib)} aria-label="Contributors">
            <button
              type="button"
              aria-expanded={contribOpen}
              onClick={() => setContribOpen((o) => !o)}
              {...stylex.props(styles.contribHead)}
            >
              <span {...stylex.props(styles.caret, contribOpen && styles.caretOpen)}>
                <IconCaretDown size={10} />
              </span>
              <span {...stylex.props(styles.contribLabel)}>contributors</span>
              <DisplayBadge severity="medium">preview</DisplayBadge>
              <span {...stylex.props(styles.spacer)} />
              <span {...stylex.props(styles.contribCount)}>
                {contributors.length} {contributors.length === 1 ? 'author' : 'authors'}
              </span>
            </button>
            {contribOpen && (
              <>
                <div {...stylex.props(styles.contribList)} role="list">
                  {contributors.map((a) => {
                    const on = (filters.author ?? new Set<string>()).has(a.author)
                    return (
                      <button
                        key={a.author}
                        type="button"
                        role="listitem"
                        aria-pressed={on}
                        title={`${a.commits} commits — ${on ? 'remove author filter' : 'filter to this author'}`}
                        onClick={() => setFilters((f) => toggleFilter(f, 'author', a.author))}
                        {...stylex.props(styles.contribRow, on && styles.contribRowOn)}
                      >
                        <span {...stylex.props(styles.contribName)}>
                          {a.author.replaceAll('_', ' ')}
                        </span>
                        <span {...stylex.props(styles.contribCommits)}>
                          {a.commits} commit{a.commits === 1 ? '' : 's'}
                        </span>
                        <span {...stylex.props(styles.diff)}>
                          <span {...stylex.props(styles.diffNum, styles.add)}>+{a.added}</span>
                          <span {...stylex.props(styles.diffNum, styles.del)}>−{a.removed}</span>
                        </span>
                        <span {...stylex.props(styles.contribTrack)} aria-hidden>
                          <span
                            {...stylex.props(styles.contribFill, on && styles.contribFillOn)}
                            style={{ width: `${Math.max(4, (a.churn / contribMax) * 100)}%` }}
                          />
                        </span>
                        <span {...stylex.props(styles.contribWhen)}>
                          last active <DisplayTimeAgo value={a.last} />
                        </span>
                      </button>
                    )
                  })}
                </div>
                <p {...stylex.props(styles.contribNote)}>
                  aggregated from the loaded fixture slice
                </p>
              </>
            )}
          </section>
          <div {...stylex.props(styles.listBar)}>
            <span {...stylex.props(styles.cmpLabel)} title="compare two fixture commits">
              <IconDiff size={11} />
              compare
            </span>
            <select
              value={baseSha ?? ''}
              onChange={(e) => setBaseSha(e.target.value || undefined)}
              aria-label="base commit"
              {...stylex.props(styles.cmpSelect)}
            >
              <option value="">base…</option>
              {MOCK_COMMITS.map((c) => (
                <option key={c.sha} value={c.sha} disabled={c.sha === headSha}>
                  {c.sha.slice(0, 8)} {c.message}
                </option>
              ))}
            </select>
            <span {...stylex.props(styles.cmpDots)}>…</span>
            <select
              value={headSha ?? ''}
              onChange={(e) => setHeadSha(e.target.value || undefined)}
              aria-label="head commit"
              {...stylex.props(styles.cmpSelect)}
            >
              <option value="">head…</option>
              {MOCK_COMMITS.map((c) => (
                <option key={c.sha} value={c.sha} disabled={c.sha === baseSha}>
                  {c.sha.slice(0, 8)} {c.message}
                </option>
              ))}
            </select>
            <button
              type="button"
              title="swap base and head"
              aria-label="swap base and head"
              disabled={!baseSha && !headSha}
              onClick={() => {
                setBaseSha(headSha)
                setHeadSha(baseSha)
              }}
              {...stylex.props(iconButtons.mini)}
            >
              <ArrowsLeftRight size={10} />
            </button>
            {(baseSha || headSha) && (
              <button
                type="button"
                title="clear compare"
                aria-label="clear compare"
                onClick={() => {
                  setBaseSha(undefined)
                  setHeadSha(undefined)
                }}
                {...stylex.props(iconButtons.mini)}
              >
                <IconX size={10} />
              </button>
            )}
            {baseSha && headSha && baseSha !== headSha && (
              <button
                type="button"
                title="open as compare page"
                aria-label="open as compare page"
                onClick={() => navigate(`/compare?base=${baseSha}&head=${headSha}`)}
                {...stylex.props(iconButtons.mini)}
              >
                <IconArrowUpRight size={10} />
              </button>
            )}
            <span {...stylex.props(styles.spacer)} />
            {days.length > 0 && (
              <nav {...stylex.props(styles.jump)} aria-label="Jump to day">
                {days.map(([day]) => (
                  <button
                    key={day}
                    type="button"
                    onClick={() =>
                      document
                        .getElementById(dayId(day))
                        ?.scrollIntoView({ behavior: 'smooth', block: 'start' })
                    }
                    {...stylex.props(styles.jumpChip)}
                  >
                    {day}
                  </button>
                ))}
              </nav>
            )}
          </div>
          {compare && (
            <section {...stylex.props(styles.compareBox)} aria-label="Commit comparison">
              <header {...stylex.props(styles.compareHead)}>
                <IconDiff size={12} />
                <code {...stylex.props(styles.sha)} title={compare.base.sha}>
                  {compare.base.sha.slice(0, 8)}
                </code>
                <CopyButton text={compare.base.sha} title={`copy ${compare.base.sha.slice(0, 8)}`} />
                <span {...stylex.props(styles.cmpDots)}>…</span>
                <code {...stylex.props(styles.sha)} title={compare.head.sha}>
                  {compare.head.sha.slice(0, 8)}
                </code>
                <CopyButton text={compare.head.sha} title={`copy ${compare.head.sha.slice(0, 8)}`} />
                <span {...stylex.props(styles.compareMeta)}>
                  {compare.files.length} file{compare.files.length === 1 ? '' : 's'}
                </span>
                <DiffStat added={compare.added} removed={compare.removed} />
                <span {...stylex.props(styles.spacer)} />
                <button
                  type="button"
                  title="close comparison"
                  aria-label="close comparison"
                  onClick={() => {
                    setBaseSha(undefined)
                    setHeadSha(undefined)
                  }}
                  {...stylex.props(iconButtons.mini)}
                >
                  <IconX size={10} />
                </button>
              </header>
              <div {...stylex.props(styles.compareFiles)}>
                {compare.files.map((f) => (
                  <div key={f.path} {...stylex.props(styles.fileEntry)}>
                    <div {...stylex.props(styles.fileLine)}>
                      <button
                        type="button"
                        onClick={() => navigate(fileUrl(f.path))}
                        title={`open ${f.path}`}
                        {...stylex.props(styles.fileRow)}
                      >
                        <span {...stylex.props(styles.comparePath)}>
                          <DisplayFilePath path={f.path} icon />
                          {f.commits > 1 && (
                            <DisplayBadge color={false} title="touched by both commits">
                              both
                            </DisplayBadge>
                          )}
                        </span>
                        <DiffStat added={f.added} removed={f.removed} />
                      </button>
                    </div>
                  </div>
                ))}
              </div>
              <p {...stylex.props(styles.compareNote)}>
                Fixture union — filesChanged merged and +/− summed across the two selected
                commits, not a real git diff.
              </p>
            </section>
          )}
          {days.map(([day, commits]) => (
            <section key={day} id={dayId(day)} {...stylex.props(styles.daySection)}>
              <header {...stylex.props(styles.dayHead)}>
                <span {...stylex.props(styles.dayIcon)}>
                  <IconGitCommit size={12} />
                </span>
                <span {...stylex.props(styles.dayLabel)}>{day}</span>
                <span {...stylex.props(styles.dayCount)}>
                  {commits.length} commit{commits.length === 1 ? '' : 's'}
                </span>
              </header>
              <ol {...stylex.props(styles.dayBox)}>
                {commits.map((c) => (
                  <CommitRow
                    key={c.sha}
                    commit={c}
                    open={expanded.has(c.sha)}
                    onToggle={() => toggle(c.sha)}
                    onOpenFile={(path, line) => navigate(fileUrl(path, line))}
                  />
                ))}
              </ol>
            </section>
          ))}
          {days.length === 0 && (
            <p {...stylex.props(styles.empty)}>no commits match the filter</p>
          )}
          {limit < MOCK_COMMITS.length && (
            <button
              type="button"
              onClick={() => setLimit((l) => l + PAGE)}
              {...stylex.props(styles.loadMore)}
            >
              <IconHistoryClock size={11} />
              Load older commits
              <span {...stylex.props(styles.loadMoreCount)}>
                {MOCK_COMMITS.length - limit} more in fixture
              </span>
            </button>
          )}
          <p {...stylex.props(styles.note)}>
            Fixture history — git log is not wired to the index yet. Diffstats, diffs,
            parents, and signatures describe the fixture, not the repository.
          </p>
        </div>
      </div>
    </div>
  )
}

/** One commit: summary hairline row + inline-expanded changed-file list. */
function CommitRow({
  commit,
  open,
  onToggle,
  onOpenFile,
}: {
  commit: MockCommit
  open: boolean
  onToggle: () => void
  onOpenFile: (path: string, line?: number) => void
}) {
  const t = totals(commit)
  const navigate = useNavigate()
  // Which file diffs are open — local to the row, survives collapse/reopen.
  const [openFiles, setOpenFiles] = useState<ReadonlySet<string>>(() => new Set())
  // Hover reveals the trailing copy action; `:focus-within` keeps it
  // keyboard-reachable. Space is reserved so columns never shift.
  const [hover, setHover] = useState(false)
  const toggleFile = (path: string) =>
    setOpenFiles((prev) => {
      const next = new Set(prev)
      if (next.has(path)) next.delete(path)
      else next.add(path)
      return next
    })
  return (
    <li
      {...stylex.props(styles.item)}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
    >
      <div {...stylex.props(styles.rowLine)}>
        <button
          type="button"
          aria-expanded={open}
          onClick={onToggle}
          {...stylex.props(styles.commitRow)}
        >
          <span {...stylex.props(styles.caret, open && styles.caretOpen)}>
            <IconCaretDown size={10} />
          </span>
          <code {...stylex.props(styles.sha)} title={commit.sha}>
            {commit.sha.slice(0, 8)}
          </code>
          <span {...stylex.props(styles.msg)}>
            <span {...stylex.props(styles.msgText)}>{commit.message}</span>
            {commit.verified && (
              <DisplayBadge severity="low" icon={<SealCheck size={9} />} title="signed commit">
                verified
              </DisplayBadge>
            )}
          </span>
          <span {...stylex.props(styles.author)}>{commit.author}</span>
          <span {...stylex.props(styles.files)}>
            {commit.filesChanged.length} file{commit.filesChanged.length === 1 ? '' : 's'}
          </span>
          <DiffStat added={t.added} removed={t.removed} />
          <span {...stylex.props(styles.when)}>
            <DisplayTimeAgo value={commit.at} />
          </span>
        </button>
        <span {...stylex.props(styles.rowActions, hover && styles.rowActionsShown)}>
          <CopyButton text={commit.sha} title={`copy ${commit.sha.slice(0, 8)}`} />
          <button
            type="button"
            title={`open ${commit.sha.slice(0, 8)}`}
            aria-label={`Open commit ${commit.sha}`}
            onClick={(e) => {
              e.stopPropagation()
              navigate(`/commit/${commit.sha}`)
            }}
            {...stylex.props(iconButtons.mini)}
          >
            <IconArrowUpRight size={11} />
          </button>
        </span>
      </div>
      {open && (
        <div {...stylex.props(styles.detail)}>
          {commit.filesChanged.map((f) => (
            <ChangedFile
              key={f.path}
              file={f}
              open={openFiles.has(f.path)}
              onToggle={() => toggleFile(f.path)}
              onOpenFile={onOpenFile}
            />
          ))}
          <div {...stylex.props(styles.meta)}>
            <span>
              commit <code {...stylex.props(styles.metaSha)}>{commit.sha}</code>
            </span>
            <CopyButton text={commit.sha} title={`copy ${commit.sha.slice(0, 8)}`} />
            <span {...stylex.props(styles.metaSep)}>·</span>
            {commit.parents.length === 0 ? (
              <span>root commit</span>
            ) : (
              <span>
                {commit.parents.length === 1 ? 'parent' : 'parents'}{' '}
                {commit.parents.map((p) => (
                  <code key={p} {...stylex.props(styles.metaSha)} title={p}>
                    {p.slice(0, 8)}
                  </code>
                ))}
              </span>
            )}
          </div>
        </div>
      )}
    </li>
  )
}

/**
 * One touched file inside an expanded commit. The row toggles the mock
 * unified diff; the trailing button keeps the `/browse` deep link one
 * click away — same split as GitHub's commit file list.
 */
function ChangedFile({
  file,
  open,
  onToggle,
  onOpenFile,
}: {
  file: MockCommitFile
  open: boolean
  onToggle: () => void
  onOpenFile: (path: string, line?: number) => void
}) {
  return (
    <div {...stylex.props(styles.fileEntry)}>
      <div {...stylex.props(styles.fileLine)}>
        <button
          type="button"
          aria-expanded={open}
          onClick={onToggle}
          {...stylex.props(styles.fileRow)}
        >
          <span {...stylex.props(styles.fileLead)}>
            <span {...stylex.props(styles.caret, open && styles.caretOpen)}>
              <IconCaretDown size={9} />
            </span>
            <DisplayFilePath path={file.path} icon />
          </span>
          <DiffStat added={file.added} removed={file.removed} />
        </button>
        <button
          type="button"
          title={`open ${file.path}`}
          aria-label={`open ${file.path} in Browse`}
          onClick={() => onOpenFile(file.path)}
          {...stylex.props(iconButtons.mini)}
        >
          <IconArrowUpRight size={11} />
        </button>
      </div>
      {open && <FileDiff file={file} onOpenAt={(n) => onOpenFile(file.path, n)} />}
    </div>
  )
}


/**
 * One live `/v1/diff` hit — commit sha chip (from `symbolName`), the touched
 * `path:line`, and the matched patch line colored by its leading sign.
 * Clicking opens the file at that line.
 */
function PatchHit({
  hit,
  onOpen,
}: {
  hit: SearchHit
  onOpen: (path: string, line?: number) => void
}) {
  const sha = hit.symbolName?.startsWith('commit:')
    ? hit.symbolName.slice(7, 15)
    : undefined
  const path = hit.address?.path
  const line = hit.address?.startLine
  const text = (hit.snippet.split('\n')[0] ?? '').trimEnd()
  const sign = text.startsWith('+') ? 'add' : text.startsWith('-') ? 'del' : 'ctx'
  return (
    <li {...stylex.props(styles.patchHit)}>
      <button
        type="button"
        title={hit.explanation?.[0]}
        onClick={() => path && onOpen(path, line)}
        {...stylex.props(styles.patchHitBtn)}
      >
        {sha && <span {...stylex.props(styles.sha)}>{sha}</span>}
        <span {...stylex.props(styles.patchHitPath)}>
          {path}
          {line != null && <span {...stylex.props(styles.patchHitLine)}>:{line}</span>}
        </span>
        <code
          {...stylex.props(
            styles.patchHitCode,
            sign === 'add' ? styles.add : sign === 'del' ? styles.del : undefined,
          )}
        >
          {text || '(empty line)'}
        </code>
      </button>
    </li>
  )
}

const styles = stylex.create({
  root: {
    display: 'flex',
    flexDirection: 'column',
    gap: 14,
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
  spacer: {
    flex: 1,
  },
  search: {
    width: 240,
  },
  body: {
    display: 'flex',
    gap: 16,
    alignItems: 'flex-start',
  },
  list: {
    flex: 1,
    minWidth: 0,
    display: 'flex',
    flexDirection: 'column',
    gap: 14,
  },
  patch: {
    display: 'flex',
    flexDirection: 'column',
    gap: 8,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderMute,
    borderRadius: 6,
    paddingTop: 10,
    paddingBottom: 10,
    paddingLeft: 12,
    paddingRight: 12,
    backgroundColor: vars.bgSecondary,
  },
  patchBar: {
    display: 'flex',
    alignItems: 'center',
    gap: 6,
    color: vars.colorFaint,
  },
  patchLabel: {
    fontSize: 11,
    fontWeight: 500,
    color: vars.colorMuted,
  },
  patchNote: {
    fontSize: 10,
    color: vars.colorFaint,
  },
  patchCount: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
  },
  patchErr: {
    fontSize: 11,
    color: vars.accentError,
  },
  patchEmpty: {
    fontSize: 11,
    color: vars.colorFaint,
  },
  patchList: {
    display: 'flex',
    flexDirection: 'column',
    marginTop: 0,
    marginBottom: 0,
    marginLeft: 0,
    marginRight: 0,
    paddingTop: 6,
    paddingBottom: 0,
    paddingLeft: 0,
    paddingRight: 0,
    listStyleType: 'none',
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
  },
  patchHit: {
    display: 'flex',
  },
  patchHitBtn: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    width: '100%',
    paddingTop: 5,
    paddingBottom: 5,
    paddingLeft: 4,
    paddingRight: 4,
    borderWidth: 0,
    borderRadius: 4,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    cursor: 'pointer',
    fontFamily: 'inherit',
    fontSize: 'inherit',
    textAlign: 'left',
  },
  patchHitPath: {
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorMuted,
    whiteSpace: 'nowrap',
    flexShrink: 0,
  },
  patchHitLine: {
    color: vars.colorFaint,
  },
  patchHitCode: {
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorBase,
    whiteSpace: 'nowrap',
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    flex: 1,
    minWidth: 0,
  },
  // ── Contributors strip ───────────────────────────────────────────
  contrib: {
    display: 'flex',
    flexDirection: 'column',
    borderRadius: 8,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgRaised,
    overflow: 'hidden',
  },
  contribHead: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    flexWrap: 'wrap',
    width: '100%',
    borderWidth: 0,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    fontFamily: 'inherit',
    fontSize: 11,
    color: vars.colorMuted,
    paddingTop: 7,
    paddingBottom: 7,
    paddingLeft: 10,
    paddingRight: 10,
    cursor: 'pointer',
    textAlign: 'left',
  },
  contribLabel: {
    fontWeight: 600,
  },
  contribCount: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    color: vars.colorFaint,
  },
  contribList: {
    display: 'flex',
    flexDirection: 'column',
    gap: 1,
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
    paddingTop: 4,
    paddingBottom: 4,
    paddingLeft: 6,
    paddingRight: 6,
  },
  contribRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    flexWrap: 'wrap',
    width: '100%',
    borderWidth: 0,
    borderRadius: 4,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    fontFamily: 'inherit',
    fontSize: 11,
    color: vars.colorBase,
    paddingTop: 5,
    paddingBottom: 5,
    paddingLeft: 6,
    paddingRight: 6,
    cursor: 'pointer',
    textAlign: 'left',
  },
  contribRowOn: {
    color: vars.colorActive,
  },
  contribName: {
    fontWeight: 500,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  contribCommits: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    color: vars.colorFaint,
    whiteSpace: 'nowrap',
  },
  contribTrack: {
    flex: 1,
    minWidth: 60,
    height: 8,
    borderRadius: 4,
    backgroundColor: vars.bgSunken,
    overflow: 'hidden',
  },
  contribFill: {
    display: 'block',
    height: '100%',
    borderRadius: 4,
    backgroundColor: vars.bgActive,
  },
  contribFillOn: {
    backgroundColor: vars.colorActive,
  },
  contribWhen: {
    display: 'inline-flex',
    alignItems: 'baseline',
    gap: 4,
    fontSize: 10,
    color: vars.colorFaint,
    whiteSpace: 'nowrap',
  },
  contribNote: {
    margin: 0,
    fontSize: 10,
    color: vars.colorFaint,
    paddingTop: 6,
    paddingBottom: 7,
    paddingLeft: 10,
    paddingRight: 10,
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
  },
  daySection: {
    display: 'flex',
    flexDirection: 'column',
    gap: 6,
    scrollMarginTop: 10,
  },
  dayHead: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    paddingLeft: 2,
  },
  dayIcon: {
    display: 'inline-flex',
    color: vars.colorFaint,
    flexShrink: 0,
  },
  dayLabel: {
    fontSize: 12,
    fontWeight: 600,
    color: vars.colorMuted,
  },
  dayCount: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    color: vars.colorFaint,
  },
  dayBox: {
    margin: 0,
    padding: 0,
    listStyle: 'none',
    display: 'flex',
    flexDirection: 'column',
    borderRadius: 8,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgRaised,
    overflow: 'hidden',
  },
  item: {
    display: 'flex',
    flexDirection: 'column',
    borderTopWidth: {
      default: 1,
      ':first-child': 0,
    },
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
  },
  // The commit/file rows are `line` flex containers: the wide toggle button
  // fills the row, the trailing mini action (copy / open) is a sibling so no
  // interactive element nests inside another. Hover wash lives on the line.
  rowLine: {
    display: 'flex',
    alignItems: 'center',
    gap: 4,
    paddingRight: 6,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
  },
  commitRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    flex: 1,
    minWidth: 0,
    borderWidth: 0,
    backgroundColor: 'transparent',
    fontFamily: 'inherit',
    fontSize: 12,
    color: vars.colorBase,
    paddingTop: 8,
    paddingBottom: 8,
    paddingLeft: 10,
    paddingRight: 6,
    cursor: 'pointer',
    textAlign: 'left',
  },
  // Trailing hover actions — opacity-0 space is always reserved so columns
  // never shift; `:focus-within` keeps them reachable by keyboard.
  rowActions: {
    display: 'inline-flex',
    alignItems: 'center',
    flexShrink: 0,
    paddingLeft: 4,
    paddingRight: 4,
    opacity: { default: 0, ':focus-within': 1 },
    pointerEvents: { default: 'none', ':focus-within': 'auto' },
    transitionProperty: 'opacity',
    transitionDuration: '120ms',
  },
  rowActionsShown: {
    opacity: 1,
    pointerEvents: 'auto',
  },
  caret: {
    display: 'inline-flex',
    color: vars.colorFaint,
    flexShrink: 0,
    transform: 'rotate(-90deg)',
    transitionProperty: 'transform',
    transitionDuration: '120ms',
  },
  caretOpen: {
    transform: 'rotate(0deg)',
  },
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
  msg: {
    flex: 1,
    minWidth: 0,
    display: 'inline-flex',
    alignItems: 'center',
    gap: 6,
  },
  msgText: {
    minWidth: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  author: {
    fontSize: 11,
    color: vars.colorMuted,
    flexShrink: 0,
    width: 48,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
    textAlign: 'right',
  },
  files: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    color: vars.colorFaint,
    flexShrink: 0,
    width: 48,
    textAlign: 'right',
  },
  diff: {
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'flex-end',
    gap: 6,
    flexShrink: 0,
  },
  diffNum: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    whiteSpace: 'nowrap',
  },
  add: {
    color: vars.scaleLow,
  },
  del: {
    color: vars.scaleHigh,
  },
  when: {
    flexShrink: 0,
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
    width: 76,
    textAlign: 'right',
  },
  detail: {
    display: 'flex',
    flexDirection: 'column',
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
    backgroundColor: vars.bgSunken,
    paddingTop: 2,
    paddingBottom: 6,
    paddingLeft: 32,
    paddingRight: 12,
  },
  fileEntry: {
    display: 'flex',
    flexDirection: 'column',
    borderTopWidth: {
      default: 1,
      ':first-child': 0,
    },
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
  },
  fileLine: {
    display: 'flex',
    alignItems: 'center',
    gap: 4,
    paddingRight: 4,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
  },
  fileRow: {
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: 10,
    flex: 1,
    minWidth: 0,
    borderWidth: 0,
    backgroundColor: 'transparent',
    fontFamily: 'inherit',
    color: vars.colorBase,
    paddingTop: 5,
    paddingBottom: 5,
    paddingLeft: 4,
    paddingRight: 4,
    cursor: 'pointer',
    textAlign: 'left',
  },
  fileLead: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 6,
    minWidth: 0,
  },
  // ── Compare bar + drawer ─────────────────────────────────────────────
  listBar: {
    display: 'flex',
    alignItems: 'center',
    gap: 6,
    flexWrap: 'wrap',
  },
  cmpLabel: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 5,
    fontSize: 11,
    color: vars.colorFaint,
    flexShrink: 0,
  },
  cmpDots: {
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorFaint,
    flexShrink: 0,
  },
  cmpSelect: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    color: vars.colorBase,
    backgroundColor: vars.bgSunken,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    borderRadius: 4,
    paddingTop: 2,
    paddingBottom: 2,
    paddingLeft: 6,
    paddingRight: 4,
    maxWidth: 210,
    cursor: 'pointer',
    outline: 'none',
  },
  jump: {
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'flex-end',
    gap: 4,
    flexWrap: 'wrap',
  },
  jumpChip: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    color: {
      default: vars.colorMuted,
      ':hover': vars.colorActive,
    },
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: {
      default: vars.borderBase,
      ':hover': vars.borderActive,
    },
    borderRadius: 999,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    paddingTop: 1,
    paddingBottom: 1,
    paddingLeft: 7,
    paddingRight: 7,
    cursor: 'pointer',
  },
  compareBox: {
    display: 'flex',
    flexDirection: 'column',
    borderRadius: 8,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgRaised,
    overflow: 'hidden',
  },
  compareHead: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    flexWrap: 'wrap',
    paddingTop: 7,
    paddingBottom: 7,
    paddingLeft: 10,
    paddingRight: 10,
    fontSize: 11,
    color: vars.colorMuted,
    borderBottomWidth: 1,
    borderBottomStyle: 'solid',
    borderBottomColor: vars.borderMute,
  },
  compareFiles: {
    display: 'flex',
    flexDirection: 'column',
    backgroundColor: vars.bgSunken,
    paddingLeft: 10,
    paddingRight: 10,
  },
  comparePath: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 6,
    minWidth: 0,
  },
  compareMeta: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    color: vars.colorFaint,
  },
  compareNote: {
    margin: 0,
    fontSize: 10,
    color: vars.colorFaint,
    paddingTop: 6,
    paddingBottom: 7,
    paddingLeft: 10,
    paddingRight: 10,
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
  },
  loadMore: {
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    gap: 8,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    borderRadius: 8,
    backgroundColor: {
      default: vars.bgRaised,
      ':hover': vars.bgHover,
    },
    color: {
      default: vars.colorMuted,
      ':hover': vars.colorBase,
    },
    fontFamily: 'inherit',
    fontSize: 11,
    paddingTop: 7,
    paddingBottom: 7,
    cursor: 'pointer',
  },
  loadMoreCount: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    color: vars.colorFaint,
  },
  meta: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    flexWrap: 'wrap',
    paddingTop: 6,
    paddingLeft: 4,
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
  },
  metaSha: {
    color: vars.colorActive,
  },
  metaSep: {
    opacity: vars.opFade,
  },
  empty: {
    margin: 0,
    fontSize: 12,
    color: vars.colorFaint,
  },
  note: {
    margin: 0,
    fontSize: 11,
    color: vars.colorFaint,
  },
})
