// Computes the next release tag from the tags that already exist on GitHub, so
// the release launchers can auto-increment without any manual bookkeeping.
//
// Usage:
//   node scripts/next-version.mjs           -> next patch tag (e.g. v0.1.0 -> v0.1.1)
//   node scripts/next-version.mjs patch     -> next patch tag
//   node scripts/next-version.mjs minor     -> next minor tag (v0.1.3 -> v0.2.0)
//   node scripts/next-version.mjs major     -> next major tag (v0.1.3 -> v1.0.0)
//   node scripts/next-version.mjs --latest  -> highest existing tag, or "none"
//
// When no tags exist yet, the first release seeds from package.json's version.
// Requires the GitHub CLI (`gh`) to read remote tags; if that is unavailable it
// falls back to the package.json seed so the first release still works.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import path from "node:path";

const rootDir = path.resolve(import.meta.dirname, "..");
const arg = (process.argv[2] || "patch").toLowerCase();

function gh(args) {
  const result = spawnSync("gh", args, {
    cwd: rootDir,
    encoding: "utf8",
    shell: process.platform === "win32"
  });
  return result.status === 0 ? result.stdout.trim() : "";
}

function compareSemver(a, b) {
  const pa = a.slice(1).split(".").map(Number);
  const pb = b.slice(1).split(".").map(Number);
  for (let i = 0; i < 3; i += 1) {
    if (pa[i] !== pb[i]) {
      return pa[i] - pb[i];
    }
  }
  return 0;
}

function latestTag() {
  const repo = gh(["repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"]);
  if (!repo) {
    return null;
  }
  const output = gh(["api", `repos/${repo}/tags`, "--paginate", "--jq", ".[].name"]);
  const tags = output
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((tag) => /^v\d+\.\d+\.\d+$/.test(tag))
    .sort(compareSemver);
  return tags.length > 0 ? tags[tags.length - 1] : null;
}

function seedVersion() {
  try {
    const pkg = JSON.parse(readFileSync(path.join(rootDir, "package.json"), "utf8"));
    return typeof pkg.version === "string" && pkg.version.length > 0 ? pkg.version : "0.1.0";
  } catch {
    return "0.1.0";
  }
}

const latest = latestTag();

if (arg === "--latest" || arg === "current") {
  process.stdout.write(`${latest ?? "none"}\n`);
  process.exit(0);
}

let next;
if (!latest) {
  // First ever release: use the package.json version as-is.
  next = `v${seedVersion()}`;
} else {
  const [major, minor, patch] = latest.slice(1).split(".").map(Number);
  if (arg === "major") {
    next = `v${major + 1}.0.0`;
  } else if (arg === "minor") {
    next = `v${major}.${minor + 1}.0`;
  } else {
    next = `v${major}.${minor}.${patch + 1}`;
  }
}

process.stdout.write(`${next}\n`);
