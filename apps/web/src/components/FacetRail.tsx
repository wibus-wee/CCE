import * as stylex from '@stylexjs/stylex'
import { useState } from 'react'
import type { ActiveFilters, FacetGroup } from '../lib/facets'
import { IconCaretDown, IconCheck } from '../ui/icons'
import { font, vars } from '../ui/tokens.stylex'

/**
 * Sourcegraph-style filter rail — collapsible facet groups with
 * right-aligned hit counts and multi-select options. Counts are computed
 * client-side from the returned page of hits (see lib/facets).
 */
export function FacetRail({
  groups,
  active,
  onToggle,
}: {
  groups: FacetGroup[]
  active: ActiveFilters
  onToggle: (groupId: string, value: string) => void
}) {
  const [aggGroup, setAggGroup] = useState<string>()
  const aggregate = groups.find((g) => g.id === aggGroup) ?? groups[0]

  return (
    <aside aria-label="Result filters" {...stylex.props(styles.rail)}>
      {aggregate && (
        <section {...stylex.props(styles.group)}>
          <div {...stylex.props(styles.aggHead)}>
            <span {...stylex.props(styles.aggTitle)}>Group by</span>
            <select
              value={aggregate.id}
              onChange={(e) => setAggGroup(e.target.value)}
              aria-label="Aggregation dimension"
              {...stylex.props(styles.aggSelect)}
            >
              {groups.map((g) => (
                <option key={g.id} value={g.id}>
                  {g.label.toLowerCase()}
                </option>
              ))}
            </select>
          </div>
          <div {...stylex.props(styles.aggBars)} role="list">
            {aggregate.options.slice(0, 8).map((option) => {
              const max = aggregate.options[0]?.count ?? 1
              const on = (active[aggregate.id] ?? new Set()).has(option.value)
              return (
                <button
                  key={option.value}
                  type="button"
                  role="listitem"
                  aria-pressed={on}
                  onClick={() => onToggle(aggregate.id, option.value)}
                  title={`${option.count} hits — ${on ? 'remove filter' : 'filter to this'}`}
                  {...stylex.props(styles.aggRow, on && styles.aggRowOn)}
                >
                  <span {...stylex.props(styles.aggLabel)}>{option.label.replaceAll('_', ' ')}</span>
                  <span {...stylex.props(styles.aggTrack)}>
                    <span
                      {...stylex.props(styles.aggFill, on && styles.aggFillOn)}
                      style={{ width: `${Math.max(4, (option.count / max) * 100)}%` }}
                    />
                  </span>
                  <span {...stylex.props(styles.optionCount)}>{option.count}</span>
                </button>
              )
            })}
          </div>
        </section>
      )}
      {groups.map((group) => (
        <FacetSection
          key={group.id}
          group={group}
          selected={active[group.id] ?? new Set()}
          onToggle={(value) => onToggle(group.id, value)}
        />
      ))}
    </aside>
  )
}

function FacetSection({
  group,
  selected,
  onToggle,
}: {
  group: FacetGroup
  selected: Set<string>
  onToggle: (value: string) => void
}) {
  const [open, setOpen] = useState(true)
  const [expanded, setExpanded] = useState(false)
  const LIMIT = 6
  const options = expanded ? group.options : group.options.slice(0, LIMIT)

  return (
    <section {...stylex.props(styles.group)}>
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        {...stylex.props(styles.groupHead)}
      >
        <span {...stylex.props(styles.caret, open && styles.caretOpen)}>
          <IconCaretDown size={10} />
        </span>
        {group.label}
      </button>
      {open && (
        <div role="group" aria-label={group.label}>
          {options.map((option) => {
            const on = selected.has(option.value)
            return (
              <button
                key={option.value}
                type="button"
                aria-pressed={on}
                onClick={() => onToggle(option.value)}
                {...stylex.props(styles.option, on && styles.optionOn)}
              >
                <span {...stylex.props(styles.check, on && styles.checkOn)}>
                  {on && <IconCheck size={10} />}
                </span>
                <span {...stylex.props(styles.optionLabel)}>{option.label.replaceAll('_', ' ')}</span>
                <span {...stylex.props(styles.optionCount)}>{option.count}</span>
              </button>
            )
          })}
          {group.options.length > LIMIT && (
            <button
              type="button"
              onClick={() => setExpanded(!expanded)}
              {...stylex.props(styles.more)}
            >
              {expanded ? 'show less' : `${group.options.length - LIMIT} more`}
            </button>
          )}
        </div>
      )}
    </section>
  )
}

