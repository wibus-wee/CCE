import * as stylex from '@stylexjs/stylex'
import { useEffect, useMemo, useState } from 'react'
import { useNavigate } from 'react-router'
import {
  describeError,
  FileListEntry,
  gateway,
  RepoEntry,
  ViewManifest,
} from '../api'
import { queryUrl } from '../lib/navigation'
import { useAppStore } from '../lib/store'
import { getHashColorFromString } from '../ui/format'
import { DisplayNumber, DisplayTimeAgo } from '../ui/DisplayNumber'
import { font, vars } from '../ui/tokens.stylex'
import { useColorScheme } from '../ui/useDark'

/**
 * Repository overview — the Sourcegraph repo-landing analog: what this
 * snapshot actually contains, shown in Browse when no file is selected.
 * Language composition, top directories (each a runnable `path:` query),
 * and snapshot facts. All real data from `/v1/files` + `/v1/repos` +
 * manifest; nothing decorative.
 */
export function RepoOverview({ files }: { files: FileListEntry[] }) {
  const manifest = useAppStore((s) => s.manifest)
  const [repos, setRepos] = useState<RepoEntry[]>()
  const navigate = useNavigate()

  useEffect(() => {
    gateway
      .repos()
      .then(setRepos)
      .catch(() => setRepos([]))
  }, [])

  const languages = useMemo(() => {
    const counts = new Map<string, number>()
    for (const f of files) {
      const lang = f.language ?? 'other'
      counts.set(lang, (counts.get(lang) ?? 0) + 1)
    }
    return [...counts.entries()].sort((a, b) => b[1] - a[1])
  }, [files])

  const topDirs = useMemo(() => {
    const dirs = new Map<string, Map<string, number>>()
    for (const f of files) {
      const seg = f.path.includes('/') ? f.path.split('/')[0]! : '.'
      const langs = dirs.get(seg) ?? new Map<string, number>()
      langs.set(f.language ?? 'other', (langs.get(f.language ?? 'other') ?? 0) + 1)
      dirs.set(seg, langs)
    }
    return [...dirs.entries()]
      .map(([dir, langs]) => ({
        dir,
        files: [...langs.values()].reduce((a, b) => a + b, 0),
        langs: [...langs.entries()].sort((a, b) => b[1] - a[1]),
      }))
      .sort((a, b) => b.files - a.files)
  }, [files])

  const runQuery = (q: string) => navigate(queryUrl(q))

  return (
    <div {...stylex.props(styles.root)}>
      <header {...stylex.props(styles.head)}>
        <span {...stylex.props(styles.title)}>repository overview</span>
        <span {...stylex.props(styles.meta)}>
          {files.length.toLocaleString()} files · {languages.length} languages ·{' '}
          {topDirs.length} roots
        </span>
      </header>
      <div {...stylex.props(styles.row)}>
        <div {...stylex.props(styles.col)} style={{ flexBasis: '38%' }}>
          <Sub label="composition" aside="files by language" />
          <Composition languages={languages} total={files.length} />
        </div>
        <span {...stylex.props(styles.sep)} />
        <div {...stylex.props(styles.col)} style={{ flexBasis: '37%' }}>
          <Sub label="top directories" aside="click → path: query" />
          <TopDirs dirs={topDirs} onRunQuery={runQuery} />
        </div>
        <span {...stylex.props(styles.sep)} />
        <div {...stylex.props(styles.col)} style={{ flexBasis: '25%' }}>
          <Sub label="snapshot facts" aside="real" />
          <SnapshotFacts
            manifest={manifest}
            repos={repos}
            filesCount={files.length}
            langCount={languages.length}
          />
        </div>
      </div>
    </div>
  )
}

function Sub({ label, aside }: { label: string; aside?: string }) {
  return (
    <div {...stylex.props(styles.sub)}>
      <span {...stylex.props(styles.subLabel)}>{label}</span>
      {aside && <span {...stylex.props(styles.subAside)}>{aside}</span>}
    </div>
  )
}

