#!/usr/bin/env bash
# Tests the profile validator with isolated, neutral fixtures. No network or configured command runs.
DIR="$(cd "$(dirname "$0")/.." && pwd)"
. "$DIR/tests/helper.sh"
VALIDATOR="$DIR/skills/design-space/scripts/validate-profile.mjs"
echo "design_space_test:"

TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
REPO="$TMP/repository"
mkdir -p "$REPO/design-space/flows" "$REPO/src/styles"
touch "$REPO/src/styles/tokens.css"

# profile <file> <body lines>: writes a profile with the given fields after schemaVersion.
profile() {
  local file="$1"; shift
  printf '{\n  "schemaVersion": 1,\n%s\n}\n' "$*" > "$REPO/$file"
}
valid() { node "$VALIDATOR" "$REPO/$1" >/dev/null 2>&1; }

profile full.json '  "layout": "app",
  "start": { "command": "touch should-not-exist", "url": "http://127.0.0.1:4100/" },
  "paths": { "roots": ["design-space"], "tokenSource": "src/styles/tokens.css" },
  "feedback": { "provider": "agentation", "endpoint": "http://localhost:4747" }'
cp "$REPO/full.json" "$REPO/.design-space.json"
OUT="$(cd "$REPO" && node "$VALIDATOR" 2>&1)"; RC=$?
assert_eq "valid full profile exits 0" "0" "$RC"
assert_ok "valid profile reports success" grep -qF "profile valid" <<<"$OUT"
assert_fail "validator never runs start command" test -e "$REPO/should-not-exist"

profile minimal.json '  "layout": "app",
  "start": { "url": "http://[::1]:4100/" },
  "paths": { "roots": ["design-space"] }'
assert_ok "optional fields may be omitted" valid minimal.json

profile name-ok.json '  "layout": "app",
  "name": "Acme",
  "start": { "url": "http://127.0.0.1:4100/" },
  "paths": { "roots": ["design-space"] }'
assert_ok "name accepts a non-empty string" valid name-ok.json
profile name-empty.json '  "layout": "app",
  "name": "",
  "start": { "url": "http://127.0.0.1:4100/" },
  "paths": { "roots": ["design-space"] }'
assert_fail "name rejects an empty string" valid name-empty.json

profile base-branch.json '  "layout": "app",
  "baseBranch": "development",
  "start": { "url": "http://127.0.0.1:4100/" },
  "paths": { "roots": ["design-space"] }'
assert_ok "baseBranch accepts a branch name" valid base-branch.json

profile bad-base-branch.json '  "layout": "app",
  "baseBranch": "-x; rm",
  "start": { "url": "http://127.0.0.1:4100/" },
  "paths": { "roots": ["design-space"] }'
assert_fail "baseBranch rejects an unsafe value" valid bad-base-branch.json

profile no-layout.json '  "start": { "url": "http://127.0.0.1:4100/" },
  "paths": { "roots": ["design-space"] }'
assert_fail "layout is required" valid no-layout.json

profile other-layout.json '  "layout": "other",
  "start": { "url": "http://127.0.0.1:4100/" },
  "paths": { "roots": ["design-space"] }'
assert_fail "layout other than app rejected" valid other-layout.json

profile external-url.json '  "layout": "app",
  "start": { "url": "https://example.test/" },
  "paths": { "roots": ["design-space"] }'
assert_fail "external start URL rejected" valid external-url.json

profile external-feedback.json '  "layout": "app",
  "start": { "url": "http://127.0.0.1:4100/" },
  "paths": { "roots": ["design-space"] },
  "feedback": { "endpoint": "https://example.test:4747" }'
assert_fail "external feedback endpoint rejected" valid external-feedback.json

printf '{ "schemaVersion": 2, "layout": "app", "start": { "url": "http://127.0.0.1:4100/" }, "paths": { "roots": ["design-space"] } }\n' \
  > "$REPO/unsupported.json"
assert_fail "unsupported schema rejected" valid unsupported.json

profile missing-path.json '  "layout": "app",
  "start": { "url": "http://127.0.0.1:4100/" },
  "paths": { "roots": ["design-space"], "tokenSource": "src/styles/missing.css" }'
assert_fail "missing configured path rejected" valid missing-path.json

mkdir -p "$REPO/src/components/ui"
profile components.json '  "layout": "app",
  "start": { "url": "http://127.0.0.1:4100/" },
  "paths": { "roots": ["design-space"], "components": "src/components/ui" }'
assert_ok "paths.components accepts an existing folder" valid components.json
profile components-missing.json '  "layout": "app",
  "start": { "url": "http://127.0.0.1:4100/" },
  "paths": { "roots": ["design-space"], "components": "src/components/missing" }'
assert_fail "paths.components rejects a missing folder" valid components-missing.json
assert_ok "paths.components error names the field" grep -qF 'paths.components: configured path does not exist' \
  <(node "$VALIDATOR" "$REPO/components-missing.json" 2>&1)

mkdir -p "$TMP/outside"
profile traversal.json '  "layout": "app",
  "start": { "url": "http://127.0.0.1:4100/" },
  "paths": { "roots": ["../outside"] }'
assert_fail "repository traversal rejected" valid traversal.json

profile absolute.json "  \"layout\": \"app\",
  \"start\": { \"url\": \"http://127.0.0.1:4100/\" },
  \"paths\": { \"roots\": [\"$REPO/design-space\"] }"
assert_fail "absolute configured path rejected" valid absolute.json

ln -s "$TMP/outside" "$REPO/design-space/external-link"
profile symlink.json '  "layout": "app",
  "start": { "url": "http://127.0.0.1:4100/" },
  "paths": { "roots": ["design-space/external-link"] }'
assert_fail "symlink escape rejected" valid symlink.json

printf '{not json}\n' > "$REPO/malformed.json"
assert_fail "malformed JSON rejected" valid malformed.json

finish_tests
