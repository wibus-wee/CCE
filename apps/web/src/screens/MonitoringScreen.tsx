import * as stylex from '@stylexjs/stylex'
import { useEffect, useRef, useState } from 'react'
import { useNavigate } from 'react-router'
import { describeError } from '../api'
import { fileUrl, queryUrl } from '../lib/navigation'
import {
  addMonitor,
  checkMonitor,
  probeMonitor,
  removeMonitor,
  setMonitorEnabled,
  unreadCount,
  useMonitors,
  type Monitor,
  type MonitorHit,
} from '../lib/monitors'
import { queryTokens } from '../lib/querySyntax'
import { ActionButton } from '../ui/ActionButton'
import { ActionIconButton } from '../ui/ActionIconButton'
import { DisplayBadge } from '../ui/DisplayBadge'
import { DisplayFilePath } from '../ui/DisplayFilePath'
import { DisplayTimeAgo } from '../ui/DisplayNumber'
import { FeedbackEmptyState } from '../ui/FeedbackStates'
import { FormTextInput } from '../ui/FormInputs'
import { FormSwitch } from '../ui/FormSwitch'
import {
  IconArrowUpRight,
  IconBell,
  IconCaretDown,
  IconPlus,
  IconX,
} from '../ui/icons'
import { font, vars } from '../ui/tokens.stylex'

/**
 * Code Monitoring — REAL local snapshot watcher (see lib/monitors). A
 * monitor pins a `/v1/search` query to a baseline snapshot+hit set; when
 * the index advances (reindex lands), hits absent from baseline fire an
 * event listing the new matches with deep links. Checks run on screen
 * mount, on the 45s tick while open, and on demand — no daemon push, no
 * background scheduler, and no delivery channels beyond this event log
 * are claimed (email/slack/webhook from the Sourcegraph fixture removed).
 */
const TICK_MS = 45_000

export function MonitoringScreen() {
  const monitors = useMonitors()
  const navigate = useNavigate()
  const [text, setText] = useState('')
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set())
  const [checking, setChecking] = useState(false)
  const busyRef = useRef(false)

  const checkAll = async (list: Monitor[]) => {
    if (busyRef.current) return
    busyRef.current = true
    setChecking(true)
    for (const m of list) {
      if (m.enabled) await checkMonitor(m)
    }
    busyRef.current = false
    setChecking(false)
  }

  // Prime/check on mount, then on a slow tick while the screen is open.
  useEffect(() => {
    void checkAll(monitors)
    const t = setInterval(() => void checkAll(monitors), TICK_MS)
    return () => clearInterval(t)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const create = () => {
    const q = text.trim()
    if (!q) return
    addMonitor(q)
    setText('')
  }

  const toggle = (id: string) =>
    setExpanded((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })

  const unread = unreadCount(monitors)

  return (
    <div {...stylex.props(styles.root)}>
      <header {...stylex.props(styles.head)}>
        <h2 {...stylex.props(styles.title)}>Code monitoring</h2>
        <DisplayBadge severity="low">local</DisplayBadge>
        <span {...stylex.props(styles.count)}>
          {monitors.length} monitor{monitors.length === 1 ? '' : 's'}
          {unread > 0 && ` · ${unread} event${unread === 1 ? '' : 's'}`}
        </span>
        <span {...stylex.props(styles.spacer)} />
        <ActionButton
          size="sm"
          disabled={checking || monitors.every((m) => !m.enabled)}
          title="re-run every enabled monitor against the current snapshot"
          onClick={() => void checkAll(monitors)}
        >
          {checking ? 'checking…' : 'check all now'}
        </ActionButton>
      </header>

      <div {...stylex.props(styles.createRow)}>
        <FormTextInput
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && create()}
          placeholder="query to watch — e.g. type:diff auth or lang:rust unsafe"
          aria-label="New monitor query"
        />
        <ActionButton
          size="sm"
          icon={<IconPlus size={12} />}
          disabled={!text.trim()}
          title="create a monitor — baseline primes on next check"
          onClick={create}
        >
          New monitor
        </ActionButton>
      </div>

      {monitors.length === 0 ? (
        <FeedbackEmptyState
          icon={<IconBell size={20} />}
          title="no monitors"
          description="watch a query — new hits fire an event when the index snapshot advances. Or star a search and pick Watch on the Saved searches screen."
        />
      ) : (
        <ol {...stylex.props(styles.list)}>
          {monitors.map((m) => (
            <MonitorRow
              key={m.id}
              monitor={m}
              open={expanded.has(m.id)}
              onToggle={() => toggle(m.id)}
              onCheck={() => void checkMonitor(m)}
              onEnable={(v) => setMonitorEnabled(m.id, v)}
              onDelete={() => removeMonitor(m.id)}
              onRun={() => navigate(queryUrl(m.query))}
            />
          ))}
        </ol>
      )}

      <p {...stylex.props(styles.note)}>
        Fires when the index snapshot advances — checks run while this screen is
        open and on the 45s tick. Events persist in localStorage; daemon-side
        push and external delivery are not claimed.
      </p>
    </div>
  )
}

