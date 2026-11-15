#!/bin/bash
# Real round trip between tools/mock-server (mock nacho-server + stand-in login, test only) and the dev CLI
# `examples/nacho-pet-cli.rs` (the same PetSession the app uses): PKCE login → /api/v1/me, /ws/chat streaming and stop,
# capture → pet (picture upload), shelf → pet (file upload), refused file type, connector commands (window, gadget,
# list) through /ws/connector, server features (config/usage, history, delta recovery, tool progress, task + choice
# card, plan task start / cancel / changed estimate, memory forget/undo/feedback, dictation, read mark), token refresh
# after a restart, logout; the W5a gadget tools (window grid + undo, monitor / disk with approval, desktop tidy plan ->
# approved apply -> approved undo on a real folder, guide, mouse). Output: target/pet-roundtrip/*.log, server*.jsonl, summary.txt. Exit 0 only if every check
# passes. Needs Rust and Node >= 18. CARGO may point at a wrapper (e.g. a container with a C linker).
set -uo pipefail
cd "$(dirname "$0")/.."
OUT=target/pet-roundtrip
rm -rf "$OUT"; mkdir -p "$OUT"
PORT="${PET_PORT:-8791}"
PORT2="${PET_PORT2:-8792}"
S1="http://127.0.0.1:$PORT"
S2="http://127.0.0.1:$PORT2"
CARGO="${CARGO:-cargo}"
$CARGO build --locked --example nacho-pet-cli 2>&1 | tail -2
CLI="target/debug/examples/nacho-pet-cli"
[ -x "$CLI" ] || CLI="$CLI.exe"
node tools/mock-server/server.mjs --port "$PORT" --log "$OUT/server.jsonl" --delay-ms 15 &
SRV1=$!
node tools/mock-server/server.mjs --port "$PORT2" --log "$OUT/server-refresh.jsonl" --delay-ms 15 --access-ttl 62 &
SRV2=$!
trap 'kill $SRV1 $SRV2 2>/dev/null' EXIT
for _ in $(seq 1 50); do curl -s "$S1/health" >/dev/null 2>&1 && curl -s "$S2/health" >/dev/null 2>&1 && break; sleep 0.2; done
STORE="$OUT/store"
fail=0
check() { # name, 0 = pass
  if [ "$2" -eq 0 ]; then echo "PASS $1" | tee -a "$OUT/summary.txt"; else echo "FAIL $1" | tee -a "$OUT/summary.txt"; fail=1; fi
}
cnt() { grep -c -- "$1" "$2" 2>/dev/null || true; }
run() { local name="$1"; shift; "$CLI" "$@" > "$OUT/$name.log" 2>&1; echo $?; }

rc=$(run login login --server "$S1" --store "$STORE")
check "login: PKCE S256 + state, tokens stored, GET /api/v1/me 200" $(( rc != 0 || $(cnt 'GET /api/v1/me -> 200' "$OUT/login.log") != 1 || $(cnt '"pkce":"S256 verified"' "$OUT/server.jsonl") != 1 ))
if [ "$(uname -s)" = Linux ]; then
  perm_dir=$(stat -c %a "$STORE"); perm_files=$(find "$STORE" -type f -printf '%m\n' | sort -u | tr '\n' ' ')
  [ "$perm_dir" = 700 ] && [ "$perm_files" = "600 " ]; check "dev secret store is 0700 with 0600 files ($perm_dir / $perm_files)" $?
fi

rc=$(run restart restart --server "$S1" --store "$STORE")
check "restart: a new session over the same store is signed in, no login page" $(( rc != 0 || $(cnt 'auth signedIn (restored)' "$OUT/restart.log") != 1 || $(cnt 'browser GET' "$OUT/restart.log") != 0 ))

rc=$(run chat chat --server "$S1" --store "$STORE" "안녕 나초")
check "chat: Bearer token on /ws/chat, streamed deltas, done" $(( rc != 0 || $(cnt ' delta ' "$OUT/chat.log") < 2 || $(cnt '\] done text=' "$OUT/chat.log") != 1 ))

rc=$(run stop stop --server "$S1" --store "$STORE" "/long")
check "stop: partial text kept, turn stopped" $(( rc != 0 || $(cnt 'stopped partial=' "$OUT/stop.log") != 1 ))

