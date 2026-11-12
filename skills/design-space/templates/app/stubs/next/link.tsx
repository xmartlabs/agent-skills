import { flowHref } from "./flow-href";
import type { AnchorHTMLAttributes, ReactNode } from "react";

type Props = Omit<AnchorHTMLAttributes<HTMLAnchorElement>, "href"> & {
  href: string | { pathname?: string };
  children?: ReactNode;
  // Next.js props that make no sense here; accepted and ignored.
  prefetch?: boolean;
  replace?: boolean;
  scroll?: boolean;
  shallow?: boolean;
};

export default function Link({ href, prefetch, replace, scroll, shallow, ...rest }: Props) {
  void prefetch; void replace; void scroll; void shallow;
  const path = typeof href === "string" ? href : (href.pathname ?? "");
  // Flow routes live behind the hash: "/orders" becomes "#/preview/<flow>/orders" inside a flow.
  return <a href={flowHref(path)} {...rest} />;
}
