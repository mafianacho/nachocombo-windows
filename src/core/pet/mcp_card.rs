//! The approval card an MCP client's call waits for (Windows only; NachoCombo MCP, `core::mcp::service`). It is drawn
//! like the server's approval cards in the pet's chat, but it is a local card: its id starts with
//! [`McpCards::PREFIX`], its rows answer through `nachopet://mcpapprove`, number keys never go to it and nothing is
//! sent to nacho-server. The card names the tool, the gadget and the key's label — never the call's arguments.
use super::card_rules::{PetCardRowRole, PetCardStyle};
use super::gadget_tools::{PetGadgetTools, PetToolRisk};
use super::transcript::{PetCardItem, PetCardItemOption, PetLinks, PetText};
use crate::core::mcp::service::McpApprovalRequest;

pub struct McpCards;

impl McpCards {
    pub const PREFIX: &'static str = "mcp-";
    pub const LINK: &'static str = "mcpapprove";

    pub fn card_id(id: u64) -> String { format!("{}{id}", Self::PREFIX) }

    pub fn parse_id(card_id: &str) -> Option<u64> { card_id.strip_prefix(Self::PREFIX)?.parse().ok() }

    pub fn link(id: u64, allow: bool) -> String { PetLinks::url(Self::LINK, &[("id", &id.to_string()), ("a", if allow { "allow" } else { "deny" })]) }

    /// What the risk means for the user, one line.
    fn risk_line(risk: PetToolRisk) -> &'static str {
        match risk {
            PetToolRisk::ContentOut => "이 PC의 내용이 MCP 앱으로 나가요",
            PetToolRisk::InputInject => "MCP 앱이 이 PC에서 입력해요",
            PetToolRisk::Destructive => "되돌릴 수 없는 동작이에요",
            PetToolRisk::Read | PetToolRisk::Change => "이 PC를 바꾸는 동작이에요",
        }
    }

    /// The card. `answering`: the user pressed a row and the answer is on its way (rows stop being clickable).
    pub fn item(r: &McpApprovalRequest, answering: bool) -> PetCardItem {
        // A tool without a Korean name shows its MCP name rather than a bare "도구".
        let tool = match PetText::humanize_action_names(&r.connector) { t if t == PetText::tool_name("") => r.tool.clone(), t => t };
        let gadget = PetGadgetTools::korean_name(&r.gadget);
        let open = !answering;
        PetCardItem {
            id: Self::card_id(r.id),
            symbol: "checkmark.shield".into(),
            title: format!("MCP 앱이 '{tool}' 도구를 쓰려고 해요"),
            detail: None,
            options: vec![
                PetCardItemOption::new("허용", open.then(|| Self::link(r.id, true)), false, PetCardRowRole::Primary),
                PetCardItemOption::new("거절", open.then(|| Self::link(r.id, false)), false, PetCardRowRole::Quiet),
            ],
            status: Some(if answering { "보내는 중…".into() } else { "60초 안에 답하지 않으면 거절돼요".into() }),
            status_waits: open,
            style: PetCardStyle::Approval,
            summary: vec![format!("가젯: {gadget}"), format!("키: {}", r.key_label), Self::risk_line(r.risk).into()],
            confirm: None,
            hint: None,
            retry: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> McpApprovalRequest {
        McpApprovalRequest { id: 7, tool: "clipboard_get_item".into(), connector: "combo_clipboard_get_item".into(), gadget: "clipboard".into(), risk: PetToolRisk::ContentOut, key_label: "Claude Desktop".into() }
    }

    #[test]
    fn the_card_names_tool_gadget_and_key_and_answers_locally() {
        let c = McpCards::item(&req(), false);
        assert_eq!(c.id, "mcp-7");
        assert_eq!(McpCards::parse_id(&c.id), Some(7));
        assert!(!c.title.contains("combo_"), "the raw name is not shown: {}", c.title);
        assert!(c.title.contains("클립보드"), "{}", c.title);
        assert_eq!(c.style, PetCardStyle::Approval);
        assert_eq!(c.summary[0], "가젯: 클립보드 기록");
        assert_eq!(c.summary[1], "키: Claude Desktop");
        assert_eq!(c.options[0].label, "허용");
        assert_eq!(c.options[0].link.as_deref(), Some("nachopet://mcpapprove?a=allow&id=7"));
        assert_eq!(c.options[1].link.as_deref(), Some("nachopet://mcpapprove?a=deny&id=7"));
        assert!(c.status_waits);
    }

    #[test]
    fn rows_close_while_the_answer_goes_out() {
        let c = McpCards::item(&req(), true);
        assert!(c.options.iter().all(|o| o.link.is_none()));
        assert!(!c.status_waits);
        assert_eq!(McpCards::parse_id("plan-7"), None);
        assert_eq!(McpCards::parse_id("mcp-x"), None);
    }

    #[test]
    fn destructive_says_so() {
        let mut r = req();
        r.risk = PetToolRisk::Destructive;
        r.connector = "combo_quit_app".into();
        r.gadget = "appswitch".into();
        let c = McpCards::item(&r, false);
        assert!(c.title.contains("앱 종료"), "{}", c.title);
        assert_eq!(c.summary[2], "되돌릴 수 없는 동작이에요");
    }
}
