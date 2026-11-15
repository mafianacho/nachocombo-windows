#!/bin/bash
# Round trip of in-app updates: scripts/mock-releases.py plays GitHub Releases, the update_cli example plays the app's
# steps (launch / Check Now / "Update and Restart" / "Go Back") on the same core UpdateStore the app uses, against fake
# exes in a temporary "Programs" folder. Nothing is ever installed without the "install" step (the user's confirm).
# Checks: a check only reports, the user's install downloads + verifies (size, SHA-256, exe version) and swaps, the old
# exe is filed as the only previous copy at the next launch, at most 2 kept copies, automatic rollback after two
# unhealthy launches, the rolled-back version not offered again, corrupt / wrong-size / wrong-version releases refused
# with nothing changed, a new exe that cannot start is undone, manual rollback, leftovers cleaned at launch.
# Output: build/update-roundtrip/*.log and summary.txt. Exit 0 only if every check passes. Needs cargo and Python 3.
set -uo pipefail
cd "$(dirname "$0")/.."
OUT=build/update-roundtrip
rm -rf "$OUT"; mkdir -p "$OUT/feed/files" "$OUT/Programs"
PORT="${UPDATE_PORT:-8794}"
FEED="http://127.0.0.1:$PORT/repos/mafianacho/nachocombo-windows/releases/latest"
cargo build --locked --example update_cli 2>&1 | tail -1
CLI="${CARGO_TARGET_DIR:-target}/debug/examples/update_cli"
python3 scripts/mock-releases.py --port "$PORT" --dir "$OUT/feed" > "$OUT/feed.log" 2>&1 &
SRV=$!
trap 'kill $SRV 2>/dev/null' EXIT
for _ in $(seq 1 50); do python3 -c "import urllib.request,sys; urllib.request.urlopen('http://127.0.0.1:$PORT/')" 2>/dev/null && break; sleep 0.2; done

TARGET="$OUT/Programs/NachoCombo.exe"
ROOT="$OUT/LocalAppData/NachoCombo/Updates"
fail=0
check() { if [ "$2" -eq 0 ]; then echo "PASS $1" | tee -a "$OUT/summary.txt"; else echo "FAIL $1" | tee -a "$OUT/summary.txt"; fail=1; fi; }
has() { grep -q -- "$1" "$OUT/$2.log"; echo $?; }
fake_exe() { printf 'MZ fake NachoCombo %s' "$1"; }

publish() { # version [corrupt|wrongsize|wrongexe|elsewhere]
  local v="$1" how="${2:-}" zip="NachoCombo-windows-$1-x64.zip" exe="$1"
  [ "$how" = wrongexe ] && exe="0.0.1"
  python3 - "$OUT/feed/files/$zip" "$(fake_exe "$exe")" <<'PY'
import sys, zipfile
with zipfile.ZipFile(sys.argv[1], "w", zipfile.ZIP_DEFLATED) as z:
    z.writestr("NachoCombo.exe", sys.argv[2] + " " * 4000)
    z.writestr("README.md", "readme")
    z.writestr("LICENSE", "MIT")
PY
  local sum size; sum=$(sha256sum "$OUT/feed/files/$zip" | cut -c1-64); size=$(stat -c %s "$OUT/feed/files/$zip")
  [ "$how" = corrupt ] && sum=$(printf '0%.0s' $(seq 1 64))
  [ "$how" = wrongsize ] && size=$((size + 1))
  local base="http://127.0.0.1:$PORT/files"
  [ "$how" = elsewhere ] && base="https://example.com/files"
  echo "$sum  $zip" > "$OUT/feed/files/SHA256SUMS.txt"
  cat > "$OUT/feed/latest.json" <<JSON
{"tag_name":"v$v","name":"NachoCombo for Windows $v","draft":false,"prerelease":false,"html_url":"https://example.invalid/v$v","body":"test",
 "assets":[{"name":"$zip","size":$size,"browser_download_url":"$base/$zip"},
           {"name":"SHA256SUMS.txt","size":1,"browser_download_url":"$base/SHA256SUMS.txt"}]}
JSON
}

cli() { local name="$1"; shift; "$CLI" "$@" --root "$ROOT" --target "$TARGET" --feed "$FEED" > "$OUT/$name.log" 2>&1; echo "rc=$?" >> "$OUT/$name.log"; }
beside() { ls -A "$OUT/Programs" | tr '\n' ' '; }
copies() { find "$ROOT" -maxdepth 2 -name NachoCombo.exe 2>/dev/null | wc -l | tr -d ' '; }

fake_exe 0.5.0 > "$TARGET"