rc=$(run queued queued --server "$S1" --store "$STORE" "/long" "답 끝나면 보내 줘")
check "send while answering: text waits, goes out by itself when the answer ends" $(( rc != 0 || $(cnt 'queued text=' "$OUT/queued.log") != 1 || $(cnt 'auto-sent' "$OUT/queued.log") != 1 ))

rc=$(run ask-image ask-image --server "$S1" --store "$STORE" assets/AppIcon.png "이 그림 뭐야?")
check "capture -> pet: picture uploaded, attachment id in the message, server read 1024x1024" $(( rc != 0 || $(cnt 'uploading 1 file' "$OUT/ask-image.log") < 1 || $(cnt '1024×1024' "$OUT/ask-image.log") < 1 || $(cnt '"attachments":\[{"attachment_id"' "$OUT/server.jsonl") < 1 ))

printf '# 회의 메모\n- 금요일 배포\n' > "$OUT/notes.md"
rc=$(run send-file send-file --server "$S1" --store "$STORE" "$OUT/notes.md" "요약해 줘")
check "shelf -> pet: text file uploaded and read" $(( rc != 0 || $(cnt '글: \\"# 회의 메모' "$OUT/send-file.log") < 1 ))

printf 'echo hi\n' > "$OUT/run.bat"
rc=$(run send-refused send-file --server "$S1" --store "$STORE" "$OUT/run.bat" "실행해 줘")
check "shelf -> pet: unsupported file type refused before upload" $(( rc == 0 || $(cnt '이 파일 형식은 보낼 수 없어요' "$OUT/send-refused.log") != 1 ))

rc=$(run cmd-window command --server "$S1" --store "$STORE" "지금 창 왼쪽으로 붙여 줘")
check "chat -> window arrangement: connector binding, hello, request/response, reply" $(( rc != 0 || $(cnt 'pc: arrange window left' "$OUT/cmd-window.log") != 1 || $(cnt 'combo_arrange_window 결과' "$OUT/cmd-window.log") < 1 || $(cnt '"event":"binding_issued","kind":"nacho_combo"' "$OUT/server.jsonl") != 1 ))

rc=$(run cmd-gadget command --server "$S1" --store "$STORE" "킵 어웨이크 켜 줘")
check "chat -> gadget on (binding token reused, no second binding)" $(( rc != 0 || $(cnt 'pc: gadget keepAwake -> on' "$OUT/cmd-gadget.log") != 1 || $(cnt '"event":"binding_issued"' "$OUT/server.jsonl") != 1 ))

rc=$(run cmd-off command --server "$S1" --store "$STORE" "포커스 딤 꺼 줘")
check "chat -> gadget off" $(( rc != 0 || $(cnt 'pc: gadget focusDim -> off' "$OUT/cmd-off.log") != 1 ))

rc=$(run cmd-list command --server "$S1" --store "$STORE" "가젯 목록 보여줘")
check "chat -> gadget list" $(( rc != 0 || $(cnt 'pc: list gadgets' "$OUT/cmd-list.log") != 1 ))

rc=$(run cmd-cu command --server "$S1" --store "$STORE" "컴퓨터 사용 켜 줘")
check "chat -> Computer Use refused by the app (never reaches the PC)" $(( rc != 0 || $(cnt 'pc: gadget computerUse' "$OUT/cmd-cu.log") != 0 || $(cnt 'not allowed: Computer Use' "$OUT/server.jsonl") < 1 ))

