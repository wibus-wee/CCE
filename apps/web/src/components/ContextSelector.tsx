import { At } from '@phosphor-icons/react'
import * as stylex from '@stylexjs/stylex'
import { useState } from 'react'
import { useNavigate } from 'react-router'
import { MOCK_CONTEXTS, type MockSearchContext } from '../mock/contexts'
import { DisplayBadge } from '../ui/DisplayBadge'
import { IconCaretDown, IconCheckSmall, IconGlobe, IconTag } from '../ui/icons'
import { OverlayDropdown, OverlayDropdownItem, OverlayDropdownSeparator } from '../ui/OverlayMenu'
import { font, vars } from '../ui/tokens.stylex'

/**
 * Context selector — Sourcegraph's `context:` picker seated at the query
 * field's left edge. The trigger mirrors the leading `context:<name>`
 * token in the query text (`global` when unscoped); picking an entry
 * rewrites the token — replace when one leads the query, prepend when it
 * does not, strip entirely for `global` (unscoped, honest).
 *
 * The token is the whole contract: the backend passes `context:` through
 * verbatim, so the scopes listed here come from `src/mock/contexts` and
 * are never presented as resolved index truth.
 */

/** A `context:` token only scopes the query when it leads it (SG grammar). */
const CONTEXT_TOKEN = /^\s*context:(\S+)\s*/i

/** The leading `context:<name>` value, or `undefined` when unscoped. */
function activeContext(query: string): string | undefined {
  return CONTEXT_TOKEN.exec(query)?.[1]
}

/**
 * Write the scope into the query text: replace a leading `context:` token,
 * prepend one when the query opens with plain text, and drop the token for
 * `global` — the unscoped scope carries no token.
 */
function queryWithContext(query: string, name: string): string {
  const rest = query.replace(CONTEXT_TOKEN, '').trimStart()
  return name === 'global' ? rest : `context:${name} ${rest}`
}

/** One-line spec readout — same shape ContextsScreen carries on its rows. */
function specSummary(c: MockSearchContext): string {
  const parts = [`repos: ${c.spec.repositories.join(', ') || '—'}`]
  if (c.spec.revisions?.length) parts.push(`rev: ${c.spec.revisions.join(', ')}`)
  if (c.spec.excludeForks) parts.push('no forks')
  if (c.spec.excludeArchived) parts.push('no archived')
  return parts.join(' · ')
}

/** Row glyph — mirrors ContextsScreen: globe for global, @ for auto-user. */
function contextGlyph(name: string) {
  if (name === 'global') return <IconGlobe size={13} />
  if (name.startsWith('@')) return <At size={13} />
  return <IconTag size={13} />
}

export function ContextSelector({
  value,
  onChange,
}: {
  /** The query text — a leading `context:` token marks the active scope. */
  value: string
  /** Receives the query rewritten for the picked scope. */
  onChange: (query: string) => void
}) {
  const navigate = useNavigate()
  const [open, setOpen] = useState(false)
  // An unknown token still displays by name — never crash on stale scopes.
  const active = activeContext(value) ?? 'global'

  return (
    <OverlayDropdown
      open={open}
      onOpenChange={setOpen}
      align="start"
      trigger={
        <button
          type="button"
          aria-label={`Search context: ${active}`}
          title="Search context — scope the query"
          {...stylex.props(styles.trigger)}
        >
          <span {...stylex.props(styles.triggerLabel)}>
            {active === 'global' ? (
              <span {...stylex.props(styles.triggerMuted)}>global</span>
            ) : (
              <>
                <span {...stylex.props(styles.triggerPrefix)}>context:</span>
                {active}
              </>
            )}
          </span>
          <span {...stylex.props(styles.triggerCaret, open && styles.triggerCaretOpen)}>
            <IconCaretDown size={10} />
          </span>
        </button>
      }
    >
      {MOCK_CONTEXTS.map((c) => (
        <OverlayDropdownItem
          key={c.name}
          icon={contextGlyph(c.name)}
          onClick={() => onChange(queryWithContext(value, c.name))}
        >
          <span {...stylex.props(styles.row)}>
            <span {...stylex.props(styles.name)}>{c.name}</span>
            <DisplayBadge color={false}>{c.visibility}</DisplayBadge>
            <span {...stylex.props(styles.desc)}>{c.description}</span>
            <code {...stylex.props(styles.spec)} title={specSummary(c)}>
              {specSummary(c)}
            </code>
            <span {...stylex.props(styles.check)}>
              {active === c.name && <IconCheckSmall size={11} />}
            </span>
          </span>
        </OverlayDropdownItem>
      ))}
      <OverlayDropdownSeparator />
      <OverlayDropdownItem onClick={() => navigate('/contexts')}>
        Manage contexts →
      </OverlayDropdownItem>
      <div {...stylex.props(styles.note)}>context resolution passes through to the query</div>
    </OverlayDropdown>
  )
}

const styles = stylex.create({
  // A full-height segment attached to the field's left edge, parted from
  // the query text by a hairline — Sourcegraph's in-field picker chrome.
  trigger: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 4,
    alignSelf: 'stretch',
    marginLeft: -4,
    paddingTop: 0,
    paddingBottom: 0,
    paddingLeft: 6,
    paddingRight: 6,
    borderWidth: 0,
    borderRightWidth: 1,
    borderRightStyle: 'solid',
    borderRightColor: vars.borderMute,
    borderTopLeftRadius: 5,
    borderBottomLeftRadius: 5,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    color: vars.colorBase,
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 11,
    lineHeight: 1,
    cursor: 'pointer',
    whiteSpace: 'nowrap',
    flexShrink: 0,
    outline: 'none',
    boxShadow: {
      default: 'none',
      ':focus-visible': `0 0 0 2px ${vars.ringPrimary}`,
    },
  },
  triggerLabel: {
    display: 'inline-flex',
    alignItems: 'baseline',
  },
  triggerMuted: {
    color: vars.colorMuted,
  },
  triggerPrefix: {
    color: vars.colorFaint,
  },
  triggerCaret: {
    display: 'inline-flex',
    color: vars.colorFaint,
    transitionProperty: 'transform',
    transitionDuration: '120ms',
  },
  triggerCaretOpen: {
    transform: 'rotate(180deg)',
  },
  // Fixed row width keeps the popup uniform; the description takes the
  // flex space and truncates, the spec summary caps at its own ellipsis.
  row: {
    display: 'flex',
    alignItems: 'center',
    gap: 6,
    width: 384,
    minWidth: 0,
  },
  name: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 11,
    fontWeight: 600,
    color: vars.colorBase,
    flexShrink: 0,
  },
  desc: {
    flex: 1,
    minWidth: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
    fontSize: 11,
    color: vars.colorMuted,
  },
  spec: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    color: vars.colorFaint,
    flexShrink: 0,
    maxWidth: 170,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  // Always-rendered slot keeps rows aligned whether or not a check shows.
  check: {
    width: 12,
    display: 'inline-flex',
    justifyContent: 'center',
    color: vars.colorActive,
    flexShrink: 0,
  },
  note: {
    paddingTop: 2,
    paddingBottom: 4,
    paddingLeft: 8,
    paddingRight: 8,
    fontSize: 10,
    color: vars.colorFaint,
  },
})
