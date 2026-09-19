import * as stylex from '@stylexjs/stylex'
import {
  defaultFileIconRules,
  defaultFolderIconColors,
  getFileType,
  getFolderIconColor,
  type FileIconRule,
} from './fileicons'
import { IconFile, IconFolder, IconFolderOpen } from './icons'

/**
 * Port of `DisplayFileIcon` — ext/path -> colored file glyph via a
 * configurable rule list (`rules`, `folderRules`). The iconify classes of
 * the original become accent colors on the local stroke set, so the port
 * stays offline.
 */
export function DisplayFileIcon({
  path,
  rules,
  directory = false,
  open = false,
  folderRules,
  size = 14,
  invert = false,
}: {
  /** File path or module id (or folder name when `directory`). */
  path: string
  /** Override the default rule list. */
  rules?: FileIconRule[]
  /** Render a folder icon instead of a file icon. */
  directory?: boolean
  /** For directories: show the open-folder glyph. */
  open?: boolean
  /** Named-folder color lookup for directories. */
  folderRules?: Record<string, string>
  size?: number
  /** Escape hatch for dark icon sets on light surfaces — inverts the glyph. */
  invert?: boolean
}) {
  const name = directory ? path.replace(/\/+$/, '').split('/').pop() || 'folder' : undefined
  const color = directory
    ? getFolderIconColor(name, open, folderRules)
    : getFileType(path, rules ?? defaultFileIconRules).color

  const glyph = directory ? (
    open ? <IconFolderOpen size={size} /> : <IconFolder size={size} />
  ) : (
    <IconFile size={size} />
  )

  return (
    <span
      {...stylex.props(styles.icon, invert && styles.invert)}
      style={{ color }}
      aria-hidden
    >
      {glyph}
    </span>
  )
}

const styles = stylex.create({
  icon: {
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    flexShrink: 0,
    lineHeight: 0,
  },
  invert: {
    filter: 'invert(1) hue-rotate(180deg) saturate(0.7)',
  },
})
