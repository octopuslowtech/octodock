use gpui::{Context, Entity, IntoElement, Render, Subscription, Window, div, prelude::*};
use zeron_proto::{AgentAccount, AgentAccountsSnapshot, HarnessId};

use crate::popover;
use crate::state::AppState;
use crate::theme::Theme;

const CONTEXT_MENU_ID: &str = "context-usage-menu";

pub fn active_account(
    snapshot: &AgentAccountsSnapshot,
    harness: HarnessId,
) -> Option<&AgentAccount> {
    snapshot
        .accounts
        .iter()
        .find(|account| account.harness == harness && account.active)
}

pub struct AccountUsage {
    state: Entity<AppState>,
    popup: popover::Popup<()>,
    _state: Subscription,
}

impl AccountUsage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        Self {
            _state: cx.observe(&state, |_, _, cx| cx.notify()),
            state,
            popup: popover::Popup::default(),
        }
    }

    fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.popup.take_press_was_open() || self.popup.is_open() {
            self.dismiss(cx);
            return;
        }
        self.popup.open(());
        cx.notify();
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        if self.popup.begin_close() {
            popover::reap_popup(cx, |usage: &mut Self| &mut usage.popup);
        }
        cx.notify();
    }
}

impl Render for AccountUsage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let context = self.state.read(cx).context_usage;
        if !crate::context_usage::has_window(context) {
            return div().into_any_element();
        }
        let chip = crate::context_usage::chip(context, self.popup.get().is_some(), &theme)
            .relative()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|usage, _, _, _| usage.popup.note_trigger_press()),
            )
            .on_click(cx.listener(|usage, _, _, cx| usage.toggle(cx)));
        if self.popup.get().is_none() {
            return chip.into_any_element();
        }
        let content = crate::context_usage::card(context, &Theme::of(cx).for_popup())
            .on_mouse_down_out(cx.listener(|usage, _, _, cx| usage.dismiss(cx)))
            .into_any_element();
        chip.child(popover::anchored_menu_above_end(
            CONTEXT_MENU_ID,
            content,
            self.popup.closing_since(),
        ))
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(harness: HarnessId, active: bool) -> AgentAccount {
        serde_json::from_value(serde_json::json!({
            "id": format!("{harness:?}-{active}"),
            "harness": harness,
            "email": null,
            "planLabel": null,
            "active": active,
            "switchable": true,
            "usageWindows": [],
        }))
        .unwrap()
    }

    #[test]
    fn active_account_is_scoped_to_the_harness() {
        let snapshot = AgentAccountsSnapshot {
            accounts: vec![
                account(HarnessId::ClaudeCode, true),
                account(HarnessId::Codex, false),
                account(HarnessId::Codex, true),
            ],
            warnings: Vec::new(),
        };
        assert_eq!(
            active_account(&snapshot, HarnessId::Codex).map(|a| a.id.as_str()),
            Some("Codex-true")
        );
        assert!(active_account(&snapshot, HarnessId::Cursor).is_none());
    }
}