const styles = stylex.create({
  rail: {
    display: 'flex',
    flexDirection: 'column',
    gap: 2,
    width: 168,
    flexShrink: 0,
    alignSelf: 'flex-start',
    position: 'sticky',
    top: 0,
  },
  group: {
    display: 'flex',
    flexDirection: 'column',
  },
  aggHead: {
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'space-between',
    paddingTop: 7,
    paddingBottom: 5,
    paddingLeft: 4,
    paddingRight: 4,
  },
  aggTitle: {
    color: vars.colorMuted,
    fontSize: 10,
    fontWeight: 600,
    textTransform: 'uppercase',
    letterSpacing: '0.05em',
  },
  aggSelect: {
    borderWidth: 0,
    backgroundColor: 'transparent',
    color: vars.colorFaint,
    fontSize: 10,
    fontFamily: 'inherit',
    cursor: 'pointer',
    outline: 'none',
  },
  aggBars: {
    display: 'flex',
    flexDirection: 'column',
    gap: 1,
    paddingBottom: 6,
  },
  aggRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 6,
    width: '100%',
    borderWidth: 0,
    borderRadius: 4,
    backgroundColor: { default: 'transparent', ':hover': vars.bgHover },
    color: vars.colorMuted,
    fontSize: 11,
    fontFamily: 'inherit',
    paddingTop: 3,
    paddingBottom: 3,
    paddingLeft: 4,
    paddingRight: 6,
    cursor: 'pointer',
    textAlign: 'left',
  },
  aggRowOn: {
    color: vars.colorActive,
  },
  aggLabel: {
    width: 62,
    flexShrink: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  aggTrack: {
    flex: 1,
    minWidth: 0,
    height: 4,
    borderRadius: 2,
    backgroundColor: vars.bgSunken,
    overflow: 'hidden',
  },
  aggFill: {
    display: 'block',
    height: '100%',
    borderRadius: 2,
    backgroundColor: vars.colorFaint,
  },
  aggFillOn: {
    backgroundColor: vars.colorActive,
  },
  groupHead: {
    display: 'flex',
    alignItems: 'center',
    gap: 5,
    borderWidth: 0,
    backgroundColor: 'transparent',
    color: vars.colorMuted,
    fontSize: 10,
    fontWeight: 600,
    textTransform: 'uppercase',
    letterSpacing: '0.05em',
    paddingTop: 7,
    paddingBottom: 5,
    paddingLeft: 4,
    paddingRight: 4,
    cursor: 'pointer',
    fontFamily: 'inherit',
    textAlign: 'left',
  },
  caret: {
    display: 'inline-flex',
    transform: 'rotate(-90deg)',
    transitionProperty: 'transform',
    transitionDuration: '120ms',
  },
  caretOpen: {
    transform: 'rotate(0deg)',
  },
  option: {
    display: 'flex',
    alignItems: 'center',
    gap: 6,
    width: '100%',
    borderWidth: 0,
    borderRadius: 4,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    color: vars.colorBase,
    fontSize: 12,
    fontFamily: 'inherit',
    paddingTop: 4,
    paddingBottom: 4,
    paddingLeft: 4,
    paddingRight: 6,
    cursor: 'pointer',
    textAlign: 'left',
  },
  optionOn: {
    color: vars.colorActive,
  },
  check: {
    width: 13,
    height: 13,
    flexShrink: 0,
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: 3,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    color: vars.onPrimary,
  },
  checkOn: {
    backgroundColor: vars.primary500,
    borderColor: 'transparent',
  },
  optionLabel: {
    flex: 1,
    minWidth: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  optionCount: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    color: vars.colorFaint,
    flexShrink: 0,
  },
  more: {
    borderWidth: 0,
    backgroundColor: 'transparent',
    color: vars.colorActive,
    fontSize: 11,
    fontFamily: 'inherit',
    paddingTop: 4,
    paddingBottom: 4,
    paddingLeft: 23,
    cursor: 'pointer',
    textAlign: 'left',
    opacity: { default: vars.opFade, ':hover': 1 },
  },
})
