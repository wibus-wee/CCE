import { useState } from 'react'
import {
  api,
  DefinitionsReport,
  describeError,
  ReferencesReport,
  SearchHit,
  SearchResult,
  SourceAddress,
} from './api'

export function SearchResults({ result }: { result: SearchResult }) {
  // Stale/partial/unavailable views are product-contract caveats, not
  // decoration: they go in a dedicated block above the hits, never hidden.
  const degradedViews = Object.entries(result.manifest.views).filter(
    ([, view]) => view.state !== 'ready',
  )
  const hasCaveats = result.missingCapabilities.length > 0 || degradedViews.length > 0

  return (
    <section className="search-results" aria-label="Search results">
      <div className="section-heading">
        <div>
          <p className="eyebrow">{result.plan.intent.replaceAll('_', ' ')}</p>
          <h2>
            {result.hits.length} hit{result.hits.length === 1 ? '' : 's'}
          </h2>
        </div>
        <code>
          {result.latencyMs} ms · snapshot {result.request.snapshotId.slice(0, 21)}
        </code>
      </div>

      {hasCaveats && (
        <aside className="warnings" role="status">
          <h3>Freshness &amp; capability caveats</h3>
          {result.missingCapabilities.map((capability) => (
            <p key={capability}>{capability}</p>
          ))}
          {degradedViews.map(([name, view]) => (
            <p key={name}>
              view <strong>{name}</strong> is {view.state.replaceAll('_', ' ')}
              {view.message ? ` — ${view.message}` : ''}
            </p>
          ))}
        </aside>
      )}

      <p className="plan-line">
        routes: {result.plan.routes.join(' + ') || 'none'} · graph:{' '}
        {result.plan.graphPolicy.replaceAll('_', ' ')}
      </p>
      {result.plan.reasons.length > 0 && (
        <details className="plan-reasons">
          <summary>planner rationale</summary>
          <ul>
            {result.plan.reasons.map((reason) => (
              <li key={reason}>{reason}</li>
            ))}
          </ul>
        </details>
      )}

      {result.hits.length === 0 ? (
        <p className="empty">
          No hits. The committed snapshot answered honestly — try different terms or refresh the
          index.
        </p>
      ) : (
        result.hits.map((hit) => <HitCard key={hit.documentId} hit={hit} />)
      )}
    </section>
  )
}

type Lookup =
  | { kind: 'def' | 'refs'; status: 'loading' }
  | { kind: 'def'; status: 'done'; report: DefinitionsReport }
  | { kind: 'refs'; status: 'done'; report: ReferencesReport }
  | { kind: 'def' | 'refs'; status: 'error'; error: string }

function HitCard({ hit }: { hit: SearchHit }) {
  const [lookup, setLookup] = useState<Lookup>()

  async function runLookup(kind: 'def' | 'refs') {
    const name = hit.symbolName
    if (!name) return
    setLookup({ kind, status: 'loading' })
    try {
      if (kind === 'def') {
        setLookup({ kind, status: 'done', report: await api.definitions(name) })
      } else {
        setLookup({ kind, status: 'done', report: await api.references(name) })
      }
    } catch (value) {
      setLookup({ kind, status: 'error', error: describeError(value) })
    }
  }

  return (
    <article className="hit">
      <div className="hit-head">
        <span className="hit-rank">#{hit.rank}</span>
        <h3>{hit.symbolName ?? hit.documentId}</h3>
        <span className="hit-score" title="fused score">
          {hit.score.toPrecision(3)}
        </span>
      </div>
      {hit.address && <code className="hit-path">{formatAddress(hit.address)}</code>}
      {hit.snippet && <pre className="hit-snippet">{hit.snippet}</pre>}
      <div className="hit-meta">
        {hit.contributingRoutes.map((route) => (
          <span className="chip" key={route}>
            {route.replaceAll('_', ' ')}
          </span>
        ))}
        <span className="chip chip-dim">{hit.representation.replaceAll('_', ' ')}</span>
        {!hit.verifiedCurrent && <span className="chip chip-warn">unverified</span>}
        {hit.symbolName && (
          <span className="hit-actions">
            <button
              type="button"
              className="link"
              disabled={lookup?.status === 'loading'}
              onClick={() => void runLookup('def')}
            >
              def
            </button>
            <button
              type="button"
              className="link"
              disabled={lookup?.status === 'loading'}
              onClick={() => void runLookup('refs')}
            >
              refs
            </button>
          </span>
        )}
      </div>
      {hit.explanation.length > 0 && <p className="hit-why">{hit.explanation.join(' · ')}</p>}
      {lookup && <LookupResult lookup={lookup} />}
    </article>
  )
}

function LookupResult({ lookup }: { lookup: Lookup }) {
  if (lookup.status === 'loading') {
    return <p className="lookup muted">resolving {lookup.kind}…</p>
  }
  if (lookup.status === 'error') {
    return <p className="lookup lookup-error">{lookup.error}</p>
  }
  if (lookup.kind === 'def') {
    return (
      <div className="lookup">
        {lookup.report.definitions.length === 0 ? (
          <p className="muted">no definitions found</p>
        ) : (
          lookup.report.definitions.map((definition, index) => (
            <p key={`${definition.name}:${index}`}>
              <strong>{definition.name}</strong> <small>{definition.kind}</small>
              {definition.qualifiedName && <small> · {definition.qualifiedName}</small>}
              {definition.address && (
                <>
                  {' — '}
                  <code>{formatAddress(definition.address)}</code>
                </>
              )}
            </p>
          ))
        )}
      </div>
    )
  }
  return (
    <div className="lookup">
      {lookup.report.references.length === 0 ? (
        <p className="muted">no references found</p>
      ) : (
        lookup.report.references.map((reference, index) => (
          <p key={`${reference.fromName}:${index}`}>
            <strong>{reference.fromName}</strong>{' '}
            <small>
              {reference.via} · {reference.origin} · conf {reference.confidence}
            </small>
            {reference.evidence[0] && (
              <>
                {' — '}
                <code>{formatAddress(reference.evidence[0])}</code>
              </>
            )}
          </p>
        ))
      )}
      {lookup.report.truncated && <p className="muted">list truncated — narrow the name</p>}
    </div>
  )
}

function formatAddress(address: SourceAddress): string {
  const lines =
    address.endLine > address.startLine
      ? `${address.startLine}–${address.endLine}`
      : `${address.startLine}`
  return `${address.path}:${lines}`
}
