use std::time::{Duration, Instant};

use gpui::{Context, IntoElement, Render, SharedString, Task, Window, div, prelude::*, px};

use crate::icons::{self, icon};
use crate::settings::accounts::{usage_color, usage_level};
use crate::sidebar_usage::meter;
use crate::theme::Theme;
use crate::typography::ui_rems;

const SAMPLE_INTERVAL: Duration = Duration::from_secs(2);
const IGNORED_INTERFACES: [&str; 5] = ["lo", "veth", "docker", "br-", "virbr"];

#[derive(Clone, Copy, Debug, PartialEq)]
struct Counters {
    idle: u64,
    total: u64,
    memory: f32,
    rx: u64,
    tx: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct HostSample {
    cpu: f32,
    memory: f32,
    rx_per_sec: f64,
    tx_per_sec: f64,
}

fn parse_cpu(stat: &str) -> Option<(u64, u64)> {
    let fields = stat
        .lines()
        .next()?
        .strip_prefix("cpu ")?
        .split_ascii_whitespace()
        .take(8)
        .map(|field| field.parse::<u64>().ok())
        .collect::<Option<Vec<_>>>()?;
    let idle = *fields.get(3)? + fields.get(4).copied().unwrap_or(0);
    Some((idle, fields.iter().sum()))
}

fn parse_memory(meminfo: &str) -> Option<f32> {
    let kilobytes = |key: &str| {
        meminfo.lines().find_map(|line| {
            line.strip_prefix(key)?
                .split_ascii_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
        })
    };
    let total = kilobytes("MemTotal:")?;
    let available = kilobytes("MemAvailable:")?;
    (total > 0).then(|| 1.0 - available as f32 / total as f32)
}

fn parse_network(dev: &str) -> (u64, u64) {
    dev.lines()
        .skip(2)
        .filter_map(|line| {
            let (name, counters) = line.split_once(':')?;
            let name = name.trim();
            if IGNORED_INTERFACES
                .iter()
                .any(|prefix| name.starts_with(prefix))
            {
                return None;
            }
            let mut fields = counters
                .split_ascii_whitespace()
                .map(|field| field.parse::<u64>().unwrap_or(0));
            Some((fields.next()?, fields.nth(7)?))
        })
        .fold((0, 0), |(rx, tx), (r, t)| (rx + r, tx + t))
}

fn read_counters() -> Option<Counters> {
    let (idle, total) = parse_cpu(&std::fs::read_to_string("/proc/stat").ok()?)?;
    let memory = parse_memory(&std::fs::read_to_string("/proc/meminfo").ok()?)?;
    let (rx, tx) = parse_network(&std::fs::read_to_string("/proc/net/dev").ok()?);
    Some(Counters {
        idle,
        total,
        memory,
        rx,
        tx,
    })
}

fn rates(previous: Counters, current: Counters, seconds: f64) -> HostSample {
    let seconds = seconds.max(0.001);
    let total = current.total.saturating_sub(previous.total);
    let idle = current.idle.saturating_sub(previous.idle);
    HostSample {
        cpu: if total > 0 {
            1.0 - idle as f32 / total as f32
        } else {
            0.0
        },
        memory: current.memory,
        rx_per_sec: current.rx.saturating_sub(previous.rx) as f64 / seconds,
        tx_per_sec: current.tx.saturating_sub(previous.tx) as f64 / seconds,
    }
}

fn format_percent(fraction: f32) -> String {
    let percent = fraction.clamp(0.0, 1.0) * 100.0;
    if percent < 0.05 {
        "0%".to_string()
    } else if percent < 9.95 {
        format!("{percent:.1}%")
    } else {
        format!("{}%", percent.round())
    }
}

fn format_rate(bytes_per_sec: f64) -> String {
    if bytes_per_sec < 999.5 {
        return format!("{}", bytes_per_sec.max(0.0).round());
    }
    let mut value = bytes_per_sec / 1024.0;
    for unit in ["K", "M", "G"] {
        if value < 9.95 {
            return format!("{value:.1}{unit}");
        }
        if value < 999.5 || unit == "G" {
            return format!("{}{unit}", value.round());
        }
        value /= 1024.0;
    }
    String::new()
}

pub struct SidebarHost {
    sample: Option<HostSample>,
    _poll: Task<()>,
}

impl SidebarHost {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let poll = if cfg!(test) {
            Task::ready(())
        } else {
            cx.spawn(async move |this, cx| {
                let mut previous: Option<(Counters, Instant)> = None;
                while let Some(current) = cx
                    .background_executor()
                    .spawn(async { read_counters() })
                    .await
                {
                    let now = Instant::now();
                    if let Some((before, at)) = previous.replace((current, now)) {
                        let sample = rates(before, current, now.duration_since(at).as_secs_f64());
                        let updated = this.update(cx, |host, cx| {
                            host.sample = Some(sample);
                            cx.notify();
                        });
                        if updated.is_err() {
                            break;
                        }
                    }
                    cx.background_executor().timer(SAMPLE_INTERVAL).await;
                }
            })
        };
        Self {
            sample: None,
            _poll: poll,
        }
    }
}

