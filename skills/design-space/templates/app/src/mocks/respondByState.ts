import { delay, HttpResponse } from "msw";

export type PrototypeState = "loading" | "empty" | "error" | "success";
export const STATES: PrototypeState[] = ["success", "loading", "empty", "error"];

type Reply = { status?: number; body?: unknown };
export type Responses = { success: Reply; empty: Reply; error?: Reply };

// The state is picked in the toolbar and lives in the page URL: "#/preview/<flow>?state=empty".
export function currentState(): PrototypeState {
  const query = window.location.hash.split("?")[1] ?? "";
  const state = new URLSearchParams(query).get("state");
  return STATES.find((known) => known === state) ?? "success";
}

// Answers a mocked request the way the picked state calls for.
// `loading` never answers. `error` answers 500 unless the flow gives its own reply.
export async function respondByState(state: PrototypeState, responses: Responses): Promise<Response> {
  if (state === "loading") {
    await delay("infinite");
    return new Response(null, { status: 504 });
  }
  const reply = state === "error" ? (responses.error ?? { status: 500, body: { message: "Something went wrong" } }) : responses[state];
  return HttpResponse.json(reply.body ?? null, { status: reply.status ?? 200 });
}
