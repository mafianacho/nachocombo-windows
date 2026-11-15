//! How to use one gadget: the answer of `combo_gadget_guide` (and, for the gadget MCP, the resource
//! `nachocombo://gadgets/<id>/guide`). Port of the macOS `GadgetGuide.swift` and the guides next to each macOS
//! `Gadgets/Tools/<Gadget>Tools.swift`, rewritten for what the Windows gadgets do (no Accessibility or Screen Recording
//! permission, Windows names, Tray Organizer instead of the menu bar, whole-drive Disk Map …).
//!
//! The text lists only the tools this PC declares (`declared`), so a guide never offers a tool Windows does not run.
use super::gadget_tools::{PetGadgetTool, PetGadgetTools};
use serde_json::{json, Value};

/// Which name a guide uses for a tool: the connector's declared name (Nacho chat) or the MCP name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PetGuideExit { Connector, Mcp }

pub struct PetGuideExample {
    /// Action of a tool of the same gadget (`place`), or a full MCP name of another gadget's tool.
    pub action: &'static str,
    pub arguments: Value,
    /// Korean: when this call is the right one.
    pub note: &'static str,
}

pub struct PetGadgetGuide {
    pub gadget: &'static str,
    pub summary: &'static str,
    pub can_do: &'static [&'static str],
    pub steps: &'static [&'static str],
    pub examples: Vec<PetGuideExample>,
    pub limits: &'static [&'static str],
}

fn ex(action: &'static str, arguments: Value, note: &'static str) -> PetGuideExample { PetGuideExample { action, arguments, note } }

pub struct PetGadgetGuides;

impl PetGadgetGuides {
    /// Gadgets with a guide on Windows (macOS ids; macOS also has `computer`, which Windows does not have yet).
    pub const GADGETS: [&'static str; 15] = ["combo", "window", "menubar", "monitor", "disk", "clipboard", "shelf", "capture", "dim", "awake",
                                              "device", "mouse", "appswitch", "calendar", "desktop"];