# Desktop actions (W3): the Windows declaration, approval cards, "always allow" per gadget.
TRUST="$OUT/trust/gadget-trust.json"
rc=$(run cmd-approve command --server "$S1" --store "$STORE" --trust "$TRUST" "클립보드 1번 읽어 줘")
check "content_out: approval card first, approved -> PC reads clipboard item 1 -> result" $(( rc != 0 || $(cnt 'approval required: .*combo_clipboard_get_item' "$OUT/cmd-approve.log") != 1 || $(cnt 'card approval "PC에서 클립보드 항목 읽기 실행" -> approve' "$OUT/cmd-approve.log") != 1 || $(cnt 'pc: clipboard read item 1 (17 characters)' "$OUT/cmd-approve.log") != 1 || $(cnt '"event":"approval_card","capability":"combo_clipboard_get_item"' "$OUT/server.jsonl") != 1 || $(cnt 'combo_clipboard_get_item 결과: Clipboard item 1.' "$OUT/cmd-approve.log") < 1 ))
rc=$(run cmd-deny command --server "$S1" --store "$STORE" --trust "$TRUST" --deny "클립보드 1번 읽어 줘")
check "content_out: denied card -> the PC is never asked" $(( rc != 0 || $(cnt 'card approval .* -> deny' "$OUT/cmd-deny.log") != 1 || $(cnt 'pc: clipboard read' "$OUT/cmd-deny.log") != 0 || $(cnt '"answer":"deny"' "$OUT/server.jsonl") != 1 || $(cnt 'user_denied' "$OUT/cmd-deny.log") < 1 ))
rc=$(run trust-on trust --trust "$TRUST" clipboard on)
check "always allow: switched on for Clipboard History, stored in the macOS shape" $(( rc != 0 || $(cnt '"alwaysAllowed":\["clipboard"\],"version":1' "$TRUST") != 1 ))
rc=$(run trust-bad trust --trust "$TRUST" awake on)
check "always allow: a gadget without a content_out action has no switch" $(( rc == 0 || $(cnt '"alwaysAllowed":\["clipboard"\]' "$TRUST") != 1 ))
rc=$(run cmd-trusted command --server "$S1" --store "$STORE" --trust "$TRUST" "클립보드 1번 읽어 줘")
check "always allow (read back in a new run): declared without approval, no card, PC reads" $(( rc != 0 || $(cnt 'approval required: .*combo_clipboard_get_item' "$OUT/cmd-trusted.log") != 0 || $(cnt 'card approval' "$OUT/cmd-trusted.log") != 0 || $(cnt 'pc: clipboard read item 1' "$OUT/cmd-trusted.log") != 1 || $(cnt '"event":"approval_card","capability":"combo_clipboard_get_item"' "$OUT/server.jsonl") != 2 ))
rc=$(run cmd-quit command --server "$S1" --store "$STORE" --trust "$TRUST" "메모장 종료해 줘")
check "destructive (quit app): card even with always allow on" $(( rc != 0 || $(cnt 'card approval "PC에서 앱 종료 실행" -> approve' "$OUT/cmd-quit.log") != 1 || $(cnt 'pc: quit 메모장' "$OUT/cmd-quit.log") != 1 ))
rc=$(run trust-off trust --trust "$TRUST" clipboard off)
rc2=$(run cmd-untrusted command --server "$S1" --store "$STORE" --trust "$TRUST" "클립보드 1번 읽어 줘")
check "always allow switched off and stored: the card is back" $(( rc != 0 || rc2 != 0 || $(cnt '"alwaysAllowed":\[\]' "$TRUST") != 1 || $(cnt 'card approval "PC에서 클립보드 항목 읽기 실행" -> approve' "$OUT/cmd-untrusted.log") != 1 ))
rc=$(run cmd-awake command --server "$S1" --store "$STORE" --trust "$TRUST" "30분 동안 깨워 줘")
check "change (Keep Awake for 30 min): runs at once, no card" $(( rc != 0 || $(cnt 'pc: keep awake 30 min' "$OUT/cmd-awake.log") != 1 || $(cnt 'card approval' "$OUT/cmd-awake.log") != 0 ))
rc=$(run cmd-clear command --server "$S1" --store "$STORE" --trust "$TRUST" "선반 비워 줘")
check "deleting action (empty the shelf) is not declared on Windows: never reaches the PC" $(( rc != 0 || $(cnt 'pc: ClearShelf' "$OUT/cmd-clear.log") != 0 || $(cnt 'connector_unavailable' "$OUT/cmd-clear.log") < 1 ))
leak=$(cat "$OUT"/*.log "$OUT"/server*.jsonl 2>/dev/null | grep -c '4321' || true)
check "clipboard content never lands in a log ($leak)" $(( leak ))

rc=$(run usage usage --server "$S1" --store "$STORE")
check "client config (no token) + GET /api/v1/me/usage shown" $(( rc != 0 || $(cnt 'config dictation=true max_sec=60' "$OUT/usage.log") != 1 || $(cnt 'usage 크레딧 180/200 · 가벼운 대화 3/50' "$OUT/usage.log") != 1 ))

rc=$(run history history --server "$S1" --store "$STORE")
check "history: earlier messages of the room (view=app, seeded note shown, X-Has-More)" $(( rc != 0 || $(cnt 'history seq=1 role=user type=text text="어제 파스타집 알아봐 줬지?"' "$OUT/history.log") != 1 || $(cnt 'type=system_note' "$OUT/history.log") < 1 || $(cnt '"event":"history","before":null' "$OUT/server.jsonl") < 1 ))

rc=$(run recover recover --server "$S1" --store "$STORE")
check "dropped socket: turn interrupted, reconnect, GET delta fills in the answer the server finished" $(( rc != 0 || $(cnt 'interrupted (socket dropped)' "$OUT/recover.log") != 1 || $(cnt 'recovered from delta text="모의 나초예요' "$OUT/recover.log") != 1 || $(cnt '"event":"delta"' "$OUT/server.jsonl") < 1 ))

rc=$(run search search --server "$S1" --store "$STORE")
check "tool progress: tool_call_started/finished + interim line shown as progress, not as the answer" $(( rc != 0 || $(cnt 'tools search_web:ok' "$OUT/search.log") != 1 || $(cnt 'progress \["웹에서 찾아보고 있어요"\]' "$OUT/search.log") != 1 ))

rc=$(run task task --server "$S1" --store "$STORE")
check "task path: done{task_id}, hub task_update/interim, choice card answered (expected_version), task_result joins the turn" $(( rc != 0 || $(cnt 'card choice "어느 시간이 좋아요?"' "$OUT/task.log") != 1 || $(cnt 'task done answer="저녁 7시으로 예약했어요."' "$OUT/task.log") != 1 || $(cnt '"event":"answered","id":"[^"]*","action":"choose"' "$OUT/server.jsonl") != 1 ))

rc=$(run plan-start plan-start --server "$S1" --store "$STORE")
check "plan task: planned + estimate shown, [시작] sends expected_estimate_id, task runs" $(( rc != 0 || $(cnt 'plan 시작 확인 필요 약 40~60크레딧' "$OUT/plan-start.log") != 1 || $(cnt 'started state=running' "$OUT/plan-start.log") != 1 || $(cnt '"event":"started"' "$OUT/server.jsonl") != 1 ))

rc=$(run plan-cancel plan-cancel --server "$S1" --store "$STORE")
check "plan task: [취소] cancels it" $(( rc != 0 || $(cnt 'canceled state=canceled' "$OUT/plan-cancel.log") != 1 || $(cnt '"event":"canceled"' "$OUT/server.jsonl") != 1 ))

rc=$(run plan-change plan-change --server "$S1" --store "$STORE")
check "plan task: changed estimate is not an error — new estimate shown, second [시작] starts" $(( rc != 0 || $(cnt 'notice 예상 사용량이 바뀌었어요 (약 50~80크레딧)' "$OUT/plan-change.log") != 1 || $(cnt 'started state=running' "$OUT/plan-change.log") != 1 ))

rc=$(run remember remember --server "$S1" --store "$STORE")
check "memory: line shown, forget (DELETE) + undo within 10 s (restore), feedback with used memory ids" $(( rc != 0 || $(cnt 'memory 기억했어요: 고양이를 키워요 used=1' "$OUT/remember.log") != 1 || $(cnt 'restored after' "$OUT/remember.log") != 1 || $(cnt 'feedback ok' "$OUT/remember.log") != 1 || $(cnt '"event":"feedback","kind":"correction","memory_ids":\["' "$OUT/server.jsonl") != 1 || $(cnt '"idempotency_key":"present"' "$OUT/server.jsonl") != 1 ))

printf 'NACHO-STT:내일 일정 알려줘' > "$OUT/voice.m4a"
rc=$(run dictate dictate --server "$S1" --store "$STORE" "$OUT/voice.m4a" audio/mp4)
check "dictation: multipart to POST /api/v1/dictation, text back (not sent by itself)" $(( rc != 0 || $(cnt 'dictation text="내일 일정 알려줘"' "$OUT/dictate.log") != 1 || $(cnt '"event":"transcribed","type":"audio/mp4"' "$OUT/server.jsonl") != 1 ))
rc=$(run dictate-refused dictate --server "$S1" --store "$STORE" "$OUT/voice.m4a" video/mp4)
check "dictation: unsupported audio type refused before upload" $(( rc != 2 || $(cnt '"event":"transcribed"' "$OUT/server.jsonl") != 1 ))
check "read mark sent as WS read frame after answers" $(( $(cnt '"event":"read","seq":' "$OUT/server.jsonl") < 1 ))

# Gadget tools (W5a): the 57-tool declaration, window grid + undo (real planner), data gadgets (approval on Windows),
# desktop tidy plan -> approved apply -> approved undo on a real folder (nothing deleted), guides, settings.
rc=$(run cmd-tile command --server "$S1" --store "$STORE" --trust "$TRUST" "창 2x2 바둑판으로 깔고 되돌려 줘")
check "W5a declaration: 57 capabilities, monitor/disk/desktop apply+undo ask" $(( rc != 0 || $(cnt 'declare 57 capabilities, approval required: .*combo_monitor_stats.*combo_disk_top_items,combo_disk_scan.*combo_desktop_tidy_apply,combo_desktop_tidy_undo,combo_desktop_arrange_apply' "$OUT/cmd-tile.log") != 1 ))
check "window tile 2x2 of every window, then undo: change, no card, 4 placed and 4 restored" $(( rc != 0 || $(cnt 'pc: window tool placed 4 window(s)' "$OUT/cmd-tile.log") != 1 || $(cnt 'pc: window undo restored 4 window(s)' "$OUT/cmd-tile.log") != 1 || $(cnt 'card approval' "$OUT/cmd-tile.log") != 0 ))
rc=$(run cmd-monitor command --server "$S1" --store "$STORE" --trust "$TRUST" "시스템 모니터 수치 알려 줘")
check "monitor stats (program names leave the PC): approval card on Windows, approved -> numbers" $(( rc != 0 || $(cnt '"event":"approval_card","capability":"combo_monitor_stats"' "$OUT/server.jsonl") != 1 || $(cnt 'card approval "PC에서 시스템 수치 보기 실행" -> approve' "$OUT/cmd-monitor.log") != 1 || $(cnt 'pc: monitor stats' "$OUT/cmd-monitor.log") != 1 || $(cnt 'combo_monitor_stats 결과: Right now: CPU 38%' "$OUT/cmd-monitor.log") < 1 ))
rc=$(run trust-monitor trust --trust "$TRUST" monitor on)
rc2=$(run cmd-monitor-trusted command --server "$S1" --store "$STORE" --trust "$TRUST" "시스템 모니터 수치 알려 줘")
check "monitor stats with always allow on: no card" $(( rc != 0 || rc2 != 0 || $(cnt 'card approval' "$OUT/cmd-monitor-trusted.log") != 0 || $(cnt 'pc: monitor stats' "$OUT/cmd-monitor-trusted.log") != 1 ))
run trust-monitor-off trust --trust "$TRUST" monitor off >/dev/null
rc=$(run cmd-disk command --server "$S1" --store "$STORE" --trust "$TRUST" "디스크에서 큰 거 뭐가 차지해?")
check "disk top items (paths leave the PC): card, approved -> largest 2 items" $(( rc != 0 || $(cnt 'card approval "PC에서 디스크 큰 항목 보기 실행" -> approve' "$OUT/cmd-disk.log") != 1 || $(cnt 'pc: disk top 5' "$OUT/cmd-disk.log") != 1 ))
DESK="$OUT/desk/Desktop"; mkdir -p "$DESK/폴더"
for f in "스크린샷 2026-11-15 101010.png" "보고서.pdf" "사진.jpg" "자료.zip" "setup.exe" "메모.txt"; do printf 'x' > "$DESK/$f"; done
printf '[.ShellClassInfo]' > "$DESK/desktop.ini"; printf 'L' > "$DESK/앱.lnk"; printf 'k' > "$DESK/폴더/keep.txt"
touch -d '10 minutes ago' "$DESK"/* "$DESK"/폴더/*
files_before=$(find "$OUT/desk" -type f ! -name 'desktop-tidy.json*' | wc -l)
rc=$(run cmd-tidy-plan command --server "$S1" --store "$STORE" --trust "$TRUST" --desktop "$DESK" "바탕 화면 정리 계획 보여 줘")
check "desktop tidy plan: read, no card, 6 plain files (shortcut, desktop.ini, folder skipped), nothing moved" $(( rc != 0 || $(cnt 'pc: desktop tidy_plan ok=true total=6' "$OUT/cmd-tidy-plan.log") != 1 || $(cnt 'card approval' "$OUT/cmd-tidy-plan.log") != 0 || $(find "$DESK" -maxdepth 1 -type f | wc -l) != 8 ))
rc=$(run cmd-tidy-deny command --server "$S1" --store "$STORE" --trust "$TRUST" --deny --desktop "$DESK" "바탕 화면 정리해 줘")
check "desktop tidy apply denied: plan made, card denied, no file moved" $(( rc != 0 || $(cnt 'card approval "PC에서 바탕 화면 정리 실행" -> deny' "$OUT/cmd-tidy-deny.log") != 1 || $(cnt 'pc: desktop tidy_apply' "$OUT/cmd-tidy-deny.log") != 0 || $(find "$DESK" -maxdepth 1 -type f | wc -l) != 8 ))
rc=$(run cmd-tidy-apply command --server "$S1" --store "$STORE" --trust "$TRUST" --desktop "$DESK" "바탕 화면 정리해 줘")
moved=$(find "$DESK/정리됨" -type f 2>/dev/null | wc -l)
check "desktop tidy apply: plan -> approval card -> 6 files moved into 정리됨\<category>, none deleted" $(( rc != 0 || $(cnt 'card approval "PC에서 바탕 화면 정리 실행" -> approve' "$OUT/cmd-tidy-apply.log") != 1 || $(cnt 'pc: desktop tidy_apply ok=true total=0 moved=6' "$OUT/cmd-tidy-apply.log") != 1 || moved != 6 || $(find "$OUT/desk" -type f ! -name 'desktop-tidy.json*' | wc -l) != files_before ))
rc=$(run cmd-tidy-undo command --server "$S1" --store "$STORE" --trust "$TRUST" --desktop "$DESK" "바탕 화면 정리 되돌려 줘")
check "desktop tidy undo (new run, from the journal): card on Windows, 6 files back, none deleted" $(( rc != 0 || $(cnt '"event":"approval_card","capability":"combo_desktop_tidy_undo"' "$OUT/server.jsonl") != 1 || $(cnt 'card approval "PC에서 바탕 화면 정리 되돌리기 실행" -> approve' "$OUT/cmd-tidy-undo.log") != 1 || $(cnt 'pc: desktop tidy_undo ok=true total=0 moved=0 restored=6' "$OUT/cmd-tidy-undo.log") != 1 || $(find "$DESK" -maxdepth 1 -type f | wc -l) != 8 || $(find "$OUT/desk" -type f ! -name 'desktop-tidy.json*' | wc -l) != files_before ))
rc=$(run cmd-guide command --server "$S1" --store "$STORE" --trust "$TRUST" "desktop 가젯 안내 해 줘")
check "gadget guide (read): Windows guide text, no card" $(( rc != 0 || $(cnt 'pc: guide desktop (' "$OUT/cmd-guide.log") != 1 || $(cnt 'card approval' "$OUT/cmd-guide.log") != 0 ))
rc=$(run cmd-mouse command --server "$S1" --store "$STORE" --trust "$TRUST" "마우스 휠 반대로 해 줘")
check "mouse set (change): runs at once with the Windows options" $(( rc != 0 || $(cnt 'pc: MouseSet(PetMouseChange { reverse_scroll: Some(true)' "$OUT/cmd-mouse.log") != 1 || $(cnt 'card approval' "$OUT/cmd-mouse.log") != 0 ))

rc=$(run refresh-login login --server "$S2" --store "$OUT/store2")
sleep 3
rc2=$(run refresh-chat chat --server "$S2" --store "$OUT/store2" "토큰 갱신 시험")
check "token refresh: stale access token refreshed before /ws/chat, in a new process (no new login)" $(( rc != 0 || rc2 != 0 || $(cnt '"grant":"refresh_token"' "$OUT/server-refresh.jsonl") != 1 || $(cnt 'browser GET' "$OUT/refresh-chat.log") != 0 ))

rc=$(run logout logout --server "$S1" --store "$STORE")
rc2=$(run after-logout chat --server "$S1" --store "$STORE" "아직 있니?")
# The store holds one bundle item (like the app's Credential Manager item); after logout it must hold no secret.
left=$(cat "$STORE"/* 2>/dev/null | python3 -c 'import json,sys; t=sys.stdin.read().strip(); print(len(json.loads(t).get("items", {})) if t else 0)')
check "logout: login session ended, secrets deleted ($left left), chat refused" $(( rc != 0 || rc2 == 0 || $(cnt '"known_session":true' "$OUT/server.jsonl") != 1 || left != 0 ))

leaks=$(cat "$OUT"/*.log | grep -c -E 'mock-at-|mock-rt-|nct_[0-9a-f]{8}' || true)
leaks2=$(cat "$OUT"/*.jsonl | grep -c -E 'mock-at-|mock-rt-|nct_[0-9a-f]{8}|mock-code-' || true)
check "no token, code or binding secret in any log ($leaks / $leaks2)" $(( leaks + leaks2 ))

echo "---- summary ----"; cat "$OUT/summary.txt"
exit $fail
