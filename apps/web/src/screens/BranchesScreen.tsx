import * as stylex from '@stylexjs/stylex'
// No IconLock in ../ui/icons — the contract is to pull missing glyphs from
// Phosphor at the call site (same set the icon wrappers use).
import { LockSimple } from '@phosphor-icons/react'
import { useMemo, useState } from 'react'
import { useNavigate } from 'react-router'
import { MOCK_BRANCHES, MOCK_TAGS, type MockBranch, type MockTag } from '../mock/branches'
import { MOCK_COMMITS } from '../mock/commits'
import { CopyButton } from '../ui/CopyButton'
import { DisplayBadge } from '../ui/DisplayBadge'
import { DisplayTimeAgo } from '../ui/DisplayNumber'
import { FeedbackEmptyState } from '../ui/FeedbackStates'
import { FormSearchField } from '../ui/FormSearchField'
import {
  IconArrowUpRight,
  IconCaretDown,
  IconGitBranch,
  IconGitCommit,
  IconStar,
  IconTag,
} from '../ui/icons'
import { iconButtons } from '../ui/recipes.stylex'
import { font, vars } from '../ui/tokens.stylex'

/** Refs untouched for 14+ days read as stale — badge and filter chip share this rule. */
const STALE_MS = 14 * 24 * 3600_000
const isStale = (at: string) => Date.now() - new Date(at).getTime() > STALE_MS

/**
 * Branches — Sourcegraph's repo branch list: every ref, its tip commit, and
 * its divergence from the default branch. Rows expand inline to the commits
 * the ref is ahead by (sampled from MOCK_COMMITS) plus a compare hint that
 * opens the fixture history, and a collapsible Tags section carries the
 * release line. MOCK: no `/v1/branches` endpoint and git refs are not wired
 * to the index yet, so rows come from `src/mock/branches` and the screen
 * carries a `preview` badge — fixture refs are never presented as live repo
 * truth.
 */
export function BranchesScreen() {
  const [text, setText] = useState('')
  const [staleOnly, setStaleOnly] = useState(false)
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(() => new Set())
  const [tagsOpen, setTagsOpen] = useState(true)

  const rows = useMemo(() => {
    const needle = text.trim().toLowerCase()
    return MOCK_BRANCHES.filter(
      (b) =>
        (!staleOnly || isStale(b.at)) &&
        (!needle ||
          `${b.name} ${b.headMessage} ${b.author}`.toLowerCase().includes(needle)),
    ).sort((a, b) =>
      // The default ref is pinned to the top; the rest newest-tip first.
      a.isDefault === b.isDefault ? b.at.localeCompare(a.at) : a.isDefault ? -1 : 1,
    )
  }, [text, staleOnly])

  /** Tags newest-first — the fixture is authored that way but the sort is cheap. */
  const tags = useMemo(() => [...MOCK_TAGS].sort((a, b) => b.at.localeCompare(a.at)), [])

  const toggle = (name: string) =>
    setExpanded((prev) => {
      const next = new Set(prev)
      if (next.has(name)) next.delete(name)
      else next.add(name)
      return next
    })

  return (
    <div {...stylex.props(styles.root)}>
      <header {...stylex.props(styles.head)}>
        <h2 {...stylex.props(styles.title)}>Branches</h2>
        <DisplayBadge severity="medium">preview</DisplayBadge>
        <span {...stylex.props(styles.count)}>{rows.length === MOCK_BRANCHES.length ? `${rows.length} branches` : `${rows.length} of ${MOCK_BRANCHES.length} branches`}</span>
        <span {...stylex.props(styles.spacer)} />
        <button
          type="button"
          aria-pressed={staleOnly}
          title="refs untouched for 14+ days"
          onClick={() => setStaleOnly((v) => !v)}
          {...stylex.props(styles.chip, staleOnly && styles.chipOn)}
        >
          stale
        </button>
        <div {...stylex.props(styles.search)}>
          <FormSearchField
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder="name, message, or author"
            onClear={() => setText('')}
            aria-label="Filter branches"
          />
        </div>
      </header>

      {rows.length === 0 ? (
        <FeedbackEmptyState
          icon={<IconGitBranch size={18} />}
          title={staleOnly && !text.trim() ? 'No stale refs' : 'No matching refs'}
          description={
            staleOnly && !text.trim()
              ? 'Every branch tip is under 14 days old.'
              : `No branch name, commit subject, or author contains '${text.trim()}'.`
          }
        />
      ) : (
        <ol {...stylex.props(styles.list)}>
          {rows.map((b) => (
            <BranchRow
              key={b.name}
              branch={b}
              open={expanded.has(b.name)}
              onToggle={() => toggle(b.name)}
            />
          ))}
        </ol>
      )}

      <section {...stylex.props(styles.tagsSection)}>
        <button
          type="button"
          aria-expanded={tagsOpen}
          onClick={() => setTagsOpen((v) => !v)}
          {...stylex.props(styles.tagsHead)}
        >
          <span {...stylex.props(styles.caret, tagsOpen && styles.caretOpen)}>
            <IconCaretDown size={10} />
          </span>
          <span {...stylex.props(styles.refIcon)}>
            <IconTag size={13} />
          </span>
          <span {...stylex.props(styles.tagsTitle)}>Tags</span>
          <span {...stylex.props(styles.count)}>{MOCK_TAGS.length}</span>
        </button>
        {tagsOpen && (
          <ol {...stylex.props(styles.list)}>
            {tags.map((t) => (
              <TagRow key={t.name} tag={t} />
            ))}
          </ol>
        )}
      </section>

      <p {...stylex.props(styles.note)}>
        fixture refs — git refs are not wired to the index yet; expanded rows sample
        ahead commits from the shared fixture history
      </p>
    </div>
  )
}

