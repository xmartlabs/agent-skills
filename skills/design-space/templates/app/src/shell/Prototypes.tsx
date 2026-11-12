import { useState } from "react";
import { flows } from "./flows";

// "#/prototypes": every flow, grouped by status, with a search box.
export function Prototypes() {
  const [search, setSearch] = useState("");
  const matching = flows.filter(({ meta }) => meta.title.toLowerCase().includes(search.trim().toLowerCase()));
  const statuses = [...new Set(matching.map(({ meta }) => meta.status))];
  const counts = [...new Set(flows.map(({ meta }) => meta.status))].map((status) => ({
    status,
    count: flows.filter(({ meta }) => meta.status === status).length,
  }));
  return (
    <div className="ds-page">
      <header className="ds-page-head">
        <h1 className="ds-page-title">Prototypes</h1>
        <p className="ds-lede">Flows built with the product's components and mocked data. Open one to preview it.</p>
        {counts.length > 0 && (
          <ul className="ds-pill-row" aria-label="Prototypes by status">
            {counts.map(({ status, count }) => (
              <li key={status} className="ds-badge">{status}<span className="ds-count">{count}</span></li>
            ))}
          </ul>
        )}
      </header>
      <input
        className="ds-input"
        type="search"
        placeholder="Search prototypes"
        aria-label="Search flows"
        value={search}
        onChange={(event) => setSearch(event.target.value)}
      />
      {flows.length === 0 && (
        <div className="ds-card ds-empty">
          <p className="ds-card-text">No prototypes yet. Start one with the <code className="ds-code">new</code> step.</p>
          <table className="ds-table">
            <thead>
              <tr><th>Title</th><th>Status</th></tr>
            </thead>
            <tbody />
          </table>
        </div>
      )}
      {flows.length > 0 && matching.length === 0 && <p className="ds-muted-text">No flows found.</p>}
      {statuses.map((status) => (
        <section key={status} className="ds-section">
          <h2 className="ds-section-title ds-status-title">{status}</h2>
          <div className="ds-card ds-table-card">
            <table className="ds-table">
              <thead>
                <tr><th>Title</th><th>Roles</th><th>Start page</th></tr>
              </thead>
              <tbody>
                {matching.filter(({ meta }) => meta.status === status).map(({ name, meta }) => (
                  <tr key={name}>
                    <td><a className="ds-link" href={`#/preview/${name}`}>{meta.title}</a></td>
                    <td>{meta.roles.join(", ") || "—"}</td>
                    <td><code className="ds-code">{meta.startRoute}</code></td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      ))}
    </div>
  );
}
