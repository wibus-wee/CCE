import * as stylex from '@stylexjs/stylex'
import { Collapsible } from '@base-ui-components/react'
import { useMemo, useState, type ReactNode } from 'react'
import { DisplayFileIcon } from './DisplayFileIcon'
import { IconCaretRight } from './icons'
import { toTree, type TreeNode } from './tree'
import { vars } from './tokens.stylex'

/**
 * Port of `DisplayTree` — nests a flat `items` list (via `getPath`) or
 * pre-built `nodes` into an expandable file/folder tree on Base UI
 * `Collapsible`. `leaf`/`render` customize node labels.
 */
export function DisplayTree<T>({
  items,
  getPath,
  nodes,
  separator = '/',
  flatten = true,
  defaultExpanded = true,
  fileIcons = false,
  leaf,
  render,
}: {
  /** Flat list of items to nest into a tree. */
  items?: T[]
  /** Maps an item to its `separator`-delimited path. */
  getPath?: (item: T) => string
  /** Pre-built nodes (pass `items` + `getPath` instead for flat data). */
  nodes?: TreeNode<T>[]
  separator?: string
  /** Collapse single-child chains into one node. Default `true`. */
  flatten?: boolean
  defaultExpanded?: boolean
  /** Show a `DisplayFileIcon` glyph on leaf nodes. */
  fileIcons?: boolean
  /** Customize a leaf label (falls back to `render`, then `node.name`). */
  leaf?: (node: TreeNode<T>) => ReactNode
  /** Fully customize any node's label. */
  render?: (node: TreeNode<T>) => ReactNode
}) {
  const tree = useMemo<TreeNode<T>[]>(
    () =>
      nodes ??
      (items && getPath ? toTree(items, getPath, { separator, flatten }) : []),
    [items, getPath, nodes, separator, flatten],
  )
  return (
    <div {...stylex.props(styles.tree)} role="tree">
      {tree.map(node => (
        <TreeRow
          key={node.path}
          node={node}
          indent={8}
          defaultExpanded={defaultExpanded}
          fileIcons={fileIcons}
          leaf={leaf}
          render={render}
        />
      ))}
    </div>
  )
}

function TreeRow<T>({
  node,
  indent,
  defaultExpanded,
  fileIcons,
  leaf,
  render,
}: {
  node: TreeNode<T>
  /** Row left padding in px — the panel margin carries the depth offset. */
  indent: number
  defaultExpanded: boolean
  fileIcons: boolean
  leaf?: (node: TreeNode<T>) => ReactNode
  render?: (node: TreeNode<T>) => ReactNode
}) {
  const [open, setOpen] = useState(defaultExpanded)
  const branch = node.children.length > 0
  const label = render?.(node) ?? (branch ? node.name : (leaf?.(node) ?? node.name))

  if (!branch) {
    return (
      <div
        role="treeitem"
        {...stylex.props(styles.row)}
        style={{ paddingLeft: indent }}
      >
        <span {...stylex.props(styles.caretSpace)} />
        {fileIcons && <DisplayFileIcon path={node.path} size={13} />}
        <span {...stylex.props(styles.name)}>{label}</span>
      </div>
    )
  }

  return (
    <Collapsible.Root open={open} onOpenChange={setOpen}>
      <Collapsible.Trigger
        {...stylex.props(styles.row, styles.branch)}
        style={{ paddingLeft: indent }}
        role="treeitem"
        aria-expanded={open}
      >
        <IconCaretRight
          size={11}
          // inline transform — direction depends on runtime state
          {...{ style: { transform: open ? 'rotate(90deg)' : undefined, transition: 'transform 120ms' } }}
        />
        <DisplayFileIcon path={node.name} directory open={open} size={13} />
        <span {...stylex.props(styles.name)}>{label}</span>
      </Collapsible.Trigger>
      {/* JetBrains/SG parity — open dirs get a vertical guide at the caret. */}
      <Collapsible.Panel
        {...stylex.props(styles.panel, open && styles.panelGuide)}
        style={{ marginLeft: indent + 5 }}
        role="group"
      >
        {node.children.map(child => (
          <TreeRow
            key={child.path}
            node={child}
            indent={9}
            defaultExpanded={defaultExpanded}
            fileIcons={fileIcons}
            leaf={leaf}
            render={render}
          />
        ))}
      </Collapsible.Panel>
    </Collapsible.Root>
  )
}

const styles = stylex.create({
  tree: {
    fontSize: 12,
    lineHeight: '1.5',
    color: vars.colorBase,
  },
  row: {
    display: 'flex',
    alignItems: 'center',
    gap: 5,
    paddingTop: 1,
    paddingBottom: 1,
    paddingRight: 8,
    width: '100%',
    boxSizing: 'border-box',
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    borderRadius: 4,
  },
  branch: {
    borderWidth: 0,
    fontFamily: 'inherit',
    fontSize: 'inherit',
    color: 'inherit',
    textAlign: 'left',
    cursor: 'pointer',
    padding: 0,
    outline: 'none',
  },
  caretSpace: {
    width: 11,
    flexShrink: 0,
  },
  name: {
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  panel: {
    display: 'flex',
    flexDirection: 'column',
  },
  panelGuide: {
    borderLeftWidth: 1,
    borderLeftStyle: 'solid',
    borderLeftColor: vars.borderMute,
  },
})
