#!/bin/bash
# Checks that every REST route the pet uses (PET_SERVER_ROUTES in src/core/pet/server_api.rs) is still in nacho-server's
# spec. usage: scripts/check-server-contract.sh <nacho-server repo> [git ref, default origin/main]
# Read only: the server repo is read with `git show`. Exit 1 lists the routes that are gone.
set -euo pipefail
cd "$(dirname "$0")/.."
repo="${1:?nacho-server repo path}"
ref="${2:-origin/main}"
spec=$(python3 scripts/server-contract-routes.py "$repo" "$ref")
routes=$(sed -n '/^pub const PET_SERVER_ROUTES/,/^];/s/^    "\([A-Z]* \/[^"]*\)",$/\1/p' src/core/pet/server_api.rs)
[ -n "$routes" ] || { echo "no routes found in src/core/pet/server_api.rs" >&2; exit 2; }
missing=0
while IFS= read -r route; do
  if ! grep -qxF "$route" <<<"$spec"; then echo "MISSING in $ref: $route"; missing=1; fi
done <<<"$routes"
echo "spec $ref = $(git -C "$repo" rev-parse --short "$ref"), $(wc -l <<<"$spec") routes; pet uses $(wc -l <<<"$routes")"
[ "$missing" = 0 ] && echo "OK: every pet route is in the spec"
exit "$missing"
