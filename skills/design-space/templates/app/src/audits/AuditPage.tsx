import type { ReactNode } from "react";
import mechanism from "../theme";
import { useTheme } from "../shell/ThemeSwitch";

/** Name of the theme the page reports on: the stored theme, or the OS setting for a media-query theme. */
export function useActiveTheme(): string {
  const { theme } = useTheme();
  if (mechanism.kind === "media") return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  return theme;
}

export const swatch = (c: { r: number; g: number; b: number }) => `rgb(${Math.round(c.r)} ${Math.round(c.g)} ${Math.round(c.b)})`;

export type BadgeTone = "ok" | "warn" | "bad" | "neutral";

/** A small status pill: green for a pass, amber for something to check, red for a failure. */
export function Badge({ tone = "neutral", children }: { tone?: BadgeTone; children: ReactNode }) {
  return <span className={tone === "neutral" ? "ds-badge" : `ds-badge ds-badge-${tone}`}>{children}</span>;
}

/** Shared frame of an audit page: title, what it checks, the report-only note and the theme the numbers are for. */
export function AuditPage({ title, lede, children }: { title: string; lede?: ReactNode; children: ReactNode }) {
  const theme = useActiveTheme();
  return (
    <div className="ds-page">
      <header className="ds-page-head">
        <a className="ds-link ds-back" href="#/audits">All audits</a>
        <h1 className="ds-page-title">{title}</h1>
        {lede && <p className="ds-lede">{lede}</p>}
        <p className="ds-head-meta">
          <Badge>Report only</Badge>
          <span>Does not block a pull request.</span>
          <span>Active theme: {theme}. Switch the theme to audit the other one.</span>
        </p>
      </header>
      {children}
    </div>
  );
}

export function Unreadable({ names }: { names: string[] }) {
  if (names.length === 0) return null;
  return (
    <section className="ds-section">
      <h2 className="ds-section-title">Could not read a color</h2>
      <p className="ds-section-lede">These pairs use a variable that is empty or not a color in the active theme.</p>
      <ul className="ds-card ds-list">{names.map((name) => <li key={name}>{name}</li>)}</ul>
    </section>
  );
}

/**
 * Shown on pages that read colors live: designer tokens in library/ that redefine a product
 * variable change what the page measures, so it names them instead of hiding the change.
 */
export function OverrideNotice({ tokens }: { tokens: Array<{ cssVar: string | null; name: string; overrides?: boolean }> }) {
  const names = tokens.filter((t) => t.overrides).map((t) => t.cssVar ?? t.name);
  if (names.length === 0) return null;
  return (
    <section role="note" className="ds-card ds-note">
      <h2 className="ds-note-title">Designer tokens override product tokens</h2>
      <p className="ds-note-text">
        These variables are redefined in library/, so the colors below may be the designer's values, not the
        product's: {names.join(", ")}. See the token usage audit.
      </p>
    </section>
  );
}
