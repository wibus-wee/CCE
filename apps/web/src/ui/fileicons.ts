/**
 * Port of `@antfu/design/utils/icon` — extension/path -> file-type rules.
 * The original maps to catppuccin iconify classes; this port returns a
 * semantic `name` + `color` pair rendered by `DisplayFileIcon` with the
 * local stroke set (no runtime icon font, stays offline).
 */

export interface FileIconRule {
  match: RegExp
  name: string
  /** Semantic accent for the glyph (hex; readable on both schemes). */
  color: string
  description?: string
}

export interface FileTypeInfo {
  name: string
  color: string
  description?: string
}

const C = {
  vue: '#41b883',
  react: '#61dafb',
  ts: '#3178c6',
  js: '#f0db4f',
  json: '#b8b052',
  yaml: '#cb171e',
  toml: '#9c4221',
  md: '#519aba',
  html: '#e34c26',
  css: '#663399',
  sass: '#cd6799',
  svg: '#ffb13b',
  image: '#a074c4',
  font: '#e8590c',
  wasm: '#654ff0',
  npm: '#cb3837',
  test: '#6a9f58',
  rs: '#dea584',
  py: '#3572a5',
  go: '#00add8',
  lock: '#8a8a8a',
  file: '#8a8a8a',
} as const

export const defaultFileIconRules: FileIconRule[] = [
  { match: /\.vue$/, name: 'vue', color: C.vue },
  { match: /\.tsx$/, name: 'tsx', color: C.react },
  { match: /\.jsx$/, name: 'jsx', color: C.react },
  { match: /\.d\.[cm]?ts$/, name: 'dts', color: C.ts },
  { match: /\.[cm]?ts$/, name: 'ts', color: C.ts },
  { match: /\.[cm]?js$/, name: 'js', color: C.js },
  { match: /\.rs$/, name: 'rust', color: C.rs },
  { match: /\.py$/, name: 'python', color: C.py },
  { match: /\.go$/, name: 'go', color: C.go },
  { match: /\.json5?$/, name: 'json', color: C.json },
  { match: /\.ya?ml$/, name: 'yaml', color: C.yaml },
  { match: /\.toml$/, name: 'toml', color: C.toml },
  { match: /\.md$/, name: 'markdown', color: C.md },
  { match: /\.html?$/, name: 'html', color: C.html },
  { match: /\.(?:css|postcss)$/, name: 'css', color: C.css },
  { match: /\.s[ac]ss$/, name: 'sass', color: C.sass },
  { match: /\.svg$/, name: 'svg', color: C.svg },
  { match: /\.(?:png|jpe?g|gif|webp|avif|ico)$/, name: 'image', color: C.image },
  { match: /\.(?:woff2?|ttf|otf|eot)$/, name: 'font', color: C.font },
  { match: /\.wasm$/, name: 'wasm', color: C.wasm },
  { match: /package\.json$/, name: 'npm', color: C.npm },
  { match: /(?:pnpm-lock\.yaml|package-lock\.json|yarn\.lock|Cargo\.lock)$/, name: 'lock', color: C.lock },
  { match: /\.(?:test|spec)\.[cm]?[jt]sx?$/, name: 'test', color: C.test },
]

export function stripModuleQuery(id: string): string {
  return id.replace(/[?#].*$/, '')
}

export function getFileType(
  path: string,
  rules: FileIconRule[] = defaultFileIconRules,
): FileTypeInfo {
  const clean = stripModuleQuery(path)
  for (const rule of rules) {
    if (rule.match.test(clean))
      return { name: rule.name, color: rule.color, description: rule.description }
  }
  return { name: 'file', color: C.file }
}

/** Named-folder lookup for directories (name -> accent color). */
export const defaultFolderIconColors: Record<string, string> = {
  src: '#41b883',
  dist: '#b8b052',
  node_modules: '#cb3837',
  test: '#6a9f58',
  tests: '#6a9f58',
  public: '#519aba',
  components: '#61dafb',
  utils: '#a074c4',
  config: '#8a8a8a',
  assets: '#ffb13b',
  scripts: '#9c4221',
  styles: '#cd6799',
  lib: '#3178c6',
}

export function getFolderIconColor(
  name?: string,
  open = false,
  rules: Record<string, string> = defaultFolderIconColors,
): string {
  if (open) return '#e8b03c'
  const named = name ? rules[name.toLowerCase()] : undefined
  return named ?? '#8a8a8a'
}
