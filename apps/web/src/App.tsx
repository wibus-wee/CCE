import { FormEvent, useCallback, useEffect, useState } from 'react'
import {
  api,
  ContextPack,
  describeError,
  ProviderReport,
  SearchResult,
  ViewManifest,
} from './api'
import { ProvidersPanel } from './ProvidersPanel'
import { SearchResults } from './SearchResults'

export function App() {
  const [manifest, setManifest] = useState<ViewManifest>()
  const [providers, setProviders] = useState<ProviderReport[]>()
  const [providersError, setProvidersError] = useState<string>()
  const [searchText, setSearchText] = useState('snapshot freshness')
  const [limit, setLimit] = useState(25)
  const [result, setResult] = useState<SearchResult>()
  const [searchError, setSearchError] = useState<string>()
  const [query, setQuery] = useState('Where is snapshot freshness decided?')
  const [budget, setBudget] = useState(4096)
  const [pack, setPack] = useState<ContextPack>()
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string>()

  const refresh = useCallback(async () => {
    setBusy(true)
    setError(undefined)
    // Status and provider probes are independent: one failing must not
    // blank the other.
    const [status, providerReports] = await Promise.allSettled([
      api.status(),
      api.providers(),
    ])
    if (status.status === 'fulfilled') {
      setManifest(status.value)
    } else {
      setError(describeError(status.reason))
    }
    if (providerReports.status === 'fulfilled') {
      setProviders(providerReports.value)
      setProvidersError(undefined)
    } else {
      setProvidersError(describeError(providerReports.reason))
    }
    setBusy(false)
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  async function reindex() {
    setBusy(true)
    setError(undefined)
    try {
      const report = await api.index()
      setManifest(report.manifest)
      // IndexReport.providers carries run outcomes (duration, SCIP counts);
      // fresher than re-probing.
      setProviders(report.providers)
      setProvidersError(undefined)
    } catch (value) {
      setError(describeError(value))
    } finally {
      setBusy(false)
    }
  }

  async function search(event: FormEvent) {
    event.preventDefault()
    const text = searchText.trim()
    if (!text) return
    setBusy(true)
    setSearchError(undefined)
    try {
      const safeLimit = Number.isFinite(limit) ? Math.min(200, Math.max(1, Math.trunc(limit))) : 20
      setResult(await api.search({ query: text, limit: safeLimit }))
    } catch (value) {
      setResult(undefined)
      setSearchError(describeError(value))
    } finally {
      setBusy(false)
    }
  }

  async function ask(event: FormEvent) {
    event.preventDefault()
    if (!query.trim()) return
    setBusy(true)
    setError(undefined)
    try {
      const result = await api.context(query, budget)
      setPack(result)
      setManifest((current) =>
        current && current.snapshotId === result.snapshotId ? current : undefined,
      )
    } catch (value) {
      setError(describeError(value))
    } finally {
      setBusy(false)
    }
  }

  const degradedViews = manifest
    ? Object.entries(manifest.views).filter(([, view]) => view.state !== 'ready')
    : []

  return (
    <main>
      <header>
        <div>
          <p className="eyebrow">Repository intelligence runtime</p>
          <h1>CCE</h1>
          <p className="subtitle">The smallest source-linked world sufficient for the task.</p>
        </div>
        <button type="button" onClick={() => void reindex()} disabled={busy}>
          {busy ? 'Working…' : 'Refresh index'}
        </button>
      </header>

      {error && <div className="error" role="alert">{error}</div>}

      <section className="manifest" aria-label="Index views">
        <div className="section-heading">
          <h2>Materialized views</h2>
          <code>{manifest?.snapshotId.slice(0, 21) ?? 'connecting…'}</code>
        </div>
        {degradedViews.length > 0 && (
          <p className="stale-note" role="status">
            Not fully current:{' '}
            {degradedViews
              .map(([name, view]) => `${name} (${view.state.replaceAll('_', ' ')})`)
              .join(', ')}
          </p>
        )}
        {manifest ? (
          Object.keys(manifest.views).length === 0 ? (
            <p className="empty">
              No views reported yet — press “Refresh index” to build the first snapshot.
            </p>
          ) : (
            <div className="view-grid">
              {Object.entries(manifest.views).map(([name, view]) => (
                <article className="view" key={name}>
                  <div className="view-title">
                    <h3>{name}</h3>
                    <span className={`state state-${view.state}`}>
                      {view.state.replaceAll('_', ' ')}
                    </span>
                  </div>
                  {view.capabilities.map((capability) => (
                    <p key={capability.name} title={capability.reason}>
                      {capability.name} <small>{capability.level}</small>
                    </p>
                  ))}
                  {view.message && <p className="muted">{view.message}</p>}
                </article>
              ))}
            </div>
          )
        ) : error ? (
          <p className="empty">Status unavailable — see the error above.</p>
        ) : (
          <div className="view-grid">
            {Array.from({ length: 6 }, (_, index) => (
              <div className="view skeleton" key={index} />
            ))}
          </div>
        )}
      </section>

      <ProvidersPanel providers={providers} error={providersError} />

      <section>
        <h2>Search</h2>
        <form onSubmit={(event) => void search(event)}>
          <label>
            Query
            <input
              value={searchText}
              onChange={(event) => setSearchText(event.target.value)}
              placeholder="symbol, path, or question — lang:rust path:crates/ narrows"
            />
          </label>
          <label className="budget">
            Limit
            <input
              type="number"
              min={1}
              max={200}
              value={limit}
              onChange={(event) => setLimit(Number(event.target.value))}
            />
          </label>
          <button disabled={busy || !searchText.trim()}>Search</button>
        </form>
        {searchError && <div className="error" role="alert">{searchError}</div>}
      </section>

      {result && <SearchResults result={result} />}

      <section>
        <h2>Build context</h2>
        <form onSubmit={(event) => void ask(event)}>
          <label>
            Repository question
            <textarea value={query} onChange={(event) => setQuery(event.target.value)} rows={3} />
          </label>
          <label className="budget">
            Token budget
            <input
              type="number"
              min={256}
              max={128000}
              value={budget}
              onChange={(event) => setBudget(Number(event.target.value))}
            />
          </label>
          <button disabled={busy || !query.trim()}>Build source-linked pack</button>
        </form>
      </section>

      {pack && (
        <section className="pack">
          <div className="section-heading">
            <div>
              <p className="eyebrow">{pack.intent.replaceAll('_', ' ')}</p>
              <h2>Context pack</h2>
            </div>
            <strong>
              {pack.usedTokens.toLocaleString()} / {pack.budgetTokens.toLocaleString()} tokens
            </strong>
          </div>
          {pack.items.map((item) => (
            <details key={item.id} open={item.kind === 'orientation' || item.provenance.rank < 4}>
              <summary>
                <span>{item.title}</span>
                <small>
                  {item.kind} · {item.estimatedTokens}t
                </small>
              </summary>
              <pre>{item.body}</pre>
              <footer>
                {item.provenance.route} · rank {item.provenance.rank} ·{' '}
                {item.provenance.whyRetrieved}
              </footer>
            </details>
          ))}
          {pack.uncertainties.length > 0 && (
            <aside>
              <h3>Uncertainties</h3>
              {pack.uncertainties.map((uncertainty) => (
                <p key={`${uncertainty.capability}:${uncertainty.message}`}>
                  <strong>{uncertainty.capability}</strong> — {uncertainty.message}
                </p>
              ))}
            </aside>
          )}
        </section>
      )}
    </main>
  )
}
