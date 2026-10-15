#!/usr/bin/env bash
# Per-crate dependency allowlist + no-UI gate (Phase 0 / docs/plan/22-acceptance-criteria.md).
set -euo pipefail
cd "$(dirname "$0")/.."

fail=0

# Extract the direct dependency names from a crate's [dependencies] table.
deps_of() {
  awk '/^\[dependencies\]/{f=1;next} /^\[/{f=0} f' "crates/$1/Cargo.toml" \
    | sed -n 's/^\([a-zA-Z0-9_-]\{1,\}\)[[:space:].].*/\1/p'
}

check() {
  local crate="$1"; shift
  local allowed=" $* "
  local dep
  while IFS= read -r dep; do
    [ -z "$dep" ] && continue
    case "$allowed" in
      *" $dep "*) ;;
      *) echo "dependency-hygiene: crate '$crate' has disallowed dependency '$dep'"; fail=1 ;;
    esac
  done < <(deps_of "$crate")
}

# Keep this allowlist in sync with docs/design/12-crate-layout.md §3.
check riga-core      serde serde_json sha2 thiserror
check riga-market    riga-core serde serde_json thiserror
check riga-engine    riga-core riga-market rand rand_chacha rand_distr serde serde_json sha2 thiserror
check riga-risk      riga-core riga-engine
check riga-scheduler serde
check riga-cli       riga-core riga-engine serde_json
check riga-mcp       axum riga-core riga-engine riga-risk rmcp schemars serde serde_json tokio

# no-ui: riga-flow ships no frontend assets.
if find crates -type f \( -name '*.html' -o -name '*.css' -o -name '*.tsx' -o -name '*.jsx' \) | grep -q .; then
  echo "no-ui: riga-flow must not ship frontend assets"
  fail=1
fi

if [ "$fail" -ne 0 ]; then
  exit 1
fi
echo "dependency-hygiene + no-ui: OK"
