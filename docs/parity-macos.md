## macOS 판과의 기능 대조표

NachoCombo for Windows가 [Nacho Combo for macOS](https://github.com/mafianacho/nachocombo)의 어떤 기능을 어떻게 갖추고
있는지 정리한 표입니다. 이름이 같아도 동작이 다르면 그 차이를 적었습니다.

- **기준:** macOS `main` 342c8e7 (v0.10.2), W5a 가젯 행동은 `main` a2e4d76 · Windows 통합 브랜치(`main` f8f77b3 v0.1.0 기반, Calendar Clock·App Switch·
  Mouse Tune과 Nacho Pet 코어(W1) 포함) · 작성일 2026-11-15
- **상태:** **있음** = 맥과 같은 일을 함 · **부분** = 일부만, 또는 다른 방식으로 함 · **없음** = 아직 없음(만들 수 있음) ·
  **불가** = Windows에서 공개된 방법으로는 할 수 없음(이유를 적음)
- **담당:** 이식 단계. W1 펫 코어(`src/core/pet/`), W2 펫 UI, W3 데스크톱 동작(커넥터 기능), W4 가젯(W4a 달력 시계,
  W4b 앱 전환, W4c 마우스 조정, W4d 벤토 위젯, W4e 앱 내 업데이트), W5 맥 쪽 병합 이후 기능(가젯 MCP, 가젯별 나초 동작,
  연결 탭, 컴퓨터 사용 MCP). "—" = 정해진 단계 없음.
- 서버 주소는 맥 공개 레포에 있는 기본값만 적고, 그 밖에는 "설정값"으로 씁니다. 확인하지 못한 것은 "확인 필요"로
  표시했습니다.
- 펫 코어(W1)는 브랜치에서 이식을 마쳤습니다(`src/core/pet/` 32개 모듈). 코어 파일은 맥 `Sources/NachoComboCore/Pet`의 Swift 파일
  하나당 Rust 파일 하나로 옮기며, `cargo test --lib`로 어느 OS에서나 테스트합니다. Windows 셸 조각(`src/pet_win.rs`)과 함께 아직 펫 UI에
  연결되지 않았습니다(W1 시점). W2에서 가젯 `Nacho Pet`(기본 꺼짐)으로 연결했습니다. 개발 CLI(`examples/nacho-pet-cli.rs`)는 같은 세션으로 목 서버와 왕복합니다.

## 1. Nacho Pet

맥에서는 Nacho Pet이 처음부터 켜져 있는 유일한 가젯입니다. Windows에는 가젯 `Nacho Pet`(`ModuleId::NachoPet`, 키 `pet`)이 W2에서 생겼고 **기본 꺼짐**입니다(새 기능은 기본 꺼짐 원칙). 켜도 채팅을 열 때까지 서버에 연결하지 않습니다.

### 1.1 캐릭터와 말풍선

| 영역 | 맥 기능(동작) | Windows 상태 | 차이·이유 | 담당 |
|---|---|---|---|---|
| 캐릭터 창 | 화면 오른쪽 아래(위치 저장)에 112pt 캐릭터. 클릭하면 채팅을 열고 닫음. 3px 넘게 움직이면 드래그, 2초 누르고 있으면 말하기. 마우스를 올리면 이름표, 파일을 끌어오면 점선 링. 우클릭 메뉴: 채팅·파일 주기·로그인/로그아웃·오른쪽 아래로 돌려놓기·채팅 크기 초기화·설정·숨기기 | 있음 — `src/pet_ui.rs`: 레이어드 창(항상 위, 활성화 안 됨), 클릭·드래그·누르고 있기, 위치 저장, 모니터 경계 보정(`core/pet/placement.rs`), 파일 끌어놓기, 우클릭 메뉴 | 맥과 다른 점: 이름표와 끌어올 때 점선 링은 아직 없음. 우클릭 메뉴는 채팅 열기/닫기·로그인/로그아웃·채팅 크기 초기화·위치 초기화·설정·숨기기(맥의 "파일 주기"는 채팅의 클립 버튼). 위치는 화면 픽셀(맥은 포인트)로 저장하고, 연결이 끊긴 모니터에 있던 위치는 주 모니터 오른쪽 아래로 돌아옴. 가젯은 기본 꺼짐(맥은 켜짐) | W2 (완료)|
| 표정(PetFace) | 표정 8개: 대기, 듣기, 생각, 말하기, 기쁨(답 뒤 4초), 기다림(연결 중), 피곤(10분 조용하면), 잠(30분). 받아쓰기·채팅 상태로 정해짐. 듣기·생각·잠 배지 | 있음 — W1 core 이식 + W2 UI(`face.rs`)| 상태 → 표정 규칙은 코어에 있음. 그림을 그리는 창은 W2 | W1 → W2 (완료) |
| 움직임(PetMotion) | 200ms 교차 페이드와 작은 팝, 대기 중 숨쉬기(2.4초), 2–5.2초마다 눈 깜빡임(130ms), 상태별 루프, 기쁨 점프(420ms)와 착지, 피곤할 때 끄덕임, 누름·드래그 기울기·착지 찌그러짐. 동작 줄이기 설정을 따르고, 숨겨졌거나 화면이 꺼졌거나 전체 화면 앱이 앞에 있으면 멈춤 | 있음 — W1 core 이식 + W2 UI(`motion.rs`, 시간의 순수 함수)| 수치는 디자인 토큰에서 나옴. Windows는 "애니메이션 효과" 설정(SPI_GETCLIENTAREAANIMATION)으로 동작 줄이기를 판단해야 함. 전체 화면 감지는 SHQueryUserNotificationState 등으로 대체 | W1 → W2 (완료) |
| 디자인 토큰·테마 | `PetDesignTokens.swift`는 `design/tokens.json`에서 생성(크기, 간격, 모서리, 글꼴, 밝은/어두운 색, 움직임 시간·곡선). `PetTheme`이 AppKit 색·글꼴로 바꾸고 한글 단어 단위 줄바꿈 | 있음 — W1 core 이식 + W2 UI(`design_tokens.rs`, `color_hex.rs`)| 토큰은 `assets/pet/design-tokens.json`(맥 원본의 사본)을 Rust 상수로 옮기고 테스트가 JSON과 대조(맥은 스크립트로 생성). 글꼴은 맥 시스템 글꼴 대신 Windows 글꼴(맑은 고딕 등)로 대응, 줄바꿈은 DirectWrite 설정으로 맞춰야 함 | W1 → W2 (완료) |
| 캐릭터 그림(서버) | 공개 API에서 받음(토큰 없음): `GET /api/v1/character-assets`(ETag) → `GET /api/v1/character-assets/files/{sha256}.{png\|webp}`. 1MB 이하, PNG/WebP 시그니처, sha256 일치 확인. sha256 이름으로 캐시하고 목록에서 빠진 파일은 지움. 시작할 때와 앱이 활성화될 때 새로 받음. `original` 그림만 씀 | 부분 — W1 core 이식됨(`character_assets.rs`: 목록·ETag·크기·시그니처·sha256 검사, 캐시 정리, 대체 순서), UI는 W2 | 캐시는 `%LOCALAPPDATA%` 아래. WebP 디코딩은 WIC 코덱에 기대야 함(Windows 판 WebP 코덱 유무 확인 필요) | W1 → W2 |
| 캐릭터 그림(번들) | 서버 그림이 없으면 앱에 들어 있는 그림(`Resources/Nacho`)을 씀 | 있음 — `assets/nacho/`에 맥 그림 10개를 바이트 그대로 둠(git blob 해시 동일, NACHO-170 결정). 서버 그림(sha256 확인)이 우선 | `calling-eyeless.webp`는 WebP라 WIC WebP 코덱이 없는 Windows에서는 같은 표정의 `calling.png`로 대체. 그림·이름은 MIT 대상이 아님(루트 `LICENSE`, `TRADEMARKS.md`, `NOTICE`, `assets/nacho/LICENSE`) | W2 (완료)|
| 말풍선 위치·따라가기 | 활성화하지 않는 테두리 없는 창. 캐릭터 위에 오른쪽 끝을 맞춰 4pt 간격으로 붙고, 위가 모자라면 아래로 뒤집힘. 늘 캐릭터가 있는 화면 안. 캐릭터를 끌면 따라옴 | 있음 — `core/pet/chat_layout.rs` 계산을 모니터 DPI에 맞춰 사용 | — | W1 → W2 (완료) |
| 말풍선 크기·꼬리 | 기본 360×520, 최소 320×420, 최대 = 화면 − 여백. 캐릭터 반대쪽 가장자리와 그 모서리만 크기 조절. 크기는 저장(기본값이면 저장 안 함). 꼬리 12pt가 캐릭터 중앙을 가리킴 | 있음 — W1 core 이식 + W2 UI(`chat_layout.rs`)| 크기 규칙은 코어에 있음. 크기 조절은 WM_NCHITTEST로 자유 가장자리만 허용 | W1 → W2 (완료) |
| 말풍선 구성 | 머리(아바타·이름·상태·크레딧), 탭 두 개: 대화, 연결 | 있음 — 머리·대화·**연결 탭**(`src/pet_chat.rs`, `src/pet_connections.rs`, 규칙은 `core/pet/connections.rs`). 연결 탭: 도구·커넥터·가젯 세 구역과 상세(행동 목록·위험도 문구·행동별 스위치·"이 가젯은 항상 허용"·권한 범위·연결 해제) | 맥과 다른 점: 새 서비스 연결(카탈로그·`begin`·앱 비밀번호)은 아직 없음(목록·스위치·범위·해제만). 서버 도구 이름은 맥의 한글 단어표 없이 이름 그대로 보임. 가젯 이름은 영어 제품명. 맥 권한 행·컴퓨터 사용 가젯 없음. 행동 목록은 가젯 도구 레지스트리(`PetGadgetTools`)에서 읽어 레지스트리에 도구가 늘면 자동으로 늘어남 | W2 → W5b (완료) |
| 굵게 등 마크다운 | 인라인만: 굵게, 기울임, 인라인 코드, 링크(http·https·mailto만 클릭). 블록 문법은 그대로 두고, 스트리밍 중 닫히지 않은 표시는 평문 | 있음 — W1 core 이식 + W2 UI(`markdown.rs`)| 맥은 Foundation 파서, Windows는 보이는 결과가 같은 작은 CommonMark 인라인 파서(`~~지움~~`은 표시만 지우고 취소선은 없음) | W1 → W2 (완료) |
| 빈 채팅·안내 | 빈 채팅에 인사와 예시 문장. 서비스 상태(`/client/config`)에 따라 점검·종료·지연 안내 | 있음 — W1 core 이식 + W2 UI(`server_models.rs`의 서비스 상태)| — | W1 → W2 (완료) |

### 1.2 로그인과 연결

| 영역 | 맥 기능(동작) | Windows 상태 | 차이·이유 | 담당 |
|---|---|---|---|---|
| PKCE 로그인 | Keycloak 인가 코드 + PKCE(S256, 32바이트 verifier, 16바이트 state), 클라이언트 `nacho-gadget-app`, 리다이렉트 `nachogadget://auth/callback`. 콜백은 scheme·host·path·state를 엄격히 확인. 만료 60초 전에 갱신하고 동시에 부른 쪽은 한 번의 갱신을 같이 기다림. 갱신 토큰이 죽으면 로그아웃 상태. 로그아웃은 Keycloak 로그아웃 엔드포인트 호출 | 있음 — W1 core 이식 + W2 UI(`auth.rs`: 인가 URL·PKCE·콜백 검사·토큰 해석·갱신 시점, `crypto.rs`) | OS 난수는 셸이 넣음(`src/pet_win.rs`의 `BCryptGenRandom`) | W1 → W2 (완료) |
| 로그인 브라우저·콜백 | `ASWebAuthenticationSession` 시트가 사용자 지정 scheme을 직접 받음. URL scheme 등록도 루프백 서버도 없음 | 있음 — `core/pet/login_browser.rs`(`PetCallbackBrowser`) + `pet_ui.rs`: 기본 브라우저, `nachogadget://`를 **로그인 버튼을 누를 때** HKCU에만 등록, 새로 뜬 exe가 `WM_COPYDATA`로 실행 중인 앱에 넘기고 끝남. state·PKCE는 코어가 검사. 가젯을 끄면 등록을 지움 | 다른 앱이 같은 scheme을 가로챌 수 있어 state 확인이 방어선. 10분 안에 끝나지 않은 로그인은 취소로 처리 | W2 (완료)|
| 로그인 버튼 | 말풍선과 우클릭 메뉴의 **나초 계정으로 로그인**, 설정 탭의 로그인/로그아웃 | 있음 — 채팅 안 **Sign in** 버튼과 펫 우클릭 메뉴(로그인/로그아웃), 가젯 설정 페이지 | 맥은 설정 탭에도 같은 버튼이 있음 | W2 (완료) |
| 토큰 저장 | 로그인 키체인 항목 하나에 모든 비밀(토큰, 커넥터 바인딩)을 묶어 저장(이 기기만, 첫 잠금 해제 후). 계정 범위(서버 + 발급자)별 키. 키체인 읽기를 거부해도 로그아웃으로 보지 않음 | 있음 — W1 core 이식 + W2 UI(`secrets.rs`의 `PetSecretBundle`·`PetSecretStore`), Windows 저장소는 `src/pet_win.rs`(자격 증명 관리자)에 있고| Windows 자격 증명 관리자 일반 자격 증명(이 사용자). 항목 하나가 2560바이트까지라 묶음을 2048바이트 조각 여러 항목으로 나눠 저장. 맥처럼 "읽기 실패 ≠ 로그아웃" 규칙 유지. 키체인 허용 대화상자는 Windows에 없음 | W1(셸) → W2 (완료) |
| 채팅 WebSocket | `/ws/chat`, Bearer 헤더. `message`(`client_msg_id`, 첨부 ID), `stop`, `read{room_id,seq}`, `ready` 뒤 30초마다 JSON `ping`. 처음 채팅을 열거나 보낼 때 연결, 펫을 숨기면 모든 연결을 닫음 | 있음 — W1 core 이식 + W2 UI(`chat_client.rs`의 `ChatClient`: 연결·스트림·멈춤·핑·재연결, `frames.rs`), Windows 전송은 `src/pet_win.rs`(WinHTTP WebSocket)| 맥은 `URLSessionWebSocketTask`. Windows는 WinHTTP WebSocket(Windows 8+, TLS·시스템 프록시). RFC 6455 직접 구현(`ws_framing.rs`, `socket_transport.rs`)은 개발 CLI·테스트용으로만 씀(이 컴퓨터의 `ws://`만, TLS 없음) | W1 → W2 (완료) |
| 재연결 | 1초부터 두 배씩, 최대 30초, 지터 50–100%, 최대 8회. 401/403은 다시 시도하지 않고 토큰을 한 번 갱신한 뒤 한 번 다시 연결 | 있음(코어) — `reconnect.rs`, 세션(`session.rs`)이 채팅·커넥터 연결에 씀 | 지터는 밖에서 넣어 테스트가 결정적 | W1 |
| 기록·델타 복구 | 처음 연결 시 `GET /api/v1/rooms/{id}/history?view=app&limit=50`, **이전 대화 더 보기**는 `before_seq`. 재연결 뒤 `GET /api/v1/rooms/{id}/delta?since_seq=…`를 `has_more`로 최대 5쪽. 끊긴 동안 끝난 답을 질문의 seq로 원래 차례에 붙임. 답이 끝나면 `read` | 있음 — W1 core 이식 + W2 UI: 리듀서·복구는 `chat_state.rs`, 서버 모델은 `server_models.rs`. 서버 호출은 `server_api.rs`, 복구 흐름은 `session.rs`.| — | W1 → W2 (완료) |
| 도구 진행 | `activity` 프레임은 한국어 상태 줄(도구 11종 + 생각·입력·재시도), `tool_call_*`는 차례마다 렌치 줄(한국어 도구 이름, 원래 이름은 툴팁, ✓/✕/…). 중간 메시지는 답이 아닌 진행 줄. 나초가 만든 파일은 클릭할 수 있는 칩(http·https 서명 URL만) | 있음 — W1 core 이식 + W2 UI: 상태 처리는 `chat_state.rs`, 도구 이름·대화 항목은 `transcript.rs`.| — | W1 → W2 (완료) |
| 알림 프레임 | `notification`(크레딧 지급, 커넥터)을 한 줄로 표시 | 있음 — W1 core 이식 + W2 UI| — | W1 → W2 (완료) |
| 오류 문구 | 서버 오류 코드를 한국어로 표시(다시 시도 가능한 속도 제한 포함). 메시지는 4000 코드포인트까지 | 있음 — W1 core 이식 + W2 UI: 길이·빈 메시지 거절은 `chat_client.rs`, 오류 모델은 `server_models.rs` | Rust는 길이를 Unicode 스칼라로 셈(맥과 같은 단위) | W1 → W2 (완료) |
| 서버 설정·모드 | 운영(기본, 서버 `https://nacho.nachotown.com`, 로그인 `https://auth.nachotown.com/realms/nachotown`), 로컬(이 컴퓨터의 nacho-server), 목 서버, 사용자 지정(루프백 밖은 https만, 200자 이하, 계정·쿼리·조각 금지, 발급자는 `/realms/<이름>` 꼴). 설정은 `pet.json`에 저장하고 읽을 때 다시 검사 | 부분 — W1 core 이식됨: `settings.rs`(서버 모드·URL 검사, 펫 위치, 채팅 크기 등 `PetSettings` 전체, `pet.json` 저장, 깨진 파일은 `pet.json.corrupt`로 옮기고 기본값). 앱은 운영 서버를 쓰고, 서버를 바꾸는 화면은 아직 없음(파일을 고치면 적용) | Windows는 앱의 다른 설정 옆(`%APPDATA%\NachoCombo`)에 `pet.json` | W1 → W2 |
| 펫 설정 화면 | 계정, 서버, "이 Mac의 앱·창·가젯을 나초가 쓰게 하기", 음성(첫 문장 읽기), 구석으로 돌려놓기, 선택 막대, 작업 공간 목록(손쉬운 사용 권한 상태와 삭제) | 부분 — 가젯 설정 페이지에 상태, 채팅 열기, 로그인/로그아웃, 위치·채팅 크기 초기화 | 서버 선택, 나초 제어 스위치, 음성, 선택 막대, 작업 공간 목록은 W3 이후 | W2 |

### 1.3 대화 기능

| 영역 | 맥 기능(동작) | Windows 상태 | 차이·이유 | 담당 |
|---|---|---|---|---|
| 승인 카드 | 무엇을·결과·되돌릴 수 있는지 요약, **실행** / **거절**(같은 크기) | 있음 — W1 core 이식 + W2 UI: 카드 모델은 `server_models.rs`, 상태는 `chat_state.rs`. 카드 규칙은 `card_rules.rs`.| — | W1 → W2 (완료) |
| 승인 + `rule_options` | **이번만 실행**, 규칙마다 **실행하고 항상 허용**(`allow_rule.rule_option_id`로 보냄), **거절** | 있음 — W1 core 이식 + W2 UI(`card_rules.rs`)| — | W1 → W2 (완료) |
| 선택 카드 | 하나 고르기: 번호 줄, 클릭하면 답함. 여러 개: 체크 후 **확정**. 입력칸이 비어 있으면 숫자 키로 표시, Enter 답, Esc 지움 | 있음 — W1 core 이식 + W2 UI(`card_rules.rs`)| 숫자 키 처리는 말풍선 창이 포커스를 받을 때만 가능(맥은 활성화 없는 패널) | W1 → W2 (완료) |
| 제안 칩 | 답 아래 빠른 답 칩, 클릭하면 그 글을 다음 메시지로 보냄 | 있음 — W1 core 이식 + W2 UI(`card_rules.rs`)| — | W1 → W2 (완료) |
| 규칙 제안 카드 | 규칙 문장과 **적용** / **아니요**(선택지가 여러 개면 각 선택지 + 아니요) | 있음 — W1 core 이식 + W2 UI(`card_rules.rs`)| — | W1 → W2 (완료) |
| 카드 보내기·충돌 | `POST /api/v1/chat/requests/{id}/answer`(`expected_version`). 이미 답함·만료·대체·버전 충돌이면 `GET`으로 다시 읽음. 실패 시 **다시 시도**, 만료 시각은 KST | 있음 — W1 core 이식 + W2 UI(`server_api.rs`의 답 보내기·다시 읽기, `card_rules.rs`)| KST 표시는 PC 시간대와 무관하게 고정 | W1 → W2 (완료) |
| 휴대폰 전용 카드 | 로그인·인증번호·결제 카드는 휴대폰 앱으로 안내 | 있음 — W1 core 이식 + W2 UI(`card_rules.rs`)| — | W1 → W2 (완료) |
| 작업 시작·취소 카드 | `planned` 작업에 예상 크레딧과 **시작**(`POST /api/v1/tasks/{id}/start`, `expected_estimate_id`) / **취소**(`POST /api/v1/tasks/{id}/cancel`). 409 `estimate_changed`면 새 견적을 보여 주고 다시 누르게 함. 요청 중 두 번째 누름 무시. 진행·결과는 `task_id`로 그 차례에 붙음 | 있음 — W1 core 이식 + W2 UI: 작업 모델·상태는 `server_models.rs`·`chat_state.rs`, 호출과 견적 변경 처리는 `server_api.rs`·`session.rs`.| — | W1 → W2 (완료) |
| 기억 | "🧠 기억했어요" 줄(`update_id`로 중복 제거), **잊어 줘**(`DELETE /api/v1/memory/facts\| 있음 — W1 core + W2 UI(잊어 줘·되돌리기 링크) | **기억이 틀렸어요**는 대화상자 대신 입력창에 써서 보냄(다음 입력이 정정으로 감) | W1 → W2 (완료) | W1 → W2 |
| 사용량 | 읽기 전용. `usage` 이벤트와 `GET /api/v1/me/usage`. 머리에 "크레딧 N", 소진·부족·차단 시 로그 줄. 구매는 휴대폰 앱 | 있음 — W1 core 이식 + W2 UI: 사용량 모델(`server_models.rs`), 숫자·금액 표시(`transcript.rs`).| — | W1 → W2 (완료) |
| 답하는 중 보내기(대기열) | 나초가 답하는 동안 보내면 한 칸에 담음(뒤 글은 줄바꿈으로 이어 붙임), "답이 끝나면 보낼게요 · 취소" 표시. 차례가 끝나면(완료·오류·멈춤) 자동으로 보내고, 거절되면 입력칸으로 되돌림 | 있음 — W1 core 이식 + W2 UI(`session.rs`의 `send_or_queue`)| — | W1 → W2 (완료) |
| 멈추기 | 답하는 동안 보내기 버튼이 **멈추기**로 바뀜(`stop` 프레임) | 있음 — W1 core 이식 + W2 UI: 프레임. 버튼은 W2 | — | W1 → W2 (완료) |
| 업로드(공통) | `POST /api/v1/media/uploads`(JSON, base64). 첨부 4개, 개당 20MB. png·jpg·gif·webp·heic·pdf·txt·md·csv·mp4. 5MB 넘는 그림은 긴 쪽 2048px JPEG로 줄임. 서버는 jpeg/png/webp/gif(5MB 이하)를 보고 txt/md/csv를 읽음 | 있음 — W1 core 이식 + W2 UI: 종류·크기 규칙과 요청은 `media.rs`. 그림 줄이기와 파일 읽기는 셸(WIC)에서 | heic는 Windows에 코덱이 없을 수 있어 줄이기가 안 될 수 있음(확인 필요) | W1 → W2 (완료) |
| 캡처 → 펫 | 떠 있는 캡처 우클릭 ▸ **Ask Nacho About This…**: 다음 메시지에 그림 첨부 | 없음 | Capture Float 메뉴 연결은 W3 이후(파일 끌어놓기와 클립 버튼으로 첨부는 됨) | W2 |
| 선반 → 펫 | 선반 항목 우클릭 ▸ **Give to Nacho**, 펫에 끌어다 놓기(파일, 그림, 글 → 글 파일), 클립 버튼·파일 고르기 | 부분 — 펫 창에 파일을 끌어다 놓기와 채팅의 클립 버튼(파일 고르기) | 선반 항목의 우클릭 메뉴(Give to Nacho)는 아직 없음. 그림 줄이기(5MB 초과)는 아직 없음 — 서버가 거절하면 안내 | W2 |
| 말하기(누르고 있기) | 로그인 상태에서 나초를 누른 채 가만히: 2초 동안 링이 차고, 녹음 중에는 마이크 크기를 보여 줌. Esc·밖으로 끌기 = 취소, 0.5초 미만은 안 보냄. 최대 길이는 서버 `dictation.max_sec`(없으면 60초, 2,000,000바이트). 결과는 메시지로 자동 전송(약 30초 재시도). 첫 사용 때 마이크 권한 | 있음 — 2초 누름 링(`PetHoldGesture`), 녹음(`src/pet_audio.rs`, wave-in 16kHz 모노)과 마이크 크기 링, Esc·밖으로 끌기=취소, 0.5초 미만 안 보냄, 결과를 자동 전송(약 30초 재시도) | 녹음 형식은 WAV(맥은 m4a). 마이크 접근이 막혀 있으면 안내 줄과 "설정 열기"(`ms-settings:privacy-microphone`) | W1 → W2 (완료)|
| 받아쓰기 API | `POST /api/v1/dictation` multipart: `language=ko`, `duration_sec`(소수 둘째 자리), `file`(`dictation.m4a`, `audio/mp4`, AAC 16kHz 모노). 허용 MIME: mp4, m4a, x-m4a, aac, wav, webm, mpeg | 있음 — `POST /api/v1/dictation`에 `audio/wav`(`core/pet/wav.rs`) | WAV 16kHz 모노 16비트는 60초에 약 1.9MB. 서버 한도(2,000,000바이트)에 가까워 서버가 `dictation_max_sec`을 더 짧게 주면 그에 맞춰 자름 | W1 → W2 (완료)|
| 입력칸 마이크 | 서버가 받아쓰기를 켜 두면 서버로, 아니면 기기 안 한국어 음성 인식. 글은 입력칸에만 들어가고 혼자 보내지지 않음 | 부분 — 서버 받아쓰기만. 글은 입력칸에만 들어가고 혼자 보내지지 않음 | 기기 안 음성 인식(맥의 대체 경로)은 없음. 서버가 받아쓰기를 꺼 두면 "이 서버는 받아쓰기를 지원하지 않아요." | W2 |
| 음성 답 | 말로 물은 차례의 답 첫 문장(140자, 마크다운·URL·이모지 제거)을 기기 안 한국어 음성으로 한 번 읽음. 서버로 아무것도 안 보냄. 새 입력·멈춤·새 누름이면 멈춤. 기본 켜짐 | 없음 | 첫 문장 읽기(음성 합성)는 다음 단계. 설정 `speakVoiceReplies`는 저장만 됨 | W1 → W2 |
| 선택 막대 | 다른 앱에서 글을 고르면(3px 넘는 드래그, 더블클릭, Shift+화살표 뒤) 작은 막대. 지연: 바로·0.3초(기본)·0.6초·⌘ 누를 때만. 손쉬운 사용 권한 필요, 기본 꺼짐, 첫 사용 카드. 보안 입력 중 숨김. 암호 관리자·키체인·시스템 설정 제외, 터미널은 기본 제외(다시 허용 가능), 사용자 제외 앱. 동작: 복사·검색(Google, Naver, DuckDuckGo)은 Mac 안에서, 번역·요약·설명·나초에게 묻기는 미리보기와 **보내기** 뒤 `선택 맥락.md` 첨부(8000자 + 앱·창 제목·URL·주변 글). 버튼 순서·숨김·1시간 끄기 | 있음 — 후크로 선택이 끝난 때를 보고(3px 넘는 드래그, 더블클릭, Shift+화살표 뒤), UI Automation(TextPattern)으로 읽음. 지연: 바로·0.3초(기본)·0.6초·Ctrl 누를 때만. 권한 불필요, 기본 꺼짐, 첫 사용 카드·설정(켜기, 지연, 검색 엔진, 버튼 순서·숨김, 터미널 허용, 제외 앱, 1시간 끄기). 암호 칸(`IsPassword`)·관리자 권한 창·읽을 수 없는 창·암호 관리자·Windows 로그인/보안/설정·터미널(다시 허용 가능)·이 앱의 창에서는 읽지 않음. 동작은 맥과 같음: 복사·검색은 PC 안에서, 번역·요약·설명·나초에게 묻기는 미리보기와 **보내기** 뒤 `선택 맥락.md` 첨부 | 메모장(드래그 선택, UIA TextPattern)과 TextPattern 없는 앱(Ctrl+C 대체 경로, 클립보드 복원 확인)을 Windows 10 VM 실제 exe로 확인함. 메모장에서는 앞뒤 글 행이 안 나옴(주변 글을 못 읽음). TextPattern이 없는 앱은 Ctrl+C로 읽고 클립보드를 원래대로 돌려 놓음(Shift·Alt·Windows 키를 누른 동안과, 클립보드를 통째로 보관할 수 없을 때는 시도 안 함). 브라우저(Chrome·Edge·Firefox 등)의 주소·제목은 문서 요소에서 읽어 보고, 못 읽으면 비움. 막대·미리보기 문구는 맥과 같은 한국어, 설정 문구는 4개 언어 | W2b (완료) |
| 펫 링크 | 채팅 안 링크는 앱 안에서만 처리하는 `nachopet://` scheme을 씀 | 있음 — `nachopet://` 링크를 앱 안에서 처리(`pet_ui::on_link`) | 기억 정정은 대화상자 대신 입력창에 쓰고 보내기 | W2 (완료)|

### 1.4 커넥터와 데스크톱 동작

| 영역 | 맥 기능(동작) | Windows 상태 | 차이·이유 | 담당 |
|---|---|---|---|---|
| 커넥터 바인딩 | `POST /api/v1/agents/{id}/connector-binding {kind:"nacho_combo", label}` → `nct_…` 토큰을 비밀 저장소에 보관 | 있음 — core(`server_api.rs`, `secrets.rs`, `connector_client.rs`) + W2에서 펫 세션에 연결(`pet_ui::make_session`) | 토큰은 자격 증명 관리자 묶음에 함께 저장. 라벨은 `Nacho Combo (Windows)`, `kind`는 같음 | W1 → W2 (완료) |
| 커넥터 연결 | `/ws/connector`, 첫 `hello` 프레임에 토큰과 `capabilities`(이름, 한국어 설명, `input_schema`, 위험한 것은 `"approval":"required"`). "항상 허용"이나 끈 기능이 바뀌면 `capabilities` 프레임으로 다시 알림. 서버 `request` → 앱 `response`. 30초 핑, 채팅과 같은 재연결, `error{replaced}`면 멈추고 다시 연결 안 함 | 있음 — core(W1·W3) + 실행기(W3, `pet_exec.rs`). 펫 세션에 붙임(W2: `executor`와 `declaration()`을 세션에 주고, "항상 허용" 스위치가 바뀌면 `watch_declaration`으로 다시 선언) | 기능 이름·입력 스키마는 맥 선언과 같음(`fixtures/mac-capabilities.json` 비교 시험). 다시 알릴 때 핑 장벽, 그 사이 온 요청은 `approval_revoked`로 거절(맥과 같음). Windows에서 의미 없는 기능·삭제형 기능은 선언하지 않음(아래 표) | W1 → W3 |
| 나초 제어 끄기 | 설정 탭 "이 Mac의 앱·창·가젯을 나초가 쓰게 하기"(`allowComboControl`)로 전부 끔. 나초는 컴퓨터 사용 가젯과 자기 자신을 켜고 끄지 못하고, 권한이 남은 가젯은 켜지 않음 | 있음 — core·실행기·세션 연결(끄면 커넥터를 닫고 다시 열지 않음) + 설정 ▸ Nacho Pet의 스위치(`allowComboControl`, `pet.json`에 저장). 끄면 연결 탭 가젯 상세에 안내가 뜸 | 같은 스위치. 컴퓨터 사용은 Windows에 아직 없음 | W2·W3·W5b (완료) |
| 작업 공간 저장소 | 실행 중인 앱·창 위치·가젯을 이름 붙여 `workspaces.json`에 저장(20개, 이름 40자). 불러오면 없는 앱을 열고 어떤 앱도 끄지 않음 | 있음 — core(`desktop.rs`, `desktop_runner.rs`) + 실행기(W3) | `bundleID` 자리에 exe 전체 경로를 기록(없는 앱을 다시 열려고)하므로 맥 파일과 호환되지 않음. 같은 이름 규칙·20개 한도 | W3 |

`Sources/NachoComboCore/Pet/PetCommands.swift`의 기능(처음 커넥터 계약의 16개, hello 순서). 승인 열은 맥 `main`의 선언
기준입니다(맥은 레지스트리 `GadgetToolRegistry`에서 위험도로 정함: read·change는 승인 없음, content_out·input_inject는
"항상 허용"이 아니면 승인, destructive는 늘 승인). 이름·입력 스키마·매개변수 검사는 W1에서 `commands.rs`·`desktop.rs`로
이식했고, 아래 상태 열은 Windows 셸이 그 동작을 실제로 실행하는지(W3)를 뜻합니다.

| 영역 | 맥 기능(동작) | Windows 상태 | 차이·이유 | 담당 |
|---|---|---|---|---|
| `combo_arrange_window` | 앞 창(또는 지정 앱의 창)을 Window Snap 위치로 배치 | 있음 — 선언·실행(W3) | Window Snap 위치를 그대로 씀. "앞 창"은 나초콤보 자신(펫 대화창)을 뺀 맨 앞 창. 관리자 권한 앱의 창은 못 옮김(이유와 함께 실패) | W3 |
| `combo_layout_windows` | 여러 창을 화면별로 한 번에 배치(위치마다 디스플레이 번호) | 있음 — 선언·실행(W3) | 같은 스키마. 디스플레이 번호는 왼쪽부터(`desktop_runner.rs`가 계산, 셸은 옮기기만) | W3 |
| `combo_list_windows` | 디스플레이 목록(번호·크기)과 앱별 열린 창의 위치·크기·디스플레이 번호. 창 제목은 보내지 않음(read, 승인 없음) | 있음 — 선언·실행(W3) | 같은 규칙: 창 제목은 보내지 않음. Windows는 창마다 `minimized`를 더 알려 줌. 60개까지 | W3 |
| `combo_list_apps` | 실행 중인 앱, `installed:true`면 설치된 앱 목록 | 있음 — 선언·실행(W3) | 실행 중인 앱은 창이 있는 프로그램(`bundle_id` 자리에 exe 이름), `hidden`은 "창이 모두 최소화됨". 설치된 앱은 시작 메뉴 바로 가기(이 사용자·모든 사용자, 제거 프로그램 제외) 이름, 300개까지. 스토어(UWP) 앱은 바로 가기가 없어 빠짐 | W3 |
| `combo_open_app` | 앱 열기(한국어·영어 이름 또는 번들 ID) | 있음 — 선언·실행(W3) | 이름은 시작 메뉴 바로 가기 이름(한국어 표시 이름)·exe 이름. 이미 실행 중이면 그 창을 앞으로 | W3 |
| `combo_focus_app` | 앱을 앞으로 | 있음 — 선언·실행(W3) | App Switch와 같은 방식(입력 큐 연결 후 SetForegroundWindow). 포그라운드 잠금 때문에 작업 표시줄만 깜박일 수 있음 | W3 |
| `combo_hide_app` | 앱 숨기기(Finder·나초콤보 제외) | 있음 — 선언·실행(W3) | Windows에는 앱 숨기기가 없어 그 앱의 창을 모두 최소화. 탐색기·나초콤보 보호 | W3 |
| `combo_quit_app` | 앱에 종료 요청(저장 안 한 작업은 앱이 물음). `approval:"required"` | 있음 — 선언·실행(W3), 늘 승인 | 창에 닫기 요청(WM_CLOSE)으로. 강제 종료는 하지 않음. 탐색기·나초콤보 보호. 맥은 3초 뒤 종료됐는지 확인하지만 Windows는 요청만 알림 | W3 |
| `combo_set_system` | 소리 크기 0–100, 내장 화면 밝기 0–100, 방해금지 | 부분 — 선언·실행(W3) | 소리: Core Audio(기기 제어 가젯이 꺼져 있어도 됨). 밝기: **기기 제어 가젯과 같은 DDC/CI 모니터만**(맥은 내장 화면만). 노트북 내장 화면(WMI)은 아직 없어 이유와 함께 실패. 방해금지: 공개 API가 없어 이유와 함께 실패(맥도 같은 방식) | W3 |
| `combo_set_gadget` | 가젯 켜기/끄기(펫·컴퓨터 사용 제외, 권한이 남은 가젯은 안 켬) | 있음 — 선언·실행(W3) | 같은 camelCase ID와 표시 이름. 목록(enum)은 Windows 가젯 전부(맥에 없는 `widgetBoard` 포함). 맥 `menuBar`는 `trayOrganizer`로 받아 줌(별칭) | W3 |
| `combo_list_gadgets` | 가젯 목록과 켜짐 상태 | 있음 — 선언·실행(W3) | 상태는 `on`/`off`/`failed` | W3 |
| `combo_set_dim_strength` | Focus Dim 강도 10–90% | 있음 — 선언·실행(W3) | 같음(꺼져 있으면 강도만 저장) | W3 |
| `combo_clear_shelf` | File Shelf 비우기. 맥 `main`에서는 destructive라 늘 승인 | **선언 안 함**(W3 결정) | 삭제형 행동이라 넣지 않음(선반이 만든 복사본을 버림). core 목록·스키마는 맥과 같게 남겨 두고 Windows 선언에서만 뺌. 요청이 와도 "선언하지 않음"으로 거절 | W3 |
| `combo_workspace_save` | 지금 앱·창·가젯을 작업 공간으로 저장 | 있음 — 선언·실행(W3) | 위 "작업 공간 저장소" 참고. 최소화된 창은 저장하지 않음 | W3 |
| `combo_workspace_apply` | 작업 공간 불러오기(없는 앱 열기, 아무것도 안 끔) | 있음 — 선언·실행(W3) | 같음. 저장할 때 없던 디스플레이의 창은 첫 디스플레이 안으로 당김 | W3 |
| `combo_workspace_list` | 저장된 작업 공간 목록 | 있음 — 선언·실행(W3) | 같음 | W3 |

맥 `main`은 위 16개 뒤에 가젯별 도구(`Sources/NachoComboCore/Gadgets/Tools/*`)도 커넥터로 선언합니다(맥 `a2e4d76` 기준 75개).
W3에서 기존 Windows 가젯으로 바로 되는 14개를, W5a에서 나머지 28개(가젯 안내, 창 도구 8, 앱 전환 설정 2, 트레이 4, 모니터 1,
디스크 2, 마우스 2, 달력 2, 바탕 화면 정리 6)를 같은 이름·같은 입력 스키마로 옮겼습니다(`gadget_tools.rs`의 레지스트리).
Windows 선언은 처음 계약 15개(+`combo_clear_shelf` 제외) + 가젯 도구 42개 = **57개**(대화 첨부가 붙으면 선반 보내기·캡처 보내기를
더해 59개)이고, 맥 hello 순서를 따릅니다. 선언하지 않는 맥 기능은 `PetGadgetTools::LEFT_OUT`에 이유와 함께 있고(삭제형 4개,
`menubar_show_hidden`), 컴퓨터 사용 도구는 W5d입니다.

**개인정보 규칙(W3).** PC의 내용(클립보드 글, 화면 캡처, 파일 자체)을 서버로 보내는 행동(content_out)은 hello에
`"approval":"required"`로 선언해 나초서버가 매번 승인 카드를 띄웁니다. 가젯별 "이 가젯은 항상 허용"을 켜면 그 가젯의
content_out 행동만 승인 없이 선언합니다. 기본 꺼짐, `gadget-trust.json`(설정 파일 옆, 맥과 같은 모양
`{"version":1,"alwaysAllowed":[…]}`)에 저장, 끄면 저장되고 바로 다시 선언합니다(핑 장벽, 그 사이 온 요청은
`approval_revoked`). destructive(앱 종료)는 항상 허용과 상관없이 늘 승인. 목록 행동은 내용 없이 번호·종류·크기만
보내므로 승인 없음. 결과의 내용은 로그에 남기지 않습니다(길이만). 가젯이 꺼져 있으면 `gadget_off`(이유·힌트·한국어 안내)로
답합니다(맥 `requiresGadgetOn`과 같음, 처음 계약 15개는 꺼져 있어도 동작).

| 영역 | 맥 기능(동작) | Windows 상태 | 차이·이유 | 담당 |
|---|---|---|---|---|
| 깨어 있기 도구 | `combo_awake_status` / `start {minutes 0–1440}` / `stop` | 있음 — 선언·실행(W3) | 상태의 `reason`은 manual·app·battery_guard·none. 맥의 `keep_display`·`power_only`·`battery_floor_percent`는 Windows 가젯 설정 이름이 달라 아직 보내지 않음 | W3 |
| 기기 도구 | `combo_device_status` / `set_volume {volume, muted}` / `set_mic_muted {muted}` | 있음 — 선언·실행(W3) | 마이크 음소거는 기기 제어 가젯의 단축키와 같이 이 PC의 모든 마이크에 적용 | W3 |
| 흐리게 도구 | `combo_dim_status` / `pause` / `resume` | 있음 — 선언·실행(W3), 범위 설정(W5a) | 상태에 `scope`(`frontWindow`/`frontApp`, 맥과 같은 값)·`other_displays`를 더함(W5a). 맥의 `only_in_session`은 Windows에 그 옵션이 없어 보내지 않음. 맥처럼 범위는 읽기만 함 | W3 → W5a |
| 클립보드 도구 | `combo_clipboard_list`(read, 내용 없음) / `get_item {index, id}`(**content_out**, 4000자까지) / `copy_again`(change) | 있음 — 선언·실행(W3) | id는 내용의 SHA-256 앞 8자리(Windows 클립보드 기록에는 UUID가 없음). 복사 시각이 없어 `copied_at`을 보내지 않음. 그림은 `not_text` | W3 |
| 선반·캡처 목록 | `combo_shelf_list`, `combo_capture_list_pinned`(read) | 있음 — 선언·실행(W3) | 선반의 글·링크는 이름 대신 `text snippet`·`web link`(맥과 같음). 캡처 크기는 떠 있는 창의 픽셀 크기 | W3 |
| 나초에게 넘기기 | `combo_shelf_give {index, id}`, `combo_capture_ask {mode}`(**content_out**, 다음 채팅 메시지에 첨부) | 부분 — core(이름·스키마·위험·승인 규칙·시험)만, **선언 안 함** | 다음 메시지 첨부는 펫 대화창(W2)이 있어야 함. W2가 붙으면 `windows_declaration(.., chat_handoff=true)`로 선언(선반·캡처에도 "항상 허용" 스위치가 생김) | W2 → W3 |
| 삭제형 도구 | `combo_clipboard_clear`(destructive), `combo_shelf_forget`, `combo_capture_close_all`, `combo_clear_shelf` | **넣지 않음**(W3 결정) | 이 이슈 규칙: 삭제형 행동은 넣지 않는다 | — |

| 영역 | 맥 기능(동작) | Windows 상태 | 차이·이유 | 담당 |
|---|---|---|---|---|
| 창 도구 | `window_displays`, `window_place`, `window_tile`, `window_split`, `window_ratio`, `window_move_display`, `window_arrange_apps`, `window_undo` | 있음 — 선언·실행(W5a) | 맥 `WindowTiling`을 `window_tools.rs`로 이식(왼쪽 위 원점, 디스플레이 번호는 왼쪽부터). 창 배치 가젯이 켜져 있어야 함(맥과 같음). 좌표의 기준은 작업 영역(작업 표시줄 제외). 관리자 권한 앱의 창은 Windows가 막아 `reason: elevated`. `window N`은 `combo_list_windows`와 같은 번호(최소화 창 포함, 최소화 창은 `missing`). 같은 창을 두 번 가리키면 두 번째는 `missing`. `displays`의 x·y는 실제 화면 좌표(맥은 늘 0). undo 기록은 앱을 끄면 사라짐(맥과 같음) | W5a |
| 앱 도구 | `appswitch_settings` / `set_settings`(반복 시 숨김) | 있음 — 선언·실행(W5a) | 같은 스키마. 단축키 자체는 바꾸지 않음(맥과 같음). 숨김은 Windows에서 최소화 | W5a |
| 데이터 가젯 도구(나머지) | 모니터 `stats`, 디스크(`top_items`, `scan`) | 있음 — 선언·실행(W5a), **Windows는 승인** | 프로그램 이름·폴더·파일 경로가 PC 밖으로 나가므로 Windows는 세 도구를 `"approval":"required"`로 선언(가젯별 "항상 허용"을 켜면 승인 없이, 기본 꺼짐). 맥은 승인 없음. 모니터: 맥의 메모리 압력 자리에 커밋 비율, 스왑·부하 평균은 Windows에 없어 null·빈 목록, `gpu_percent`·`processor_queue`를 더 보냄. 디스크: Windows 디스크 지도는 드라이브 전체를 읽으므로 `scan`의 `path`는 `~`(사용자 폴더의 드라이브)·`D:`·드라이브 안의 폴더를 받아 그 드라이브를 스캔(맥은 홈 안 폴더). 고정 드라이브만. 삭제 도구 없음 | W5a |
| 상태·설정 도구(나머지) | 마우스(`status`, `set`), 달력(`status`, `set`), 메뉴 막대(`status`, `list_items`, `tidy`, `set_collapsed`, `show_hidden`) | 있음 — 선언·실행(W5a), `menubar_show_hidden`만 미선언 | 아래 "W5a 스키마 차이" 표. 메뉴 막대 도구는 같은 이름으로 트레이 정리(Windows 11 알림 영역)에 다시 정의: `status`(접힘·자동 접힘 초·아이콘 수·늘 보이게 둔 수), `list_items`(프로그램 이름·`visible`/`hidden`, 위치·툴팁 없음), `tidy`·`set_collapsed`(같음). `show_hidden`은 Windows에 늘 숨김 구역이 없어 선언하지 않음(`set_collapsed false`가 모든 아이콘을 보임). Windows 10에서는 트레이 정리가 켜지지 않아 `gadget_off` | W5a |
| 바탕 화면 정리 | `desktop_tidy_plan`, `tidy_apply`(늘 승인), `tidy_undo`, `tidy_history`, `arrange_suggest`, `arrange_apply`(늘 승인): 바탕 화면 파일을 `정리됨/<분류>/`로 옮기고 되돌림 | 있음 — 선언·실행(W5a), **`tidy_undo`도 늘 승인** | 맥 `DesktopTidyEngine`을 `desktop_tidy.rs`로 이식. 바탕 화면 위치는 알려진 폴더 API(`FOLDERID_Desktop`, OneDrive 백업 포함). 옮기기만 하고 덮어쓰지 않음(`MoveFileExW`를 바꾸기 플래그 없이, 드라이브 사이 복사 없음). 건너뜀: 폴더·바로 가기(`.lnk` `.url` `.appref-ms`)·`desktop.ini`·`Thumbs.db`·숨김/시스템 파일·받는 중(`.crdownload` `.part` `.partial` `.tmp` `.download`)·2분 안에 바뀐 파일. 스크린샷은 Windows 이름(`스크린샷 …`, `화면 캡처 …`)도, 설치 파일은 `.exe` `.msi` `.msix` `.appx`도 분류. 맥은 되돌리기에 승인이 없지만 Windows는 파일을 다시 옮기므로 매번 승인. apply·undo·arrange_apply는 대화(커넥터)에만(맥과 같음). `arrange_apply`는 창 배치 가젯을 거치므로 그 가젯이 켜져 있어야 함. 기록은 `desktop-tidy.json`(설정 폴더) | W5a |
| 가젯 안내 | `combo_gadget_guide`(가젯 설명·할 수 있는 일·한계) | 있음 — 선언·실행(W5a) | `gadget_guide.rs`에 15개 가젯 안내를 Windows 동작으로 다시 씀(권한 문구 없음, 트레이·드라이브 단위 디스크 지도 등). 안내의 도구 목록·예시는 이 PC가 선언한 도구만 씀. 맥의 `computer` 안내는 W5d 전이라 없음(gadget 목록에서 뺌) | W5a |
| 승인 카드의 도구 이름 | (맥은 서버 제목 그대로) | 다름(W5a) | `PetText::tool_name`에 가젯 도구 44개의 이름(4개 언어)을 더해, 승인 카드가 "PC에서 도구 실행" 대신 "PC에서 바탕 화면 정리 되돌리기 실행"처럼 보임 | W5a |
| 컴퓨터 사용 도구(나초용) | 화면 캡처(content_out), 디스플레이 목록, 마우스·키 입력(input_inject), 클립보드 읽기 등 일부를 커넥터에도 선언 | 없음 | 컴퓨터 사용 MCP가 생긴 뒤. 화면·입력은 호출마다 승인 | W5 |

**W5a 스키마 차이.** 아래 말고는 이름·입력 스키마(종류·범위·필수)·hello 순서가 맥 `main` a2e4d76과 같습니다
(`fixtures/mac-capabilities.json`, `gadget_tools.rs`의 `mac_contract` 시험). 승인은 Windows가 맥보다 적게 묻는 곳이 없음을
시험으로 고정했습니다.

| 도구 | 맥 | Windows | 이유 |
|---|---|---|---|
| `combo_mouse_set` | `reverse_scroll`, `smooth_scroll`, `speed`(0.5·1·1.5·2·3), `side_buttons`, `middle_button`(click·missionControl) | `reverse_scroll`, `speed`(같은 값), `pointer_speed`(0·4·6·8·10·12·14·16, 0=그대로), `precision`(keep·on·off) | Windows 마우스 조정에는 부드러운 스크롤·옆 버튼·가운데 버튼 옵션이 없고(옆 버튼은 Windows가 이미 뒤로/앞으로), 포인터 속도·"포인터 정확도 향상"이 있음. 맥 전용 필드를 주면 이유와 함께 거절 |
| `combo_mouse_status` | 위 5개 + `speed_choices` | 위 4개 + `speed_choices`·`pointer_speed_choices`·`precision_choices` | 같은 이유 |
| `combo_calendar_set`·`status` | `show_next_event` 있음 | 없음 | Windows 달력 시계는 일정을 읽지 않음. 세계 시계는 시계가 아는 시간대 ID만 |
| `combo_gadget_guide` | `gadget` 목록에 `computer` | 없음 | 컴퓨터 사용은 W5d |
| `combo_monitor_stats`, `combo_disk_top_items`, `combo_disk_scan` | 승인 없음 | 승인(항상 허용으로 해제 가능) | 프로그램 이름·경로가 나감 |
| `combo_desktop_tidy_undo` | 승인 없음 | 늘 승인 | 파일을 다시 옮김 |
| `combo_menubar_show_hidden` | 있음 | 선언 안 함 | 늘 숨김 구역이 없음 |


## 2. 가젯

맥은 15개(메뉴 막대 정리, 시스템 모니터, 창 배치, 파일 선반, 캡처 플로트, 포커스 디밍, 깨어 있기, 클립보드 기록, 기기 제어,
앱 전환, 마우스 조정, 달력 시계, 디스크 지도, Nacho Pet, 컴퓨터 사용), Windows는 14개(맥의 벤토 보드를 Widget Board 가젯으로 둠)입니다. 맥은 Nacho Pet만 기본으로 켜고,
Windows는 모두 기본 꺼짐입니다. 단축키는 대체로 ⌃⌥ → Ctrl+Alt, ⌘ → Win으로 옮겼고 다른 것만 적었습니다.

| 영역 | 맥 기능(동작) | Windows 상태 | 차이·이유 | 담당 |
|---|---|---|---|---|
| Window Snap | 키보드 명령 24개(⌃⌥ 화살표, U I J K, D F G, E R T, Return, C, -, =, Backspace, ⌃⌥⌘←/→ 디스플레이 이동), 반복 시 ½ → ⅔ → ⅓, 드래그 스냅과 미리보기, 간격, ⌃⌘ 드래그 이동. 창 배치 저장/복원 동작. 손쉬운 사용 권한 필요 | 부분 | 명령 24개와 키는 같음. Windows만: 같은 디스플레이 구성이 다시 연결되면 창을 자동으로 되돌림, Win+Ctrl+T 항상 위(노란 테두리), 오른쪽 드래그 크기 조절. 맥만: 배치 저장/복원 동작(`layout.save`, `layout.restore`). 이동 드래그는 Ctrl+Alt. 간격은 맥 pt, Windows 픽셀. 관리자 권한 앱의 창은 못 옮김, Windows Snap이 켜져 있으면 가장자리 드래그는 Windows에 맡김, AltGr 자판과 Ctrl+Alt+글자가 겹침 | — |
| Capture Float | 영역·창·화면 캡처(⌘⇧2)를 떠 있게. 화살표·상자 그리기, 글자·QR 복사, 색 고르기. **화면 녹화**: 영역·화면을 MP4(선택 시 GIF도)로 바탕 화면에 저장(`capture.record`). 화면 기록 권한(macOS 15는 매달 다시 물음) | 부분 | 캡처 키 Ctrl+Alt+2. Windows가 그리기는 더 많음(상자·화살표·펜·글자·모자이크, 6색, Ctrl+E), 라이브 플로트 있음. 화면 녹화는 **없음**(Windows.Graphics.Capture + Media Foundation으로 만들 수 있음). 보호된 콘텐츠는 검게 찍힘. 글자 인식은 Windows에 설치된 OCR 언어만 | — |
| Clipboard History | ⌃⌥V 목록, 검색, Enter 붙여넣기, ⌥Return 서식 없이, 고정 항목 디스크 저장, 비밀로 표시된 복사와 8MB 넘는 그림은 건너뜀. 원래 앱에 붙여넣기는 손쉬운 사용 권한(선택) | 있음 | 서식 없이 붙여넣기는 Shift+Enter, Ctrl+1…9. 건너뛰는 기준은 Windows 형식(`ExcludeClipboardContentFromMonitorProcessing`, `CanIncludeInClipboardHistory`). 관리자 권한 앱에는 붙여넣기가 거부됨 | — |
| System Monitor | 메뉴 막대에 그래프·숫자, SMC 온도·팬, IOReport(비공개 라이브러리) CPU/GPU 클럭, 마우스를 올리면 큰 차트, 패널(기록, 코어별, 프로세스, 볼륨, 인터페이스, 알림) | 부분 | 트레이에는 숫자만(그래프 없음), 큰 차트 대신 패널. 성능 카운터 사용. 팬·코어별 온도는 **불가**(커널 드라이버가 필요하고 이 앱은 설치하지 않음, "–" 표시). 부하 평균은 **불가**(Windows에 없음, 프로세서 큐 길이로 대신). 프로세스별 네트워크는 관리자 권한이 필요해 안 보임(프로세스별 디스크는 보임) | — |
| Keep Awake | 컵 클릭, 15분–5시간, 지정 프로그램 실행 중, 화면도 켜 둘지, 배터리 보호, 전원 연결 시에만 | 부분 | `SetThreadExecutionState` 사용. "전원 연결 시에만" 옵션 없음. 덮개·전원 단추는 전원 옵션을 따르고 회사 정책이 절전을 강제할 수 있음(맥은 외부 화면·전원이 없으면 덮개를 닫으면 잠듦) | — |
| Device Control | 출력 장치와 소리, 마이크 전체 음소거, 다음 출력, 선호 장치 자동 선택, 외부 모니터 DDC 밝기(Apple silicon만) | 있음(Windows가 더 많음) | Windows만: 앱별 소리 크기(맥은 오디오 드라이버가 필요해 없음), 밝은/어두운 모드 일정(`device.theme`). 기본 장치 선택은 비공개 인터페이스(IPolicyConfig). 외부 모니터 DDC/CI, 노트북 내장 화면은 Windows 슬라이더 | — |
| File Shelf | 드래그를 흔들면 열리는 선반(파일, 그림, 링크, 글), 꺼내기(복사), ⌃⌥S | 있음 | Windows는 "드래그 중" 공개 신호가 없어 드래그 이미지로 알아챔(이미지 없이 끄는 앱은 단축키 필요). Outlook 첨부도 받음. Windows만: 탐색기에서 Space로 미리보기(바탕 화면은 안 됨). 선반 → 펫은 W2 | — |
| Focus Dim | 앞 창 뒤를 흐리게, 디스플레이별, 페이드, 일시 정지 ⌃⌥⌘H. **집중 세션**: 25분 타이머와 휴식, 메뉴 막대 카운트다운, 집중 중에만 흐리게, 시작·끝에 단축어 실행(`dim.session`) | 부분 | 일시 정지 Ctrl+Alt+Win+H. 집중 세션은 **없음**(만들 수 있음, 단축어 실행은 Windows에 대응물 없음). 다른 앱의 항상 위 창은 밝게 남음 | — |
| Menu Bar Organizer ↔ Tray Organizer | 맥: 자기 구분선 상태 항목을 늘려 아이콘을 밀어냄, ⌘ 드래그로 직접 배치, 늘 숨김 구역(`menuBar.showAll`), 올리면 펼치기, 노치 아래 아이콘은 못 보임 | 부분 | Windows: 아이콘별 표시 스위치(`HKCU\Control Panel\NotifyIconSettings`, 비공개, Windows 11 22H2+)로 넘침 메뉴에 접고 되돌림, 자동 접기, 끄면 원래 값 복원. 순서는 Windows가 정함, 늘 숨김 구역과 직접 배치는 **불가**(트레이 순서를 바꿀 공개 방법 없음). Windows 10에서는 동작 안 함 | — |
| Disk Map | 선버스트·블록 두 보기, 마지막 변경으로 색칠, 정리 목록, "숨은 공간" 항목, 훑어보기. 휴지통으로 보내기, 영구 삭제는 한 번 더 확인. 전체 디스크 접근(선택) | 부분 | Windows: 블록 보기만, 한 번에 고정 드라이브 하나, 휴지통으로만(영구 삭제 없음). Windows·Program Files·프로필·다른 계정 폴더는 제시 안 함, OneDrive 자리표시자 제외. 선버스트·변경 시각 색·정리 목록은 없음 | — |
| Calendar Clock | 메뉴 막대에 글자 시계(1초마다), 날짜·요일·초·12/24시간, **다음 일정과 참가 버튼**(15분 전부터 이름과 남은 시간, 화상 회의 링크 16종 인식), 월 달력(일정 있는 날 점, 고른 날 일정 6개까지), 세계 시계(도시 38개, 개수 제한 없음, 시스템 시간대), 달력 열기, 캘린더 권한(선택) | 부분 | Windows: 트레이 아이콘이 오늘 날짜를 보여 주고 시계 글자는 툴팁(Windows 작업 표시줄 구석에 글자를 둘 자리가 없음). 월 달력(주 시작은 지역 설정), 오늘로부터 며칠(Windows만), 세계 시계 최대 6개(툴팁에는 3개), 도시 검색(결과 5개), 날짜·시간 설정 열기. 일정·남은 시간·참가 버튼은 **불가**(모든 앱이 함께 쓰는 공용 일정 저장소가 포터블 exe에 없음). 시간대·서머타임은 앱 안 규칙이라 규칙이 바뀌면 앱 업데이트 필요. 날짜 형식은 앱이 지원하는 4개 언어만 | W4a |
| App Switch | ⌥Tab / ⌥⇧Tab 창 전환(⌥를 놓으면 전환, Esc 취소, 지금 Space의 창 앞에서부터 + 최소화된 창, 24개, 실제 앱 아이콘), 다른 앱 숨기기, 앱 단축키(번들 ID, 열기·앞으로·앞에 있으면 숨기기, 두 개 이상 수정 키). 손쉬운 사용 권한 | 부분 | Windows: Ctrl+Alt+` / Ctrl+Alt+Shift+`(Alt+Tab은 Windows 것), Ctrl+Alt를 놓으면 전환. 같은 목록 규칙(24개, 최소화 창 뒤), 길면 스크롤과 "n / m". 앱 아이콘 대신 색 글자 배지. 다른 가상 데스크톱의 창은 빠짐(맥은 고르면 그 Space로 감). "다른 창 숨기기"는 숨기기 대신 **최소화**(Windows에 앱 숨기기가 없음), 24개 제한 없이. 앱 단축키는 exe 경로, 숨기기 = 최소화, 앞으로는 첫 창만. 권한 불필요, 관리자 권한 앱은 앞으로 안 올 수 있음 | W4b |
| Mouse Tune | 이벤트 탭으로 휠 마우스만 바꿈(트랙패드 제외): 휠 방향 반대, 부드러운 스크롤, 스크롤 속도 0.5–3배(이벤트 크기 조절), 옆 버튼 뒤로/앞으로(⌘[ / ⌘]), 가운데 버튼 = 가운데 클릭 또는 Mission Control. 손쉬운 사용 권한 | 부분 | Windows: 저수준 훅으로 휠 한 칸(120 배수)만 뒤집음(정밀 터치패드는 그대로, 관리자 권한 앱에는 안 됨). 스크롤 속도는 시스템 "한 번에 스크롤할 줄 수"를 바꿔서(휠 마우스만이 아니라 전체). Windows만: 포인터 속도, 포인터 정밀도, 끌 때·종료·비정상 종료 뒤 원래 값 복원. 부드러운 스크롤 **불가**(시스템 설정이 없고 앱이 각자 함), 옆 버튼은 이미 브라우저·탐색기에서 뒤로/앞으로라 제공 안 함, Mission Control 대응 없음. 실제 마우스·터치패드로는 아직 확인 안 함 | W4c |
| 벤토 위젯 | 메인 창에 모든 도구를 위젯으로 놓는 벽, 메뉴 막대에 고정 위젯 작은 보드. 편집 모드(⌘E): 옮기기·숨기기·크기(작게·넓게·크게)·고정·단축키. 게이지·타이머·목록·달력 등. UI 문구는 한국어 | 부분 | Windows: **Widget Board** 가젯(기본 꺼짐). 메인 창이 아니라 트레이 패널·설정 쪽·`board.show` 단축키(기본 키 없음)로 여는 창 하나. 배치·내용 규칙은 `src/core/bento.rs`(6/4/2열, 작게·넓게·크게, 숨김·복귀, 끌어놓기와 ←→ 키 이동), 편집은 어떤 가젯도 켜거나 끄지 않음. 꺼진 가젯은 한 줄 안내 타일. 맥과 다른 점: 나초 펫 타일 없음(펫 UI는 W2), 컴퓨터 사용 타일 없음, 달력 시계 타일은 다음 일정 대신 요일·날짜·세계 시계 2개(일정 읽기 **불가**), 클립보드 행에 복사 시각 없음(기록에 시각이 없음), 디스크 지도 타일은 시스템 드라이브 막대만(폴더별 몫은 미연결), 메뉴 막대 위젯 대신 Tray Organizer 접힘 상태, 메뉴 막대 고정 작은 보드·위젯 고정·위젯별 단축키 없음. 문구는 4개 언어 | W4d |
| 앱 안 업데이트 | GitHub Releases `latest`를 시작 시·6시간마다·지금 확인. 초안·프리릴리스·되돌린 버전 건너뜀. `SHA256SUMS.txt`, 번들 ID, 버전, 코드 서명 확인. 자동 설치 기본 켜짐: 앱을 끌 때 교체(쓰는 중에는 재시작 안 함), "재시작하여 업데이트"·되돌리기는 확인 대화상자. 이전 판 하나 보관, 두 번 비정상 시작이면 스스로 되돌림 | 없음 | 지금은 사용자가 zip을 직접 받고 `certutil`로 `SHA256SUMS.txt`와 비교. Windows 판은 GitHub Release 확인 → 내려받기·해시 확인 → 사용자가 확인하면 교체로 계획. 실행 중인 exe는 이름 바꾸기로 교체해야 함. exe가 코드 서명되지 않아 서명 확인은 아직 불가 | W4e |
| 컴퓨터 사용 MCP | 실험 기능. `127.0.0.1:47653/mcp` Streamable HTTP MCP 서버, Bearer 키(`nccu_`, 해시만 저장, 8개). 도구 묶음: 화면, 앱·창, 입력, 클립보드, 열기, 시스템 정보. 고위험(AppleScript, 셸, 파일 읽기·쓰기, 프로세스 종료)은 따로 켜고 켤 때 확인. 가젯·묶음·LAN 모드 모두 기본 꺼짐, 조합이 켜고 끄지 않음, 나초도 못 켬. LAN 모드는 TLS만(자체 서명, 지문 고정), 사설 주소만. 분당 240회 제한, 키 5번 틀리면 60초 잠금. 비상 정지 ⌃⌥⌘. | 없음 | Windows 판은 기본 꺼짐, `127.0.0.1`만으로 계획. AppleScript 묶음은 **불가**(Windows에 없음, PowerShell은 셸 묶음과 같은 위험으로 따로 판단). 입력은 SendInput(관리자 권한 앱에는 안 됨, 보안 데스크톱 불가) | W5 |
| 가젯 MCP | NachoCombo MCP 서버(`127.0.0.1:47653/mcp`, Streamable HTTP, 요청마다 JSON 응답 하나). 설정 ▸ MCP: 서버 스위치(`computerUse.serverEnabled`), 가젯별 MCP 스위치(`enabledGadgets`), 키(`nccu_`+base64url 43자, SHA-256 해시만 `computer-use-keys.json`에 저장, 8개, 라벨 40자), 연결 안내, 최근 호출(이름·키·결과만). 검사 순서: 실행 중 → `/mcp` → 주소 → Host/Origin → 잠금 → 키 → 클라이언트·키별 분당 240회 → POST → JSON-RPC → 스위치 → 실행. 키 5번 틀리면 60초 잠금. 도구는 커넥터와 같은 레지스트리(W5a `PetGadgetTools::declared`)에서 `mcp_name`(`<가젯>_<동작>`)·`on_mcp`로, 가젯 안내는 `resources/list`·`resources/read`(`nachocombo://gadgets/<id>/guide`, MCP 이름으로 렌더) | 있음 — core `src/core/mcp/`(HTTP·JSON-RPC·키·제한·주소 정책·설정·도구·서비스·소켓, Linux 시험), 셸 `src/mcp.rs`(서버 수명·키·승인 카드·`ctl mcp`), 설정 ▸ MCP 쪽(`pages.rs`). 도구 이름·입력 스키마·위험 등급·출구는 `src/core/mcp/fixtures/mac-mcp-tools.json`(맥 `GadgetToolRegistry.all`을 Swift로 출력)과 비교 시험 | **LAN 모드 없음**: 127.0.0.1에만 바인드하고 루프백이 아닌 상대는 키 확인 전에 끊고(403), Host/Origin은 `localhost`·`127.0.0.1`·`[::1]`만. 이유: 다른 PC에 열려면 TLS 인증서·방화벽 예외(관리자 권한이 필요할 수 있음)·사설 주소 판정이 필요하고, 이 이슈 범위의 위험보다 큼. `allowedHosts`(터널)도 없음. **승인이 맥보다 엄격**: 맥은 MCP 호출을 스위치만으로 실행하지만, Windows는 커넥터가 `approval:"required"`로 선언할 도구(destructive 늘, content_out은 "항상 허용"이 아니면)를 나초 펫 대화창의 로컬 승인 카드(`mcp-` 카드, 서버로 안 감)에서 허용해야 실행. 펫이 꺼져 있으면 `approval_unavailable`, 60초 답 없으면 `approval_timeout`. **꺼진 가젯의 도구는 `tools/list`에 없음**(모듈이 필요한 도구만, 처음 계약 15개는 맥처럼 꺼져 있어도 동작). 프롬프트는 아직 빈 목록(맥의 `window_side_by_side` 등은 Windows 문구가 없음). 삭제형 도구(`shelf_clear`)는 MCP에도 없음. 비상 정지·도구 묶음은 컴퓨터 사용 MCP(W5d)와 함께 | W5c |
| 조합(Combos) | No.1 Starter – No.7 Full Combo + 사용자 조합 5개(이름 24자), 바뀌는 것 미리보기, 5분 안 되돌리기. 조합별 "쉬워지는 순간"과 물어볼 권한 | 부분 | 7개 프리셋과 사용자 조합 5개는 같음. 다른 점: No.1·No.3·No.4·No.6은 메뉴 막대 정리 대신 Tray Organizer. **No.5 Meeting**: 맥은 달력 시계·기기 제어·깨어 있기(화면 켜 둠), Windows는 기기 제어·깨어 있기·클립보드 기록(화면 켜 둠 조정 없음). **No.6 Presenter**: 맥 흐리게 0.7·바로 접기·다른 화면도 흐리게, Windows 흐리게 0.6만. No.7은 맥 13개(펫·컴퓨터 사용 제외), Windows 14개(Widget Board 포함). Windows 조정 항목에 "화면 켜 두기" 없음 | — |
| 가젯 동작·단축키 | 가젯별 전역 단축키 동작 목록과 기본 키, 도구 패널 열기 단축키(동작은 실행 안 함), 가젯이 켜져 있을 때만 등록 | 있음 | 맥만: `menuBar.showAll`, `layout.save`, `layout.restore`, `capture.record`, `dim.session`. Windows만: `window.onTop`, `device.theme`. 충돌 검사는 양쪽 다 있음 | — |
| 빠른 패널 | 메뉴 막대 팝오버(도구 목록 → 도구 조작) + 벤토 작은 보드 | 있음 | Windows는 트레이 패널(`src/panel.rs`). 벤토 보드는 Widget Board 가젯(패널의 "Open Widget Board" 단추, W4d) | — |
| 설정 창 | 사이드바, 마우스를 올리면 개요 카드, 단축키·**권한**·일반·MCP 쪽 | 부분 | Windows는 가젯별 개요/설정/단축키 탭. 권한이 필요 없어 권한 쪽 없음. MCP 쪽 있음(W5c: 서버·가젯 스위치·키·연결 안내·최근 호출) | — |
| 언어 | 영어 UI, 일부 한국어 고정(벤토, 패널·펫 일부). 현지화 파일 없음 | 있음(Windows가 더 많음) | Windows는 English·한국어·日本語·简体中文(Windows를 따르거나 일반에서 고름) | — |
| 로그인 시 열기 | `SMAppService`(일반 ▸ Open at Login) | 있음 | HKCU `Run` 값(일반 ▸ Open at Sign-In) | — |
| 진단 | `state.json`에 모듈별 켜짐·상태 기록(꺼진 가젯이 타이머·단축키·상태 항목을 실제로 놓았는지 밖에서 확인) | 부분 | Windows는 `--debug-ctl`과 `NachoCombo.exe ctl …`가 상태 JSON을 출력(파일 대신 명령) | — |
| 설치·서명 | zip + dmg, ad-hoc 서명(새 빌드는 키체인 허용을 다시 물을 수 있음) | 부분 | 포터블 zip 하나, 설치·관리자 권한 없음. 코드 서명 없음(SmartScreen 경고) | — |

## 3. 구조·테스트·도구

| 영역 | 맥 기능(동작) | Windows 상태 | 차이·이유 | 담당 |
|---|---|---|---|---|
| 코어/셸 분리 | `NachoComboCore`(플랫폼 없는 논리) + `NachoCombo`(AppKit 앱) + 개발 CLI(`NachoPetCLI`, `NachoUpdateCLI`) | 부분 | Windows는 `src/core/*`(라이브러리 타깃, Windows 호출 없음)와 `src/*.rs`(Win32·Direct2D 셸). 펫은 `src/core/pet/`(Swift 파일 하나당 Rust 파일 하나, 같은 타입 이름)와 셸 조각(WinHTTP, 자격 증명 관리자, OS 난수, 시간대). 펫 개발 CLI는 `examples/nacho-pet-cli.rs`(앱과 같은 `PetSession`). 업데이트 CLI는 W4e | W1 |
| 단위 테스트 | `swift test`(Core 전체) | 있음 | `cargo test --lib`이 코어 테스트를 Linux·macOS에서도 돌림. `i18n`·`trayorg`·`diskmap` 테스트는 셸 쪽이라 Windows에서 `cargo test`로만 | W1 |
| `scripts/pet-roundtrip.sh` | 목 nacho-server + 대리 로그인과 펫 CLI 사이 실제 왕복: PKCE 로그인, 채팅 스트리밍·멈춤, 캡처·선반 업로드, 거절 파일, 커넥터 명령, 기록·델타, 도구 진행, 작업·카드, 기억, 받아쓰기, 토큰 갱신, 로그아웃 | 있음 — `tools/mock-server` 상대로 50개 확인 모두 PASS(다시 로그인 없는 재시작, 계획 작업 시작·취소·견적 변경, W3 데스크톱 동작, W5a 가젯 행동: 창 바둑판+되돌리기, 모니터·디스크 승인, 바탕 화면 정리 계획→거절/승인→되돌리기를 실제 폴더에서, 안내, 마우스) | Rust와 Node 18+ 필요. 출력은 `target/pet-roundtrip/` | W1 |
| 펫 화면 문구의 현지화 | 맥의 펫 문구(`PetTranscript`·`PetCardRules` 등)는 한국어 고정 | 다름 — 4개 언어 | 한국어 문장은 코드에 그대로 두고(맥과 같은 문장·같은 시험), 그리는 곳에서 `i18n::t`가 그 문장을 키로 en·ja·zh를 찾는다(`assets/i18n/en.json` 신설, `ja.json`·`zh.json`에 추가. ko는 원문 그대로). `{}`가 든 문장은 패턴, 담긴 한국어 조각은 따로 번역. 서버로 가는 글(도구 설명·선택 맥락 파일·번역/요약 요청문)은 번역하지 않음(`i18n-exempt` 표시). 새 한국어 문장을 넣고 세 파일에 없으면 `core/pet/i18n_check.rs`가 실패함 | i18n 정리 |
| 승인 카드의 행동 이름 | 서버가 보낸 카드 제목을 그대로 보임 | 다름 | 서버가 제목·세부·허용 규칙 문장에 `combo_quit_app` 같은 내부 이름을 쓰면 `PetText::humanize_action_names`가 도구 이름(예: "앱 종료")으로 바꿔 보임. 내부 이름은 보이지 않음(도구 칩의 툴팁에만 남음). 맥 코드는 서버 문장을 그대로 보인다(맥에는 이 변환이 없음) | i18n 정리 |
| `scripts/check-server-contract.sh` | 펫이 쓰는 REST 경로가 nacho-server 명세에 아직 있는지 읽기 전용으로 확인 | 있음 — 19/19 경로가 nacho-server 명세에 있음(`server_api.rs`의 `PET_SERVER_ROUTES`) | 서버 레포 위치는 실행할 때 인자로 줌 | W1 |
| `scripts/update-roundtrip.sh` | 목 GitHub Releases와 가짜 앱 번들로 업데이트 내려받기·확인·교체·되돌리기·손상 릴리스 거부 확인 | 없음 | 앱 안 업데이트와 함께 | W4e |
| 디자인 도구 | `gen-design-tokens.mjs`(토큰 생성), 디자인·움직임 캡처·감사 스크립트, `screenshots.sh` | 부분 | Windows는 `ctl snapshot`으로 화면 캡처. 토큰은 생성 스크립트 대신 `design_tokens.rs`의 테스트가 JSON 사본과 대조 | W1 → W2 |
| CI | `ci.yml`(macos-15: `swift test`, 업데이트 왕복, 유니버설 빌드, 번들 확인), `release.yml`(태그 → zip·dmg·SHA256SUMS, Ubuntu 작업이 합계 확인 후 릴리스), 릴리스 알림 | 부분 | `ci.yml`(windows-2025: `cargo test --locked`, 릴리스 빌드, exe 업로드), `release.yml`(태그와 `Cargo.toml` 버전 확인, 테스트, 빌드, exe 확인, zip·SHA256SUMS, Ubuntu 작업이 확인 후 릴리스). Linux에서 `cargo test --lib` 작업과 펫 왕복 작업은 아직 없음. 릴리스 알림 없음 | W1 |
| 설정 파일 | `~/Library/Application Support/NachoCombo/settings.json`(모두 기본 꺼짐), 고정 클립보드·창 배치 파일, `pet.json`, `workspaces.json`. 꺼짐 계약: `stop()`이 타이머·모니터·이벤트 탭·상태 항목과 단축키를 모두 놓음 | 있음 | `%APPDATA%\NachoCombo`(`NACHOCOMBO_DIR`로 바꿀 수 있음). 같은 꺼짐 계약. 펫 파일은 W1·W2에서 추가 | — |