/** One proportional bar + per-language rows with real counts. */
function Composition({
  languages,
  total,
}: {
  languages: [string, number][]
  total: number
}) {
  const dark = useColorScheme() === 'dark'
  const shown = languages.slice(0, 7)
  const tailCount = languages.slice(7).reduce((n, [, c]) => n + c, 0)
  const rows = shown.map(([l, c]) => ({ lang: l, count: c }))
  const otherRow = rows.find((r) => r.lang === 'other')
  if (otherRow) otherRow.count += tailCount
  else if (tailCount > 0) rows.push({ lang: 'other', count: tailCount })
  return (
    <div {...stylex.props(styles.compoWrap)}>
      <div {...stylex.props(styles.compoBar)}>
        {rows.map((r) => (
          <span
            key={r.lang}
            {...stylex.props(styles.compoSeg)}
            style={{
              width: `${(r.count / total) * 100}%`,
              backgroundColor: getHashColorFromString(r.lang, 1, dark),
            }}
            title={`${r.lang} — ${r.count} files`}
          />
        ))}
      </div>
      <div {...stylex.props(styles.compoLegend)}>
        {rows.map((r) => (
          <span key={r.lang} {...stylex.props(styles.compoItem)}>
            <span
              {...stylex.props(styles.compoDot)}
              style={{ backgroundColor: getHashColorFromString(r.lang, 1, dark) }}
            />
            <code {...stylex.props(styles.compoLang)}>{r.lang}</code>
            <span {...stylex.props(styles.compoCount)}>
              <DisplayNumber value={r.count} />
            </span>
            <span {...stylex.props(styles.compoPct)}>{((r.count / total) * 100).toFixed(0)}%</span>
          </span>
        ))}
      </div>
    </div>
  )
}

/** Real first-level structure; clicking scopes a `path:` query. */
function TopDirs({
  dirs,
  onRunQuery,
}: {
  dirs: { dir: string; files: number; langs: [string, number][] }[]
  onRunQuery: (q: string) => void
}) {
  const dark = useColorScheme() === 'dark'
  const max = Math.max(...dirs.map((d) => d.files), 1)
  return (
    <div {...stylex.props(styles.dirList)}>
      {dirs.slice(0, 7).map((d) => (
        <button
          key={d.dir}
          type="button"
          {...stylex.props(styles.dirRow)}
          onClick={() => onRunQuery(`path:${d.dir === '.' ? '' : `${d.dir}/`}`)}
          title={`${d.dir}/ — ${d.files} files`}
        >
          <code {...stylex.props(styles.dirName)}>{d.dir === '.' ? '(root)' : `${d.dir}/`}</code>
          <span {...stylex.props(styles.dirBarTrack)}>
            <span
              {...stylex.props(styles.dirBarFill)}
              style={{
                width: `${(d.files / max) * 100}%`,
                backgroundColor: getHashColorFromString(d.langs[0]?.[0] ?? 'other', 0.5, dark),
              }}
            />
          </span>
          <span {...stylex.props(styles.dirFiles)}>
            <DisplayNumber value={d.files} />
          </span>
          <span {...stylex.props(styles.dirLangs)}>
            {d.langs.slice(0, 2).map(([l]) => l).join(' · ')}
          </span>
        </button>
      ))}
    </div>
  )
}

/** Compact key/value truth about the snapshot being browsed. */
function SnapshotFacts({
  manifest,
  repos,
  filesCount,
  langCount,
}: {
  manifest?: ViewManifest
  repos?: RepoEntry[]
  filesCount: number
  langCount: number
}) {
  const repo = repos?.[0]
  const ready = manifest
    ? Object.values(manifest.views).filter((v) => v.state === 'ready').length
    : 0
  const rows: { k: string; v: React.ReactNode }[] = [
    { k: 'repository', v: repo?.name ?? manifest?.repositoryId?.slice(0, 20) ?? '—' },
    { k: 'snapshot', v: manifest ? manifest.snapshotId.slice(0, 21) : '—' },
    { k: 'files', v: <DisplayNumber value={filesCount} /> },
    { k: 'languages', v: langCount },
    { k: 'views', v: manifest ? `${ready}/${Object.keys(manifest.views).length} ready` : '—' },
  ]
  if (repo?.lastPush) {
    rows.push({ k: 'last push', v: <DisplayTimeAgo value={repo.lastPush.at} /> })
  }
  return (
    <div {...stylex.props(styles.factList)}>
      {rows.map((r) => (
        <div key={r.k} {...stylex.props(styles.factRow)}>
          <span {...stylex.props(styles.factKey)}>{r.k}</span>
          <span {...stylex.props(styles.factVal)}>{r.v}</span>
        </div>
      ))}
    </div>
  )
}

