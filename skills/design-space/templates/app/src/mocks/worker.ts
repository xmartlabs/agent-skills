import { http, HttpResponse, type RequestHandler } from "msw";
import { setupWorker } from "msw/browser";
import { FEEDBACK_ENDPOINT } from "../feedback";

// Last in line: a request nobody mocked answers 501 and says so in the console.
// The app's own files (anything with a file extension, Vite internals) and the feedback server are not API calls.
const isAppFile = ({ pathname }: URL) => /\.[A-Za-z0-9]+$/.test(pathname) || /^\/(@|__|node_modules\/)/.test(pathname);
const unmocked = http.all("*", ({ request }) => {
  const url = new URL(request.url);
  if (request.destination || request.url.startsWith(FEEDBACK_ENDPOINT)) return undefined;
  if (url.origin === window.location.origin && isAppFile(url)) return undefined;
  // A frame loading the app itself (a preview or an audit frame) is a page load, not an API call.
  if (url.origin === window.location.origin && request.mode === "navigate") return undefined;
  console.warn(`[prototype] No mock for ${request.method} ${request.url}`);
  return HttpResponse.json({ message: `No mock for ${request.method} ${request.url}` }, { status: 501 });
});

const worker = setupWorker();

export async function startMocks() {
  await worker.start({
    serviceWorker: { url: new URL("mockServiceWorker.js", document.baseURI).href },
    quiet: true,
    // The option is named onUnhandledRequest up to msw 2 and onUnhandledFrame from msw 3.
    ...({ onUnhandledRequest: "bypass", onUnhandledFrame: "bypass" } as object),
  });
  setFlowHandlers([]);
}

// Replaces the mocks with the ones of the flow on screen.
export function setFlowHandlers(handlers: RequestHandler[]) {
  worker.resetHandlers(...handlers, unmocked);
}