    // i18n-exempt:begin (sent to the server as tool text, not drawn)
    pub fn guide(gadget: &str) -> Option<PetGadgetGuide> {
        let g = match gadget {
            "combo" => PetGadgetGuide { gadget: "combo",
                summary: "나초콤보 자체를 다뤄요: 가젯을 켜고 끄고, 지금 PC 상태를 작업 공간으로 저장했다가 다시 불러와요.",
                can_do: &["가젯 목록과 켜짐 상태 보기", "가젯 하나 켜기/끄기(나초 펫은 사용자만 바꿔요)", "작업 공간 저장·목록·불러오기", "가젯별 사용 가이드 보기"],
                steps: &["가젯 상태를 모르면 먼저 목록을 봐요.", "꺼진 가젯이 필요하면 사용자에게 켤지 물어본 뒤 켜요.", "작업 공간을 불러오기 전에 목록으로 이름을 확인해요."],
                examples: vec![ex("list_gadgets", json!({}), "어떤 가젯이 켜져 있는지 볼 때"),
                               ex("set_gadget", json!({"gadget": "focusDim", "on": true}), "사용자가 켜도 된다고 한 뒤 집중 디밍을 켤 때"),
                               ex("workspace_apply", json!({"name": "업무 모드"}), "저장해 둔 '업무 모드'로 돌아갈 때")],
                limits: &["불러오기는 다른 앱을 종료하지 않아요.", "관리자 권한으로 실행 중인 앱의 창은 Windows가 옮기지 못하게 해요.",
                          "결과가 ok:false, reason:gadget_off이면 사용자에게 켤지 물어보고 combo_set_gadget을 불러요."] },
            "window" => PetGadgetGuide { gadget: "window",
                summary: "창 배치 가젯으로 창을 원하는 자리에 자유롭게 놓아요. 개수 제한 없이(한 번에 200개까지) 아무 좌표·겹침·앞뒤 순서로 놓을 수 있고, \
                          rows×cols 바둑판·전체 바둑판·여러 디스플레이 걸치기·2/3/4등분·비율·앱 묶음 배치와 한 번에 되돌리기도 돼요.",
                can_do: &["자유 배치(place): 창마다 x, y, w, h(0~1 비율 또는 픽셀), 겹침·앞뒤(z) 가능, 개수 제한 없음",
                          "바둑판(tile): rows×cols 아무 크기, 채우는 순서, 간격, 여러 디스플레이에 걸치기·화면별",
                          "전체 바둑판: all_windows:true로 열린 창 전부를 모든 화면에 자동으로 깔기",
                          "2·3·4등분(split), 지정 비율(ratio), 다른 디스플레이로 옮기기(move_display), 앱 묶음 배치(arrange_apps)",
                          "맨 앞 창 하나(arrange)나 여러 앱 창(layout)을 이름 붙은 자리로(반·4등분·3등분·최대화)",
                          "직전 배치를 한 번에 되돌리기(undo, 최대 5번 전까지)", "디스플레이와 열린 창 목록 보기(displays, list_windows, 창 제목은 보내지 않아요)"],
                steps: &["디스플레이가 여러 개이거나 어떤 창이 열려 있는지 모르면 먼저 displays와 list_windows로 번호를 확인해요.",
                         "창은 {app, window}(또는 id \"앱#번호\")로 가리켜요. window는 그 앱의 몇 번째 창인지(앞에서부터 1)이고 list_windows에 같은 번호가 나와요.",
                         "격자 모양이면 tile(rows, cols), 열린 창 전부면 tile all_windows:true, 그 밖의 모든 모양은 place에 좌표를 계산해서 보내요. 칸 수 제한은 없어요.",
                         "결과의 windows에서 창마다 requested(요청한 자리)와 applied(실제 자리), reason을 봐요. 앱이 최소 크기를 지키면 둘이 달라요.",
                         "마음에 안 들면 undo로 한 번에 되돌려요.",
                         "좌표 약속(place, unit ratio): 디스플레이 작업 영역(작업 표시줄 제외)의 왼쪽 위가 (0,0), 오른쪽 아래가 (1,1)이에요. 화면 밖 값은 화면 안으로 맞춰져요."],
                examples: vec![ex("displays", json!({}), "디스플레이 번호와 크기를 확인할 때"),
                               ex("list_windows", json!({}), "배치 전에 디스플레이 번호와 창 위치를 확인할 때"),
                               ex("tile", json!({"rows": 2, "cols": 3, "all_windows": true}), "열린 창 전부를 2×3 바둑판으로"),
                               ex("split", json!({"parts": 3, "direction": "horizontal", "windows": [{"app": "Code"}, {"app": "Chrome"}, {"app": "Windows Terminal"}]}),
                                  "3등분으로 나란히(코드·브라우저·터미널)"),
                               ex("ratio", json!({"ratios": [70, 30], "windows": [{"app": "Code"}, {"app": "메모장"}]}), "왼쪽 70%, 오른쪽 30%"),
                               ex("move_display", json!({"windows": [{"app": "Slack"}], "display": 2}), "슬랙을 2번 디스플레이로"),
                               ex("arrange_apps", json!({"apps": ["Chrome", "메모장"], "arrangement": "columns"}), "두 앱을 나란히"),
                               ex("undo", json!({}), "방금 한 배치를 한 번에 되돌리기")],
                limits: &["창 배치 가젯이 켜져 있어야 새 배치 도구(displays·place·tile·split·ratio·move_display·arrange_apps·undo)가 동작해요. arrange·layout·list_windows는 꺼져 있어도 돼요.",
                          "관리자 권한으로 실행 중인 앱의 창은 Windows가 옮기지 못하게 해요. 그 창은 reason elevated로 알려요.",
                          "한 번의 호출은 창 200개까지예요. 최소화된 창과 실행 중이 아닌 앱의 창은 옮기지 않아요.",
                          "undo는 이 도구들로 한 직전 배치를 기억해요(나초콤보를 끄면 사라져요). 닫힌 창은 건너뛰어요."] },
            "menubar" => PetGadgetGuide { gadget: "menubar",
                summary: "트레이 정리 가젯은 작업 표시줄 알림 영역의 아이콘을 접어 숨겨요(Windows 11). 도구로는 상태 확인, 아이콘 목록, 접기·펼치기·정리를 할 수 있어요.",
                can_do: &["접혀 있는지, 다시 접히는 시간, 늘 보이게 둔 아이콘 수 보기", "알림 영역 아이콘의 프로그램 이름과 숨김 여부 보기", "지금 접기·펼치기·정리"],
                steps: &["먼저 status로 접혀 있는지 봐요.", "어떤 아이콘이 있는지 알고 싶으면 list_items를 불러요.",
                         "정리하려면 tidy, 모두 보이게 하려면 set_collapsed(false)를 불러요."],
                examples: vec![ex("status", json!({}), "접혀 있는지 확인할 때"), ex("list_items", json!({}), "알림 영역에 어떤 프로그램 아이콘이 있는지 볼 때"),
                               ex("tidy", json!({}), "알림 영역을 지금 정리할 때"), ex("set_collapsed", json!({"collapsed": false}), "아이콘을 모두 보여 줄 때")],
                limits: &["Windows 11의 알림 영역 설정을 바꿔요. Windows 10에서는 가젯이 켜지지 않아 gadget_off예요.",
                          "어떤 아이콘을 늘 보이게 둘지는 사용자가 가젯 설정에서 정해요. 아이콘 하나만 숨기거나 보이는 도구는 없어요.",
                          "Windows에는 항상 숨김 구역이 없어서 show_hidden 도구는 없어요. set_collapsed(false)가 모든 아이콘을 보여 줘요.",
                          "아이콘의 툴팁이나 그림은 읽지 않아요. 프로그램 이름만 보내요."] },
            "monitor" => PetGadgetGuide { gadget: "monitor",
                summary: "시스템 모니터 가젯이 재는 PC의 지금 수치(CPU·메모리·네트워크·디스크·배터리·온도·GPU)를 읽어요.",
                can_do: &["CPU 사용률과 프로세서 대기열 보기", "메모리 사용률·메모리 부하·페이지 파일 사용 보기", "네트워크 내려받기·올리기 속도 보기",
                          "시스템 드라이브 사용률 보기", "배터리 잔량·충전 여부·전원 연결 보기", "온도·GPU 사용률 보기(PC가 알려 줄 때만)"],
                steps: &["stats를 한 번 불러 지금 수치를 봐요. 바쁜 프로그램 이름이 담길 수 있어 매번 승인 카드가 떠요('항상 허용'을 켜면 바로 실행).",
                         "숫자가 높으면(CPU 80% 이상, 메모리 부하 높음) 무엇이 부담인지 쉬운 말로 설명해요.",
                         "디스크가 거의 찼으면 디스크 지도(disk_top_items)로 큰 항목을 함께 봐요."],
                examples: vec![ex("stats", json!({}), "\"PC가 왜 이렇게 느려?\"에 답하기 전에 지금 수치를 볼 때")],
                limits: &["시스템 모니터 가젯이 켜져 있어야 해요. 꺼져 있으면 gadget_off를 돌려줘요.",
                          "켠 직후 몇 초는 수치가 없을 수 있어요(no_reading). 잠시 뒤 다시 불러요.",
                          "온도·배터리·GPU는 PC가 알려 주는 경우에만 있어요(없으면 null). 부하 평균은 Windows에 없어요.",
                          "프로그램별 사용량(top_apps)은 모니터의 CPU·메모리 창이 열려 있을 때만 들어 있어요."] },
            "disk" => PetGadgetGuide { gadget: "disk",
                summary: "디스크 지도 가젯이 스캔한 드라이브에서 공간을 많이 차지하는 폴더와 파일을 찾아요. 이 도구들은 아무것도 지우거나 옮기지 않아요.",
                can_do: &["마지막 스캔에서 큰 폴더·파일 상위 N개 보기(최대 50개)", "드라이브를 새로 스캔하기"],
                steps: &["top_items를 먼저 불러요. 스캔이 없으면(no_scan) scan으로 시작해요.", "스캔 중이면(scanning) 1–2분 뒤 다시 top_items를 불러요.",
                         "결과의 큰 항목을 알려 주고, 지울지는 사용자가 디스크 지도 창에서 직접 정하게 해요."],
                examples: vec![ex("top_items", json!({"count": 10}), "디스크를 무엇이 차지하는지 볼 때"), ex("scan", json!({"path": "D:"}), "D 드라이브를 새로 스캔할 때")],
                limits: &["디스크 지도 가젯이 켜져 있어야 해요.", "Windows의 디스크 지도는 드라이브 전체를 읽어요. 폴더를 주면 그 폴더가 있는 드라이브를 스캔해요. 고정 드라이브만 돼요.",
                          "폴더·파일 경로가 PC 밖으로 나가므로 top_items·scan은 매번 승인 카드가 떠요('항상 허용'을 켜면 바로 실행).",
                          "창을 닫고 10분이 지나면 결과가 메모리에서 지워져요.", "삭제 도구는 없어요. 휴지통으로 보내기는 사용자가 디스크 지도 창에서 직접 해요."] },
            "clipboard" => PetGadgetGuide { gadget: "clipboard",
                summary: "클립보드 기록 가젯이 모아 둔 복사 기록을 번호로 다뤄요. 목록에는 내용이 없고, 내용은 사용자 승인을 받은 뒤 한 항목씩만 읽어요.",
                can_do: &["기록 목록 보기(번호·종류·길이·고정, 미리보기 없음)", "N번 항목 내용 읽기(승인)", "N번 항목 다시 복사하기"],
                steps: &["list로 번호와 종류를 확인해요.", "사용자가 고른 항목만 get_item으로 읽어요. 목록의 id를 함께 주면 그사이 목록이 바뀐 경우 list_changed로 멈춰요.",
                         "다시 붙여 넣으려면 copy_again을 불러요. 그러면 번호가 바뀌니 다음 호출 전에 list를 다시 봐요."],
                examples: vec![ex("list", json!({}), "어떤 항목이 있는지 볼 때(내용은 오지 않아요)"),
                               ex("get_item", json!({"index": 2, "id": "3f2a9c1b"}), "사용자가 \"두 번째 거 읽어 줘\"라고 할 때"),
                               ex("copy_again", json!({"index": 3}), "세 번째 항목을 다시 클립보드에 올릴 때")],
                limits: &["클립보드 기록 가젯이 켜져 있어야 해요.", "get_item은 글을 4000자까지만 돌려줘요(truncated). 그림 항목은 글로 읽지 못해요(not_text).",
                          "기록 비우기는 되돌릴 수 없어서 Windows에서는 도구로 열지 않았어요."] },
            "shelf" => PetGadgetGuide { gadget: "shelf",
                summary: "파일 선반 가젯에 잠시 올려 둔 파일·그림·글·링크를 번호로 다뤄요. 선반은 원본 파일을 옮기거나 지우지 않아요.",
                can_do: &["선반 목록 보기(이름·종류·크기, 내용 없음)", "N번 항목을 나초 대화에 첨부하기(승인)"],
                steps: &["list로 번호와 이름을 확인해요.", "사용자가 고른 파일만 give로 첨부해요. 사용자가 보내기를 눌러야 나초에게 가요."],
                examples: vec![ex("list", json!({}), "선반에 무엇이 있는지 볼 때"), ex("give", json!({"index": 1}), "사용자가 \"선반 첫 번째 파일 봐 줘\"라고 할 때")],
                limits: &["파일 선반 가젯이 켜져 있어야 해요.", "give는 나초 대화에서만 쓸 수 있어요.",
                          "선반에서 빼기·비우기는 Windows에서 도구로 열지 않았어요(사용자가 선반에서 직접 해요)."] },
            "capture" => PetGadgetGuide { gadget: "capture",
                summary: "캡처 플로트 가젯으로 화면을 캡처해 나초에게 보여 주고, 화면에 고정해 둔 캡처 창을 세어요.",
                can_do: &["사용자가 드래그한 영역이나 포인터가 있는 화면 전체를 캡처해 나초 대화에 첨부하기(승인)", "고정 캡처 창 개수·크기 보기"],
                steps: &["사용자가 화면의 무엇을 물으면 ask를 불러요. 영역을 정해야 하면 area, 화면 전체면 screen이에요.",
                         "첨부되면 사용자에게 보내기를 눌러 달라고 해요. 다음 메시지에서 그림을 보고 답해요."],
                examples: vec![ex("ask", json!({"mode": "area"}), "\"이 부분 뭐야?\"처럼 화면 일부를 물을 때"), ex("list_pinned", json!({}), "고정 캡처가 몇 개인지 볼 때")],
                limits: &["캡처 플로트 가젯이 켜져 있어야 해요.", "캡처한 그림은 결과에 들어가지 않고 다음 메시지 첨부로만 가요.",
                          "고정 캡처 모두 닫기는 저장하지 않은 캡처를 버리므로 Windows에서는 도구로 열지 않았어요."] },
            "dim" => PetGadgetGuide { gadget: "dim",
                summary: "집중 디밍 가젯은 맨 앞 창만 밝게 두고 나머지 화면을 어둡게 해요. 도구로는 상태와 범위 확인, 잠시 멈춤·다시 시작, 어둡기 바꾸기를 할 수 있어요.",
                can_do: &["어둡게 하는 중인지, 어둡기와 범위(앞 창만·앱 전체, 다른 화면) 보기", "잠시 멈추기와 다시 시작", "어둡기 10~90% 바꾸기"],
                steps: &["status로 멈춤 여부와 어둡기를 봐요.", "어둡기를 바꾸려면 set_strength에 percent(10~90)를 넣어요.",
                         "화면을 잠깐 원래대로 보려면 pause, 다시 어둡게 하려면 resume을 불러요."],
                examples: vec![ex("status", json!({}), "지금 어둡게 하는 중인지 볼 때"), ex("set_strength", json!({"percent": 70}), "더 어둡게 해 달라고 할 때"),
                               ex("pause", json!({}), "잠깐 화면을 원래대로 보고 싶을 때")],
                limits: &["가젯을 켜고 끄는 일은 combo_set_gadget이에요. pause·resume은 켜진 가젯 안에서만 돼요(꺼져 있으면 gadget_off).",
                          "set_strength는 꺼져 있어도 강도만 저장돼요.", "범위(앞 창만·앱 전체)와 다른 화면 설정은 가젯 설정에서 바꿔요. 도구는 읽기만 해요."] },
            "awake" => PetGadgetGuide { gadget: "awake",
                summary: "잠자기 방지 가젯은 PC가 잠들지 않게 붙잡아요. 도구로는 상태 확인, 시간 지정 켜기, 끄기를 할 수 있어요.",
                can_do: &["지금 깨어 있는지, 남은 시간, 이유 보기", "정한 시간(분) 동안 또는 끌 때까지 깨워 두기", "수동 유지 풀기"],
                steps: &["status로 이미 깨어 있는지 봐요.", "켜려면 start에 minutes를 넣어요(0이면 끌 때까지).", "끝나면 stop을 불러요."],
                examples: vec![ex("status", json!({}), "깨어 있는지 확인할 때"), ex("start", json!({"minutes": 60}), "한 시간 동안 PC가 잠들지 않게 할 때"), ex("stop", json!({}), "발표가 끝나서 풀 때")],
                limits: &["가젯이 꺼져 있으면 gadget_off를 돌려줘요.", "배터리 보호와 정한 프로그램 규칙은 가젯 설정이 그대로 적용해요. 도구로 바꾸지 않아요."] },
            "device" => PetGadgetGuide { gadget: "device",
                summary: "기기 제어 가젯은 소리와 마이크, 모니터 밝기를 다뤄요. 도구로는 상태 읽기, 소리 크기·음소거, 마이크 음소거, 밝기(set)를 할 수 있어요.",
                can_do: &["출력 장치 이름, 소리 크기, 마이크 음소거 보기", "소리 크기 0~100과 음소거 바꾸기", "마이크 음소거 켜고 끄기", "DDC/CI 모니터 밝기 바꾸기(set)"],
                steps: &["status로 지금 값을 봐요.", "소리는 set_volume, 마이크는 set_mic_muted를 불러요. 밝기는 combo_set_system에 brightness를 넣어요."],
                examples: vec![ex("status", json!({}), "소리와 마이크 상태를 볼 때"), ex("set_volume", json!({"volume": 30}), "소리를 줄여 달라고 할 때"),
                               ex("set_mic_muted", json!({"muted": true}), "회의 전에 마이크를 끌 때")],
                limits: &["마이크 음소거는 이 PC의 모든 마이크에 함께 적용돼요.", "노트북 내장 화면 밝기와 방해금지는 Windows에서 앱이 바꿀 공개 방법이 없어 실패 이유로 돌아와요.",
                          "status·set_volume·set_mic_muted는 가젯이 꺼져 있으면 gadget_off예요."] },
            "mouse" => PetGadgetGuide { gadget: "mouse",
                summary: "마우스 조정 가젯은 마우스 휠 방향·스크롤 속도와 포인터 속도·정확도를 바꿔요(정밀 터치패드는 그대로). 도구로는 설정 읽기와 바꾸기를 할 수 있어요.",
                can_do: &["휠 방향·스크롤 속도 보기와 바꾸기", "포인터 속도와 '포인터 정확도 향상' 보기와 바꾸기"],
                steps: &["status로 지금 설정을 봐요.", "set에 바꿀 항목만 넣어요. speed·pointer_speed는 choices 중 하나만 돼요."],
                examples: vec![ex("status", json!({}), "지금 마우스 설정을 볼 때"), ex("set", json!({"reverse_scroll": true, "speed": 1.5}), "휠 방향을 반대로 하고 조금 빠르게 할 때"),
                               ex("set", json!({"pointer_speed": 12, "precision": "off"}), "포인터를 조금 빠르게 하고 정확도 향상을 끌 때")],
                limits: &["속도는 0.5·1·1.5·2·3, 포인터 속도는 0(그대로)·4·6·8·10·12·14·16, 정확도는 keep·on·off만 받아요.",
                          "부드러운 스크롤·옆 버튼·가운데 버튼 설정은 Windows판에 없어요.", "가젯을 끄면 사용자가 원래 쓰던 값으로 돌아가요.",
                          "가젯이 꺼져 있으면 gadget_off예요."] },
            "appswitch" => PetGadgetGuide { gadget: "appswitch",
                summary: "앱 전환 가젯은 앱을 열고 전환하고 최소화하고 닫기 요청을 해요. 도구로는 앱 목록, 열기, 전환, 최소화, 닫기와 설정 읽기·바꾸기를 할 수 있어요.",
                can_do: &["실행 중인 앱 목록 보기(list)", "앱 열기·전환·최소화(open·focus·hide)", "앱 닫기 요청(quit, 승인 필요)", "단축키 반복 시 숨김 설정 보기·바꾸기"],
                steps: &["앱 이름이 불확실하면 list를 먼저 불러요.", "열기는 open, 이미 실행 중인 앱으로 전환은 focus, 치우기는 hide를 불러요.",
                         "설정은 settings로 읽고 set_settings로 hide_on_repeat만 바꿔요."],
                examples: vec![ex("list", json!({}), "어떤 앱이 켜져 있는지 볼 때"), ex("focus", json!({"app": "Chrome"}), "크롬으로 전환할 때"),
                               ex("set_settings", json!({"hide_on_repeat": false}), "다시 눌러도 숨기지 않게 할 때")],
                limits: &["탐색기와 나초콤보는 최소화하거나 닫지 않아요. quit은 되돌릴 수 없어서 매번 승인이 필요해요.",
                          "단축키를 새로 만들거나 지우는 일은 도구로 열지 않았어요. 가젯 설정에서 해요.",
                          "settings·set_settings는 가젯이 꺼져 있으면 gadget_off예요. list·open·focus·hide·quit은 꺼져 있어도 동작해요."] },
            "calendar" => PetGadgetGuide { gadget: "calendar",
                summary: "달력 시계 가젯은 작업 표시줄 위에 날짜·시간과 세계 시계를 보여 줘요. 도구로는 표시 설정 읽기와 바꾸기를 할 수 있어요.",
                can_do: &["날짜·요일·초·시간 표기 설정 보기", "세계 시계 시간대 바꾸기(최대 6개)"],
                steps: &["status로 지금 설정과 세계 시계를 봐요.", "set에 바꿀 항목만 넣어요. 시간대는 Asia/Seoul 같은 ID로 넣어요."],
                examples: vec![ex("status", json!({}), "시계에 무엇이 보이는지 볼 때"), ex("set", json!({"show_seconds": true, "hours": "twentyFour"}), "초를 보이고 24시간제로 할 때"),
                               ex("set", json!({"world_clocks": ["Asia/Seoul", "America/New_York"]}), "세계 시계를 서울·뉴욕으로 맞출 때")],
                limits: &["Windows판 달력 시계는 일정을 읽지 않아요. 다음 일정 표시(show_next_event)가 없어요.",
                          "hours는 system·twelve·twentyFour만, 세계 시계는 시계가 아는 시간대 ID 최대 6개만 받아요.", "가젯이 꺼져 있으면 gadget_off예요."] },
            "desktop" => PetGadgetGuide { gadget: "desktop",
                summary: "바탕 화면의 흩어진 파일을 분류해 정리됨 폴더로 옮기고, 열린 창을 보기 좋게 배치하는 걸 도와요. 먼저 계획을 보여 주고, 사용자가 승인한 것만 실행해요.",
                can_do: &["바탕 화면 파일을 스크린샷·이미지·문서·압축·설치 파일·기타로 나눈 이동 계획 보기",
                          "승인받은 계획으로 파일 옮기기(지우기·덮어쓰기 없음)와 한 번에 되돌리기(승인)", "30일 안의 정리 기록 보기",
                          "열린 창으로 배치안 만들기와 승인받은 배치 적용(되돌리기는 window_undo)"],
                steps: &["tidy_plan으로 계획을 만들고 분류별 개수와 파일 이름을 사용자에게 보여 줘요. 이때는 아무것도 옮기지 않아요.",
                         "사용자가 정리를 원하면 같은 plan_id로 tidy_apply를 불러요. 승인 카드가 먼저 떠요. 계획은 10분 안에만 써요(plan_stale이면 다시 계획을 만들어요).",
                         "결과의 skipped(계획 뒤에 바뀐 파일)를 알려 주고, 마음에 안 들면 tidy_undo로 되돌려요(승인 카드가 떠요).",
                         "창 배치는 arrange_suggest로 안을 만들어 보여 주고, 사용자가 원하면 arrange_apply를 불러요. 되돌리기는 window_undo예요."],
                examples: vec![ex("tidy_plan", json!({}), "바탕 화면을 정리하기 전에 계획부터 보여 줄 때"),
                               ex("tidy_apply", json!({"plan_id": "a1b2c3d4"}), "사용자가 계획대로 정리하라고 했을 때(승인 카드가 떠요)"),
                               ex("tidy_undo", json!({}), "방금 정리를 취소하고 싶을 때"), ex("arrange_suggest", json!({}), "열린 창을 배치할 안을 만들 때")],
                limits: &["바탕 화면 바로 아래의 일반 파일만 옮겨요. 폴더·바로 가기(.lnk·.url)·숨김·시스템 파일·정리됨 폴더·받는 중인 파일·최근 2분 안에 바뀐 파일은 건너뛰어요.",
                          "파일은 바탕 화면\\정리됨\\<분류>\\로만 옮기고, 이름이 겹치면 \"이름 2.ext\"처럼 번호를 붙여요. 지우거나 덮어쓰지 않아요.",
                          "바탕 화면 위치는 Windows가 알려 주는 대로 써요(OneDrive 백업 중이면 OneDrive의 바탕 화면).",
                          "되돌릴 때 원래 자리에 다른 파일이 있으면 그대로 두고 \"되돌리지 못함\"으로 알려요.",
                          "tidy_apply·tidy_undo·arrange_apply는 사용자 승인을 매번 받아요('항상 허용'이 없어요). 창 배치는 창 배치 가젯이 켜져 있어야 해요."] },
            _ => return None,
        };
        Some(g)
    }

