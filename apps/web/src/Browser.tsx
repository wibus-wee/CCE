import { useCallback, useEffect, useMemo, useState } from 'react'
import { api, describeError, FileContent, FileListEntry } from './api'

export function Browser({ refreshKey }: { refreshKey: number }) {
  const [report, setReport] = useState<{ snapshotId: string; files: FileListEntry[] }>()
  const [filter, setFilter] = useState('')
  const [selected, setSelected] = useState<string>()
  const [file, setFile] = useState<FileContent>()
  const [listError, setListError] = useState<string>()
  const [fileError, setFileError] = useState<string>()
  const [loadingFile, setLoadingFile] = useState(false)

  const reload = useCallback(async () => {
    setListError(undefined)
    try {
      const result = await api.files()
      setReport(result)
      // The snapshot may have rotated under the open file — mark it stale
      // implicitly by keeping the snapshotId beside the content.
    } catch (value) {
      setReport(undefined)
      setListError(describeError(value))
    }
  }, [])

  useEffect(() => {
    void reload()
  }, [reload, refreshKey])

  useEffect(() => {
    if (!selected) return
    let cancelled = false
    setLoadingFile(true)
    setFileError(undefined)
    api
      .file(selected)
      .then((content) => {
        if (!cancelled) setFile(content)
      })
      .catch((value) => {
        if (!cancelled) {
          setFile(undefined)
          setFileError(describeError(value))
        }
      })
      .finally(() => {
        if (!cancelled) setLoadingFile(false)
      })
    return () => {
      cancelled = true
    }
  }, [selected])

  const visible = useMemo(() => {
    const needle = filter.trim().toLowerCase()
    const all = report?.files ?? []
    return needle ? all.filter((entry) => entry.path.toLowerCase().includes(needle)) : all
  }, [report, filter])

  return (
    <section>
      <div className="section-heading">
        <h2>Code browser</h2>
        <code>{report ? `${report.files.length} files` : 'connecting…'}</code>
      </div>
      {listError && <div className="error" role="alert">{listError}</div>}
      {report && (
        <div className="browser">
          <div className="browser-list">
            <input
              value={filter}
              onChange={(event) => setFilter(event.target.value)}
              placeholder="filter paths…"
              aria-label="Filter files"
            />
            <ul>
              {visible.slice(0, 500).map((entry) => (
                <li key={entry.path}>
                  <button
                    type="button"
                    className={entry.path === selected ? 'selected' : ''}
                    onClick={() => setSelected(entry.path)}
                  >
                    {entry.path}
                  </button>
                </li>
              ))}
              {visible.length > 500 && (
                <li className="muted">…{visible.length - 500} more — narrow the filter</li>
              )}
              {visible.length === 0 && <li className="muted">no matching files</li>}
            </ul>
          </div>
          <div className="browser-view">
            {selected ? (
              <>
                <div className="section-heading">
                  <code>{selected}</code>
                  <small className="muted">
                    {file?.language ?? ''}
                    {file ? ` · ${file.snapshotId.slice(0, 16)}` : ''}
                  </small>
                </div>
                {fileError && <div className="error" role="alert">{fileError}</div>}
                {loadingFile && <p className="muted">loading…</p>}
                {file?.binary && <p className="empty">Binary file — content not shown.</p>}
                {file && !file.binary && (
                  <>
                    <pre>{file.content}</pre>
                    {file.truncated && (
                      <p className="stale-note">Truncated at 1 MiB — the index has the rest.</p>
                    )}
                  </>
                )}
              </>
            ) : (
              <p className="empty">Pick a file to view its indexed content.</p>
            )}
          </div>
        </div>
      )}
    </section>
  )
}
