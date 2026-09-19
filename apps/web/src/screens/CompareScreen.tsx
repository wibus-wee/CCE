import { ArrowsLeftRight } from '@phosphor-icons/react'
import * as stylex from '@stylexjs/stylex'
import { useMemo, useState } from 'react'
import { useNavigate, useSearchParams } from 'react-router'
import { DiffStat, FileDiff } from '../components/FileDiff'
import { fileUrl } from '../lib/navigation'
import { MOCK_COMMITS, type MockCommit, type MockCommitFile } from '../mock/commits'
import { CopyButton } from '../ui/CopyButton'
import { DisplayBadge } from '../ui/DisplayBadge'
import { DisplayFilePath } from '../ui/DisplayFilePath'
import { IconCaretRight, IconDiff } from '../ui/icons'
import { LayoutBreadcrumb } from '../ui/LayoutStructure'
import { iconButtons } from '../ui/recipes.stylex'
import { font, vars } from '../ui/tokens.stylex'

interface MergedFile {
  path: string
  added: number
  removed: number
  /** Commits (base/head) that touched this path — drives the `both` badge. */
  sources: { commit: MockCommit; file: MockCommitFile }[]
}

/**
 * Union of both commits' filesChanged with +/− summed per path. This is a
 * fixture merge — each commit ships authored hunks, so an expanded file
 * shows both commits' hunks side by side, never a synthesized cross-diff.
 */
function mergeFiles(base: MockCommit, head: MockCommit): MergedFile[] {
  const merged = new Map<string, MergedFile>()
  for (const commit of [base, head]) {
    for (const file of commit.filesChanged) {
      const m = merged.get(file.path) ?? { path: file.path, added: 0, removed: 0, sources: [] }
      m.added += file.added
      m.removed += file.removed
      m.sources.push({ commit, file })
      merged.set(file.path, m)
    }
  }
  return [...merged.values()]
}

/**
 * Compare — Sourcegraph's `/-/compare/<base>...<head>` page: pick two
 * revisions, see the combined file list with aggregate +/−, expand a file
 * for its hunks. `?base=&head=` make the pair deep-linkable.
 *
 * MOCK: there is no per-revision diff endpoint — the daemon indexes a
 * single HEAD snapshot. The result is the union of the two fixture
 * commits' authored hunks, labeled as such; it is not a real `git diff`.
 */
export function CompareScreen() {
  const navigate = useNavigate()
  const [params, setParams] = useSearchParams()
  const baseSha = params.get('base') ?? ''
  const headSha = params.get('head') ?? ''

  const setParam = (key: 'base' | 'head', value: string) => {
    const next = new URLSearchParams(params)
    if (value) next.set(key, value)
    else next.delete(key)
    setParams(next, { replace: true })
  }

  const base = MOCK_COMMITS.find((c) => c.sha === baseSha)
  const head = MOCK_COMMITS.find((c) => c.sha === headSha)
  const compare = useMemo(() => {
    if (!base || !head || base.sha === head.sha) return undefined
    const files = mergeFiles(base, head)
    return {
      files,
      added: files.reduce((n, f) => n + f.added, 0),
      removed: files.reduce((n, f) => n + f.removed, 0),
    }
  }, [base, head])

  return (
    <div {...stylex.props(styles.root)}>
      <LayoutBreadcrumb
        items={[{ label: 'Commits', onClick: () => navigate('/commits') }, { label: 'Compare' }]}
      />

      <header {...stylex.props(styles.head)}>
        <h2 {...stylex.props(styles.title)}>Compare revisions</h2>
        <DisplayBadge severity="medium">preview</DisplayBadge>
      </header>

      <div {...stylex.props(styles.pickBar)}>
        <IconDiff size={12} />
        <select
          value={baseSha}
          onChange={(e) => setParam('base', e.target.value)}
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
          value={headSha}
          onChange={(e) => setParam('head', e.target.value)}
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
          onClick={() =>
            setParams(
              (p) => {
                const next = new URLSearchParams(p)
                const b = next.get('base')
                const h = next.get('head')
                if (h) next.set('base', h)
                else next.delete('base')
                if (b) next.set('head', b)
                else next.delete('head')
                return next
              },
              { replace: true },
            )
          }
          {...stylex.props(iconButtons.mini)}
        >
          <ArrowsLeftRight size={10} />
        </button>
        <span {...stylex.props(styles.pickNote)}>fixture commits — per-rev diff pending</span>
      </div>

      {baseSha && headSha && baseSha === headSha && (
        <p {...stylex.props(styles.same)}>base and head are the same revision.</p>
      )}

      {compare && base && head && (
        <section {...stylex.props(styles.result)} aria-label="Comparison result">
          <header {...stylex.props(styles.resultHead)}>
            <code {...stylex.props(styles.sha)} title={base.sha}>
              {base.sha.slice(0, 8)}
            </code>
            <CopyButton text={base.sha} title={`copy ${base.sha.slice(0, 8)}`} />
            <span {...stylex.props(styles.cmpDots)}>…</span>
            <code {...stylex.props(styles.sha)} title={head.sha}>
              {head.sha.slice(0, 8)}
            </code>
            <CopyButton text={head.sha} title={`copy ${head.sha.slice(0, 8)}`} />
            <span {...stylex.props(styles.resultMeta)}>
              {compare.files.length} file{compare.files.length === 1 ? '' : 's'}
            </span>
            <DiffStat added={compare.added} removed={compare.removed} />
          </header>

          <ul {...stylex.props(styles.files)}>
            {compare.files.map((f) => (
              <CompareFile key={f.path} file={f} onOpen={(line) => navigate(fileUrl(f.path, line))} />
            ))}
          </ul>

          <p {...stylex.props(styles.note)}>
            Fixture union — filesChanged merged and +/− summed across the two selected
            commits; expanded hunks are each commit's authored diff, not a real git diff.
          </p>
        </section>
      )}

      {!compare && !(baseSha && headSha) && (
        <p {...stylex.props(styles.empty)}>Pick a base and a head to compare.</p>
      )}
    </div>
  )
}

