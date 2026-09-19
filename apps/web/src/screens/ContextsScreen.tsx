// No IconAt/IconGlobe/IconLock in ../ui/icons — the contract is to pull
// missing glyphs from Phosphor at the call site (same set the wrappers use).
import { At, Globe, LockSimple } from '@phosphor-icons/react'
import * as stylex from '@stylexjs/stylex'
import { useMemo, useState } from 'react'
import { useNavigate } from 'react-router'
import { queryUrl } from '../lib/navigation'
import { MOCK_CONTEXTS, type MockSearchContext } from '../mock/contexts'
import { ActionButton } from '../ui/ActionButton'
import { DisplayBadge } from '../ui/DisplayBadge'
import { DisplayTimeAgo } from '../ui/DisplayNumber'
import { FeedbackEmptyState } from '../ui/FeedbackStates'
import { FormCheckbox } from '../ui/FormCheckbox'
import { FormField } from '../ui/FormField'
import { FormTextInput, FormTextarea } from '../ui/FormInputs'
import { FormSearchField } from '../ui/FormSearchField'
import { FormSegmentedControl } from '../ui/FormRadioGroup'
import { FormSelect } from '../ui/FormSelect'
import { IconCaretDown, IconPlus, IconSearch, IconTag } from '../ui/icons'
import { LayoutBreadcrumb } from '../ui/LayoutStructure'
import { font, vars } from '../ui/tokens.stylex'

/**
 * Search contexts — Sourcegraph's `context:` selector page: the named
 * repo/revision scopes a query can be pinned to, auto contexts (global,
 * @user) first. Clicking a row expands the full spec inline with a
 * "search in this context" action that runs for real — the `context:`
 * token goes through to the query text verbatim.
 *
 * MOCK: `/v1/contexts` doesn't exist and the engine does not resolve a
 * `context:` token yet, so rows come from `src/mock/contexts` and the
 * screen carries a `preview` badge — fixture scopes are never presented
 * as live index truth. `New context` opens an inline create panel in the
 * same spirit: fields edit a local draft whose spec renders live, but
 * Create stays disabled until the endpoint lands — nothing is saved.
 */

/** auto scopes pin to the top (Sourcegraph order), then public, private. */
const VIS_ORDER: Record<MockSearchContext['visibility'], number> = {
  auto: 0,
  public: 1,
  private: 2,
}

/**
 * The create panel's picks — `context:` names are query tokens, so the
 * charset matches a token's, and owners mirror the fixture's (`global`
 * is unowned/system; `@…` namespaces a user or team).
 */
const CONTEXT_NAME_RE = /^[a-zA-Z0-9_.-]+$/

const OWNER_OPTIONS = [
  { value: 'global', label: 'global' },
  { value: '@wibus', label: '@wibus' },
  { value: '@team', label: '@team' },
]

const DRAFT_VIS_OPTIONS = [
  { value: 'public', label: 'public' },
  { value: 'private', label: 'private' },
]

/** Repo globs / refs accept one-per-line or comma-separated input. */
function parseSpecList(text: string): string[] {
  return text
    .split(/[\n,]+/)
    .map((s) => s.trim())
    .filter(Boolean)
}

/** The one-line spec readout a collapsed row carries. */
function specSummary(c: MockSearchContext): string {
  const parts = [`repos: ${c.spec.repositories.join(', ') || '—'}`]
  if (c.spec.revisions?.length) parts.push(`rev: ${c.spec.revisions.join(', ')}`)
  if (c.spec.excludeForks) parts.push('no forks')
  if (c.spec.excludeArchived) parts.push('no archived')
  return parts.join(' · ')
}

/** The expanded detail — the whole spec as declarative mono lines. */
function specBlock(c: MockSearchContext): string {
  const lines = [`context: ${c.name}`, 'repositories:']
  for (const repo of c.spec.repositories) lines.push(`  - ${repo}`)
  if (c.spec.revisions?.length) {
    lines.push('revisions:')
    for (const rev of c.spec.revisions) lines.push(`  - ${rev}`)
  }
  if (c.spec.excludeForks != null) lines.push(`excludeForks: ${c.spec.excludeForks}`)
  if (c.spec.excludeArchived != null) lines.push(`excludeArchived: ${c.spec.excludeArchived}`)
  return lines.join('\n')
}

