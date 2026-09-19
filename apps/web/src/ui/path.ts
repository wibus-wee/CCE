/**
 * Port of `@antfu/design/utils/path` — module-path normalization, pnpm-store
 * decoding, and readable relative paths for `DisplayFilePath`.
 */

export function normalizeModulePath(path: string): string {
  return path.replace(/\\/g, '/')
}

export function isNodeModulePath(path: string): boolean {
  return /\bnode_modules\b/.test(normalizeModulePath(path))
}

export function isPackageName(id: string): boolean {
  if (!id || id.startsWith('.') || id.startsWith('/') || /^[a-z]+:/i.test(id) || /^[a-z]:[\\/]/i.test(id))
    return false
  return /^(?:@[\w.-]+\/)?[\w.-]+(?:\/[\w.-]+)*$/.test(id)
}

export interface PnpmPackageInfo {
  name: string
  version: string
}

export function parsePnpmSegment(segment: string): PnpmPackageInfo | undefined {
  // Strip peer-dependency suffix (`_react@18...`).
  const base = segment.split('_')[0] ?? ''
  const at = base.lastIndexOf('@')
  if (at <= 0) return undefined
  const rawName = base.slice(0, at)
  const version = base.slice(at + 1)
  // pnpm encodes scopes as `@scope+name` (sometimes without the leading `@`).
  let name = rawName
  if (rawName.includes('+')) {
    name = rawName.replace('+', '/')
    if (!name.startsWith('@')) name = `@${name}`
  }
  return { name, version }
}

export function getPnpmPackageInfoFromPath(path: string): PnpmPackageInfo | undefined {
  const match = normalizeModulePath(path).match(/\.pnpm\/([^/]+)/)
  if (match?.[1] == null) return undefined
  return parsePnpmSegment(match[1])
}

export function getModuleNameFromPath(path: string): string | undefined {
  const normalized = normalizeModulePath(path)
  const pnpm = getPnpmPackageInfoFromPath(normalized)
  if (pnpm) return pnpm.name
  const idx = normalized.lastIndexOf('node_modules/')
  if (idx === -1) return undefined
  const rest = normalized.slice(idx + 'node_modules/'.length)
  const [first, second] = rest.split('/')
  if (first == null) return undefined
  return first.startsWith('@') && second != null ? `${first}/${second}` : first
}

export function collapsePnpmPath(path: string, replacement = '~/'): string {
  return normalizeModulePath(path).replace(/.*\.pnpm\/[^/]+\/node_modules\//, replacement)
}

export interface RelativeModulePathOptions {
  /** Prefix substituted for a `.pnpm` store chunk. Defaults to `'~/'`. */
  pnpmCollapse?: string
  /** Beyond this many `../` levels, return the absolute path instead. Defaults to `3`. */
  maxUp?: number
}

export function relativeModulePath(id: string, root: string, options: RelativeModulePathOptions = {}): string {
  const { pnpmCollapse = '~/', maxUp = 3 } = options
  const path = normalizeModulePath(id)
  const normRoot = normalizeModulePath(root).replace(/\/$/, '')
  if (path.includes('.pnpm/')) return collapsePnpmPath(path, pnpmCollapse)
  if (!normRoot) return path
  if (path === normRoot) return '.'
  if (path.startsWith(`${normRoot}/`)) {
    const rest = path.slice(normRoot.length + 1)
    return rest.startsWith('.') ? rest : `./${rest}`
  }
  // Outside the root: synthesize `../`, but bail to absolute past `maxUp` hops.
  const rootParts = normRoot.split('/')
  const pathParts = path.split('/')
  let common = 0
  while (common < rootParts.length && common < pathParts.length && rootParts[common] === pathParts[common])
    common++
  const up = rootParts.length - common
  if (up > maxUp || common === 0) return path
  return `${'../'.repeat(up)}${pathParts.slice(common).join('/')}`
}

export interface ReadablePath {
  /** Display path. */
  path: string
  /** Resolved package name when the path lives in `node_modules`. */
  moduleName?: string
}

export function parseReadablePath(path: string, root: string, options?: RelativeModulePathOptions): ReadablePath {
  const moduleName = isNodeModulePath(path) ? getModuleNameFromPath(path) : undefined
  return { path: relativeModulePath(path, root, options), moduleName }
}
