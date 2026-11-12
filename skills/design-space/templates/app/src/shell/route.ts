import { useSyncExternalStore } from "react";
import { STATES, type PrototypeState } from "../mocks/respondByState";

export type Device = "desktop" | "tablet" | "mobile";
export const DEVICES: Record<Device, number | null> = { desktop: null, tablet: 768, mobile: 390 };

const subscribe = (notify: () => void) => {
  window.addEventListener("hashchange", notify);
  return () => window.removeEventListener("hashchange", notify);
};
const read = () => window.location.hash.replace(/^#/, "") || "/";

// The route lives in the URL hash: "#/preview/<flow>/<page>?state=empty&role=admin&device=mobile".
export function useRoute() {
  const full = useSyncExternalStore(subscribe, read, () => "/");
  const [path, search = ""] = full.split("?");
  const query = new URLSearchParams(search);
  const state = STATES.find((known) => known === query.get("state")) ?? "success";
  const device = (Object.keys(DEVICES) as Device[]).find((known) => known === query.get("device")) ?? "desktop";
  return { path, query, state: state as PrototypeState, role: query.get("role") ?? "", device };
}

export function setQueryValue(key: string, value: string) {
  const [path, search = ""] = read().split("?");
  const query = new URLSearchParams(search);
  if (value) query.set(key, value);
  else query.delete(key);
  const next = query.toString();
  window.location.hash = next ? `${path}?${next}` : path;
}

export { auditName } from "./audit-route.mjs";
