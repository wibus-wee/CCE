/**
 * Query-syntax tokenizer — the shared grammar for `type:`/`lang:`/`path:`-
 * style filters, boolean operators, quoted strings, and parens. Drives the
 * in-place highlight in the Query input and the syntax-colored example
 * rows on Home.
 */
export const QUERY_TOKEN =
  /(-?\b(?:type|lang|path|file|repo|rev|context|count|select|case|patternType|archived|fork|content):)|(\bAND\b|\bOR\b|\bNOT\b)|("[^"]*")|([()])/g

export type QueryTokenKind = 'filter' | 'op' | 'str' | 'paren'

export function queryTokens(q: string): { text: string; kind?: QueryTokenKind }[] {
  const out: { text: string; kind?: QueryTokenKind }[] = []
  let last = 0
  for (const m of q.matchAll(QUERY_TOKEN)) {
    const i = m.index ?? 0
    if (i > last) out.push({ text: q.slice(last, i) })
    const kind: QueryTokenKind = m[1] ? 'filter' : m[2] ? 'op' : m[3] ? 'str' : 'paren'
    out.push({ text: m[0], kind })
    last = i + m[0].length
  }
  if (last < q.length) out.push({ text: q.slice(last) })
  return out
}
