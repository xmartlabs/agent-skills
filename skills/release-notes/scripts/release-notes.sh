#!/usr/bin/env bash
# release-notes.sh [--config CFG] [--publish <tag>] [--create] [RANGE]
#
# Aggregates the merged PRs in a git range into release notes that embed the visual evidence those
# PRs already produced (found in each PR's kit-posted evidence comment), grouped by change type.
# Authoritative behavior: flow/release-notes-flow.md. v1 REUSES hosted evidence — it never boots,
# captures, or (re)generates anything. Best-effort: a PR that can't be read is skipped, not fatal.
#
#   RANGE       tagA..tagB (default <lastTag>..HEAD)
#   --publish   write the rendered notes into that GitHub Release's body (gh release edit)
#   --create    allow creating the release if <tag> has none yet (else publish to existing only)
#   --append    append to the existing release body instead of replacing it (preserves hand-written
#               notes); requires --publish
#   (no flag)   print markdown to stdout — nothing is mutated
set -u
DIR="$(cd "$(dirname "$0")/.." && pwd)"
RENDER="$DIR/scripts/lib/render-notes.py"

fail() { echo "release-notes: $1" >&2; exit 1; }
warn() { echo "release-notes: $1" >&2; }

CFG=".pr-evidence/config.json"; PUBLISH=""; CREATE=""; APPEND=""; RANGE=""
while [ $# -gt 0 ]; do
  case "$1" in
    --config)  CFG="$2"; shift 2 ;;
    --publish) PUBLISH="$2"; shift 2 ;;
    --create)  CREATE=1; shift ;;
    --append)  APPEND=1; shift ;;
    --) shift; break ;;
    --*) fail "unknown flag '$1'" ;;
    *) RANGE="$1"; shift ;;
  esac
done
[ -n "$APPEND" ] && [ -z "$PUBLISH" ] && fail "--append requires --publish <tag>"

command -v gh >/dev/null 2>&1 || fail "gh not found on PATH"
command -v python3 >/dev/null 2>&1 || fail "python3 not found on PATH"
gh auth status >/dev/null 2>&1 || fail "gh is not authenticated"

# Optional Jira/issue URL template from config (kept generic — no client URL baked into the kit).
TEMPLATE=""
if [ -f "$CFG" ]; then
  TEMPLATE="$(python3 - "$CFG" <<'PY' 2>/dev/null || true
import json,sys
try: d=json.load(open(sys.argv[1]))
except Exception: sys.exit(0)
print(((d.get("tracker") or {}).get("issueUrlTemplate")) or "")
PY
)"
fi

# Resolve range.
if [ -z "$RANGE" ]; then
  last="$(git tag --sort=-creatordate 2>/dev/null | head -1)"
  [ -n "$last" ] || fail "no git tags found and no RANGE given — nothing to aggregate"
  RANGE="$last..HEAD"
fi

# Collect PR numbers from first-parent history: merge-commit form and squash '(#N)' form. Dedupe,
# preserve first-seen order.
subjects="$(git log "$RANGE" --first-parent --format=%s 2>/dev/null)" \
  || fail "could not read git range '$RANGE'"
PRS="$(printf '%s\n' "$subjects" \
  | grep -oE '(Merge pull request #[0-9]+|\(#[0-9]+\))' \
  | grep -oE '[0-9]+' \
  | awk '!seen[$0]++' \
  | tr '\n' ' ')"

# Gather each PR (best-effort) and build the intermediate JSON document the renderer consumes.
DOC="$(RANGE="$RANGE" TEMPLATE="$TEMPLATE" python3 - "$PRS" <<'PY'
import json, os, re, subprocess, sys

nums = [n for n in sys.argv[1].split() if n]
template = os.environ.get("TEMPLATE", "")
rng = os.environ.get("RANGE", "")

