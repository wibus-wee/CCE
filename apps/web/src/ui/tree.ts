/**
 * Port of `@antfu/design/utils/tree` — nests a flat list of path-bearing
 * items into a tree, collapsing single-child chains when `flatten`.
 */

export interface TreeNode<T> {
  /** The display name for this segment (may be a flattened chain like `a/b`). */
  name: string
  /** Full path up to and including this node. */
  path: string
  /** The original item, present on leaf nodes. */
  item?: T
  children: TreeNode<T>[]
}

export interface ToTreeOptions {
  /** Separator between path segments. Default `/`. */
  separator?: string
  /** Collapse chains of single-child nodes into one. Default `true`. */
  flatten?: boolean
}

function flattenChain<T>(node: TreeNode<T>, separator: string): void {
  while (node.children.length === 1 && node.item == null) {
    const child = node.children[0]
    if (child == null) break
    node.name = `${node.name}${separator}${child.name}`
    node.path = child.path
    node.item = child.item
    node.children = child.children
  }
  for (const child of node.children) flattenChain(child, separator)
}

export function toTree<T>(
  items: T[],
  getPath: (item: T) => string,
  options: ToTreeOptions = {},
): TreeNode<T>[] {
  const { separator = '/', flatten = true } = options
  const root: TreeNode<T> = { name: '', path: '', children: [] }

  for (const item of items) {
    const segments = getPath(item).split(separator).filter(Boolean)
    let current = root
    let acc = ''
    segments.forEach((segment, i) => {
      acc = acc ? `${acc}${separator}${segment}` : segment
      let next = current.children.find(c => c.name === segment && c.item == null)
      if (!next) {
        next = { name: segment, path: acc, children: [] }
        current.children.push(next)
      }
      if (i === segments.length - 1) next.item = item
      current = next
    })
  }

  if (flatten) {
    for (const child of root.children) flattenChain(child, separator)
  }

  return root.children
}