/**
 * One tag — same hover-revealed trailing actions as branch rows: ↗ opens
 * Browse at the ref label (content stays HEAD, honestly bannered) and the
 * copy button puts the tag name on the clipboard for `rev:` queries.
 */
function TagRow({ tag: t }: { tag: MockTag }) {
  const navigate = useNavigate()
  const [hover, setHover] = useState(false)
  return (
    <li
      {...stylex.props(styles.tagRow)}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
    >
      <span {...stylex.props(styles.refIcon)}>
        <IconTag size={13} />
      </span>
      <span {...stylex.props(styles.name)}>{t.name}</span>
      {t.release && <DisplayBadge severity="low">latest</DisplayBadge>}
      <code {...stylex.props(styles.sha)}>{t.sha}</code>
      {t.message ? (
        <span {...stylex.props(styles.msg)}>{t.message}</span>
      ) : (
        <span {...stylex.props(styles.msg, styles.msgDim)}>lightweight tag</span>
      )}
      <span {...stylex.props(styles.when)}>
        <DisplayTimeAgo value={t.at} />
      </span>
      <span {...stylex.props(styles.rowActions, hover && styles.rowActionsShown)}>
        <button
          type="button"
          title={`browse at '${t.name}' — rev label only, content is HEAD`}
          aria-label={`Browse the tree at '${t.name}'`}
          onClick={() => navigate(`/browse?rev=${encodeURIComponent(t.name)}`)}
          {...stylex.props(iconButtons.mini)}
        >
          <IconArrowUpRight size={11} />
        </button>
        <CopyButton text={t.name} title={`copy '${t.name}'`} size={11} />
      </span>
    </li>
  )
}

/**
 * One ref: a summary hairline row that expands inline to the commits it is
 * ahead of `main` by (sampled from the shared fixture window — the mock has
 * no per-branch log), plus a compare hint and hover-revealed row actions.
 */
