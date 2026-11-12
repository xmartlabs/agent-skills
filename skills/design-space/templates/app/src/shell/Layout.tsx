import { useEffect, useState, type ReactNode } from "react";
import { CloseIcon, MenuIcon } from "./icons";
import { Brand, Sidebar } from "./Sidebar";

// The shell around every page except previews and audit frames: the index on the left, the page
// on the right. On narrow screens the index opens from a menu button.
export function Layout({ path, children }: { path: string; children: ReactNode }) {
  const [navOpen, setNavOpen] = useState(false);
  useEffect(() => setNavOpen(false), [path]);
  // Escape closes the index opened over the page.
  useEffect(() => {
    if (!navOpen) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setNavOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [navOpen]);
  return (
    <div className="ds-shell ds-layout" data-nav-open={navOpen}>
      <div className="ds-mobilebar">
        <button
          type="button"
          className="ds-icon-button"
          aria-label={navOpen ? "Close the index" : "Open the index"}
          aria-expanded={navOpen}
          onClick={() => setNavOpen((open) => !open)}
        >
          {navOpen ? <CloseIcon /> : <MenuIcon />}
        </button>
        <Brand />
      </div>
      <Sidebar path={path} onNavigate={() => setNavOpen(false)} />
      <main className="ds-shell ds-main">
        <div className="ds-content">{children}</div>
      </main>
    </div>
  );
}
