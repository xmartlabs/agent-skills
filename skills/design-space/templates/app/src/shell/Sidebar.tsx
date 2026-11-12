import { useEffect, useRef, useState, type ComponentType } from "react";
import mechanism from "../theme";
import { BranchIcon, CheckIcon, ChevronIcon, ComponentIcon, LayersIcon, PaletteIcon, PlayIcon, SearchIcon } from "./icons";
import { filterNav, type NavSection } from "./nav.mjs";
import { site } from "./site";
import { ThemeSwitch } from "./ThemeSwitch";

const SECTION_ICONS: Record<NavSection["key"], ComponentType> = {
  tokens: PaletteIcon,
  components: ComponentIcon,
  shared: LayersIcon,
  prototypes: PlayIcon,
  audits: CheckIcon,
};

const SHORTCUT = /Mac|iPhone|iPad/.test(navigator.platform) ? "⌘K" : "Ctrl K";

// Long sections start closed unless they hold the current page.
const OPEN_UP_TO = 8;

const isActive = (href: string, path: string) => href === `#${path}`;

export function Brand() {
  return (
    <a className="ds-brand" href="#/">
      <span className="ds-brand-mark" aria-hidden="true">{site.product.charAt(0).toUpperCase()}</span>
      <span className="ds-brand-name">{site.product}</span>
    </a>
  );
}

export function Sidebar({ path, onNavigate }: { path: string; onNavigate?: () => void }) {
  const [query, setQuery] = useState("");
  const [toggled, setToggled] = useState<Record<string, boolean>>({});
  const search = useRef<HTMLInputElement>(null);
  const sections = filterNav(site.nav, query);
  const searching = query.trim() !== "";

  // Cmd/Ctrl+K focuses the search box from anywhere in the app, except inside a specimen, whose
  // component may use the shortcut itself.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.target instanceof Element && event.target.closest(".ds-canvas")) return;
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        search.current?.focus();
        search.current?.select();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const isOpen = (section: NavSection) => {
    if (searching) return true;
    if (section.key in toggled) return toggled[section.key];
    return section.items.length <= OPEN_UP_TO || section.items.some((item) => isActive(item.href, path));
  };

  return (
    <aside className="ds-sidebar" aria-label="Design space">
      <div className="ds-sidebar-head">
        <Brand />
      </div>
      <div className="ds-search">
        <SearchIcon />
        <input
          ref={search}
          className="ds-search-input"
          type="search"
          placeholder="Search"
          aria-label="Search the index"
          aria-keyshortcuts="Meta+K Control+K"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Escape") setQuery("");
            const first = sections[0]?.items[0];
            if (event.key === "Enter" && searching && first) {
              window.location.hash = first.href;
              setQuery("");
              onNavigate?.();
            }
          }}
        />
        <kbd className="ds-kbd" aria-hidden="true">{SHORTCUT}</kbd>
      </div>
      <nav className="ds-nav" aria-label="Index">
        {sections.length === 0 && <p className="ds-nav-empty">Nothing matches “{query.trim()}”.</p>}
        {sections.map((section) => {
          const Icon = SECTION_ICONS[section.key];
          const open = isOpen(section);
          const listId = `ds-nav-${section.key}`;
          return (
            <div className="ds-nav-section" key={section.key}>
              <button
                type="button"
                className="ds-nav-toggle"
                aria-expanded={open}
                aria-controls={listId}
                onClick={() => setToggled((all) => ({ ...all, [section.key]: !open }))}
              >
                <Icon />
                <span className="ds-nav-toggle-label">{section.label}</span>
                <span className="ds-nav-chevron" data-open={open}><ChevronIcon size={14} /></span>
              </button>
              {open && (
                <ul className="ds-nav-list" id={listId}>
                  {section.items.map((item) => (
                    <li key={item.href}>
                      <a
                        className="ds-nav-link"
                        href={item.href}
                        aria-current={isActive(item.href, path) ? "page" : undefined}
                        onClick={onNavigate}
                      >
                        {item.label}
                      </a>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          );
        })}
      </nav>
      <div className="ds-sidebar-foot">
        {site.branch && (
          <span className="ds-branch" title="Current git branch">
            <BranchIcon size={14} />
            <span className="ds-branch-name">{site.branch}</span>
          </span>
        )}
        {(mechanism.kind as string) === "media" && <span className="ds-foot-note">Follows the system theme.</span>}
        <ThemeSwitch />
      </div>
    </aside>
  );
}
