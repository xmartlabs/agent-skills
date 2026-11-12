// Runs inside an audit frame: when the frame URL carries "audit=a11y", waits for the page to settle, runs axe-core and
// reports to the parent.
export const AXE_RULES = ["button-name", "link-name", "label", "select-name", "image-alt"];
const QUIET_MS = 300;
const CAP_MS = 10000;

// Resolves true once the app has rendered and the DOM has been still for QUIET_MS; false when CAP_MS ran out first.
function settle(): Promise<boolean> {
  return new Promise((resolve) => {
    let quiet = 0;
    const finish = (settled: boolean) => {
      observer.disconnect();
      window.clearTimeout(quiet);
      window.clearTimeout(cap);
      resolve(settled);
    };
    const wait = () => {
      window.clearTimeout(quiet);
      quiet = window.setTimeout(() => (document.getElementById("root")?.childElementCount ? finish(true) : wait()), QUIET_MS);
    };
    const observer = new MutationObserver(wait);
    observer.observe(document.documentElement, { subtree: true, childList: true, attributes: true, characterData: true });
    const cap = window.setTimeout(() => finish(false), CAP_MS);
    wait();
  });
}

export function runAxeInFrame() {
  if (window.parent === window) return;
  const [path, search = ""] = window.location.hash.replace(/^#/, "").split("?");
  if (new URLSearchParams(search).get("audit") !== "a11y") return;
  const report = (fields: object) =>
    window.parent.postMessage({ type: "design-space:axe", target: path, violations: [], ...fields }, window.location.origin);
  (async () => {
    try {
      const settled = await settle();
      const failure = document.querySelector("[data-design-space-error]");
      if (failure) return report({ error: failure.textContent ?? "the page failed to load" });
      const axe = (await import("axe-core")).default;
      // A component page judges the specimen alone (its .ds-canvas); a flow page has none and is judged whole.
      const context = document.querySelector(".ds-canvas") ?? document;
      const results = await axe.run(context, { runOnly: { type: "rule", values: AXE_RULES } });
      report({
        unsettled: !settled,
        violations: results.violations.map(({ id, impact, nodes }) => ({ id, impact, nodes: nodes.map(({ html }) => ({ html })) })),
      });
    } catch (error) {
      report({ error: String(error) });
    }
  })();
}
