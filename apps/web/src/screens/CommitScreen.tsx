import * as stylex from '@stylexjs/stylex'
import { useNavigate, useParams } from 'react-router'
import { FileDiff } from '../components/FileDiff'
import { fileUrl } from '../lib/navigation'
import { MOCK_COMMITS, type MockCommit } from '../mock/commits'
import { ActionButton } from '../ui/ActionButton'
import { CopyButton } from '../ui/CopyButton'
import { DisplayBadge } from '../ui/DisplayBadge'
import { DisplayFilePath } from '../ui/DisplayFilePath'
import { DisplayTimeAgo } from '../ui/DisplayNumber'
import { FeedbackEmptyState } from '../ui/FeedbackStates'
import { IconArrowUpRight, IconGitCommit } from '../ui/icons'
import { LayoutBreadcrumb } from '../ui/LayoutStructure'
import { font, vars } from '../ui/tokens.stylex'

function totals(c: MockCommit): { added: number; removed: number } {
  return c.filesChanged.reduce(
    (acc, f) => ({ added: acc.added + f.added, removed: acc.removed + f.removed }),
    { added: 0, removed: 0 },
  )
}

/**
 * Commit detail — Sourcegraph's `/-/commit/<sha>` page: breadcrumb back to
 * the log, the full sha + copy, message, author/time, diffstat, parents,
 * then every changed file with its unified diff inline. List rows reach it
 * through the trailing ↗; `?L=`-style file jumps go to Browse.
 *
 * MOCK: `/v1/commits` doesn't exist and no per-commit patch endpoint is
 * wired, so the data comes from `src/mock/commits.ts` — `preview` badge,
 * never presented as live git truth.
 */
export function CommitScreen() {
  const navigate = useNavigate()
  const sha = useParams()['*'] ?? ''
  const commit = MOCK_COMMITS.find((c) => c.sha === sha || c.sha.startsWith(sha))

  if (!commit) {
    return (
      <div {...stylex.props(styles.root)}>
        <LayoutBreadcrumb
          items={[{ label: 'Commits', onClick: () => navigate('/commits') }, { label: sha || 'commit' }]}
        />
        <FeedbackEmptyState
          icon={<IconGitCommit size={20} />}
          title="Commit not found"
          description={`'${sha}' isn't in the fixture history — the commits endpoint is pending.`}
        />
        <div>
          <ActionButton size="sm" onClick={() => navigate('/commits')}>
            Back to commits
          </ActionButton>
        </div>
      </div>
    )
  }

  const t = totals(commit)
  return (
    <div {...stylex.props(styles.root)}>
      <LayoutBreadcrumb
        items={[
          { label: 'Commits', onClick: () => navigate('/commits') },
          { label: commit.sha.slice(0, 8) },
        ]}
      />

      <header {...stylex.props(styles.head)}>
        <h2 {...stylex.props(styles.title)}>{commit.message}</h2>
        <DisplayBadge severity="medium">preview</DisplayBadge>
        {commit.verified && <DisplayBadge severity="low">verified</DisplayBadge>}
      </header>

      <div {...stylex.props(styles.meta)}>
        <span {...stylex.props(styles.metaItem)}>
          <IconGitCommit size={12} />
          <code {...stylex.props(styles.shaFull)}>{commit.sha}</code>
          <CopyButton text={commit.sha} title={`copy ${commit.sha.slice(0, 8)}`} />
        </span>
        <span {...stylex.props(styles.metaItem)}>{commit.author}</span>
        <span {...stylex.props(styles.metaItem)}>
          <DisplayTimeAgo value={commit.at} />
        </span>
        <span {...stylex.props(styles.metaItem, styles.stats)}>
          {commit.filesChanged.length} file{commit.filesChanged.length === 1 ? '' : 's'}
          {' · '}
          <span {...stylex.props(styles.add)}>+{t.added}</span>{' '}
          <span {...stylex.props(styles.del)}>−{t.removed}</span>
        </span>
        {commit.parents.length > 0 && (
          <span {...stylex.props(styles.metaItem)}>
            parent{commit.parents.length === 1 ? '' : 's'}
            {commit.parents.map((p) => (
              <button
                key={p}
                type="button"
                title={`open parent ${p.slice(0, 8)}`}
                onClick={() => navigate(`/commit/${p}`)}
                {...stylex.props(styles.parentChip)}
              >
                {p.slice(0, 8)}
              </button>
            ))}
          </span>
        )}
      </div>

      <div {...stylex.props(styles.files)}>
        {commit.filesChanged.map((f) => (
          <section key={f.path} {...stylex.props(styles.file)}>
            <header {...stylex.props(styles.fileHead)}>
              <DisplayFilePath path={f.path} />
              <span {...stylex.props(styles.fileStat)}>
                <span {...stylex.props(styles.add)}>+{f.added}</span>{' '}
                <span {...stylex.props(styles.del)}>−{f.removed}</span>
              </span>
              <button
                type="button"
                title={`open ${f.path}`}
                aria-label={`open ${f.path} in Browse`}
                onClick={() => navigate(fileUrl(f.path))}
                {...stylex.props(styles.fileOpen)}
              >
                <IconArrowUpRight size={11} />
              </button>
            </header>
            <FileDiff file={f} onOpenAt={(line) => navigate(fileUrl(f.path, line))} />
          </section>
        ))}
      </div>

      <p {...stylex.props(styles.note)}>
        Fixture data — the /v1/commits endpoint is pending; hunks are
        authored, not a real git diff.
      </p>
    </div>
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
    color: vars.colorBase,
  },
  meta: {
    display: 'flex',
    alignItems: 'center',
    gap: 14,
    flexWrap: 'wrap',
    fontSize: 11,
    color: vars.colorMuted,
  },
  metaItem: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 5,
  },
  shaFull: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 11,
    color: vars.colorActive,
  },
  stats: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
  },
  add: {
    color: vars.scaleLow,
  },
  del: {
    color: vars.scaleHigh,
  },
  parentChip: {
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
    marginLeft: 4,
    cursor: 'pointer',
  },
  files: {
    display: 'flex',
    flexDirection: 'column',
    gap: 10,
  },
  file: {
    display: 'flex',
    flexDirection: 'column',
    gap: 4,
  },
  fileHead: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    fontSize: 11,
  },
  fileStat: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
  },
  fileOpen: {
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    width: 20,
    height: 20,
    padding: 0,
    borderWidth: 0,
    borderRadius: 4,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    color: vars.colorFaint,
    cursor: 'pointer',
    marginLeft: 'auto',
  },
  note: {
    margin: 0,
    fontSize: 11,
    color: vars.colorFaint,
  },
})
