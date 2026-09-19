import * as stylex from '@stylexjs/stylex'
import { useState } from 'react'
import { useNavigate } from 'react-router'
import { addMonitor } from '../lib/monitors'
import { queryUrl } from '../lib/navigation'
import { queryTokens } from '../lib/querySyntax'
import { toggleSaved, useSearchHistory } from '../lib/searchHistory'
import { ActionIconButton } from '../ui/ActionIconButton'
import { CopyButton } from '../ui/CopyButton'
import { DisplayBadge } from '../ui/DisplayBadge'
import { FeedbackEmptyState } from '../ui/FeedbackStates'
import { FormSearchField } from '../ui/FormSearchField'
import { IconArrowUpRight, IconBell, IconSearch, IconStar } from '../ui/icons'
import { font, vars } from '../ui/tokens.stylex'

/**
 * Saved searches — REAL, not fixture. The store is the same localStorage
 * `useSearchHistory` the sidebar and the query-screen star button write
 * to: saved = starred queries, recents = the last eight runs. Every action
 * here is a real write or a real navigation — Run re-issues the query,
 * Watch turns it into a snapshot monitor (see /code-monitoring), the star
 * unsaves/saves for real. Owners, namespaces and alert toggles from the
 * Sourcegraph fixture are gone — they don't exist locally, so they aren't
 * claimed.
 */
export function SavedSearchesScreen() {
  const navigate = useNavigate()
  const { recents, saved } = useSearchHistory()
  const [text, setText] = useState('')
  const [watched, setWatched] = useState<string>()

  const needle = text.trim().toLowerCase()
  const filter = (list: string[]) =>
    !needle ? list : list.filter((q) => q.toLowerCase().includes(needle))

  const savedRows = filter(saved)
  // Recents already saved get a filled star too — the row reads "saved
  // via recents", unsaving works the same.
  const recentRows = filter(recents.filter((q) => !saved.includes(q)))

  const watch = (q: string) => {
    addMonitor(q)
    setWatched(q)
    navigate('/code-monitoring')
  }

  return (
    <div {...stylex.props(styles.root)}>
      <header {...stylex.props(styles.head)}>
        <h2 {...stylex.props(styles.title)}>Saved searches</h2>
        <DisplayBadge severity="low">local</DisplayBadge>
        <span {...stylex.props(styles.count)}>
          {saved.length} saved · {recents.length} recent
        </span>
        <span {...stylex.props(styles.spacer)} />
        <div {...stylex.props(styles.search)}>
          <FormSearchField
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder="filter queries"
            onClear={() => setText('')}
            aria-label="Filter saved searches"
          />
        </div>
      </header>

      {saved.length === 0 && recents.length === 0 && (
        <FeedbackEmptyState
          icon={<IconSearch size={20} />}
          title="no searches yet"
          description="run a query — star it from the results page or the recents list below"
        />
      )}

      {saved.length > 0 && (
        <>
          <div {...stylex.props(styles.section)}>starred ({savedRows.length})</div>
          {savedRows.length === 0 && (
            <p {...stylex.props(styles.empty)}>no starred queries match the filter</p>
          )}
          <ol {...stylex.props(styles.list)}>
            {savedRows.map((q) => (
              <QueryRow
                key={q}
                query={q}
                starred
                watched={watched === q}
                onStar={() => toggleSaved(q)}
                onRun={() => navigate(queryUrl(q))}
                onWatch={() => watch(q)}
              />
            ))}
          </ol>
        </>
      )}

      {recentRows.length > 0 && (
        <>
          <div {...stylex.props(styles.section)}>recents ({recentRows.length})</div>
          <ol {...stylex.props(styles.list)}>
            {recentRows.map((q) => (
              <QueryRow
                key={q}
                query={q}
                starred={false}
                watched={watched === q}
                onStar={() => toggleSaved(q)}
                onRun={() => navigate(queryUrl(q))}
                onWatch={() => watch(q)}
              />
            ))}
          </ol>
        </>
      )}

      <p {...stylex.props(styles.note)}>
        Persisted in this browser's localStorage — the same store the sidebar and
        the query-screen star write to. <b>Watch</b> converts a query into a
        snapshot monitor on the Code monitoring screen.
      </p>
    </div>
  )
}