function BranchRow({
  branch: b,
  open,
  onToggle,
}: {
  branch: MockBranch
  open: boolean
  onToggle: () => void
}) {
  const navigate = useNavigate()
  // StyleX has no parent-hover/child selectors — reveal the trailing actions
  // from row hover in state (same approach as the shell's resize handle).
  const [hover, setHover] = useState(false)
  const stale = isStale(b.at)
  const ahead = MOCK_COMMITS.slice(0, Math.min(b.aheadBy, 5))

  return (
    <li
      {...stylex.props(styles.item, b.isDefault && styles.rowDefault)}
      onPointerEnter={() => setHover(true)}
      onPointerLeave={() => setHover(false)}
    >
      <div {...stylex.props(styles.rowLine)}>
        <button
          type="button"
          aria-expanded={open}
          onClick={onToggle}
          {...stylex.props(styles.row)}
        >
          <span {...stylex.props(styles.caret, open && styles.caretOpen)}>
            <IconCaretDown size={10} />
          </span>
          <span {...stylex.props(styles.refIcon)}>
            <IconGitBranch size={13} />
          </span>
          <span {...stylex.props(styles.name)}>{b.name}</span>
          {b.isDefault && <DisplayBadge color={false}>default</DisplayBadge>}
          {b.protected && (
            <span {...stylex.props(styles.lock)} title="Protected ref — pushes restricted">
              <LockSimple size={11} />
            </span>
          )}
          {stale && (
            <DisplayBadge color={false} title="untouched for 14+ days">
              stale
            </DisplayBadge>
          )}
          <span {...stylex.props(styles.divergence)}>
            {!b.isDefault && b.aheadBy > 0 && (
              <DisplayBadge severity="low" title={`${b.aheadBy} ahead of main`}>
                <span {...stylex.props(styles.dvgNum)}>↑{b.aheadBy}</span>
              </DisplayBadge>
            )}
            {!b.isDefault && b.behindBy > 0 && (
              <DisplayBadge severity="medium" title={`${b.behindBy} behind main`}>
                <span {...stylex.props(styles.dvgNum)}>↓{b.behindBy}</span>
              </DisplayBadge>
            )}
          </span>
          <code {...stylex.props(styles.sha)}>{b.headSha}</code>
          <span {...stylex.props(styles.msg)}>{b.headMessage}</span>
          <span {...stylex.props(styles.author)}>{b.author}</span>
          <span {...stylex.props(styles.when)}>
            <DisplayTimeAgo value={b.at} />
          </span>
        </button>
        <span {...stylex.props(styles.rowActions, hover && styles.rowActionsShown)}>
          <button
            type="button"
            title={`browse at '${b.name}' — rev label only, content is HEAD`}
            aria-label={`Browse the tree at '${b.name}'`}
            onClick={() =>
              navigate(b.isDefault ? '/browse' : `/browse?rev=${encodeURIComponent(b.name)}`)
            }
            {...stylex.props(iconButtons.mini)}
          >
            <IconArrowUpRight size={11} />
          </button>
          <CopyButton text={b.name} title={`copy '${b.name}'`} size={11} />
          <button
            type="button"
            disabled
            title="endpoint pending — /v1/branches"
            aria-label={`Set ${b.name} as the default branch`}
            {...stylex.props(iconButtons.mini)}
          >
            <IconStar size={11} />
          </button>
        </span>
      </div>
      {open && (
        <div {...stylex.props(styles.detail)}>
          {ahead.map((c) => (
            <div key={c.sha} {...stylex.props(styles.aheadRow)}>
              <span {...stylex.props(styles.refIcon)}>
                <IconGitCommit size={11} />
              </span>
              <code {...stylex.props(styles.sha)} title={c.sha}>
                {c.sha.slice(0, 8)}
              </code>
              <span {...stylex.props(styles.msg)}>{c.message}</span>
              <span {...stylex.props(styles.author)}>{c.author}</span>
              <span {...stylex.props(styles.when)}>
                <DisplayTimeAgo value={c.at} />
              </span>
            </div>
          ))}
          {ahead.length === 0 && (
            <div {...stylex.props(styles.aheadEmpty)}>
              {b.isDefault
                ? 'the default branch — every other ref diverges from here'
                : 'tip matches main — nothing ahead'}
            </div>
          )}
          <div {...stylex.props(styles.detailFoot)}>
            <button
              type="button"
              title="opens the fixture commit history — a real compare endpoint is pending"
              onClick={() => navigate('/commits')}
              {...stylex.props(styles.compare)}
            >
              compare →
            </button>
            <span {...stylex.props(styles.detailNote)}>
              {b.aheadBy > ahead.length
                ? `first ${ahead.length} of ${b.aheadBy} — ahead commits sampled from the shared fixture history`
                : 'ahead commits sampled from the shared fixture history'}
            </span>
          </div>
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
  // Toggleable filter chip next to the search field — same pill language as
  // the badges, active state tinted like `iconButtons.active`.
  chip: {
    display: 'inline-flex',
    alignItems: 'center',
    height: 20,
    paddingLeft: 8,
    paddingRight: 8,
    borderRadius: 10,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: 'transparent',
    color: vars.colorFaint,
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    cursor: 'pointer',
    flexShrink: 0,
  },
  chipOn: {
    color: vars.colorActive,
    borderColor: vars.borderActive,
    backgroundColor: vars.bgActive,
  },
  list: {
    margin: 0,
    padding: 0,
    listStyle: 'none',
    display: 'flex',
    flexDirection: 'column',
  },
  // Flat hairline items — no card chrome; separators only. Each item is the
  // summary row plus (when expanded) the ahead-commit detail below it.
  item: {
    display: 'flex',
    flexDirection: 'column',
    borderTopWidth: { default: 1, ':first-child': 0 },
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
    fontSize: 12,
  },
  // The pinned default ref gets a subtle sunken band so it reads as fixed.
  rowDefault: {
    backgroundColor: vars.bgSunken,
  },
  rowLine: {
    display: 'flex',
    alignItems: 'center',
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
  },
  // The row itself is a button — clicking anywhere toggles the ahead-commit
  // detail; the trailing actions are siblings so buttons never nest.
  row: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    flex: 1,
    minWidth: 0,
    paddingTop: 8,
    paddingBottom: 8,
    paddingLeft: 0,
    paddingRight: 0,
    borderWidth: 0,
    backgroundColor: 'transparent',
    fontFamily: 'inherit',
    fontSize: 12,
    color: vars.colorBase,
    cursor: 'pointer',
    textAlign: 'left',
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
  // Trailing hover actions — opacity-0 space is always reserved so the `when`
  // column never shifts; `:focus-within` keeps them reachable by keyboard.
  rowActions: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 2,
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
  refIcon: {
    display: 'inline-flex',
    color: vars.colorFaint,
    flexShrink: 0,
  },
  name: {
    fontFamily: font.mono,
    fontSize: 12,
    fontWeight: 600,
    color: vars.colorBase,
    flexShrink: 1,
    minWidth: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  lock: {
    display: 'inline-flex',
    color: vars.colorFaint,
    flexShrink: 0,
  },
  // Fixed-width divergence cell keeps the sha column aligned across rows;
  // empty on the default ref (its divergence is definitionally 0/0).
  divergence: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 5,
    width: 96,
    flexShrink: 0,
  },
  dvgNum: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
  },
  sha: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorActive,
    backgroundColor: vars.bgCode,
    borderRadius: 4,
    paddingTop: 1,
    paddingBottom: 1,
    paddingLeft: 5,
    paddingRight: 5,
    width: 60,
    flexShrink: 0,
    textAlign: 'center',
    boxSizing: 'border-box',
  },
  msg: {
    flex: 1,
    minWidth: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
    color: vars.colorMuted,
  },
  author: {
    fontSize: 11,
    color: vars.colorFaint,
    width: 56,
    flexShrink: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
    textAlign: 'right',
  },
  when: {
    flexShrink: 0,
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
    width: 76,
    textAlign: 'right',
  },
  // Inline-expanded ahead commits — same sunken inset detail as CommitsScreen.
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
    // Mirrors the reserved row-actions width (4 + 20 + 2 + 20 + 4) so the
    // `author`/`when` columns in the detail align with the summary row.
    paddingRight: 50,
  },
  aheadRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    paddingTop: 5,
    paddingBottom: 5,
    borderTopWidth: {
      default: 1,
      ':first-child': 0,
    },
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
    fontSize: 11,
  },
  aheadEmpty: {
    paddingTop: 5,
    paddingBottom: 5,
    paddingLeft: 4,
    fontSize: 11,
    color: vars.colorFaint,
  },
  detailFoot: {
    display: 'flex',
    alignItems: 'baseline',
    gap: 10,
    flexWrap: 'wrap',
    paddingTop: 6,
    paddingLeft: 4,
  },
  compare: {
    display: 'inline-flex',
    alignItems: 'center',
    borderWidth: 0,
    paddingTop: 0,
    paddingBottom: 0,
    paddingLeft: 0,
    paddingRight: 0,
    backgroundColor: 'transparent',
    color: vars.colorActive,
    fontFamily: font.mono,
    fontSize: 10,
    cursor: 'pointer',
    textDecoration: {
      default: 'none',
      ':hover': 'underline',
    },
  },
  detailNote: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
  },
  // Collapsible Tags section below the branch list — same hairline rows.
  tagsSection: {
    display: 'flex',
    flexDirection: 'column',
    gap: 6,
  },
  tagsHead: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    width: '100%',
    borderWidth: 0,
    paddingTop: 0,
    paddingBottom: 0,
    paddingLeft: 2,
    paddingRight: 0,
    backgroundColor: 'transparent',
    fontFamily: 'inherit',
    fontSize: 12,
    color: vars.colorMuted,
    cursor: 'pointer',
    textAlign: 'left',
  },
  tagsTitle: {
    fontWeight: 600,
  },
  tagRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    paddingTop: 7,
    paddingBottom: 7,
    // Indent past the caret so tag icons sit under the branch icons above.
    paddingLeft: 20,
    borderTopWidth: { default: 1, ':first-child': 0 },
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
    fontSize: 12,
  },
  msgDim: {
    color: vars.colorFaint,
  },
  note: {
    margin: 0,
    fontSize: 11,
    color: vars.colorFaint,
  },
})
