#!/bin/bash
# NachoCombo MCP client round trip against `examples/nacho-mcp-harness.rs` (the app's core::mcp server with stand-in
# gadgets, 127.0.0.1 only): initialize → tools/list → tools/call (read) → tools/call that waits for the approval card →
# allowed / declined → result, plus wrong key, foreign Host, a non-loopback address and the port closing on stop.
# Output: target/mcp-roundtrip/*.log and summary.txt. Exit 0 only if every check passes. Needs Rust and curl.
# CARGO may point at a wrapper (e.g. a container with a C linker).
set -uo pipefail
cd "$(dirname "$0")/.."
OUT=target/mcp-roundtrip
rm -rf "$OUT"; mkdir -p "$OUT"
PORT="${MCP_PORT:-47693}"
URL="http://127.0.0.1:$PORT/mcp"
CARGO="${CARGO:-cargo}"
$CARGO build --locked --example nacho-mcp-harness 2>&1 | tail -1
BIN="target/debug/examples/nacho-mcp-harness"
fail=0
check() { if [ "$2" -eq 0 ]; then echo "PASS $1" | tee -a "$OUT/summary.txt"; else echo "FAIL $1" | tee -a "$OUT/summary.txt"; fail=1; fi; }

start() { # name, approve
  mkfifo "$OUT/$1.in"
  "$BIN" --dir "$OUT/$1" --port "$PORT" --gadgets clipboard,dim,appswitch --approve "$2" < "$OUT/$1.in" > "$OUT/$1-server.log" 2>&1 &
  exec 9> "$OUT/$1.in"
  for _ in $(seq 1 50); do grep -q '^listening' "$OUT/$1-server.log" 2>/dev/null && break; sleep 0.1; done
  KEY=$(cat "$OUT/$1/key.txt")
}
stop() { exec 9>&-; wait; }
rpc() { # name, json body, [extra curl args…]
  local name="$1" body="$2"; shift 2
  { echo "> POST /mcp $body"; curl -s -i -X POST "$URL" -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" \
      -H "Authorization: Bearer $KEY" --data "$body" "$@" | tr -d '\r'; echo; } > "$OUT/$name.log"
}
has() { grep -q -- "$1" "$OUT/$2.log"; echo $?; }

start allow allow
rpc 01-initialize '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"roundtrip","version":"1"}}}'
check "initialize: NachoCombo MCP, protocol 2025-06-18" $(( $(has '"name":"NachoCombo MCP"' 01-initialize) + $(has '"protocolVersion":"2025-06-18"' 01-initialize) ))
rpc 02-initialized '{"jsonrpc":"2.0","method":"notifications/initialized"}'
check "notifications/initialized: 202, no body" "$(has 'HTTP/1.1 202' 02-initialized)"
rpc 03-tools-list '{"jsonrpc":"2.0","id":2,"method":"tools/list"}'
n=$(grep -o '"name":"[a-z_]*"' "$OUT/03-tools-list.log" | sort -u | tr '\n' ' ')
echo "tools: $n" >> "$OUT/03-tools-list.log"
check "tools/list: only the switched-on gadgets (clipboard, dim, appswitch), no deleting tool" $(( $(has '"name":"clipboard_get_item"' 03-tools-list) + $(has '"name":"dim_status"' 03-tools-list) + $(has '"name":"appswitch_quit"' 03-tools-list) + ( $(has '"name":"awake_status"' 03-tools-list) == 0 ) + ( $(has '"name":"shelf_clear"' 03-tools-list) == 0 ) ))
rpc 04-call-list '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"clipboard_list","arguments":{}}}'
check "tools/call clipboard_list (read): runs at once, no content, no card" $(( $(has '"isError":false' 04-call-list) + ( $(has 'meeting at' 04-call-list) == 0 ) + ( $(grep -c '^CARD' "$OUT/allow-server.log") != 0 ) ))
rpc 05-call-get-item '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"clipboard_get_item","arguments":{"index":1}}}'
check "tools/call clipboard_get_item (content_out): card → allowed → result" $(( $(has '"isError":false' 05-call-get-item) + $(has 'meeting at 3pm' 05-call-get-item) + ( $(grep -c '^CARD mcp-1' "$OUT/allow-server.log") != 1 ) + ( $(grep -c '^ANSWER mcp-1 Allowed' "$OUT/allow-server.log") != 1 ) ))
rpc 06-call-quit '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"appswitch_quit","arguments":{"app":"Notepad"}}}'
check "tools/call appswitch_quit (destructive): card every time → allowed" $(( $(has '"isError":false' 06-call-quit) + ( $(grep -c '^CARD mcp-2' "$OUT/allow-server.log") != 1 ) ))
rpc 07-bad-params '{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"dim_set_strength","arguments":{"percent":40,"extra":1}}}'
check "closed parameter list: unknown key refused" "$(has 'bad parameters' 07-bad-params)"
rpc 08-off-gadget '{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"awake_status","arguments":{}}}'
check "gadget switched off for MCP: refused (mcp_gadget_off)" "$(has 'mcp_gadget_off' 08-off-gadget)"
KEY=nccu_wrong rpc 09-wrong-key '{"jsonrpc":"2.0","id":8,"method":"ping"}'
check "wrong key: 401 + WWW-Authenticate: Bearer" $(( $(has 'HTTP/1.1 401' 09-wrong-key) + $(has 'WWW-Authenticate: Bearer' 09-wrong-key) ))
rpc 10-foreign-host '{"jsonrpc":"2.0","id":9,"method":"ping"}' -H "Host: evil.example"
check "foreign Host (DNS rebinding): 403" "$(has 'HTTP/1.1 403' 10-foreign-host)"
LAN_IP=$(hostname -I 2>/dev/null | awk '{print $1}')
if [ -n "$LAN_IP" ]; then
  curl -s -m 3 -o /dev/null -w '%{http_code}' "http://$LAN_IP:$PORT/mcp" > "$OUT/11-non-loopback.log" 2>&1; rc=$?
  echo " curl exit $rc (connecting to $LAN_IP:$PORT)" >> "$OUT/11-non-loopback.log"
  check "non-loopback address of this machine ($LAN_IP): connection refused" $(( rc == 0 ))
fi
command -v ss >/dev/null && ss -ltn "sport = :$PORT" > "$OUT/12-listen.log"
stop
curl -s -m 3 -o /dev/null "$URL"; rc=$?
echo "after stop: curl exit $rc" > "$OUT/13-closed.log"
check "stop: the port is closed ($(grep -c '^closed' "$OUT/allow-server.log") closed line)" $(( rc == 0 ))

start deny deny
rpc 20-call-denied '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"clipboard_get_item","arguments":{"index":1}}}'
check "card declined: approval_denied, nothing ran" $(( $(has 'approval_denied' 20-call-denied) + ( $(has 'meeting at' 20-call-denied) == 0 ) + ( $(grep -c '^RUN' "$OUT/deny-server.log") != 0 ) ))
stop

start none none
rpc 30-call-unanswered '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"clipboard_get_item","arguments":{"index":1}}}'
check "card unanswered: approval_timeout, card taken away, nothing ran" $(( $(has 'approval_timeout' 30-call-unanswered) + ( $(grep -c '^CANCEL mcp-1' "$OUT/none-server.log") != 1 ) + ( $(grep -c '^RUN' "$OUT/none-server.log") != 0 ) ))
stop

rm -f "$OUT"/*/key.txt
echo "---"; cat "$OUT/summary.txt" | grep -c PASS | xargs echo "passed:"
exit $fail
