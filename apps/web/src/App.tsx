import { FormEvent, useCallback, useEffect, useState } from 'react'
import { api, ContextPack, ViewManifest } from './api'

export function App() {
  const [manifest, setManifest] = useState<ViewManifest>()
  const [query, setQuery] = useState('Where is snapshot freshness decided?')
  const [budget, setBudget] = useState(4096)
  const [pack, setPack] = useState<ContextPack>()
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string>()

  const refresh = useCallback(async () => {
    setBusy(true)
    setError(undefined)
    try {
      setManifest(await api.status())
    } catch (value) {
      setError(asMessage(value))
    } finally {
      setBusy(false)
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  async function reindex() {
    setBusy(true)
    setError(undefined)
    try {
      const result = await api.index()
      setManifest(result.manifest)
    } catch (value) {
      setError(asMessage(value))
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
      setError(asMessage(value))
    } finally {
      setBusy(false)
    }
  }

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
        <div className="view-grid">
          {manifest
            ? Object.entries(manifest.views).map(([name, view]) => (
                <article className="view" key={name}>
                  <div className="view-title">
                    <h3>{name}</h3>
                    <span className={`state state-${view.state}`}>{view.state}</span>
                  </div>
                  {view.capabilities.map((capability) => (
                    <p key={capability.name} title={capability.reason}>
                      {capability.name} <small>{capability.level}</small>
                    </p>
                  ))}
                  {view.message && <p className="muted">{view.message}</p>}
                </article>
              ))
            : Array.from({ length: 6 }, (_, index) => <div className="view skeleton" key={index} />)}
        </div>
      </section>

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
            <strong>{pack.usedTokens.toLocaleString()} / {pack.budgetTokens.toLocaleString()} tokens</strong>
          </div>
          {pack.items.map((item) => (
            <details key={item.id} open={item.kind === 'orientation' || item.provenance.rank < 4}>
              <summary>
                <span>{item.title}</span>
                <small>{item.kind} · {item.estimatedTokens}t</small>
              </summary>
              <pre>{item.body}</pre>
              <footer>
                {item.provenance.route} · rank {item.provenance.rank} · {item.provenance.whyRetrieved}
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

function asMessage(value: unknown): string {
  return value instanceof Error ? value.message : String(value)
}