function QueryRow({
  query,
  starred,
  watched,
  onStar,
  onRun,
  onWatch,
}: {
  query: string
  starred: boolean
  watched: boolean
  onStar: () => void
  onRun: () => void
  onWatch: () => void
}) {
  return (
    <li {...stylex.props(styles.row)}>
      <button
        type="button"
        title={starred ? 'remove from saved' : 'save this search'}
        aria-label={starred ? `Unsave ${query}` : `Save ${query}`}
        aria-pressed={starred}
        onClick={onStar}
        {...stylex.props(styles.star, starred && styles.starOn)}
      >
        <IconStar size={12} filled={starred} />
      </button>
      <button
        type="button"
        title={`run — ${query}`}
        onClick={onRun}
        {...stylex.props(styles.main)}
      >
        <code {...stylex.props(styles.query)}>
          {queryTokens(query).map((t, i) =>
            t.kind ? (
              <span key={i} {...stylex.props(styles[`qtok_${t.kind}`])}>
                {t.text}
              </span>
            ) : (
              t.text
            ),
          )}
        </code>
      </button>
      <span {...stylex.props(styles.rowActions)}>
        <ActionIconButton
          compact
          icon={<IconBell size={11} />}
          tooltip={watched ? 'monitor created' : 'watch — monitor on snapshot change'}
          label={`Watch ${query}`}
          onClick={onWatch}
        />
        <CopyButton text={query} title="copy query" label={`Copy ${query}`} />
        <ActionIconButton
          compact
          icon={<IconArrowUpRight size={11} />}
          tooltip="run this query"
          label={`Run ${query}`}
          onClick={onRun}
        />
      </span>
    </li>
  )
}

const styles = stylex.create({
  root: {
    display: 'flex',
    flexDirection: 'column',
    gap: 12,
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
  section: {
    fontSize: 10,
    fontWeight: 600,
    letterSpacing: '0.06em',
    textTransform: 'uppercase',
    color: vars.colorFaint,
    paddingTop: 4,
  },
  list: {
    listStyle: 'none',
    margin: 0,
    padding: 0,
    borderRadius: 8,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgRaised,
    overflow: 'hidden',
  },
  row: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    paddingTop: 8,
    paddingBottom: 8,
    paddingLeft: 12,
    paddingRight: 10,
    borderBottomWidth: 1,
    borderBottomStyle: 'solid',
    borderBottomColor: vars.borderMute,
  },
  star: {
    display: 'flex',
    alignItems: 'center',
    borderWidth: 0,
    backgroundColor: 'transparent',
    color: vars.colorFaint,
    cursor: 'pointer',
    padding: 2,
  },
  starOn: {
    color: vars.scaleMedium,
  },
  main: {
    flex: 1,
    minWidth: 0,
    borderWidth: 0,
    backgroundColor: 'transparent',
    textAlign: 'left',
    cursor: 'pointer',
    padding: 0,
    fontFamily: 'inherit',
  },
  query: {
    fontFamily: font.mono,
    fontSize: 12,
    color: vars.colorBase,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
    display: 'block',
  },
  qtok_filter: {
    color: vars.accentInfo,
  },
  qtok_op: {
    color: vars.scaleMedium,
    fontWeight: 600,
  },
  qtok_str: {
    color: vars.scaleLow,
  },
  qtok_paren: {
    color: vars.colorFaint,
  },
  rowActions: {
    display: 'flex',
    alignItems: 'center',
    gap: 4,
    flexShrink: 0,
  },
  empty: {
    margin: 0,
    fontSize: 11.5,
    color: vars.colorFaint,
  },
  note: {
    margin: 0,
    paddingTop: 4,
    fontSize: 10.5,
    color: vars.colorFaint,
  },
})
