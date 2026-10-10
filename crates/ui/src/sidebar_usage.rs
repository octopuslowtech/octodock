use std::time::Duration;

use gpui::{
    AnyElement, Context, Entity, IntoElement, Render, SharedString, Subscription, Task, Window,
    div, prelude::*, px,
};
use zeron_proto::{AgentAccountsSnapshot, AgentUsageWindow, HarnessId};
use zeron_rpc::methods;

use crate::account_usage::active_account;
use crate::icons::icon;
use crate::settings::accounts::{
    AccountsSnapshotCache, format_reset, provider_name, usage_color, usage_level,
};
use crate::state::AppState;
use crate::theme::Theme;

const POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);
const ENGINE_RETRY_INTERVAL: Duration = Duration::from_secs(2);
const HARNESSES: [HarnessId; 2] = [HarnessId::ClaudeCode, HarnessId::Codex];

pub(crate) fn fetch_local_accounts<T: 'static>(
    state: &Entity<AppState>,
    cx: &mut Context<T>,
    done: impl FnOnce(&mut T, &mut Context<T>) + 'static,
) -> Option<Task<()>> {
    let engine = state.read(cx).engine().cloned()?;
    let paint_first = cx
        .try_global::<AccountsSnapshotCache>()
        .is_none_or(|cache| !cache.0.contains_key(&None));
    Some(cx.spawn(async move |this, cx| {
        for force_usage in [false, true] {
            if !force_usage && !paint_first {
                continue;
            }
            let snapshot = engine
                .client()
                .call(
                    methods::LIST_AGENT_ACCOUNTS,
                    serde_json::json!({ "forceUsage": force_usage }),
                )
                .await
                .ok()
                .and_then(|value| serde_json::from_value::<AgentAccountsSnapshot>(value).ok());
            if let Some(snapshot) = snapshot {
                this.update(cx, |_, cx| {
                    cx.default_global::<AccountsSnapshotCache>()
                        .0
                        .insert(None, snapshot);
                })
                .ok();
            }
        }
        this.update(cx, done).ok();
    }))
}

pub(crate) fn meter(fraction: f32, theme: &Theme) -> gpui::Div {
    let fill = usage_color(usage_level(fraction), theme).opacity(0.9);
    div()
        .h(px(4.0))
        .rounded_full()
        .overflow_hidden()
        .bg(theme.wash(0.08))
        .when(fraction > 0.0, |el| {
            el.child(
                div()
                    .h_full()
                    .w(gpui::relative(fraction.max(0.015)))
                    .rounded_full()
                    .bg(fill),
            )
        })
}

pub fn countdown(
    resets_at: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<String> {
    let minutes = resets_at?.signed_duration_since(now).num_minutes().max(0);
    let (days, hours, mins) = (minutes / 1440, minutes / 60 % 24, minutes % 60);
    Some(if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else {
        format!("{mins}m")
    })
}

pub struct SidebarUsage {
    state: Entity<AppState>,
    load_task: Option<Task<()>>,
    _poll: Task<()>,
    _cache: Subscription,
}

impl SidebarUsage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let poll = if cfg!(test) {
            Task::ready(())
        } else {
            cx.spawn(async move |this, cx| {
                loop {
                    let Ok(asked) = this.update(cx, |usage, cx| usage.load(cx)) else {
                        break;
                    };
                    cx.background_executor()
                        .timer(if asked {
                            POLL_INTERVAL
                        } else {
                            ENGINE_RETRY_INTERVAL
                        })
                        .await;
                }
            })
        };
        Self {
            state,
            load_task: None,
            _poll: poll,
            _cache: cx.observe_global::<AccountsSnapshotCache>(|_, cx| cx.notify()),
        }
    }

    fn load(&mut self, cx: &mut Context<Self>) -> bool {
        let state = self.state.clone();
        self.load_task = fetch_local_accounts(&state, cx, |_, _| {});
        self.load_task.is_some()
    }
}

fn render_window(
    window: &AgentUsageWindow,
    now: chrono::DateTime<chrono::Utc>,
    theme: &Theme,
) -> AnyElement {
    let fraction = window.used_fraction.clamp(0.0, 1.0);
    let rest = match countdown(window.resets_at, now) {
        Some(left) => format!("used {left}"),
        None => "used".to_string(),
    };
    div()
        .flex_none()
        .flex()
        .flex_row()
        .gap(px(4.0))
        .child(
            div()
                .w(crate::typography::ui_rems(28.0))
                .flex()
                .justify_end()
                .text_color(usage_color(usage_level(fraction), theme))
                .child(SharedString::from(format!(
                    "{}%",
                    (fraction * 100.0).round() as u32
                ))),
        )
        .child(
            div()
                .w(crate::typography::ui_rems(74.0))
                .text_color(theme.text_muted)
                .child(SharedString::from(rest)),
        )
        .into_any_element()
}

impl Render for SidebarUsage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let now = chrono::Utc::now();
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut resets: Vec<String> = Vec::new();
        let snapshot = cx
            .try_global::<AccountsSnapshotCache>()
            .and_then(|cache| cache.0.get(&None));
        for harness in HARNESSES {
            let Some(account) = snapshot
                .and_then(|snapshot| active_account(snapshot, harness))
                .filter(|account| !account.usage_windows.is_empty())
            else {
                continue;
            };
            let (mark, tint) = crate::pickers::harness_brand_icon(harness);
            let mut windows: Vec<AnyElement> = Vec::new();
            for window in account.usage_windows.iter().take(2) {
                windows.push(
                    div()
                        .h(px(18.0))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            meter(window.used_fraction.clamp(0.0, 1.0), &theme)
                                .flex_1()
                                .min_w(px(12.0)),
                        )
                        .child(render_window(window, now, &theme))
                        .into_any_element(),
                );
                resets.push(match format_reset(window.resets_at, now) {
                    Some(reset) => {
                        format!("{} {}: {reset}", provider_name(harness), window.label)
                    }
                    None => format!("{} {}", provider_name(harness), window.label),
                });
            }
            rows.push(
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
                    .child(div().flex_1().min_w_0().flex().flex_col().children(windows))
                    .into_any_element(),
            );
        }
        if rows.is_empty() {
            return div().into_any_element();
        }
        div()
            .id("sidebar-usage-line")
            .py(px(3.0))
            .px(px(Theme::SPACE_SM))
            .rounded(px(8.0))
            .flex()
            .flex_col()
            .overflow_hidden()
            .cursor_pointer()
            .text_size(crate::typography::ui_rems(11.0))
            .hover(|el| el.bg(theme.glass_hover()))
            .tooltip(crate::settings::widgets::text_tooltip_above(
                resets.join(" · "),
            ))
            .children(rows)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdown_shows_the_two_largest_units() {
        let now = chrono::Utc::now();
        let after = |minutes| Some(now + chrono::Duration::minutes(minutes));
        assert_eq!(countdown(after(299), now).as_deref(), Some("4h 59m"));
        assert_eq!(
            countdown(after(5 * 1440 + 6 * 60), now).as_deref(),
            Some("5d 6h")
        );
        assert_eq!(countdown(after(42), now).as_deref(), Some("42m"));
        assert_eq!(countdown(after(-5), now).as_deref(), Some("0m"));
        assert_eq!(countdown(None, now), None);
    }
}
