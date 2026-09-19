import { memo, useMemo, type ReactNode } from 'react'
import * as stylex from '@stylexjs/stylex'
import { extractMarks, useHighlightLines } from './highlight'
import { font, vars } from '../ui/tokens.stylex'

/**
 * Hit snippets arrive with literal `<mark>…</mark>` tags from the engine.
 * Shiki tokenizes the stripped code and re-applies the mark ranges as
 * decorations (see `lib/highlight.tsx`), so search highlights and syntax
 * colors coexist. `role="code"` containers get `.marked` styling from
 * styles.css — no dangerouslySetInnerHTML anywhere.
 */

const styles = stylex.create({
  code: {
    display: 'flex',
    fontFamily: font.mono,
    fontSize: '12px',
    lineHeight: 1.6,
    overflowX: 'auto',
    padding: '4px 8px',
    whiteSpace: 'pre',
  },
  gutter: {
    flexShrink: 0,
    paddingRight: 8,
    marginRight: 8,
    borderRightWidth: 1,
    borderRightStyle: 'solid',
    borderRightColor: vars.borderMute,
    color: vars.colorFaint,
    textAlign: 'right',
    userSelect: 'none',
  },
  line: {
    display: 'block',
    minHeight: '1.6em',
  },
  // Extra leftmost gutter column (blame annotations) — same row metrics as
  // the number gutter, no right-align: cells own their own styling.
  gutterExtra: {
    flexShrink: 0,
    marginRight: 8,
    borderRightWidth: 1,
    borderRightStyle: 'solid',
    borderRightColor: vars.borderMute,
    color: vars.colorFaint,
    userSelect: 'none',
  },
  codeWrap: {
    whiteSpace: 'pre-wrap',
    overflowX: 'hidden',
  },
  linesWrap: {
    wordBreak: 'break-word',
    overflowWrap: 'anywhere',
  },
  lineHot: {
    backgroundColor: vars.markBg,
    marginLeft: -8,
    marginRight: -8,
    paddingLeft: 8,
    paddingRight: 8,
    borderRadius: 2,
  },
  gutterHot: {
    color: vars.colorBase,
    fontWeight: 600,
  },
  gutterBtn: {
    borderWidth: 0,
    backgroundColor: 'transparent',
    fontFamily: 'inherit',
    fontSize: 'inherit',
    color: 'inherit',
    textAlign: 'right',
    cursor: 'pointer',
    padding: 0,
    ':hover': { color: vars.colorActive },
  },
  mark: {
    backgroundColor: vars.markBg,
    color: 'inherit',
    borderRadius: 2,
    padding: '0 1px',
  },
  more: {
    display: 'block',
    color: vars.colorFaint,
  },
})

/** Split raw engine markup into text/mark segments (plain fallback path). */
export function parseMarks(raw: string): { text: string; marked: boolean }[] {
  const segments: { text: string; marked: boolean }[] = []
  const re = /<mark>([\s\S]*?)<\/mark>/g
  let last = 0
  for (const match of raw.matchAll(re)) {
    if (match.index > last) segments.push({ text: raw.slice(last, match.index), marked: false })
    segments.push({ text: match[1] ?? '', marked: true })
    last = match.index + match[0].length
  }
  if (last < raw.length) segments.push({ text: raw.slice(last), marked: false })
  return segments
}

