// `node:path` for the embedded engine: the POSIX `normalize`, `join` and
// `dirname`, ported from Node.js's lib/path.js (MIT). Any other member throws
// an EMBEDDED-UNSUPPORTED error, which the host reads as "this engine cannot
// answer", never as the script's own throw.

const SLASH = 47;
const DOT = 46;

function validateString(value) {
  if (typeof value !== "string")
    throw new TypeError(`The "path" argument must be of type string. Received ${typeof value}`);
}

function normalizeString(path, allowAboveRoot) {
  let res = "";
  let lastSegmentLength = 0;
  let lastSlash = -1;
  let dots = 0;
  let code = 0;
  for (let i = 0; i <= path.length; ++i) {
    if (i < path.length) code = path.charCodeAt(i);
    else if (code === SLASH) break;
    else code = SLASH;
    if (code === SLASH) {
      if (lastSlash === i - 1 || dots === 1) {
        // A run of slashes, or a `.` segment.
      } else if (dots === 2) {
        if (
          res.length < 2 ||
          lastSegmentLength !== 2 ||
          res.charCodeAt(res.length - 1) !== DOT ||
          res.charCodeAt(res.length - 2) !== DOT
        ) {
          if (res.length > 2) {
            const lastSlashIndex = res.lastIndexOf("/");
            if (lastSlashIndex === -1) {
              res = "";
              lastSegmentLength = 0;
            } else {
              res = res.slice(0, lastSlashIndex);
              lastSegmentLength = res.length - 1 - res.lastIndexOf("/");
            }
            lastSlash = i;
            dots = 0;
            continue;
          } else if (res.length !== 0) {
            res = "";
            lastSegmentLength = 0;
            lastSlash = i;
            dots = 0;
            continue;
          }
        }
        if (allowAboveRoot) {
          res += res.length > 0 ? "/.." : "..";
          lastSegmentLength = 2;
        }
      } else {
        if (res.length > 0) res += `/${path.slice(lastSlash + 1, i)}`;
        else res = path.slice(lastSlash + 1, i);
        lastSegmentLength = i - lastSlash - 1;
      }
      lastSlash = i;
      dots = 0;
    } else if (code === DOT && dots !== -1) {
      ++dots;
    } else {
      dots = -1;
    }
  }
  return res;
}

function normalize(path) {
  validateString(path);
  if (path.length === 0) return ".";
  const isAbsolute = path.charCodeAt(0) === SLASH;
  const trailingSeparator = path.charCodeAt(path.length - 1) === SLASH;
  path = normalizeString(path, !isAbsolute);
  if (path.length === 0) {
    if (isAbsolute) return "/";
    return trailingSeparator ? "./" : ".";
  }
  if (trailingSeparator) path += "/";
  return isAbsolute ? `/${path}` : path;
}

function join(...args) {
  if (args.length === 0) return ".";
  const parts = [];
  for (const arg of args) {
    validateString(arg);
    if (arg.length > 0) parts.push(arg);
  }
  if (parts.length === 0) return ".";
  return normalize(parts.join("/"));
}

function dirname(path) {
  validateString(path);
  if (path.length === 0) return ".";
  const hasRoot = path.charCodeAt(0) === SLASH;
  let end = -1;
  let matchedSlash = true;
  for (let i = path.length - 1; i >= 1; --i) {
    if (path.charCodeAt(i) === SLASH) {
      if (!matchedSlash) {
        end = i;
        break;
      }
    } else {
      matchedSlash = false;
    }
  }
  if (end === -1) return hasRoot ? "/" : ".";
  if (hasRoot && end === 1) return "//";
  return path.slice(0, end);
}

const unsupported = (target, label) =>
  new Proxy(target, {
    get(t, prop) {
      if (prop in t) return t[prop];
      throw new Error(`EMBEDDED-UNSUPPORTED: ${label}.${String(prop)}`);
    },
  });

export const posix = unsupported({ normalize, join, dirname, sep: "/", delimiter: ":" }, "node:path.posix");
const path = unsupported({ normalize, join, dirname, sep: "/", delimiter: ":", posix }, "node:path");
export default path;
export { normalize, join, dirname };
