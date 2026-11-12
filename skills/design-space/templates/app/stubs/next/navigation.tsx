import { useSyncExternalStore } from "react";
import { flowHref } from "./flow-href";

// Navigation state lives in the URL hash, which is where the design space router keeps it.
const subscribe = (notify: () => void) => {
  window.addEventListener("hashchange", notify);
  return () => window.removeEventListener("hashchange", notify);
};
const hash = () => window.location.hash.replace(/^#/, "") || "/";

export function usePathname() {
  const full = useSyncExternalStore(subscribe, hash, () => "/");
  return full.split("?")[0];
}

export function useSearchParams() {
  const full = useSyncExternalStore(subscribe, hash, () => "/");
  return new URLSearchParams(full.split("?")[1] ?? "");
}

export function useParams<T extends Record<string, string> = Record<string, string>>() {
  return {} as T;
}

export function useRouter() {
  return {
    push: (to: string) => { window.location.assign(flowHref(to)); },
    replace: (to: string) => { window.location.replace(flowHref(to)); },
    back: () => window.history.back(),
    forward: () => window.history.forward(),
    refresh: () => {},
    prefetch: () => {},
  };
}

export function redirect(to: string): never {
  window.location.assign(flowHref(to));
  throw new Error(`[prototype] redirect to ${to}`);
}

export function notFound(): never {
  throw new Error("[prototype] notFound()");
}