/** One merged file row — expands to both commits' authored hunks for the path. */
function CompareFile({
  file,
  onOpen,
}: {
  file: MergedFile
  onOpen: (line: number) => void
}) {
  const [open, setOpen] = useState(false)
  const toggle = () => setOpen((o) => !o)
  return (
    <li {...stylex.props(styles.fileLi)}>
      <button
        type="button"
        onClick={toggle}
        aria-expanded={open}
        {...stylex.props(styles.fileRow)}
      >
        <span {...stylex.props(styles.caret, open && styles.caretOpen)}>
          <IconCaretRight size={10} />
        </span>
        <span {...stylex.props(styles.filePath)}>
          <DisplayFilePath path={file.path} icon />
          {file.sources.length > 1 && (
            <DisplayBadge color={false} title="touched by both commits">
              both
            </DisplayBadge>
          )}
        </span>
        <DiffStat added={file.added} removed={file.removed} />
      </button>
      {open && (
        <div {...stylex.props(styles.fileBody)}>
          {file.sources.map(({ commit, file: f }) => (
            <div key={commit.sha}>
              <div {...stylex.props(styles.srcLabel)}>
                from <code {...stylex.props(styles.sha)}>{commit.sha.slice(0, 8)}</code> —{' '}
                {commit.message}
              </div>
              <FileDiff file={f} onOpenAt={onOpen} />
            </div>
          ))}
        </div>
      )}
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
  },
  title: {
    margin: 0,
    fontSize: 15,
    fontWeight: 600,
    color: vars.colorBase,
  },
  pickBar: {
    display: 'flex',
    alignItems: 'center',
    gap: 6,
    flexWrap: 'wrap',
    color: vars.colorFaint,
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
    paddingLeft: 5,
    paddingRight: 5,
    maxWidth: 240,
    textOverflow: 'ellipsis',
    cursor: 'pointer',
  },
  cmpDots: {
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorFaint,
    flexShrink: 0,
  },
  pickNote: {
    marginLeft: 4,
    fontSize: 10,
    color: vars.colorFaint,
  },
  same: {
    margin: 0,
    fontSize: 11,
    color: vars.accentError,
  },
  empty: {
    margin: 0,
    fontSize: 11,
    color: vars.colorFaint,
  },
  result: {
    display: 'flex',
    flexDirection: 'column',
    gap: 8,
  },
  resultHead: {
    display: 'flex',
    alignItems: 'center',
    gap: 6,
    fontSize: 11,
  },
  sha: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    color: vars.colorActive,
    backgroundColor: vars.bgSunken,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    borderRadius: 4,
    paddingTop: 1,
    paddingBottom: 1,
    paddingLeft: 5,
    paddingRight: 5,
  },
  resultMeta: {
    fontSize: 11,
    color: vars.colorMuted,
    marginLeft: 4,
  },
  files: {
    listStyle: 'none',
    margin: 0,
    padding: 0,
    display: 'flex',
    flexDirection: 'column',
    gap: 2,
  },
  fileLi: {
    display: 'flex',
    flexDirection: 'column',
  },
  fileRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 6,
    width: '100%',
    paddingTop: 4,
    paddingBottom: 4,
    paddingLeft: 4,
    paddingRight: 6,
    borderWidth: 0,
    borderRadius: 5,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    cursor: 'pointer',
    textAlign: 'left',
    fontSize: 11,
  },
  caret: {
    display: 'inline-flex',
    color: vars.colorFaint,
    transform: 'rotate(0deg)',
    transitionProperty: 'transform',
    transitionDuration: '120ms',
    flexShrink: 0,
  },
  caretOpen: {
    transform: 'rotate(90deg)',
    color: vars.colorMuted,
  },
  filePath: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 6,
    flex: 1,
    minWidth: 0,
  },
  fileBody: {
    marginLeft: 20,
    display: 'flex',
    flexDirection: 'column',
    gap: 2,
  },
  srcLabel: {
    fontSize: 10,
    color: vars.colorFaint,
    paddingTop: 4,
    paddingBottom: 2,
  },
  note: {
    margin: 0,
    fontSize: 11,
    color: vars.colorFaint,
  },
})
