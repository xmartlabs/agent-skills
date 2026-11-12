import { Component as ReactComponent, useEffect, useId, useMemo, useRef, useState, type ReactNode } from "react";
import { ChevronIcon } from "../shell/icons";
import type { Control, PlaygroundSpec } from "../shell/specimens";
import { snippetFor } from "./snippet.mjs";

type Values = Record<string, unknown>;

const defaultsOf = (props: Record<string, Control>): Values =>
  Object.fromEntries(Object.entries(props).map(([name, control]) => [name, control.default]));

// A child the snippet can print: text or a number.
const childText = (value: unknown) =>
  typeof value === "string" || typeof value === "number" ? String(value) : undefined;

// The controls are the shell's own form elements, never the product's components.
function ControlField({ name, control, value, onChange }: {
  name: string;
  control: Control;
  value: unknown;
  onChange: (value: unknown) => void;
}) {
  const id = useId();
  if (control.type === "boolean") {
    return (
      <div className="ds-control">
        <label className="ds-check" htmlFor={id}>
          <input id={id} className="ds-checkbox" type="checkbox" checked={Boolean(value)} onChange={(event) => onChange(event.target.checked)} />
          <span>{name}</span>
        </label>
      </div>
    );
  }
  return (
    <div className="ds-control">
      <label className="ds-control-label" htmlFor={id}>{name}</label>
      {control.type === "select" ? (
        <span className="ds-select">
          <select id={id} className="ds-input" value={String(value)} onChange={(event) => onChange(event.target.value)}>
            {control.options.map((option) => <option key={option} value={option}>{option}</option>)}
          </select>
          <span className="ds-select-chevron"><ChevronIcon size={14} /></span>
        </span>
      ) : control.type === "number" ? (
        // Uncontrolled, so a half-typed number ("-") is not snapped back; only finite values apply.
        <input
          id={id}
          className="ds-input"
          type="number"
          defaultValue={String(control.default)}
          onChange={(event) => {
            const next = event.target.valueAsNumber;
            if (Number.isFinite(next)) onChange(next);
          }}
        />
      ) : (
        <input id={id} className="ds-input" type="text" value={String(value ?? "")} onChange={(event) => onChange(event.target.value)} />
      )}
    </div>
  );
}

// A prop combination that makes the component throw shows a message instead of an empty page; the
// next change of props tries again (without remounting the component on every change).
class PreviewBoundary extends ReactComponent<{ resetKey: string; children: ReactNode }, { error: string | null }> {
  state = { error: null as string | null };
  static getDerivedStateFromError(error: unknown) {
    return { error: error instanceof Error ? error.message : String(error) };
  }
  componentDidUpdate(previous: { resetKey: string }) {
    if (this.state.error && previous.resetKey !== this.props.resetKey) this.setState({ error: null });
  }
  render() {
    if (this.state.error) {
      return <p className="ds-playground-error" role="alert">The component failed to render with these props: {this.state.error}</p>;
    }
    return this.props.children;
  }
}

function Snippet({ code }: { code: string }) {
  const [copied, setCopied] = useState(false);
  const codeRef = useRef<HTMLElement>(null);
  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), 1500);
    return () => window.clearTimeout(timer);
  }, [copied]);

  // The clipboard API needs a secure origin and permission. Without it the snippet is selected,
  // so the designer can copy it with the keyboard; nothing is reported.
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
    } catch {
      const node = codeRef.current;
      const selection = window.getSelection();
      if (!node || !selection) return;
      const range = document.createRange();
      range.selectNodeContents(node);
      selection.removeAllRanges();
      selection.addRange(range);
    }
  };

  return (
    <div className="ds-snippet">
      <pre className="ds-snippet-code"><code ref={codeRef} data-playground-snippet>{code}</code></pre>
      <button type="button" className="ds-copy-button" onClick={copy}>
        <span aria-live="polite">{copied ? "Copied" : "Copy"}</span>
      </button>
    </div>
  );
}

// Controls on the left, the live component (in .ds-canvas, unstyled by the shell) on the right,
// and the JSX for the current props below it.
export function Playground({ spec, tag }: { spec: PlaygroundSpec; tag: string }) {
  // A specimen written by hand may leave out props; it then has only children, or nothing to change.
  const controls = spec.props ?? {};
  const defaults = useMemo(() => defaultsOf(controls), [controls]);
  const [values, setValues] = useState<Values>(defaults);
  const [children, setChildren] = useState<unknown>(spec.children?.default);
  const Live = spec.component;
  const text = childText(children);
  const code = snippetFor(tag, values, text, defaults);

  return (
    <div className="ds-playground">
      <div className="ds-card ds-playground-controls">
        {spec.children && (
          <ControlField name="children" control={spec.children} value={children} onChange={setChildren} />
        )}
        {Object.entries(controls).map(([name, control]) => (
          <ControlField
            key={name}
            name={name}
            control={control}
            value={values[name]}
            onChange={(value) => setValues((all) => ({ ...all, [name]: value }))}
          />
        ))}
      </div>
      <div className="ds-playground-stage">
        <div className="ds-playground-preview">
          <div className="ds-canvas">
            <PreviewBoundary resetKey={code}>
              {text === undefined ? <Live {...values} /> : <Live {...values}>{text}</Live>}
            </PreviewBoundary>
          </div>
        </div>
        <Snippet code={code} />
      </div>
    </div>
  );
}
