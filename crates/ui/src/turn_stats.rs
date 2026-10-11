use std::collections::HashMap;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use gpui::{
    App, Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*,
    px,
};
use zeron_doc::{MessagePart, MessageRole, SessionMessageEntry};
use zeron_proto::view::Indicator;

use crate::icons::{self, icon};
use crate::popover;
use crate::state::{AppState, TranscriptTextChanged};
use crate::theme::Theme;

const BYTES_PER_TOKEN: f32 = 4.0;
const MAX_SAMPLE_GAP: Duration = Duration::from_secs(1);
const MIN_ACTIVE: Duration = Duration::from_secs(1);
const TICK: Duration = Duration::from_secs(1);
pub const MIN_COMPOSER_WIDTH: f32 = 600.0;

fn last_prompt(transcript: &[SessionMessageEntry]) -> Option<usize> {
    transcript
        .iter()
        .rposition(|entry| entry.role == MessageRole::User)
}

fn reply(transcript: &[SessionMessageEntry]) -> &[SessionMessageEntry] {
    match last_prompt(transcript) {
        Some(at) => &transcript[at + 1..],
        None => transcript,
    }
}

fn tool_calls(entries: &[SessionMessageEntry]) -> usize {
    entries
        .iter()
        .filter(|entry| entry.role == MessageRole::Assistant)
        .flat_map(|entry| &entry.parts)
        .filter(|part| matches!(part, MessagePart::Tool { .. }))
        .count()
}

fn text_bytes(entries: &[SessionMessageEntry]) -> usize {
    entries
        .iter()
        .filter(|entry| entry.role == MessageRole::Assistant)
        .flat_map(|entry| &entry.parts)
        .map(|part| match part {
            MessagePart::Text { text, .. } | MessagePart::Reasoning { text, .. } => text.len(),
            _ => 0,
        })
        .sum()
}

pub fn totals(transcript: &[SessionMessageEntry]) -> (usize, usize) {
    let turns = transcript
        .iter()
        .filter(|entry| entry.role == MessageRole::User)
        .count();
    (turns, tool_calls(transcript))
}

pub fn reply_text_bytes(transcript: &[SessionMessageEntry]) -> usize {
    text_bytes(reply(transcript))
}

fn working_secs(transcript: &[SessionMessageEntry]) -> i64 {
    transcript
        .iter()
        .filter(|entry| entry.role == MessageRole::Assistant)
        .filter_map(|entry| entry.duration_ms)
        .sum::<i64>()
        / 1000
}

fn format_tokens(tokens: f32) -> String {
    if tokens >= 1_000_000.0 {
        format!("{:.1}M", tokens / 1_000_000.0)
    } else if tokens >= 1_000.0 {
        format!("{:.1}K", tokens / 1_000.0)
    } else {
        format!("{:.0}", tokens.max(0.0))
    }
}

fn format_rate(rate: f32) -> String {
    format!("~{rate:.0} tok/s")
}

pub fn elapsed_secs(
    transcript: &[SessionMessageEntry],
    pending_started: Option<DateTime<Utc>>,
    running: bool,
    now: DateTime<Utc>,
) -> Option<i64> {
    let prompt = last_prompt(transcript).map(|at| transcript[at].created_at);
    let base = prompt
        .into_iter()
        .chain(pending_started.map(|started| started.timestamp_millis()))
        .max()?;
    if running {
        return Some((now.timestamp_millis() - base).max(0) / 1000);
    }
    let end = reply(transcript)
        .iter()
        .rev()
        .find(|entry| entry.role == MessageRole::Assistant)
        .and_then(|entry| Some(entry.created_at + entry.duration_ms?))?;
    (end >= base).then_some((end - base) / 1000)
}

#[derive(Default)]
struct RateMeter {
    turn: String,
    last_bytes: usize,
    last_at: Option<Instant>,
    bytes: usize,
    active: Duration,
}

