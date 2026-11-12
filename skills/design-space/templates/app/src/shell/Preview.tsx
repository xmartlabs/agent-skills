import { useEffect, useState, type ComponentType } from "react";
import { setFlowHandlers } from "../mocks/worker";
import { flows, loaderFor, loaders } from "./flows";
import { DEVICES, useRoute } from "./route";
import { Toolbar } from "./Toolbar";

const embedded = window.parent !== window;

// "#/preview/<flow>/<page>" shows the flow's page, or its start route. Failure states carry
// data-design-space-error, so the accessibility audit reports the page as not checked.
export function Preview({ name, page }: { name: string; page: string }) {
  const { device, query } = useRoute();
  const flow = flows.find((known) => known.name === name);
  const [pages, setPages] = useState<Record<string, ComponentType> | null>(null);
  const [failure, setFailure] = useState("");

  useEffect(() => {
    setPages(null);
    setFailure("");
    const routes = loaderFor(loaders.routes, name);
    const handlers = loaderFor(loaders.handlers, name);
    Promise.all([routes?.(), handlers?.()])
      .then(([loadedRoutes, loadedHandlers]) => {
        setFlowHandlers(loadedHandlers?.handlers ?? []);
        setPages(loadedRoutes?.default ?? {});
      })
      .catch((error) => setFailure(String(error)));
  }, [name]);

  if (!flow) return <p data-design-space-error>No flow named {name}. <a href="#/prototypes">All prototypes</a></p>;
  const width = DEVICES[device];
  // A narrow device shows the same page in a frame, so media queries see the frame's width.
  const frameQuery = new URLSearchParams(query);
  frameQuery.delete("device");
  const Page = pages?.[page || flow.meta.startRoute];

  return (
    <>
      {!embedded && <Toolbar roles={flow.meta.roles} />}
      {width && !embedded ? (
        <iframe title={flow.meta.title} src={`#/preview/${name}${page}?${frameQuery}`} style={{ display: "block", width, height: "80vh", margin: "16px auto", border: "1px solid #ddd" }} />
      ) : failure ? (
        <pre role="alert" data-design-space-error>{failure}</pre>
      ) : Page ? (
        <Page />
      ) : pages ? (
        <p data-design-space-error>This flow has no page at {page || flow.meta.startRoute}.</p>
      ) : null}
    </>
  );
}
