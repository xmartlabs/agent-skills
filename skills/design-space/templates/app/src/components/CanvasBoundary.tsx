import { Component as ReactComponent, type ErrorInfo, type ReactNode } from "react";

// Wraps a .ds-canvas render (a specimen, a shared component). A component that throws shows a
// message in the shell instead of unmounting the whole app; it still logs a console error, so the
// smoke test fails on it. Key it by the component name so another page starts clean.
export class CanvasBoundary extends ReactComponent<{ name: string; hint?: string; children: ReactNode }, { error: string | null }> {
  state = { error: null as string | null };
  static getDerivedStateFromError(error: unknown) {
    return { error: error instanceof Error ? error.message : String(error) };
  }
  componentDidCatch(error: unknown, info: ErrorInfo) {
    console.error(`[specimen] ${this.props.name} failed to render:`, error, info.componentStack ?? "");
  }
  render() {
    if (this.state.error === null) return this.props.children;
    return (
      <div className="ds-card ds-note" role="alert" data-design-space-error>
        <p className="ds-note-title">This component failed to render: {this.state.error}</p>
        {this.props.hint && <p className="ds-note-text">{this.props.hint}</p>}
      </div>
    );
  }
}
