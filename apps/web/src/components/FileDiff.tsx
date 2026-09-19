import * as stylex from '@stylexjs/stylex'
import type { MockCommitFile, MockDiffHunk } from '../mock/commits'
import { font, vars } from '../ui/tokens.stylex'

/**
 * `@@ -a,b +c,d @@` — the counts derive from the hunk body itself (ctx+del
 * lines on the old side, ctx+add on the new) so a header can't drift from
 * its lines. Like git, `,n` is omitted when the count is 1.
 */
function hunkHeader(h: MockDiffHunk): string {
  let oldCount = 0
  let newCount = 0
  for (const l of h.lines) {
    if (l[0] !== '+') oldCount++
    if (l[0] !== '-') newCount++
  }
  const range = (start: number, n: number) => (n === 1 ? `${start}` : `${start},${n}`)
  return `@@ -${range(h.oldStart, oldCount)} +${range(h.newStart, newCount)} @@`
}

/**
 * The mock unified diff for one file — `@@` hunk headers, old/new gutter
 * numbers, +/− tinted rows, context dimmed. New-side gutter numbers click
 * through to the file at that line.
 *
 * Shared by the commit list expansion, the commit detail page, and the
 * compare view — keep the metrics identical everywhere.
 */
export function FileDiff({
  file,
  onOpenAt,
}: {
  file: MockCommitFile
  onOpenAt: (line: number) => void
}) {
  return (
    <div {...stylex.props(styles.diffBox)}>
      {file.hunks.map((h, i) => {
        let oldNo = h.oldStart
        let newNo = h.newStart
        return (
          <div key={i} {...stylex.props(styles.hunk)}>
            <div {...stylex.props(styles.hunkHead)}>
              {hunkHeader(h)}
              {h.section ? ` ${h.section}` : ''}
            </div>
            <div {...stylex.props(styles.hunkBody)}>
              <div {...stylex.props(styles.hunkInner)}>
                {h.lines.map((l, j) => {
                  const marker = l[0] ?? ' '
                  const text = l.slice(1)
                  const kind = marker === '+' ? 'add' : marker === '-' ? 'del' : 'ctx'
                  const oldLine = kind === 'add' ? undefined : oldNo++
                  const newLine = kind === 'del' ? undefined : newNo++
                  return (
                    <div
                      key={j}
                      {...stylex.props(
                        styles.dLine,
                        kind === 'add' && styles.dAdd,
                        kind === 'del' && styles.dDel,
                        kind === 'ctx' && styles.dCtx,
                      )}
                    >
                      <span {...stylex.props(styles.dGutter)}>{oldLine ?? ''}</span>
                      {newLine != null ? (
                        <button
                          type="button"
                          title={`open ${file.path} at line ${newLine}`}
                          onClick={() => onOpenAt(newLine)}
                          {...stylex.props(styles.dGutter, styles.dGutterBtn)}
                        >
                          {newLine}
                        </button>
                      ) : (
                        <span {...stylex.props(styles.dGutter)} />
                      )}
                      <span
                        {...stylex.props(
                          styles.dSign,
                          kind === 'add' && styles.dSignAdd,
                          kind === 'del' && styles.dSignDel,
                        )}
                      >
                        {marker === ' ' ? ' ' : marker}
                      </span>
                      <span {...stylex.props(styles.dText)}>{text === '' ? ' ' : text}</span>
                    </div>
                  )
                })}
              </div>
            </div>
          </div>
        )
      })}
    </div>
  )
}

/** `+N −M` in the severity ramp + the 5-square GitHub-style proportion bar. */
export function DiffStat({ added, removed }: { added: number; removed: number }) {
  const total = added + removed
  const UNITS = 5
  const addUnits =
    total <= 0 ? 0 : Math.min(UNITS, Math.max(added > 0 ? 1 : 0, Math.round((added / total) * UNITS)))
  const delUnits =
    total <= 0
      ? 0
      : Math.min(UNITS - addUnits, Math.max(removed > 0 ? 1 : 0, Math.round((removed / total) * UNITS)))
  return (
    <span {...stylex.props(styles.diff)}>
      <span {...stylex.props(styles.diffNum, styles.add)}>+{added}</span>
      <span {...stylex.props(styles.diffNum, styles.del)}>−{removed}</span>
      {total > 0 && (
        <span {...stylex.props(styles.blocks)} aria-hidden>
          {Array.from({ length: addUnits }, (_, i) => (
            <span key={`a${i}`} {...stylex.props(styles.block, styles.blockAdd)} />
          ))}
          {Array.from({ length: delUnits }, (_, i) => (
            <span key={`d${i}`} {...stylex.props(styles.block, styles.blockDel)} />
          ))}
        </span>
      )}
    </span>
  )
}

const styles = stylex.create({
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
  blocks: {
    display: 'inline-flex',
    gap: 2,
    flexShrink: 0,
  },
  block: {
    display: 'block',
    width: 6,
    height: 6,
    borderRadius: 1,
  },
  blockAdd: {
    backgroundColor: vars.scaleLow,
  },
  blockDel: {
    backgroundColor: vars.scaleHigh,
  },
  diffBox: {
    display: 'flex',
    flexDirection: 'column',
    marginTop: 2,
    marginBottom: 6,
    borderRadius: 6,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderMute,
    backgroundColor: vars.bgCode,
    overflow: 'hidden',
  },
  hunk: {
    display: 'flex',
    flexDirection: 'column',
  },
  hunkHead: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    color: vars.colorFaint,
    backgroundColor: vars.tipInfoBg,
    paddingTop: 3,
    paddingBottom: 3,
    paddingLeft: 8,
    paddingRight: 8,
    borderTopWidth: {
      default: 1,
      ':first-child': 0,
    },
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
    whiteSpace: 'pre',
    overflowX: 'auto',
    userSelect: 'none',
  },
  hunkBody: {
    overflowX: 'auto',
  },
  // Shrink-to-fit wrapper — every line, and so every tint, spans the width
  // of the longest line (or the pane, whichever is wider).
  hunkInner: {
    display: 'inline-block',
    minWidth: '100%',
  },
  dLine: {
    display: 'flex',
    whiteSpace: 'pre',
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    lineHeight: '1.55',
  },
  dAdd: {
    backgroundColor: `color-mix(in srgb, ${vars.scaleLow} 10%, transparent)`,
  },
  dDel: {
    backgroundColor: `color-mix(in srgb, ${vars.scaleHigh} 10%, transparent)`,
  },
  dCtx: {
    color: vars.colorFaint,
  },
  dGutter: {
    width: 30,
    flexShrink: 0,
    textAlign: 'right',
    paddingTop: 0,
    paddingBottom: 0,
    paddingLeft: 0,
    paddingRight: 4,
    color: vars.colorFaint,
    opacity: vars.opFade,
    userSelect: 'none',
  },
  dGutterBtn: {
    borderWidth: 0,
    backgroundColor: 'transparent',
    fontFamily: 'inherit',
    fontSize: 'inherit',
    lineHeight: 'inherit',
    cursor: 'pointer',
    color: {
      default: vars.colorFaint,
      ':hover': vars.colorActive,
    },
  },
  dSign: {
    width: 14,
    flexShrink: 0,
    textAlign: 'center',
    userSelect: 'none',
  },
  dSignAdd: {
    color: vars.scaleLow,
  },
  dSignDel: {
    color: vars.scaleHigh,
  },
  dText: {
    flex: 1,
    minWidth: 0,
    paddingLeft: 2,
  },
})
