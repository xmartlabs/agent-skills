// Modules replaced by stubs in the design space.
// `match`: a repository-relative file path ("src/networking/socket.ts") or a bare package name ("next/link").
// `stub`: the replacement, relative to this folder.
// Bootstrap adds the product-specific entries (sockets, auth, analytics, native modules).
export default [
  { match: "next/link", stub: "stubs/next/link.tsx" },
  { match: "next/image", stub: "stubs/next/image.tsx" },
  { match: "next/navigation", stub: "stubs/next/navigation.tsx" },
];
