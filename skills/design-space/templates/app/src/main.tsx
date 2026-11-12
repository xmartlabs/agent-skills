import { StrictMode, useEffect } from "react";
import { createRoot } from "react-dom/client";
import { runAxeInFrame } from "./audits/axe-frame";
import { Feedback } from "./feedback";
import { startMocks } from "./mocks/worker";
import { Audits } from "./shell/Audits";
import { Home } from "./shell/Home";
import { Layout } from "./shell/Layout";
import { ComponentPage, SharedPage } from "./components/ComponentPage";
import { NotFound } from "./shell/Pages";
import { TokenKindPage } from "./tokens/TokenKindPage";
import { Preview } from "./shell/Preview";
import { Prototypes } from "./shell/Prototypes";
import { useRoute } from "./shell/route";
import { resolveRoute } from "./shell/routes.mjs";
import { applyStoredTheme, syncTheme } from "./shell/ThemeSwitch";
import "./product-styles";
// The shell's look: its variables mapped to the product's tokens, then its own rules (all under .ds-shell).
import "./shell/theme-map.css";
import "./shell/shell.css";
import { Providers } from "./providers";

// Designers' tokens apply to the whole app.
import.meta.glob("../library/**/*.css", { eager: true });

// Apply the stored theme before the first render, in the app and in preview frames alike.
applyStoredTheme();
syncTheme();
runAxeInFrame();

const embedded = window.parent !== window;

function App({ mocksFailed }: { mocksFailed: boolean }) {
  const { path, query } = useRoute();
  const route = resolveRoute(path);
  const redirect = "redirect" in route ? route.redirect : null;

  // Old routes move to the new ones without a history entry; the query (state, audit mode) is kept.
  useEffect(() => {
    if (!redirect) return;
    const search = query.toString();
    window.location.replace(`#${redirect}${search ? `?${search}` : ""}`);
  }, [redirect, query]);

  if ("redirect" in route) return null;
  const { page, params } = route;
  const alert = mocksFailed && (
    <p role="alert" style={{ margin: 0, padding: "8px 16px", background: "#fff3cd", color: "#3d2c00", font: "14px system-ui" }}>
      Mock data is unavailable, so some screens may be empty or broken. Open the browser console for details.
    </p>
  );

  // Previews are full-bleed, and audit frames load a page alone: neither gets the shell.
  if (page === "preview" || (embedded && page === "component")) {
    return (
      <>
        {alert}
        {page === "preview" ? <Preview name={params.flow} page={params.page} /> : <ComponentPage name={params.name} embedded />}
        {!embedded && <Feedback />}
      </>
    );
  }

  const content =
    page === "home" ? <Home />
    : page === "tokens" ? <TokenKindPage kind={params.kind} />
    : page === "component" ? <ComponentPage name={params.name} embedded={false} />
    : page === "shared" ? <SharedPage name={params.name} />
    : page === "prototypes" ? <Prototypes />
    : page === "audits" || page === "audit" ? <Audits path={path} />
    : <NotFound />;
  return (
    <>
      {embedded ? content : <Layout path={path}>{alert}{content}</Layout>}
      {!embedded && <Feedback />}
    </>
  );
}

// Render even when the mocks do not start (the worker script fails to load, or the page is not on a secure origin).
let mocksFailed = false;
startMocks()
  .catch((error) => {
    mocksFailed = true;
    console.error("[prototype] Mocks failed to start", error);
  })
  .finally(() => {
    createRoot(document.getElementById("root")!).render(<StrictMode><Providers><App mocksFailed={mocksFailed} /></Providers></StrictMode>);
  });
