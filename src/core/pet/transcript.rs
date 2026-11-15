//! What the chat bubble shows. Port of `PetTranscript.swift`, part 1: the shared text helpers (`PetText`). The item list
//! (`PetTranscript`) follows with the session.

pub struct PetText;

impl PetText {
    /// Tool names as the user reads them; the raw name stays in the tooltip.
    pub fn tool_name(raw: &str) -> &'static str {
        match raw {
            "search_web" => "웹 검색",
            "read_page" => "페이지 읽기",
            "browse_web" => "웹 둘러보기",
            "remember" => "기억 떠올리기",
            "read_files" => "자료 읽기",
            "write_files" | "write_file" => "파일 만들기",
            "run_command" => "명령 실행",
            "look_camera" => "사진 보기",
            "delegate" => "다른 나초에게 맡기기",
            "guide" => "안내 준비",
            "stop" => "멈추기",
            "combo_arrange_window" => "창 배치",
            "combo_set_gadget" => "가젯 켜기·끄기",
            "combo_list_gadgets" => "가젯 목록 보기",
            "combo_list_apps" => "앱 목록 보기",
            "combo_open_app" => "앱 열기",
            "combo_focus_app" => "앱 전환",
            "combo_hide_app" => "앱 숨기기",
            "combo_quit_app" => "앱 종료",
            "combo_list_windows" => "창 목록 보기",
            "combo_layout_windows" => "여러 창 배치",
            "combo_set_system" => "소리·밝기·방해금지",
            "combo_set_dim_strength" => "디밍 강도",
            "combo_clear_shelf" => "선반 비우기",
            "combo_workspace_save" => "작업 공간 저장",
            "combo_workspace_apply" => "작업 공간 적용",
            "combo_workspace_list" => "작업 공간 목록",
            _ => "도구",
        }
    }

    /// Under a pending file's name: what it is and what Nacho can do with it ("그림 · 보여요").
    pub fn pending_note(f: &super::media::PetOutgoingFile) -> String {
        use super::media::Readability::*;
        let kind = if f.mime.starts_with("image/") {
            "그림"
        } else if f.mime.starts_with("text/") {
            "글"
        } else if f.mime == "application/pdf" {
            "PDF"
        } else {
            "파일"
        };
        match f.readability() {
            Seen => format!("{kind} · 보여요"),
            Read => format!("{kind} · 읽어요"),
            StoredOnly => format!("{kind} · 전달만 돼요"),
        }
    }

    /// 39000 → "39,000" (always comma groups, whatever the region).
    pub fn number(n: i64) -> String {
        let digits = n.unsigned_abs().to_string();
        let mut out = String::new();
        for (i, ch) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i).is_multiple_of(3) { out.push(','); }
            out.push(ch);
        }
        if n < 0 { format!("-{out}") } else { out }
    }

    /// 39000 원 → "39,000원"; other currencies keep their code: "12,000 USD".
    pub fn amount(n: i64, currency: Option<&str>) -> String {
        match currency {
            Some(c) if !c.is_empty() && !c.eq_ignore_ascii_case("KRW") && c != "원" => format!("{} {c}", Self::number(n)),
            _ => format!("{}원", Self::number(n)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_amounts() {
        assert_eq!(PetText::number(0), "0");
        assert_eq!(PetText::number(999), "999");
        assert_eq!(PetText::number(39000), "39,000");
        assert_eq!(PetText::number(-1234567), "-1,234,567");
        assert_eq!(PetText::amount(39000, None), "39,000원");
        assert_eq!(PetText::amount(39000, Some("krw")), "39,000원");
        assert_eq!(PetText::amount(12000, Some("USD")), "12,000 USD");
        assert_eq!(PetText::tool_name("write_file"), "파일 만들기");
        assert_eq!(PetText::tool_name("brand_new"), "도구");
    }
}