export function ContextsScreen() {
  const navigate = useNavigate()
  const [text, setText] = useState('')
  const [vis, setVis] = useState('all')
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(() => new Set())
  const [creating, setCreating] = useState(false)

  const rows = useMemo(() => {
    const needle = text.trim().toLowerCase()
    return MOCK_CONTEXTS.filter((c) => {
      if (vis !== 'all' && c.visibility !== vis) return false
      if (!needle) return true
      return `${c.name} ${c.description} ${c.owner ?? ''} ${specSummary(c)}`
        .toLowerCase()
        .includes(needle)
    }).sort(
      (a, b) =>
        VIS_ORDER[a.visibility] - VIS_ORDER[b.visibility] ||
        a.name.localeCompare(b.name),
    )
  }, [text, vis])

  // Visibility segments carry per-value counts — the chip+count filter
  // idiom the batch/index toolbars use, as a single-select control.
  const visOptions = useMemo(() => {
    const count = (v: string) => MOCK_CONTEXTS.filter((c) => c.visibility === v).length
    return [
      { value: null, label: 'all', count: MOCK_CONTEXTS.length },
      { value: 'auto', count: count('auto') },
      { value: 'public', count: count('public') },
      { value: 'private', count: count('private') },
    ]
  }, [])

  const toggle = (name: string) =>
    setExpanded((prev) => {
      const next = new Set(prev)
      if (next.has(name)) next.delete(name)
      else next.add(name)
      return next
    })

  // Create/edit flows live on their own view with a breadcrumb back —
  // the same convention Monitoring uses, not an inline panel.
  if (creating) {
    return <CreateContextView onBack={() => setCreating(false)} />
  }

  return (
    <div {...stylex.props(styles.root)}>
      <header {...stylex.props(styles.head)}>
        <h2 {...stylex.props(styles.title)}>Search contexts</h2>
        <DisplayBadge severity="medium">preview</DisplayBadge>
        <span {...stylex.props(styles.count)}>
          {rows.length === MOCK_CONTEXTS.length
            ? `${rows.length} contexts`
            : `${rows.length} of ${MOCK_CONTEXTS.length} contexts`}
        </span>
        <span {...stylex.props(styles.spacer)} />
        <ActionButton
          size="sm"
          icon={<IconPlus size={12} />}
          onClick={() => setCreating(true)}
          title="Open a local-draft create form — the endpoint is pending, so nothing is saved"
        >
          New context
        </ActionButton>
        <FormSegmentedControl
          options={visOptions}
          value={vis === 'all' ? null : vis}
          onValueChange={(v) => setVis(v ?? 'all')}
        />
        <div {...stylex.props(styles.search)}>
          <FormSearchField
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder="name, description, or spec"
            onClear={() => setText('')}
            aria-label="Filter contexts"
          />
        </div>
      </header>

      {rows.length === 0 ? (
        <FeedbackEmptyState
          icon={<Globe size={18} />}
          title="No matching contexts"
          description={
            text.trim()
              ? `No context name, description, or spec contains '${text.trim()}'.`
              : `No ${vis} contexts in the fixture.`
          }
        />
      ) : (
        <ol {...stylex.props(styles.list)}>
          {rows.map((c) => (
            <ContextRow
              key={c.name}
              ctx={c}
              open={expanded.has(c.name)}
              onToggle={() => toggle(c.name)}
              onSearch={() => navigate(queryUrl(`context:${c.name} `))}
            />
          ))}
        </ol>
      )}

      <p {...stylex.props(styles.note)}>
        Fixture data — the /v1/contexts endpoint is pending; the scope token is
        passed through to the query
      </p>
    </div>
  )
}

