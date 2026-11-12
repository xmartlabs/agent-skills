import { STATES } from "../mocks/respondByState";
import { DEVICES, setQueryValue, useRoute, type Device } from "./route";

export function Toolbar({ roles }: { roles: string[] }) {
  const route = useRoute();
  const { state, device } = route;
  // Show and use the same role: the flow's first one until the designer picks another.
  const role = route.role || roles[0] || "";
  return (
    <div role="toolbar" aria-label="Prototype controls" style={{ display: "flex", gap: 16, padding: "8px 16px", borderBottom: "1px solid #ddd", font: "14px system-ui" }}>
      <a href="#/">Home</a>
      <label>
        State{" "}
        <select value={state} onChange={(event) => setQueryValue("state", event.target.value === "success" ? "" : event.target.value)}>
          {STATES.map((name) => <option key={name}>{name}</option>)}
        </select>
      </label>
      {roles.length > 0 && (
        <label>
          Role{" "}
          <select value={role} onChange={(event) => setQueryValue("role", event.target.value)}>
            {roles.map((name) => <option key={name}>{name}</option>)}
          </select>
        </label>
      )}
      <label>
        Device{" "}
        <select value={device} onChange={(event) => setQueryValue("device", event.target.value === "desktop" ? "" : event.target.value)}>
          {(Object.keys(DEVICES) as Device[]).map((name) => <option key={name}>{name}</option>)}
        </select>
      </label>
    </div>
  );
}
