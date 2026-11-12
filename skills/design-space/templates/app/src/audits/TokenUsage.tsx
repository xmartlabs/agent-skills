import type { ReactNode } from "react";
import { AuditPage, Badge } from "./AuditPage";
import { tokenUsageFindings } from "./pairs.mjs";
import { emittedValue, liveValue, tokenMap, tokensByVar } from "../tokens/map";

// One finding group: a title, what it means, then its list (or "None." in an empty card). The list
// follows the lede directly, so a reader can find it from the heading.
function Group({ title, lede, items }: { title: string; lede: string; items: ReactNode[] }) {
  return (
    <section className="ds-section">
      <h2 className="ds-section-title">{title}</h2>
      <p className="ds-section-lede">{lede}</p>
      {items.length === 0 ? <p className="ds-card ds-empty ds-muted-text">None.</p> : <ul className="ds-card ds-list">{items}</ul>}
    </section>
  );
}

export function TokenUsage() {
  const tokens = tokenMap();
  const byVar = tokensByVar(tokens);
  const { unresolved, duplicates, overrides } = tokenUsageFindings(
    tokens,
    new Map(tokens.map((t) => [t.name, emittedValue(t)])),
    new Map(tokens.map((t) => [t.name, liveValue(t, byVar)])),
  );
  return (
    <AuditPage title="Token usage" lede={`${tokens.length} tokens read.`}>
      <Group
        title="Tokens with no value"
        lede="Empty, or a var() reference that resolves nowhere in the active theme."
        items={unresolved.map((name) => <li key={name}><code className="ds-code">{name}</code></li>)}
      />
      <Group
        title="Designer tokens that override a product token"
        lede="A token in library/ that redefines a product variable changes what the other audits measure."
        items={overrides.map((name) => (
          <li key={name}>
            <code className="ds-code">{name}</code>
            <span>Designer token overrides a product token</span>
            <Badge tone="bad">fail</Badge>
          </li>
        ))}
      />
      <Group
        title="Tokens that share a value"
        lede="Two tokens with the same value may be one token, or may be meant to differ later: check by hand."
        items={duplicates.map(({ value, names }) => (
          <li key={`${value}|${names.join()}`}>
            <code className="ds-code">{value}</code>
            <span>{names.join(", ")}</span>
            <Badge tone="warn">check by hand</Badge>
          </li>
        ))}
      />
    </AuditPage>
  );
}