/** One context: summary hairline row + inline-expanded spec detail. */
function ContextRow({
  ctx,
  open,
  onToggle,
  onSearch,
}: {
  ctx: MockSearchContext
  open: boolean
  onToggle: () => void
  onSearch: () => void
}) {
  return (
    <li {...stylex.props(styles.item)}>
      <button
        type="button"
        aria-expanded={open}
        onClick={onToggle}
        {...stylex.props(styles.row)}
      >
        <span {...stylex.props(styles.caret, open && styles.caretOpen)}>
          <IconCaretDown size={10} />
        </span>
        <span {...stylex.props(styles.glyph)}>
          {ctx.name.startsWith('@') ? (
            <At size={13} />
          ) : ctx.name === 'global' ? (
            <Globe size={13} />
          ) : (
            <IconTag size={13} />
          )}
        </span>
        <span {...stylex.props(styles.name)}>
          {ctx.name.startsWith('@') && <span {...stylex.props(styles.nameSigil)}>@</span>}
          {ctx.name.startsWith('@') ? ctx.name.slice(1) : ctx.name}
        </span>
        {ctx.visibility === 'auto' ? (
          <DisplayBadge color={false}>auto</DisplayBadge>
        ) : ctx.visibility === 'public' ? (
          <DisplayBadge severity="low">public</DisplayBadge>
        ) : (
          <DisplayBadge severity="medium" icon={<LockSimple size={9} />}>
            private
          </DisplayBadge>
        )}
        <span {...stylex.props(styles.desc)}>{ctx.description}</span>
        <code {...stylex.props(styles.specChip)} title={specSummary(ctx)}>
          {specSummary(ctx)}
        </code>
        <span {...stylex.props(styles.when)}>
          {ctx.updatedAt && <DisplayTimeAgo value={ctx.updatedAt} />}
        </span>
      </button>
      {open && (
        <div {...stylex.props(styles.detail)}>
          <pre {...stylex.props(styles.specBlock)}>{specBlock(ctx)}</pre>
          <div {...stylex.props(styles.detailMeta)}>
            <span>
              owner{' '}
              <span {...stylex.props(styles.metaValue)}>
                {ctx.owner ? `@${ctx.owner}` : 'system'}
              </span>
            </span>
            <span {...stylex.props(styles.metaSep)}>·</span>
            <span>
              token <code {...stylex.props(styles.token)}>context:{ctx.name}</code>
            </span>
            {ctx.updatedAt && (
              <>
                <span {...stylex.props(styles.metaSep)}>·</span>
                <span>
                  updated <DisplayTimeAgo value={ctx.updatedAt} />
                </span>
              </>
            )}
            <span {...stylex.props(styles.detailSpacer)} />
            <ActionButton
              size="sm"
              icon={<IconSearch size={12} />}
              onClick={onSearch}
              title={`Run a query scoped to context:${ctx.name}`}
            >
              Search in this context
            </ActionButton>
          </div>
        </div>
      )}
    </li>
  )
}

/**
 * Create-context view — Sourcegraph's "Create search context" form kept
 * as a local draft on its own screen (the create/edit convention shared
 * with Monitoring: a view reached from the list header, breadcrumb back).
 * name/owner/visibility plus the repo/revision spec fields, with the
 * declarative spec block re-rendered live beside the form via the same
 * `specBlock` the expanded rows use. `Create context` stays disabled —
 * `/v1/contexts` is pending, so nothing is saved and no row is added.
 */