const styles = stylex.create({
  root: {
    display: 'flex',
    flexDirection: 'column',
    gap: 10,
    padding: 4,
    height: '100%',
    overflowY: 'auto',
  },
  head: {
    display: 'flex',
    alignItems: 'baseline',
    gap: 10,
  },
  title: {
    fontFamily: font.mono,
    fontSize: 11,
    fontWeight: 700,
    textTransform: 'uppercase',
    letterSpacing: '0.1em',
    color: vars.colorBase,
  },
  meta: {
    fontFamily: font.mono,
    fontSize: 10.5,
    color: vars.colorFaint,
    fontVariantNumeric: 'tabular-nums',
  },
  row: {
    display: 'flex',
    alignItems: 'stretch',
    gap: 18,
    flexWrap: { default: 'nowrap', '@media (max-width: 980px)': 'wrap' },
  },
  col: {
    flexGrow: 0,
    flexShrink: 1,
    minWidth: 0,
    display: 'flex',
    flexDirection: 'column',
    gap: 8,
  },
  sep: {
    width: 1,
    alignSelf: 'stretch',
    backgroundColor: vars.borderMute,
    flexShrink: 0,
    display: { default: 'block', '@media (max-width: 980px)': 'none' },
  },
  sub: {
    display: 'flex',
    alignItems: 'baseline',
    justifyContent: 'space-between',
    gap: 8,
  },
  subLabel: {
    fontSize: 10,
    fontWeight: 600,
    textTransform: 'uppercase',
    letterSpacing: '0.05em',
    color: vars.colorMuted,
  },
  subAside: {
    fontFamily: font.mono,
    fontSize: 9.5,
    color: vars.colorFaint,
  },
  compoWrap: {
    display: 'flex',
    flexDirection: 'column',
    gap: 10,
    paddingTop: 4,
  },
  compoBar: {
    display: 'flex',
    height: 10,
    borderRadius: 5,
    overflow: 'hidden',
    backgroundColor: vars.bgSunken,
  },
  compoSeg: {
    display: 'block',
    height: '100%',
  },
  compoLegend: {
    display: 'flex',
    flexDirection: 'column',
    gap: 3,
  },
  compoItem: {
    display: 'flex',
    alignItems: 'center',
    gap: 7,
    fontSize: 11.5,
  },
  compoDot: {
    width: 7,
    height: 7,
    borderRadius: 2,
    flexShrink: 0,
  },
  compoLang: {
    fontFamily: font.mono,
    fontSize: 10.5,
    color: vars.colorBase,
    flex: 1,
    minWidth: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  compoCount: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorMuted,
    fontVariantNumeric: 'tabular-nums',
  },
  compoPct: {
    fontFamily: font.mono,
    fontSize: 9.5,
    color: vars.colorFaint,
    width: 30,
    textAlign: 'right',
    flexShrink: 0,
  },
  dirList: {
    display: 'flex',
    flexDirection: 'column',
  },
  dirRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    paddingTop: 5,
    paddingBottom: 5,
    paddingLeft: 0,
    paddingRight: 0,
    borderWidth: 0,
    borderStyle: 'solid',
    borderColor: 'transparent',
    borderTopWidth: { default: 1, ':first-child': 0 },
    borderTopColor: vars.borderMute,
    backgroundColor: { default: 'transparent', ':hover': vars.bgHover },
    fontFamily: 'inherit',
    fontSize: 'inherit',
    textAlign: 'left',
    cursor: 'pointer',
    color: vars.colorBase,
  },
  dirName: {
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorBase,
    width: 110,
    flexShrink: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  dirBarTrack: {
    flex: 1,
    height: 4,
    borderRadius: 2,
    backgroundColor: vars.bgSunken,
    overflow: 'hidden',
    minWidth: 40,
  },
  dirBarFill: {
    display: 'block',
    height: '100%',
    borderRadius: 2,
  },
  dirFiles: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorMuted,
    width: 34,
    textAlign: 'right',
    flexShrink: 0,
    fontVariantNumeric: 'tabular-nums',
  },
  dirLangs: {
    fontFamily: font.mono,
    fontSize: 9,
    color: vars.colorFaint,
    width: 56,
    textAlign: 'right',
    flexShrink: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
    display: { default: 'block', '@media (max-width: 1100px)': 'none' },
  },
  factList: {
    display: 'flex',
    flexDirection: 'column',
  },
  factRow: {
    display: 'flex',
    alignItems: 'baseline',
    justifyContent: 'space-between',
    gap: 10,
    paddingTop: 5,
    paddingBottom: 5,
    borderTopWidth: { default: 1, ':first-child': 0 },
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
  },
  factKey: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
    textTransform: 'uppercase',
    letterSpacing: '0.05em',
    flexShrink: 0,
  },
  factVal: {
    fontFamily: font.mono,
    fontSize: 10.5,
    color: vars.colorBase,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
    textAlign: 'right',
    minWidth: 0,
  },
})