export const SnippetView = memo(function SnippetView({
  snippet,
  startLine,
  maxLines = 8,
  language,
}: {
  snippet: string
  startLine?: number
  maxLines?: number
  language?: string
}) {
  const rawLines = snippet.split('\n')
  const clipped = rawLines.length > maxLines
  const lines = clipped ? rawLines.slice(0, maxLines) : rawLines

  // Strip <mark> tags on the clipped text; ranges become token decorations.
  const joined = lines.join('\n')
  const { code, decorations } = useMemo(() => extractMarks(joined), [joined])
  const highlighted = useHighlightLines(code, language, decorations)

  const rendered: ReactNode[] =
    highlighted && highlighted.length === lines.length
      ? highlighted.map((node, i) => (
          <span key={i} {...stylex.props(styles.line)}>
            {node}
          </span>
        ))
      : lines.map((line, i) => (
          <span key={i} {...stylex.props(styles.line)}>
            {parseMarks(line).map((seg, j) =>
              seg.marked ? (
                <mark key={j} {...stylex.props(styles.mark)}>
                  {seg.text}
                </mark>
              ) : (
                seg.text
              ),
            )}
            {line === '' ? ' ' : null}
          </span>
        ))

  return (
    <div {...stylex.props(styles.code)} role="code">
      {startLine !== undefined && (
        <span {...stylex.props(styles.gutter)}>
          {lines.map((_, i) => (
            <span key={i} {...stylex.props(styles.line)}>
              {startLine + i}
            </span>
          ))}
        </span>
      )}
      <span>
        {rendered}
        {clipped && (
          <span {...stylex.props(styles.more)}>⋯ {rawLines.length - maxLines} more lines</span>
        )}
      </span>
    </div>
  )
})

const inRange = (n: number, r?: readonly [number, number]) =>
  r != null && n >= r[0] && n <= r[1]

/** Numbered code view with syntax highlighting — same chrome as SnippetView. */
export const CodePane = memo(function CodePane({
  content,
  language,
  highlightRange,
  selectedRange,
  onLineClick,
  wrap,
  lineGutter,
}: {
  content: string
  language?: string
  /** Lines to flash — deep links land here. */
  highlightRange?: readonly [number, number]
  /** The URL-anchored line range (?L=n | ?L=a-b) — stays marked. */
  selectedRange?: readonly [number, number]
  /** Line-number click — Sourcegraph/GitHub parity; shift extends a range. */
  onLineClick?: (line: number, shiftKey: boolean) => void
  /** Wrap long lines instead of scrolling horizontally. */
  wrap?: boolean
  /**
   * Extra leftmost gutter column — called with each 1-based line number;
   * the returned node owns its own styling (blame annotations slot in).
   */
  lineGutter?: (line: number) => ReactNode
}) {
  const lines = content.split('\n')
  const highlighted = useHighlightLines(content, language)
  const hot = (n: number) => inRange(n, highlightRange) || inRange(n, selectedRange)
  const lineStyle = (n: number) => stylex.props(styles.line, hot(n) && styles.lineHot)
  const rendered =
    highlighted && highlighted.length === lines.length
      ? highlighted.map((node, i) => (
          <span key={i} data-line={i + 1} {...lineStyle(i + 1)}>
            {node}
          </span>
        ))
      : lines.map((line, i) => (
          <span key={i} data-line={i + 1} {...lineStyle(i + 1)}>
            {line === '' ? ' ' : line}
          </span>
        ))
  return (
    <div {...stylex.props(styles.code, wrap && styles.codeWrap)} role="code">
      {lineGutter && (
        <span {...stylex.props(styles.gutterExtra)}>
          {lines.map((_, i) => (
            <span key={i} {...stylex.props(styles.line)}>
              {lineGutter(i + 1)}
            </span>
          ))}
        </span>
      )}
      <span {...stylex.props(styles.gutter)}>
        {lines.map((_, i) => {
          const n = i + 1
          return onLineClick ? (
            <button
              key={i}
              type="button"
              onClick={(e) => onLineClick(n, e.shiftKey)}
              {...stylex.props(styles.line, styles.gutterBtn, hot(n) && styles.gutterHot)}
              title={`line ${n} — shift-click extends the range`}
            >
              {n}
            </button>
          ) : (
            <span
              key={i}
              {...stylex.props(styles.line, hot(n) && styles.gutterHot)}
            >
              {n}
            </span>
          )
        })}
      </span>
      <span {...stylex.props(wrap && styles.linesWrap)}>{rendered}</span>
    </div>
  )
})
