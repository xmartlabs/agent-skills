#!/usr/bin/env node

import {
  existsSync,
  lstatSync,
  readFileSync,
  realpathSync,
} from "node:fs";
import { dirname, isAbsolute, relative, resolve, sep } from "node:path";

const profileArg = process.argv[2] || ".design-space.json";
const profilePath = resolve(process.cwd(), profileArg);
const repositoryRoot = dirname(profilePath);
const errors = [];

const fail = (field, message) => errors.push(`${field}: ${message}`);
const isObject = (value) => value !== null && typeof value === "object" && !Array.isArray(value);
const nonEmptyString = (value) => typeof value === "string" && value.trim().length > 0;
const isInside = (root, candidate) => {
  const pathFromRoot = relative(root, candidate);
  return pathFromRoot === "" || (!pathFromRoot.startsWith(`..${sep}`) && pathFromRoot !== ".." && !isAbsolute(pathFromRoot));
};

function isLoopbackUrl(value) {
  if (!nonEmptyString(value)) return false;
  try {
    const parsed = new URL(value);
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return false;
    if (parsed.username || parsed.password) return false;
    const host = parsed.hostname.toLowerCase();
    if (host === "localhost" || host === "[::1]" || host === "::1") return true;
    const octets = host.split(".");
    return octets.length === 4
      && octets.every((octet) => /^\d+$/.test(octet) && Number(octet) <= 255)
      && Number(octets[0]) === 127;
  } catch {
    return false;
  }
}

function validatePath(field, value) {
  if (!nonEmptyString(value)) {
    fail(field, "must be a non-empty repository-relative path");
    return;
  }
  if (isAbsolute(value)) {
    fail(field, "must be repository-relative, not absolute");
    return;
  }

  const resolvedPath = resolve(repositoryRoot, value);
  if (!isInside(repositoryRoot, resolvedPath)) {
    fail(field, "must not escape the repository");
    return;
  }
  if (!existsSync(resolvedPath)) {
    fail(field, `configured path does not exist: ${value}`);
    return;
  }

  try {
    const realPath = realpathSync(resolvedPath);
    if (!isInside(realpathSync(repositoryRoot), realPath)) {
      fail(field, "resolves through a symlink outside the repository");
    }
  } catch (error) {
    fail(field, `cannot resolve configured path: ${error.message}`);
  }
}

let profile;
if (!existsSync(profilePath)) {
  console.error(`design-space profile not found: ${profilePath}`);
  process.exit(1);
}

try {
  if (!lstatSync(profilePath).isFile()) throw new Error("profile path is not a file");
  profile = JSON.parse(readFileSync(profilePath, "utf8"));
} catch (error) {
  console.error(`invalid design-space profile: ${error.message}`);
  process.exit(1);
}

if (!isObject(profile)) {
  fail("profile", "must contain a JSON object");
} else {
  if (profile.schemaVersion !== 1) {
    fail("schemaVersion", "must be 1");
  }

  if (!isObject(profile.start)) {
    fail("start", "must be an object");
  } else {
    if (!isLoopbackUrl(profile.start.url)) {
      fail("start.url", "must be an HTTP(S) loopback URL");
    }
    if (profile.start.command !== undefined && !nonEmptyString(profile.start.command)) {
      fail("start.command", "must be a non-empty string when provided");
    }
  }

  if (!isObject(profile.paths)) {
    fail("paths", "must be an object");
  } else {
    if (!Array.isArray(profile.paths.roots) || profile.paths.roots.length === 0) {
      fail("paths.roots", "must be a non-empty array");
    } else {
      profile.paths.roots.forEach((value, index) => validatePath(`paths.roots[${index}]`, value));
    }

    for (const [key, value] of Object.entries(profile.paths)) {
      if (key !== "roots") validatePath(`paths.${key}`, value);
    }
  }

  if (profile.layout !== "app") {
    fail("layout", 'must be "app"');
  }

  if (profile.name !== undefined && !nonEmptyString(profile.name)) {
    fail("name", "must be a non-empty string when provided");
  }

  if (profile.baseBranch !== undefined
    && !(nonEmptyString(profile.baseBranch) && /^[A-Za-z0-9._\/-]+$/.test(profile.baseBranch)
      && !profile.baseBranch.startsWith("-"))) {
    fail("baseBranch", "must be a branch name (letters, digits, '.', '_', '/', '-') when provided");
  }

  if (profile.feedback !== undefined) {
    if (!isObject(profile.feedback)) {
      fail("feedback", "must be an object when provided");
    } else {
      if (profile.feedback.provider !== undefined && !nonEmptyString(profile.feedback.provider)) {
        fail("feedback.provider", "must be a non-empty string when provided");
      }
      if (profile.feedback.endpoint !== undefined && !isLoopbackUrl(profile.feedback.endpoint)) {
        fail("feedback.endpoint", "must be an HTTP(S) loopback URL when provided");
      }
    }
  }
}

if (errors.length > 0) {
  console.error(`invalid design-space profile (${errors.length} error${errors.length === 1 ? "" : "s"}):`);
  for (const error of errors) console.error(`- ${error}`);
  process.exit(1);
}

console.log(`design-space profile valid: ${profilePath}`);