impl Render for SidebarHost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(sample) = self.sample else {
            return div().into_any_element();
        };
        let theme = Theme::of(cx).clone();
        let rows = [
            ("CPU", sample.cpu, icons::ARROW_DOWN, sample.rx_per_sec),
            ("RAM", sample.memory, icons::ARROW_UP, sample.tx_per_sec),
        ]
        .map(|(label, fraction, arrow, rate)| {
            let fraction = fraction.clamp(0.0, 1.0);
            div()
                .h(px(18.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .child(meter(fraction, &theme).flex_1().min_w(px(12.0)))
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_row()
                        .gap(px(4.0))
                        .child(
                            div()
                                .w(ui_rems(28.0))
                                .flex()
                                .justify_end()
                                .text_color(usage_color(usage_level(fraction), &theme))
                                .child(SharedString::from(format_percent(fraction))),
                        )
                        .child(
                            div()
                                .w(ui_rems(74.0))
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(2.0))
                                .text_color(theme.text_muted)
                                .child(div().flex_1().child(label))
                                .child(icon(arrow).size(px(10.0)).text_color(theme.text_muted))
                                .child(
                                    div()
                                        .w(ui_rems(28.0))
                                        .flex()
                                        .justify_end()
                                        .child(SharedString::from(format_rate(rate))),
                                ),
                        ),
                )
        });
        div()
            .id("sidebar-host-line")
            .py(px(3.0))
            .px(px(Theme::SPACE_SM))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .overflow_hidden()
            .text_size(ui_rems(11.0))
            .tooltip(crate::settings::widgets::text_tooltip_above(format!(
                "CPU {} · Memory {} · Download {}B/s · Upload {}B/s",
                format_percent(sample.cpu),
                format_percent(sample.memory),
                format_rate(sample.rx_per_sec),
                format_rate(sample.tx_per_sec),
            )))
            .child(
                icon(icons::LAPTOP)
                    .size(px(14.0))
                    .flex_none()
                    .text_color(theme.text_muted),
            )
            .child(div().flex_1().min_w_0().flex().flex_col().children(rows))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_files_parse_into_counters() {
        assert_eq!(
            parse_cpu("cpu  100 0 50 800 40 0 0 10 0 0\ncpu0 1 2 3 4"),
            Some((840, 1000))
        );
        assert_eq!(parse_cpu("intr 1 2 3"), None);
        assert_eq!(
            parse_memory("MemTotal: 16000000 kB\nMemFree: 1000000 kB\nMemAvailable: 4000000 kB\n"),
            Some(0.75)
        );
        let dev = "Inter-| Receive | Transmit\n face |bytes packets\n    lo: 500 0 0 0 0 0 0 0 500 0 0 0 0 0 0 0\n  eth0: 1000 0 0 0 0 0 0 0 200 0 0 0 0 0 0 0\ndocker0: 9 0 0 0 0 0 0 0 9 0 0 0 0 0 0 0\n wlan0: 30 0 0 0 0 0 0 0 4 0 0 0 0 0 0 0\n";
        assert_eq!(parse_network(dev), (1030, 204));
    }

    #[test]
    fn rates_divide_counter_deltas_by_elapsed_time() {
        let previous = Counters {
            idle: 100,
            total: 200,
            memory: 0.5,
            rx: 1000,
            tx: 0,
        };
        let current = Counters {
            idle: 150,
            total: 400,
            memory: 0.6,
            rx: 3048,
            tx: 512,
        };
        assert_eq!(
            rates(previous, current, 2.0),
            HostSample {
                cpu: 0.75,
                memory: 0.6,
                rx_per_sec: 1024.0,
                tx_per_sec: 256.0,
            }
        );
        assert_eq!(rates(current, current, 2.0).cpu, 0.0);
    }

    #[test]
    fn labels_stay_within_four_characters() {
        assert_eq!(format_percent(0.0), "0%");
        assert_eq!(format_percent(0.004), "0.4%");
        assert_eq!(format_percent(0.37), "37%");
        assert_eq!(format_percent(1.5), "100%");
        assert_eq!(format_rate(0.0), "0");
        assert_eq!(format_rate(512.0), "512");
        assert_eq!(format_rate(1000.0), "1.0K");
        assert_eq!(format_rate(150.0 * 1024.0), "150K");
        assert_eq!(format_rate(1.2 * 1024.0 * 1024.0), "1.2M");
    }
}
