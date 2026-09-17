import { ProviderReport } from './api'

// Shows either the live detect probe (GET /v1/providers) or the provider
// outcomes recorded by the last index pass (IndexReport.providers); the
// latter carry duration and SCIP ingest counts.
export function ProvidersPanel({
  providers,
  error,
}: {
  providers?: ProviderReport[]
  error?: string
}) {
  return (
    <details className="providers" open>
      <summary>
        Provider toolchains
        {providers && <small> · {providers.length} detected</small>}
      </summary>
      {error && <p className="error">{error}</p>}
      {!providers && !error && <p className="muted">Probing providers…</p>}
      {providers?.length === 0 && <p className="muted">No providers are registered.</p>}
      {providers && providers.length > 0 && (
        <div className="provider-list">
          {providers.map((provider) => (
            <ProviderCard key={provider.providerId} provider={provider} />
          ))}
        </div>
      )}
    </details>
  )
}

function ProviderCard({ provider }: { provider: ProviderReport }) {
  const scipStats =
    provider.scipDocuments > 0
      ? `${provider.scipDocuments} docs · ${provider.scipDefinitions} defs · ${provider.scipReferenceEdges} refs`
      : undefined
  const stats = [
    provider.durationMs != null ? `${provider.durationMs} ms` : undefined,
    scipStats,
  ]
    .filter(Boolean)
    .join(' · ')

  return (
    <article className="provider">
      <div className="provider-head">
        <h3>{provider.providerId}</h3>
        <span className={`state state-${provider.state}`}>
          {provider.state.replaceAll('_', ' ')}
        </span>
      </div>
      {provider.tool && (
        <p className="provider-line">
          <small>tool</small>
          {provider.tool}
        </p>
      )}
      {provider.message && <p className="provider-line">{provider.message}</p>}
      {stats && <p className="provider-line muted">{stats}</p>}
    </article>
  )
}
