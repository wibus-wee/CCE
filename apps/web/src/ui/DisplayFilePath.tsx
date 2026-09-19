import * as stylex from '@stylexjs/stylex'
import type { ReactNode } from 'react'
import { DisplayFileIcon } from './DisplayFileIcon'
import { collapsePnpmPath } from './path'
import { font, vars } from './tokens.stylex'

/**
 * Port of `DisplayFilePath` — mono path with dimmed directories so the
 * basename carries the weight, `:line` suffix, optional `DisplayFileIcon`
 * glyph, `href` link rendering, and `.pnpm` store decoding via `pnpm`.
 */
export function DisplayFilePath({
  path,
  line,
  endLine,
  icon = false,
  directory = false,
  href,
  pnpm = true,
  children,
}: {
  path: string
  line?: number
  endLine?: number
  /** Prefix a `DisplayFileIcon` glyph. */
  icon?: boolean
  /** Treat the path as a directory (folder glyph + no basename dimming). */
  directory?: boolean
  /** Render as a link. */
  href?: string
  /** Collapse `.pnpm` store chunks to `~/` (default `true`). */
  pnpm?: boolean
  children?: ReactNode
}) {
  const display = pnpm ? collapsePnpmPath(path) : path
  const segments = display.split('/')
  const name = directory ? display : (segments.pop() ?? display)
  const dir = directory ? '' : segments.join('/')
  const lines =
    line != null ? `:${endLine != null && endLine > line ? `${line}–${endLine}` : line}` : ''

  const body = (
    <>
      {icon && <DisplayFileIcon path={display} directory={directory} size={12} />}
      {dir && <span {...stylex.props(styles.dir)}>{dir}/</span>}
      <span>{name}</span>
      {children}
      {lines && <span {...stylex.props(styles.dir)}>{lines}</span>}
    </>
  )

  if (href != null) {
    return (
      <a href={href} {...stylex.props(styles.path, styles.link)} title={`${path}${lines}`}>
        {body}
      </a>
    )
  }
  return (
    <code {...stylex.props(styles.path)} title={`${path}${lines}`}>
      {body}
    </code>
  )
}

const styles = stylex.create({
  path: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 11,
    color: 'inherit',
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
    direction: 'ltr',
    maxWidth: '100%',
    display: 'inline-flex',
    alignItems: 'center',
    gap: 4,
    verticalAlign: 'bottom',
  },
  link: {
    textDecoration: 'none',
    color: vars.colorActive,
  },
  dir: {
    opacity: 0.55,
  },
})