impl RateMeter {
    fn sample(&mut self, turn: &str, bytes: usize, now: Instant) {
        if self.turn != turn {
            *self = Self {
                turn: turn.to_owned(),
                last_bytes: bytes,
                ..Self::default()
            };
            return;
        }
        if bytes < self.last_bytes {
            self.last_bytes = bytes;
            self.last_at = None;
            return;
        }
        if bytes == self.last_bytes {
            return;
        }
        if let Some(at) = self.last_at {
            let gap = now.saturating_duration_since(at);
            if gap <= MAX_SAMPLE_GAP {
                self.bytes += bytes - self.last_bytes;
                self.active += gap;
            }
        }
        self.last_bytes = bytes;
        self.last_at = Some(now);
    }

    fn tokens_per_sec(&self) -> Option<f32> {
        (self.active >= MIN_ACTIVE)
            .then(|| self.bytes as f32 / BYTES_PER_TOKEN / self.active.as_secs_f32())
    }
}

fn counted(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

struct Details {
    reply_steps: usize,
    reply_tokens: f32,
    session_tokens: f32,
    working_secs: i64,
}

fn row(theme: &Theme, label: &'static str, value: String) -> gpui::Div {
    div()
        .px(px(8.0))
        .h(px(22.0))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(24.0))
        .text_size(px(12.0))
        .child(div().text_color(theme.text_muted).child(label))
        .child(
            div()
                .text_color(theme.text)
                .child(SharedString::from(value)),
        )
}

fn card(
    theme: &Theme,
    turns: usize,
    steps: usize,
    elapsed: Option<i64>,
    rate: Option<f32>,
    details: &Details,
) -> gpui::Div {
    let theme = &theme.for_popup();
    let missing = || "—".to_owned();
    popover::popover_card(theme)
        .w(px(232.0))
        .flex()
        .flex_col()
        .child(popover::menu_heading(theme, "Last prompt"))
        .child(row(
            theme,
            "Elapsed",
            elapsed
                .map(crate::transcript::format_elapsed)
                .unwrap_or_else(missing),
        ))
        .child(row(theme, "Steps", details.reply_steps.to_string()))
        .child(row(
            theme,
            "Speed",
            rate.map(format_rate).unwrap_or_else(missing),
        ))
        .child(row(
            theme,
            "Output",
            format!("~{} tokens", format_tokens(details.reply_tokens)),
        ))
        .child(popover::menu_heading(theme, "Session"))
        .child(row(theme, "Turns", turns.to_string()))
        .child(row(theme, "Steps", steps.to_string()))
        .child(row(
            theme,
            "Working time",
            crate::transcript::format_elapsed(details.working_secs),
        ))
        .child(row(
            theme,
            "Output",
            format!("~{} tokens", format_tokens(details.session_tokens)),
        ))
        .child(
            div()
                .px(px(8.0))
                .pt(px(6.0))
                .pb(px(4.0))
                .text_size(px(10.0))
                .text_color(theme.text_faint)
                .child("Token figures are estimated from streamed text."),
        )
}

pub struct TurnStats {
    state: Entity<AppState>,
    meters: HashMap<String, RateMeter>,
    ticking: bool,
    popup: popover::Popup<()>,
    _state: Subscription,
    _text: Subscription,
}

impl TurnStats {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        Self {
            _state: cx.observe(&state, |stats, _, cx| {
                stats.sample(cx);
                cx.notify();
            }),
            _text: cx.subscribe(&state, |stats, _, _: &TranscriptTextChanged, cx| {
                stats.sample(cx)
            }),
            state,
            meters: HashMap::new(),
            ticking: false,
            popup: popover::Popup::default(),
        }
    }

    fn sample(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let Some(chat_id) = state.selected_chat.clone() else {
            return;
        };
        let turn = last_prompt(&state.transcript)
            .map(|at| state.transcript[at].id.clone())
            .unwrap_or_default();
        let bytes = reply_text_bytes(&state.transcript);
        self.meters
            .entry(chat_id)
            .or_default()
            .sample(&turn, bytes, Instant::now());
    }

    fn running(&self, cx: &App) -> bool {
        let state = self.state.read(cx);
        state.selected_chat.as_deref().is_some_and(|chat_id| {
            matches!(
                state.indicator_for(chat_id, Utc::now()),
                Indicator::Working | Indicator::AwaitingInput
            )
        })
    }

    fn ensure_tick(&mut self, cx: &mut Context<Self>) {
        if self.ticking {
            return;
        }
        self.ticking = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let running = this
                    .update(cx, |stats, cx| {
                        let running = stats.running(cx);
                        stats.ticking = running;
                        cx.notify();
                        running
                    })
                    .unwrap_or(false);
                if !running {
                    break;
                }
            }
        })
        .detach();
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
            popover::reap_popup(cx, |stats: &mut Self| &mut stats.popup);
        }
        cx.notify();
    }
}

