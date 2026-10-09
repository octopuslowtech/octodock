use gpui::{
    AnyElement, Context, Entity, IntoElement, Render, SharedString, Subscription, Task, Window,
    div, prelude::*, px,
};
use zeron_proto::{AgentAccount, AgentUsageWindow};

use crate::icons::{self, icon};
use crate::popover;
use crate::settings::accounts::{
    AccountsSnapshotCache, PROVIDERS, format_reset, provider_name, reports_usage, usage_color,
    usage_level,
};
use crate::settings::widgets;
use crate::sidebar_usage::{fetch_local_accounts, meter};
use crate::state::AppState;
use crate::theme::Theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UsagePanelKind {
    Dialog,
    Page,
}

pub struct UsagePanel {
    state: Entity<AppState>,
    kind: UsagePanelKind,
    refreshing: bool,
    load_task: Option<Task<()>>,
    _cache: Subscription,
}

pub fn usage_accounts(accounts: &[AgentAccount]) -> Vec<AgentAccount> {
    let rank = |account: &AgentAccount| {
        PROVIDERS
            .iter()
            .position(|(harness, _, _)| *harness == account.harness)
            .unwrap_or(PROVIDERS.len())
    };
    let mut accounts: Vec<AgentAccount> = accounts
        .iter()
        .filter(|account| reports_usage(account.harness))
        .cloned()
        .collect();
    accounts.sort_by_key(|account| (rank(account), !account.active));
    accounts
}

impl UsagePanel {
    pub fn new(state: Entity<AppState>, kind: UsagePanelKind, cx: &mut Context<Self>) -> Self {
        let mut panel = Self {
            state,
            kind,
            refreshing: false,
            load_task: None,
            _cache: cx.observe_global::<AccountsSnapshotCache>(|_, cx| cx.notify()),
        };
        if !cfg!(test) {
            panel.refresh(cx);
        }
        panel
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.refreshing {
            return;
        }
        let state = self.state.clone();
        self.load_task = fetch_local_accounts(&state, cx, |panel: &mut Self, cx| {
            panel.refreshing = false;
            cx.notify();
        });
        self.refreshing = self.load_task.is_some();
        cx.notify();
    }
}

fn render_window(
    window: &AgentUsageWindow,
    now: chrono::DateTime<chrono::Utc>,
    theme: &Theme,
) -> gpui::Div {
    let fraction = window.used_fraction.clamp(0.0, 1.0);
    let level = usage_level(fraction);
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .text_size(crate::typography::ui_rems(12.0))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.text_muted)
                        .child(SharedString::from(window.label.clone())),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_row()
                        .gap(px(4.0))
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(usage_color(level, theme))
                                .child(SharedString::from(format!(
                                    "{}%",
                                    (fraction * 100.0).round() as u32
                                ))),
                        )
                        .when_some(format_reset(window.resets_at, now), |el, reset| {
                            el.child(
                                div()
                                    .text_color(theme.text_muted)
                                    .child(SharedString::from(format!("· {reset}"))),
                            )
                        }),
                ),
        )
        .child(meter(fraction, theme))
}

fn render_card(
    account: &AgentAccount,
    refreshing: bool,
    now: chrono::DateTime<chrono::Utc>,
    theme: &Theme,
) -> gpui::Div {
    let (mark, tint) = crate::pickers::harness_brand_icon(account.harness);
    let footer = account
        .email
        .clone()
        .or_else(|| account.display_name.clone());
    let missing = account.usage_windows.is_empty().then(|| {
        account.usage_error.clone().unwrap_or_else(|| {
            if refreshing {
                "Checking usage…".to_string()
            } else {
                "Usage unavailable".to_string()
            }
        })
    });
    div()
        .rounded(px(12.0))
        .bg(widgets::block_fill(theme))
        .p(px(16.0))
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(
                    icon(mark)
                        .size(px(14.0))
                        .flex_none()
                        .text_color(tint.unwrap_or(theme.text_muted)),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(crate::typography::ui_rems(13.0))
                        .text_color(theme.text)
                        .child(SharedString::from(provider_name(account.harness))),
                )
                .when_some(account.plan_label.clone(), |el, plan| {
                    el.child(widgets::badge(theme, plan))
                })
                .when(account.active, |el| {
                    el.child(widgets::badge_active(theme, "In use"))
                }),
        )
        .when_some(missing, |el, reason| {
            el.child(
                div()
                    .text_size(crate::typography::ui_rems(12.0))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(reason)),
            )
        })
        .children(
            account
                .usage_windows
                .iter()
                .map(|window| render_window(window, now, theme)),
        )
        .when_some(footer, |el, footer| {
            el.child(
                div()
                    .truncate()
                    .text_size(crate::typography::ui_rems(12.0))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(footer)),
            )
        })
}

impl Render for UsagePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = self.kind == UsagePanelKind::Page;
        let theme = if page {
            Theme::of(cx).for_settings_surface()
        } else {
            Theme::of(cx).for_popup()
        };
        let now = chrono::Utc::now();
        let accounts = cx
            .try_global::<AccountsSnapshotCache>()
            .and_then(|cache| cache.0.get(&None))
            .map(|snapshot| usage_accounts(&snapshot.accounts));

        let refresh_glyph: AnyElement = if self.refreshing {
            crate::loaders::mini_mono_spinner(
                "usage-refreshing",
                1.5,
                theme.text_muted,
                cx.entity_id(),
                cx,
            )
            .into_any_element()
        } else {
            icon(icons::REFRESH)
                .size(px(14.0))
                .text_color(theme.text_muted)
                .into_any_element()
        };
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(12.0))
            .child(if page {
                widgets::page_header(&theme, "Usage", None)
            } else {
                popover::dialog_title(&theme, "Usage")
            })
            .child(
                widgets::ghost_action(&theme)
                    .id("usage-refresh-all")
                    .flex_none()
                    .on_click(cx.listener(|panel, _, _, cx| panel.refresh(cx)))
                    .child(refresh_glyph)
                    .child(SharedString::from("Refresh all")),
            );

        let body = match accounts {
            Some(accounts) if !accounts.is_empty() => {
                div().flex().flex_col().gap(px(12.0)).children(
                    accounts
                        .iter()
                        .map(|account| render_card(account, self.refreshing, now, &theme)),
                )
            }
            other => div()
                .rounded(px(12.0))
                .bg(widgets::block_fill(&theme))
                .p(px(16.0))
                .flex()
                .justify_center()
                .text_size(crate::typography::ui_rems(13.0))
                .text_color(theme.text_muted)
                .child(SharedString::from(if other.is_none() && self.refreshing {
                    "Loading usage…"
                } else {
                    "No usage data"
                })),
        };

        if page {
            div()
                .id("usage-page-scroll")
                .size_full()
                .overflow_y_scroll()
                .child(
                    widgets::page_column()
                        .child(header)
                        .child(div().mt(px(24.0)).child(body)),
                )
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap(px(16.0))
                .child(header)
                .child(body)
                .into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::HarnessId;

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
    fn cards_follow_provider_order_with_the_live_account_first() {
        let ids: Vec<String> = usage_accounts(&[
            account(HarnessId::Codex, false),
            account(HarnessId::Antigravity, true),
            account(HarnessId::Codex, true),
            account(HarnessId::ClaudeCode, true),
        ])
        .into_iter()
        .map(|account| account.id)
        .collect();
        assert_eq!(ids, ["ClaudeCode-true", "Codex-true", "Codex-false"]);
    }
}
