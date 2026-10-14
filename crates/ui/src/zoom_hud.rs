use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClickEvent, Global, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Task, Window, div, point, px,
};

use crate::settings::widgets::{self, ActionTone};
use crate::theme::Theme;
use crate::typography;

const LINGER: Duration = Duration::from_millis(2000);

#[derive(Default)]
struct ZoomHud {
    until: Option<Instant>,
    hovered: bool,
    wake: Option<Task<()>>,
}

impl Global for ZoomHud {}

pub fn show(window: &Window, cx: &mut App) {
    let until = Instant::now() + LINGER;
    let handle = window.window_handle();
    let wake = cx.spawn(async move |cx| {
        cx.background_executor().timer(LINGER).await;
        let _ = handle.update(cx, |_, window, _| window.refresh());
    });
    let hud = cx.default_global::<ZoomHud>();
    hud.until = Some(until);
    hud.wake = Some(wake);
    cx.refresh_windows();
}

fn visible(hud: &ZoomHud, now: Instant) -> bool {
    hud.hovered || hud.until.is_some_and(|until| now < until)
}

fn step(steps: i16) -> impl Fn(&ClickEvent, &mut Window, &mut App) {
    move |_, window, cx| {
        typography::step_zoom(steps, window, cx);
        show(window, cx);
    }
}

pub fn render(window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    if !visible(cx.try_global::<ZoomHud>()?, Instant::now()) {
        return None;
    }
    let theme = Theme::of(cx).for_popup();
    let zoom = typography::zoom(cx);
    let button = |label: &'static str, id: &'static str, aria: &'static str, enabled: bool| {
        widgets::text_action(&theme, ActionTone::Quiet, label)
            .id(id)
            .role(gpui::Role::Button)
            .aria_label(aria)
            .when(!enabled, |button| button.opacity(0.4))
    };
    let card = div()
        .flex()
        .items_center()
        .gap(px(4.0))
        .p(px(4.0))
        .rounded(px(crate::popover::CARD_RADIUS))
        .border_1()
        .border_color(theme.border)
        .bg(crate::popover::surface_bg(&theme))
        .text_color(theme.text)
        .child(
            button("−", "zoom-hud-out", "Zoom out", zoom > typography::ZOOM_MIN)
                .on_click(step(-1)),
        )
        .child(
            div()
                .w(px(52.0))
                .text_center()
                .text_size(typography::ui_rems(13.0))
                .child(format!("{zoom}%")),
        )
        .child(
            button("+", "zoom-hud-in", "Zoom in", zoom < typography::ZOOM_MAX).on_click(step(1)),
        )
        .child(div().w(px(1.0)).h(px(16.0)).bg(theme.border))
        .child(
            button(
                "Reset",
                "zoom-hud-reset",
                "Reset zoom",
                zoom != typography::ZOOM_DEFAULT,
            )
            .on_click(|_, window, cx| {
                typography::set_zoom(typography::ZOOM_DEFAULT, window, cx);
                show(window, cx);
            }),
        );
    let hud = div()
        .id("zoom-hud")
        .debug_selector(|| "zoom-hud".into())
        .role(gpui::Role::Status)
        .aria_label(format!("Zoom {zoom}%"))
        .occlude()
        .on_hover(|hovered, window, cx| {
            cx.default_global::<ZoomHud>().hovered = *hovered;
            show(window, cx);
        })
        .child(crate::frost::frosted(
            crate::popover::CARD_RADIUS,
            crate::frost::MENU_BLUR,
            card,
        ));
    let viewport = window.viewport_size();
    Some(
        gpui::deferred(
            gpui::anchored().position(point(px(0.0), px(0.0))).child(
                div()
                    .w(viewport.width)
                    .h(viewport.height)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(hud),
            ),
        )
        .with_priority(4)
        .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hud_lingers_then_hides_unless_hovered() {
        let now = Instant::now();
        let mut hud = ZoomHud {
            until: Some(now + LINGER),
            ..Default::default()
        };
        assert!(visible(&hud, now));
        assert!(!visible(&hud, now + LINGER));
        hud.hovered = true;
        assert!(visible(&hud, now + LINGER));
        assert!(!visible(&ZoomHud::default(), now));
    }
}