impl Render for TurnStats {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let now = Utc::now();
        let open = self.popup.get().is_some();
        let (indicator, turns, steps, elapsed, rate, details) = {
            let state = self.state.read(cx);
            let Some(chat_id) = state.selected_chat.as_deref() else {
                return gpui::Empty.into_any_element();
            };
            let transcript = &state.transcript;
            let indicator = state.indicator_for(chat_id, now);
            let running = matches!(indicator, Indicator::Working | Indicator::AwaitingInput);
            let (turns, steps) = totals(transcript);
            let elapsed = elapsed_secs(
                transcript,
                state.pending_send_started(chat_id, now),
                running,
                now,
            );
            let rate = self.meters.get(chat_id).and_then(RateMeter::tokens_per_sec);
            let details = open.then(|| Details {
                reply_steps: tool_calls(reply(transcript)),
                reply_tokens: reply_text_bytes(transcript) as f32 / BYTES_PER_TOKEN,
                session_tokens: text_bytes(transcript) as f32 / BYTES_PER_TOKEN,
                working_secs: working_secs(transcript),
            });
            (indicator, turns, steps, elapsed, rate, details)
        };
        if turns == 0 {
            return gpui::Empty.into_any_element();
        }
        let running = matches!(indicator, Indicator::Working | Indicator::AwaitingInput);
        if running {
            self.ensure_tick(cx);
        }
        let status = if running {
            Some(
                crate::loaders::mini_mono_spinner(
                    "turn-stats-running",
                    2.0,
                    theme.text_muted,
                    cx.entity_id(),
                    cx,
                )
                .into_any_element(),
            )
        } else if indicator == Indicator::None {
            Some(
                icon(icons::CHECK)
                    .size(px(12.0))
                    .text_color(theme.success)
                    .into_any_element(),
            )
        } else {
            None
        };
        let mut label = format!("{} {}", counted(turns, "turn"), counted(steps, "step"));
        if let Some(rate) = rate {
            label.push_str(&format!(" · {}", format_rate(rate)));
        }
        if elapsed.is_some() {
            label.push_str(" ·");
        }
        let chip = div()
            .id("turn-stats")
            .relative()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(5.0))
            .h(px(24.0))
            .px(px(6.0))
            .rounded(px(6.0))
            .text_size(px(11.0))
            .text_color(theme.text_muted)
            .cursor_pointer()
            .when(open, |el| el.bg(crate::theme::ink(0.05)))
            .hover(|el| el.bg(crate::theme::ink(0.05)))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|stats, _, _, _| stats.popup.note_trigger_press()),
            )
            .on_click(cx.listener(|stats, _, _, cx| stats.toggle(cx)))
            .children(status)
            .child(SharedString::from(label))
            .children(elapsed.map(|secs| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(
                        icon(icons::CLOCK_CIRCLE)
                            .size(px(12.0))
                            .text_color(theme.text_muted),
                    )
                    .child(SharedString::from(crate::transcript::format_elapsed(secs)))
            }));
        let Some(details) = details else {
            return chip.into_any_element();
        };
        let content = card(&theme, turns, steps, elapsed, rate, &details)
            .on_mouse_down_out(cx.listener(|stats, _, _, cx| stats.dismiss(cx)))
            .into_any_element();
        chip.child(popover::anchored_menu_above_end(
            "turn-stats-menu",
            content,
            self.popup.closing_since(),
        ))
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::ToolCall;

    fn entry(
        id: &str,
        role: MessageRole,
        created_at: i64,
        duration_ms: Option<i64>,
        parts: Vec<MessagePart>,
    ) -> SessionMessageEntry {
        SessionMessageEntry {
            id: id.into(),
            role,
            parts,
            created_at,
            device_id: "d".into(),
            status: None,
            continuation_of: None,
            duration_ms,
        }
    }

    fn text(len: usize) -> MessagePart {
        MessagePart::Text {
            id: "t".into(),
            text: "x".repeat(len),
        }
    }

    fn tool() -> MessagePart {
        MessagePart::Tool {
            id: "c".into(),
            call: ToolCall::Todo { items: Vec::new() },
            is_error: false,
            resolved: true,
            output: None,
            diff: None,
            output_ref: None,
            output_bytes: None,
            diff_ref: None,
            diff_stats: None,
            subagent_ref: None,
            subagent_status: None,
            subagent_tail: None,
            duration_ms: None,
        }
    }

    fn transcript() -> Vec<SessionMessageEntry> {
        vec![
            entry("u1", MessageRole::User, 1_000, None, vec![text(5)]),
            entry(
                "a1",
                MessageRole::Assistant,
                1_200,
                Some(4_000),
                vec![text(400), tool(), tool()],
            ),
            entry("u2", MessageRole::User, 10_000, None, vec![text(5)]),
            entry(
                "a2",
                MessageRole::Assistant,
                10_500,
                Some(64_500),
                vec![tool(), text(120)],
            ),
        ]
    }

    #[test]
    fn totals_count_prompts_and_tool_calls_across_the_session() {
        assert_eq!(totals(&transcript()), (2, 3));
        assert_eq!(totals(&[]), (0, 0));
        assert_eq!(tool_calls(reply(&transcript())), 1);
        assert_eq!(working_secs(&transcript()), 68);
        assert_eq!(format_tokens(950.0), "950");
        assert_eq!(format_tokens(45_200.0), "45.2K");
        assert_eq!(format_tokens(2_300_000.0), "2.3M");
    }

    #[test]
    fn elapsed_runs_from_the_last_prompt_until_the_reply_settles() {
        let transcript = transcript();
        let at = |ms| DateTime::<Utc>::from_timestamp_millis(ms).unwrap();
        assert_eq!(
            elapsed_secs(&transcript, None, false, at(999_000)),
            Some(65)
        );
        assert_eq!(elapsed_secs(&transcript, None, true, at(40_000)), Some(30));
        assert_eq!(
            elapsed_secs(&transcript, Some(at(38_000)), true, at(40_000)),
            Some(2)
        );
        assert_eq!(
            elapsed_secs(&transcript[..3], None, false, at(40_000)),
            None
        );
        assert_eq!(elapsed_secs(&[], None, true, at(40_000)), None);
    }

    #[test]
    fn rate_counts_only_continuous_streaming_of_the_current_reply() {
        assert_eq!(reply_text_bytes(&transcript()), 120);
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let mut meter = RateMeter::default();
        meter.sample("u2", 0, at(0));
        meter.sample("u2", 100, at(100));
        assert_eq!(meter.tokens_per_sec(), None);
        meter.sample("u2", 500, at(600));
        meter.sample("u2", 900, at(1_100));
        assert_eq!(meter.tokens_per_sec(), Some(200.0));
        meter.sample("u2", 5_000, at(30_000));
        assert_eq!(meter.tokens_per_sec(), Some(200.0));
        meter.sample("u2", 5_400, at(30_500));
        assert_eq!(meter.tokens_per_sec(), Some(200.0));
        meter.sample("u3", 0, at(31_000));
        assert_eq!(meter.tokens_per_sec(), None);
    }
}