// --- rows ---------------------------------------------------------------------

function MonitorRow({
  monitor: m,
  open,
  onToggle,
  onCheck,
  onEnable,
  onDelete,
  onRun,
}: {
  monitor: Monitor
  open: boolean
  onToggle: () => void
  onCheck: () => void
  onEnable: (v: boolean) => void
  onDelete: () => void
  onRun: () => void
}) {
  const navigate = useNavigate()
  const [probe, setProbe] = useState<MonitorHit[]>()
  const [probeErr, setProbeErr] = useState<string>()
  const [probing, setProbing] = useState(false)

  const runProbe = async () => {
    setProbing(true)
    setProbeErr(undefined)
    try {
      setProbe(await probeMonitor(m.query))
    } catch (e) {
      setProbeErr(describeError(e))
    } finally {
      setProbing(false)
    }
  }

  const status = m.lastError
    ? `check failed — ${m.lastError}`
    : !m.baseline
      ? 'baseline pending — first check primes the hit set, nothing fires'
      : `baseline ${m.baseline.snapshotId.slice(0, 13)} · ${m.baseline.hitIds.length} hits pinned`

  return (
    <li {...stylex.props(styles.row)}>
      <div {...stylex.props(styles.rowMain)}>
        <FormSwitch
          checked={m.enabled}
          onCheckedChange={onEnable}
          aria-label={`Enable monitor ${m.query}`}
        />
        <button
          type="button"
          aria-expanded={open}
          onClick={onToggle}
          {...stylex.props(styles.rowButton)}
        >
          <span {...stylex.props(styles.caret, open && styles.caretOpen)}>
            <IconCaretDown size={10} />
          </span>
          <code {...stylex.props(styles.query)}>
            {queryTokens(m.query).map((t, i) =>
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
        {m.events.length > 0 && (
          <DisplayBadge severity="medium">{m.events.length} new</DisplayBadge>
        )}
        <span {...stylex.props(styles.rowMeta)}>
          {m.lastCheckedAt && (
            <>
              checked <DisplayTimeAgo value={m.lastCheckedAt} />
            </>
          )}
        </span>
        <span {...stylex.props(styles.rowActions)}>
          <ActionIconButton
            compact
            icon={<IconArrowUpRight size={10} />}
            tooltip="run query"
            label={`Run ${m.query}`}
            onClick={onRun}
          />
          <ActionButton
            size="sm"
            disabled={!m.enabled}
            title="re-run now against the current snapshot"
            onClick={onCheck}
          >
            check
          </ActionButton>
          <ActionIconButton
            compact
            icon={<IconX size={11} />}
            tooltip="delete monitor"
            label={`Delete monitor ${m.query}`}
            onClick={onDelete}
          />
        </span>
      </div>
      <div {...stylex.props(styles.status)}>{status}</div>

      {open && (
        <div {...stylex.props(styles.detail)}>
          <div {...stylex.props(styles.detailCol)}>
            <div {...stylex.props(styles.detailLabel)}>event log ({m.events.length})</div>
            {m.events.length === 0 && (
              <div {...stylex.props(styles.detailEmpty)}>
                no events yet — fires when a new snapshot adds matches
              </div>
            )}
            {m.events.map((ev) => (
              <div key={ev.id} {...stylex.props(styles.event)}>
                <div {...stylex.props(styles.eventHead)}>
                  <DisplayBadge severity="medium">+{ev.newCount}</DisplayBadge>
                  <code {...stylex.props(styles.snap)}>{ev.snapshotId.slice(0, 13)}</code>
                  <DisplayTimeAgo value={ev.at} />
                </div>
                {ev.newHits.map((h) => (
                  <button
                    key={h.id}
                    type="button"
                    title={h.path ? `open ${h.path}${h.line ? `:${h.line}` : ''}` : h.label}
                    onClick={() => h.path && navigate(fileUrl(h.path, h.line))}
                    {...stylex.props(styles.eventHit)}
                  >
                    {h.path ? <DisplayFilePath path={h.path} /> : h.label}
                    {h.line != null && `:${h.line}`}
                  </button>
                ))}
                {ev.newCount > ev.newHits.length && (
                  <div {...stylex.props(styles.detailEmpty)}>
                    +{ev.newCount - ev.newHits.length} more not listed
                  </div>
                )}
              </div>
            ))}
          </div>
          <div {...stylex.props(styles.detailCol)}>
            <div {...stylex.props(styles.detailLabel)}>
              test run — current hits (baseline untouched)
            </div>
            {!probe && !probeErr && (
              <ActionButton
                size="sm"
                disabled={probing}
                onClick={() => void runProbe()}
                title="run the query now without rolling the baseline"
              >
                {probing ? 'running…' : 'run now'}
              </ActionButton>
            )}
            {probeErr && <div {...stylex.props(styles.detailEmpty)}>probe failed — {probeErr}</div>}
            {probe?.map((h) => (
              <button
                key={h.id}
                type="button"
                title={h.path ? `open ${h.path}${h.line ? `:${h.line}` : ''}` : h.label}
                onClick={() => h.path && navigate(fileUrl(h.path, h.line))}
                {...stylex.props(styles.eventHit)}
              >
                {h.path ? <DisplayFilePath path={h.path} /> : h.label}
                {h.line != null && `:${h.line}`}
              </button>
            ))}
            {probe && probe.length === 0 && (
              <div {...stylex.props(styles.detailEmpty)}>no current matches</div>
            )}
          </div>
        </div>
      )}
    </li>
  )
}

// --- styles ---------------------------------------------------------------------

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
  createRow: {
    display: 'flex',
    gap: 8,
    alignItems: 'center',
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
  },
  row: {
    paddingTop: 9,
    paddingBottom: 9,
    paddingLeft: 12,
    paddingRight: 10,
    borderBottomWidth: 1,
    borderBottomStyle: 'solid',
    borderBottomColor: vars.borderMute,
  },
  rowMain: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    minWidth: 0,
  },
  rowButton: {
    flex: 1,
    minWidth: 0,
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    borderWidth: 0,
    backgroundColor: 'transparent',
    cursor: 'pointer',
    padding: 0,
    fontFamily: 'inherit',
    textAlign: 'left',
  },
  caret: {
    display: 'inline-flex',
    color: vars.colorFaint,
    transform: 'rotate(-90deg)',
    transition: 'transform 120ms',
  },
  caretOpen: {
    transform: 'rotate(0deg)',
  },
  query: {
    fontFamily: font.mono,
    fontSize: 12,
    color: vars.colorBase,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
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
  rowMeta: {
    fontSize: 10.5,
    color: vars.colorFaint,
    flexShrink: 0,
  },
  rowActions: {
    display: 'flex',
    alignItems: 'center',
    gap: 6,
    flexShrink: 0,
  },
  status: {
    paddingTop: 3,
    paddingLeft: 34,
    fontSize: 10.5,
    color: vars.colorFaint,
  },
  detail: {
    display: 'flex',
    gap: 20,
    marginTop: 10,
    paddingTop: 10,
    paddingBottom: 4,
    paddingLeft: 10,
    paddingRight: 10,
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
    backgroundColor: vars.bgSunken,
    borderRadius: 6,
  },
  detailCol: {
    flex: 1,
    minWidth: 0,
    display: 'flex',
    flexDirection: 'column',
    gap: 4,
  },
  detailLabel: {
    fontSize: 9.5,
    fontWeight: 600,
    letterSpacing: '0.06em',
    textTransform: 'uppercase',
    color: vars.colorFaint,
    paddingBottom: 4,
  },
  detailEmpty: {
    fontSize: 10.5,
    color: vars.colorFaint,
    paddingTop: 3,
    paddingBottom: 3,
  },
  event: {
    display: 'flex',
    flexDirection: 'column',
    gap: 3,
    paddingBottom: 8,
    marginBottom: 6,
    borderBottomWidth: 1,
    borderBottomStyle: 'solid',
    borderBottomColor: vars.borderMute,
  },
  eventHead: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    fontSize: 10.5,
    color: vars.colorMuted,
  },
  snap: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorMuted,
  },
  eventHit: {
    display: 'flex',
    alignItems: 'center',
    minWidth: 0,
    borderWidth: 0,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    fontFamily: font.mono,
    fontSize: 10.5,
    color: vars.colorBase,
    paddingTop: 3,
    paddingBottom: 3,
    paddingLeft: 4,
    paddingRight: 4,
    cursor: 'pointer',
    textAlign: 'left',
    borderRadius: 4,
  },
  note: {
    margin: 0,
    paddingTop: 4,
    fontSize: 10.5,
    color: vars.colorFaint,
  },
})