def gh_json(args):
    r = subprocess.run(["gh"] + args, capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError(r.stderr.strip() or "gh failed")
    return json.loads(r.stdout or "null")

def classify(pr):
    labels = [(l.get("name") or "").lower() for l in (pr.get("labels") or [])]
    def has(*opts): return any(o in labels for o in opts)
    if has("feature", "feat", "enhancement", "story"): return "feature"
    if has("fix", "bug", "bugfix"): return "fix"
    if has("chore", "docs", "refactor", "ci", "test", "tests", "dependencies"): return "chore"
    m = re.search(r"-\s*\[[xX]\]\s*(Fix|Story|Chore|Feature)", pr.get("body") or "")
    if m:
        return {"fix": "fix", "story": "feature", "feature": "feature", "chore": "chore"}[m.group(1).lower()]
    return "other"

def ticket(pr):
    m = re.search(r"\b([A-Z][A-Z0-9]+-\d+)\b", pr.get("title") or "")
    if not m:
        return None
    key = m.group(1)
    t = {"key": key}
    if template:
        t["url"] = template.replace("{KEY}", key).replace("{key}", key)
    return t

def evidence(pr):
    ev = None
    for c in (pr.get("comments") or []):        # chronological — keep the most recent match
        b = c.get("body") or ""
        if "PR evidence" in b or "pr-evidence" in b:
            ev = c
    if not ev:
        return None
    b = ev.get("body") or ""
    images = re.findall(r"!\[[^\]]*\]\(([^)]+)\)", b)
    links = re.findall(r"(?<!\!)\[[^\]]*\]\(([^)]+)\)", b)
    bare = re.findall(r"(?<![\(\w])(https://github\.com/user-attachments/assets/[^\s)]+)", b)
    for u in bare:
        if u not in links and u not in images:
            links.append(u)
    return {"commentUrl": ev.get("url") or pr.get("url"), "images": images, "links": links}

prs = []
for n in nums:
    try:
        pr = gh_json(["pr", "view", n, "--json", "number,title,url,labels,body,comments"])
    except Exception as e:
        sys.stderr.write(f"release-notes: skipping PR #{n}: {e}\n")
        continue
    prs.append({
        "number": pr.get("number"),
        "title": pr.get("title") or "",
        "url": pr.get("url") or "",
        "type": classify(pr),
        "ticket": ticket(pr),
        "evidence": evidence(pr),
    })

print(json.dumps({"range": rng, "prs": prs}))
PY
)" || fail "failed to gather PR data"

RENDERED="$(printf '%s' "$DOC" | python3 "$RENDER")" || fail "failed to render notes"

if [ -z "$PUBLISH" ]; then
  printf '%s\n' "$RENDERED"
  exit 0
fi

# Publish path: write into the GitHub Release body.
TMP="$(mktemp)"; trap 'rm -f "$TMP"' EXIT
if gh release view "$PUBLISH" >/dev/null 2>&1; then
  if [ -n "$APPEND" ]; then
    # Preserve the existing (often hand-written) body; append the generated notes below a rule.
    existing="$(gh release view "$PUBLISH" --json body --jq '.body' 2>/dev/null || true)"
    { printf '%s\n\n---\n\n' "$existing"; printf '%s\n' "$RENDERED"; } > "$TMP"
  else
    printf '%s\n' "$RENDERED" > "$TMP"
  fi
  verb="updated"; [ -n "$APPEND" ] && verb="appended to"
  gh release edit "$PUBLISH" --notes-file "$TMP" >/dev/null \
    && echo "release-notes: $verb release $PUBLISH" >&2
elif [ -n "$CREATE" ]; then
  # Nothing to append to on a brand-new release — just seed it with the generated notes.
  printf '%s\n' "$RENDERED" > "$TMP"
  gh release create "$PUBLISH" --title "$PUBLISH" --notes-file "$TMP" >/dev/null \
    && echo "release-notes: created release $PUBLISH" >&2
else
  warn "release '$PUBLISH' does not exist (pass --create to make it); printing to stdout instead"
  printf '%s\n' "$RENDERED"
fi