publish 0.5.1
cli 01-check check
check "check: 0.5.1 found and only reported; nothing downloaded, running exe untouched" $(( $(has 'check: 0.5.1 available' 01-check) + $(has 'installed=0.5.0 beside=- staged=- previous=- kept=0' 01-check) ))
cli 02-install install
check "install (user confirmed): downloaded, verified, swapped; old exe renamed beside it" $(( $(has 'verified 0.5.1 (size, sha256, exe version)' 02-install) + $(has 'applied 0.5.1' 02-install) + $(has 'installed=0.5.1 beside=0.5.0 staged=-' 02-install) ))
cli 03-launch launch
check "next launch: old exe filed as the only previous copy, update notice once, healthy" $(( $(has 'notice Updated' 03-launch) + $(has 'healthy 0.5.1' 03-launch) + $(has 'installed=0.5.1 beside=- staged=- previous=0.5.0 kept=1' 03-launch) ))
[ "$(beside)" = "NachoCombo.exe " ]; check "  Programs holds only NachoCombo.exe ($(beside))" $?

publish 0.5.2
cli 04-install install
cli 05-launch launch
check "0.5.2 in, previous is now 0.5.1, 0.5.0 deleted (copies kept: $(copies))" $(( $(has 'installed=0.5.2 beside=- staged=- previous=0.5.1 kept=1' 05-launch) ))
[ -z "$(grep -rl '0.5.0' "$ROOT" 2>/dev/null)" ] && [ "$(copies)" -le 2 ]; check "  no file of 0.5.0 left, at most 2 copies" $?

# A version on trial: two launches die before healthy (05 was healthy, so start a new trial with 0.5.3 first).
publish 0.5.3
cli 06-install install
cli 07-crash1 launch --crash
cli 08-crash2 launch --crash
cli 09-recover launch
check "two launches of 0.5.3 die before healthy -> third launch rolls back to 0.5.2" $(( $(has 'started=1' 07-crash1) + $(has 'started=2' 08-crash2) + $(has 'rolled back to 0.5.2' 09-recover) ))
cli 10-after launch
check "after the automatic rollback: 0.5.2 runs, 0.5.3 deleted and skipped, notice once" $(( $(has 'notice RolledBack' 10-after) + $(has 'installed=0.5.2 beside=- staged=- previous=- kept=0 skipped=0.5.3' 10-after) ))
cli 11-skip check
check "the rolled-back 0.5.3 is not offered again" $(( $(has 'ignored (0.5.3 was rolled back on this PC)' 11-skip) ))

publish 0.5.4 corrupt
cli 12-corrupt install
check "zip does not match SHA256SUMS.txt -> refused, nothing changed" $(( $(has 'does not match SHA256SUMS.txt' 12-corrupt) + $(has 'installed=0.5.2 beside=- staged=-' 12-corrupt) ))
[ ! -e "$ROOT/download" ]; check "  no download leftovers" $?
publish 0.5.4 wrongsize
cli 13-size install
check "download size differs from the release's -> refused" $(( $(has 'bytes, the release lists' 13-size) + $(has 'installed=0.5.2 beside=- staged=-' 13-size) ))
publish 0.5.4 wrongexe
cli 14-wrongexe install
check "zip named 0.5.4 holds another version -> refused" $(( $(has 'The download is version 0.0.1, not 0.5.4' 14-wrongexe) + $(has 'installed=0.5.2' 14-wrongexe) ))
publish 0.5.4 elsewhere
cli 15-elsewhere install
check "files not on the feed's host -> refused before any download" $(( $(has 'does not come from the release page' 15-elsewhere) ))
! grep -q 'example.com' "$OUT/feed.log"; check "  nothing was fetched from elsewhere" $?

publish 0.5.4
cli 16-nostart install-start-fails
check "new exe cannot be started -> swap undone, 0.5.2 keeps running, 0.5.4 stays staged" $(( $(has 'undone: 0.5.2 kept' 16-nostart) + $(has 'installed=0.5.2 beside=- staged=0.5.4 previous=- kept=1' 16-nostart) ))
cli 17-install install
cli 18-launch launch
check "install again uses the staged copy, 0.5.4 runs, 0.5.2 is previous" $(( $(has 'applied 0.5.4' 17-install) + $(has 'installed=0.5.4 beside=- staged=- previous=0.5.2' 18-launch) ))
cli 19-rollback rollback
cli 20-launch launch
check "manual 'Go Back to 0.5.2': back to 0.5.2, 0.5.4 deleted and skipped" $(( $(has 'rollback: back to 0.5.2' 19-rollback) + $(has 'installed=0.5.2 beside=- staged=- previous=- kept=0 skipped=0.5.3,0.5.4' 20-launch) ))

mkdir -p "$ROOT/download"; echo half > "$ROOT/download/NachoCombo-windows-0.5.5-x64.zip"; echo x > "$ROOT/junk.tmp"
fake_exe 0.5.2 > "$OUT/Programs/.NachoCombo-update-old.exe"
cli 21-tidy launch
[ ! -e "$ROOT/download" ] && [ ! -e "$ROOT/junk.tmp" ] && [ ! -e "$OUT/Programs/.NachoCombo-update-old.exe" ]
check "launch removes an interrupted download, stray files and a leftover old exe" $?

echo "---- summary ----"; cat "$OUT/summary.txt"
exit $fail
