import { http } from "msw";
import { currentState, respondByState } from "../../src/mocks/respondByState";
import { orders } from "./fixtures";

export const handlers = [
  http.get("/api/orders", () =>
    respondByState(currentState(), {
      success: { body: orders },
      empty: { body: [] },
    }),
  ),
];