    fn approval_note(exit: PetGuideExit) -> &'static str {
        match exit {
            PetGuideExit::Connector => "위험도 read·change는 바로 실행돼요. content_out·input_inject는 매번 사용자 승인을 받아요(가젯을 '항상 허용'으로 켠 경우만 제외). \
                destructive는 늘 승인을 받아요. Windows는 PC의 내용이 나가는 도구(시스템 모니터·디스크 지도 포함)도 매번 승인을 받아요.",
            PetGuideExit::Mcp => "MCP 클라이언트는 API 키와 나초콤보의 가젯 스위치(기본 꺼짐)로 막아요. 꺼진 가젯의 도구는 목록에 없고 호출도 거절돼요.",
        }
    }
    // i18n-exempt:end

    /// Markdown (macOS `GadgetGuide.render`). `declared`: the tools this PC offers on that exit, in order.
    pub fn render(gadget: &str, exit: PetGuideExit, declared: &[PetGadgetTool]) -> Option<String> {
        let g = Self::guide(gadget)?;
        let name = |t: &PetGadgetTool| if exit == PetGuideExit::Mcp { t.mcp_name() } else { t.name.to_string() };
        let own: Vec<&PetGadgetTool> = declared.iter().filter(|t| t.gadget == gadget).collect();
        let resolve = |action: &str| own.iter().copied().find(|t| t.action() == action)
            .or_else(|| declared.iter().find(|t| t.mcp_name() == action));
        // i18n-exempt:begin (sent to the server as tool text, not drawn)
        let mut out = format!("# {} ({}) 사용 가이드\n\n{}\n", PetGadgetTools::korean_name(gadget), PetGadgetTools::display_name(gadget), g.summary);
        if !g.can_do.is_empty() { out += &format!("\n## 할 수 있는 일\n{}\n", g.can_do.iter().map(|s| format!("- {s}")).collect::<Vec<_>>().join("\n")); }
        if !own.is_empty() {
            out += "\n## 도구\n";
            for t in &own {
                let d = PetGadgetTools::spec(t.name).map(|s| s.0).unwrap_or_default();
                out += &format!("- `{}` [{}] {d}\n", name(t), t.risk.raw_value());
            }
        }
        if !g.steps.is_empty() {
            out += &format!("\n## 순서\n{}\n", g.steps.iter().enumerate().map(|(n, s)| format!("{}. {s}", n + 1)).collect::<Vec<_>>().join("\n"));
        }
        let shown: Vec<String> = g.examples.iter().filter_map(|e| {
            let t = resolve(e.action)?;
            Some(format!("- {}\n  `{} {}`", e.note, name(t), serde_json::to_string(&e.arguments).unwrap_or_default()))
        }).collect();
        if !shown.is_empty() { out += &format!("\n## 예시 호출\n{}\n", shown.join("\n")); }
        let mut notes: Vec<&str> = g.limits.to_vec();
        notes.push(Self::approval_note(exit));
        out += &format!("\n## 제약·권한\n{}\n", notes.iter().map(|s| format!("- {s}")).collect::<Vec<_>>().join("\n"));
        // i18n-exempt:end
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_windows_gadget_has_a_guide_with_windows_wording() {
        for g in PetGadgetGuides::GADGETS {
            let guide = PetGadgetGuides::guide(g).unwrap_or_else(|| panic!("{g} has no guide"));
            let all: Vec<&str> = [guide.summary].into_iter().chain(guide.can_do.iter().copied()).chain(guide.steps.iter().copied())
                .chain(guide.limits.iter().copied()).collect();
            for s in all {
                assert!(!s.contains('맥') && !s.contains("손쉬운 사용") && !s.contains("Finder") && !s.contains("macOS") && !s.contains('⌘'), "{g}: {s}");
            }
        }
        assert!(PetGadgetGuides::guide("computer").is_none(), "Computer Use is not on Windows yet");
    }

    #[test]
    fn examples_use_declared_tools_only() {
        let declared = PetGadgetTools::declared(true);
        for g in PetGadgetGuides::GADGETS {
            let guide = PetGadgetGuides::guide(g).unwrap();
            let md = PetGadgetGuides::render(g, PetGuideExit::Connector, &declared).unwrap();
            assert!(md.starts_with("# "), "{g}");
            for e in &guide.examples {
                let known = declared.iter().any(|t| (t.gadget == g && t.action() == e.action) || t.mcp_name() == e.action);
                assert!(known, "{g}: example {} is not a declared Windows tool", e.action);
            }
        }
        let md = PetGadgetGuides::render("window", PetGuideExit::Connector, &declared).unwrap();
        assert!(md.contains("`combo_window_tile {"), "connector names in the examples");
        assert!(md.contains("`combo_arrange_window` [change]"), "legacy tools keep their declared names");
        let mcp = PetGadgetGuides::render("window", PetGuideExit::Mcp, &declared).unwrap();
        assert!(mcp.contains("`window_tile {") && mcp.contains("`window_arrange` [change]"));
        let without = PetGadgetGuides::render("shelf", PetGuideExit::Connector, &PetGadgetTools::declared(false)).unwrap();
        assert!(!without.contains("combo_shelf_give"), "a tool this PC does not declare is not offered");
    }
}