function CreateContextView({ onBack }: { onBack: () => void }) {
  const [name, setName] = useState('')
  const [owner, setOwner] = useState('global')
  const [draftVis, setDraftVis] = useState('public')
  const [repos, setRepos] = useState('**')
  const [revs, setRevs] = useState('')
  const [excludeForks, setExcludeForks] = useState(false)
  const [excludeArchived, setExcludeArchived] = useState(false)

  /** Empty reads as "required", not an error — flag typed-but-invalid input. */
  const nameInvalid = name.length > 0 && !CONTEXT_NAME_RE.test(name)

  /** Fixture-shaped draft so `specBlock` renders it verbatim, live. */
  const draft = useMemo<MockSearchContext>(
    () => ({
      name: name.trim() || 'new-context',
      description: '',
      visibility: draftVis === 'private' ? 'private' : 'public',
      owner: owner === 'global' ? undefined : owner.slice(1),
      spec: {
        repositories: parseSpecList(repos),
        revisions: parseSpecList(revs),
        excludeForks: excludeForks || undefined,
        excludeArchived: excludeArchived || undefined,
      },
    }),
    [name, owner, draftVis, repos, revs, excludeForks, excludeArchived],
  )

  return (
    <div {...stylex.props(styles.root)}>
      <LayoutBreadcrumb
        items={[
          { label: 'Search contexts', onClick: onBack },
          { label: 'New context' },
        ]}
      />

      <header {...stylex.props(styles.head)}>
        <h2 {...stylex.props(styles.title)}>New context</h2>
        <DisplayBadge severity="medium">preview</DisplayBadge>
        <span {...stylex.props(styles.faintNote)}>local draft — nothing is saved</span>
      </header>

      <div {...stylex.props(styles.createBody)}>
        <div {...stylex.props(styles.createForm)}>
          <FormField
            label="Name"
            required
            description="The token after context: — letters, digits, `.`, `_`, `-` only."
            error={
              nameInvalid
                ? 'invalid name — letters, digits, `.`, `_`, `-` only'
                : undefined
            }
          >
            <FormTextInput
              mono
              invalid={nameInvalid}
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="cce-engine"
            />
          </FormField>
          <div {...stylex.props(styles.createRow)}>
            <div {...stylex.props(styles.createField)}>
              <FormField label="Owner">
                <FormSelect
                  options={OWNER_OPTIONS}
                  value={owner}
                  onValueChange={setOwner}
                />
              </FormField>
            </div>
            <div {...stylex.props(styles.createField)}>
              <FormField label="Visibility">
                <FormSelect
                  options={DRAFT_VIS_OPTIONS}
                  value={draftVis}
                  onValueChange={setDraftVis}
                />
              </FormField>
            </div>
          </div>
          <FormField
            label="Repositories"
            description="Repo globs — one per line; `**` is everything in the index."
          >
            <FormTextarea
              mono
              rows={3}
              value={repos}
              onChange={(e) => setRepos(e.target.value)}
              placeholder={'crates/*\napps/web'}
            />
          </FormField>
          <FormField
            label="Revisions"
            description="Optional — refs inside those repos, comma- or line-separated."
          >
            <FormTextInput
              mono
              value={revs}
              onChange={(e) => setRevs(e.target.value)}
              placeholder="main, release/*"
            />
          </FormField>
          <div {...stylex.props(styles.createChecks)}>
            <FormCheckbox
              checked={excludeForks}
              onCheckedChange={setExcludeForks}
              label="Exclude forks"
            />
            <FormCheckbox
              checked={excludeArchived}
              onCheckedChange={setExcludeArchived}
              label="Exclude archived"
            />
          </div>
        </div>
        <div {...stylex.props(styles.createSpec)}>
          <span {...stylex.props(styles.createLabel)}>spec</span>
          <pre {...stylex.props(styles.specBlock)}>{specBlock(draft)}</pre>
          <div {...stylex.props(styles.detailMeta)}>
            <span>
              token{' '}
              <code {...stylex.props(styles.token)}>context:{draft.name}</code>
            </span>
            <span {...stylex.props(styles.metaSep)}>·</span>
            <span>
              owner{' '}
              <span {...stylex.props(styles.metaValue)}>
                {draft.owner ? `@${draft.owner}` : 'system'}
              </span>
            </span>
            <span {...stylex.props(styles.metaSep)}>·</span>
            <span {...stylex.props(styles.metaValue)}>{draft.visibility}</span>
          </div>
        </div>
      </div>
      <div {...stylex.props(styles.createFoot)}>
        <ActionButton
          size="sm"
          variant="primary"
          disabled
          title="endpoint pending — /v1/contexts"
        >
          Create context
        </ActionButton>
        <ActionButton size="sm" onClick={onBack}>
          Cancel
        </ActionButton>
        <span {...stylex.props(styles.faintNote)}>
          endpoint pending — /v1/contexts
        </span>
      </div>
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
  list: {
    margin: 0,
    padding: 0,
    listStyle: 'none',
    display: 'flex',
    flexDirection: 'column',
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
  // Flat hairline rows — no card chrome; separators and hover only.
  row: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    width: '100%',
    borderWidth: 0,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    fontFamily: 'inherit',
    fontSize: 12,
    color: vars.colorBase,
    paddingTop: 8,
    paddingBottom: 8,
    paddingLeft: 4,
    paddingRight: 4,
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
  glyph: {
    display: 'inline-flex',
    color: vars.colorFaint,
    flexShrink: 0,
  },
  name: {
    fontFamily: font.mono,
    fontSize: 12,
    fontWeight: 600,
    color: vars.colorBase,
    flexShrink: 0,
  },
  // The '@' sigil on auto user contexts reads as a namespace marker.
  nameSigil: {
    color: vars.colorActive,
  },
  desc: {
    flex: 1,
    minWidth: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
    color: vars.colorMuted,
  },
  specChip: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    color: vars.colorMuted,
    backgroundColor: vars.bgCode,
    borderRadius: 4,
    paddingTop: 1,
    paddingBottom: 1,
    paddingLeft: 5,
    paddingRight: 5,
    maxWidth: 280,
    flexShrink: 1,
    minWidth: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  // Fixed-width time cell keeps the row tail aligned; auto contexts are
  // computed, not edited, so theirs stays empty.
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
    gap: 8,
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
    backgroundColor: vars.bgSunken,
    paddingTop: 10,
    paddingBottom: 10,
    paddingLeft: 32,
    paddingRight: 12,
  },
  specBlock: {
    margin: 0,
    padding: 10,
    borderRadius: 6,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgCode,
    fontFamily: font.mono,
    fontSize: 11,
    lineHeight: '1.55',
    whiteSpace: 'pre-wrap',
    overflowWrap: 'anywhere',
    color: vars.colorBase,
    overflow: 'auto',
    maxHeight: 240,
  },
  detailMeta: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    flexWrap: 'wrap',
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
  },
  metaValue: {
    color: vars.colorMuted,
  },
  metaSep: {
    opacity: vars.opFade,
  },
  token: {
    color: vars.colorActive,
  },
  detailSpacer: {
    flex: 1,
  },
  note: {
    margin: 0,
    fontSize: 11,
    color: vars.colorFaint,
  },
  createLabel: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
    textTransform: 'uppercase',
    letterSpacing: '0.06em',
  },
  createBody: {
    display: 'flex',
    gap: 16,
    flexWrap: 'wrap',
  },
  createForm: {
    display: 'flex',
    flexDirection: 'column',
    gap: 12,
    flexGrow: 1,
    flexShrink: 1,
    flexBasis: 300,
    maxWidth: 460,
    minWidth: 240,
  },
  createRow: {
    display: 'flex',
    gap: 12,
  },
  createField: {
    flexGrow: 1,
    flexShrink: 1,
    flexBasis: 0,
    minWidth: 0,
    display: 'flex',
    flexDirection: 'column',
  },
  createChecks: {
    display: 'flex',
    alignItems: 'center',
    gap: 16,
    flexWrap: 'wrap',
  },
  createSpec: {
    display: 'flex',
    flexDirection: 'column',
    gap: 6,
    flexGrow: 1,
    flexShrink: 1,
    flexBasis: 240,
    minWidth: 220,
  },
  createFoot: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    flexWrap: 'wrap',
  },
  faintNote: {
    fontSize: 11,
    color: vars.colorFaint,
  },
})
