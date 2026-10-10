use super::actions_ui::{
    ACTION_CONTROL_RADIUS, action_chevron, action_control, action_divider, action_segment,
};
use super::*;

use crate::open_targets::{self, OpenTarget, OpenTargetsController};

const OPEN_IN_MENU_WIDTH: f32 = 176.0;

fn target_icon(target: &OpenTarget, size: f32, theme: &Theme) -> gpui::Svg {
    icon(target.icon).size(px(size)).text_color(
        target
            .tint
            .map_or(theme.text_muted, |tint| gpui::rgb(tint).into()),
    )
}

impl Shell {
    fn open_in_workspace_path(&self, cx: &App) -> Option<PathBuf> {
        let state = self.state.read(cx);
        let chat = state.selected_chat_row()?;
        if state.local_device_id.as_deref() != Some(chat.device_id.as_str()) {
            return None;
        }
        let cwd = PathBuf::from(chat.cwd.as_deref()?);
        cwd.is_absolute().then_some(cwd)
    }

    fn ensure_open_targets(&mut self, cx: &mut Context<Self>) {
        let cwd = self.open_in_workspace_path(cx);
        if self.open_targets.cwd == cwd {
            return;
        }
        self.open_targets = OpenTargetsController {
            cwd,
            ..Default::default()
        };
        self.refresh_open_targets(cx);
    }

    fn refresh_open_targets(&mut self, cx: &mut Context<Self>) {
        let Some(cwd) = self.open_targets.cwd.clone() else {
            return;
        };
        let detect = cx.background_spawn({
            let cwd = cwd.clone();
            async move { open_targets::detect(&cwd) }
        });
        self.open_targets.task = Some(cx.spawn(async move |this, cx| {
            let targets = detect.await;
            this.update(cx, |shell, cx| {
                if shell.open_targets.cwd.as_ref() == Some(&cwd) {
                    shell.open_targets.targets = targets;
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn close_open_targets_menu(&mut self, cx: &mut Context<Self>) {
        if self.open_targets.menu.begin_close() {
            popover::reap_popup(cx, |shell: &mut Shell| &mut shell.open_targets.menu);
        }
        cx.notify();
    }

    fn toggle_open_targets_menu(&mut self, cx: &mut Context<Self>) {
        if self.open_targets.menu.take_press_was_open() {
            self.close_open_targets_menu(cx);
            return;
        }
        self.open_targets.menu.open(());
        self.refresh_open_targets(cx);
        cx.notify();
    }

    fn open_workspace_in(&mut self, target: &OpenTarget, cx: &mut Context<Self>) {
        let Some(cwd) = self.open_targets.cwd.clone() else {
            return;
        };
        if let Err(error) = open_targets::open(target, &cwd, cx) {
            tracing::warn!(app = target.id, %error, "failed to open workspace");
        }
        if self.settings.preferred_open_target.as_deref() != Some(target.id) {
            self.settings.preferred_open_target = Some(target.id.to_string());
            self.schedule_save(cx);
        }
        self.close_open_targets_menu(cx);
    }

    pub(super) fn render_open_in_control(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.ensure_open_targets(cx);
        let targets = self.open_targets.targets.clone();
        let preferred = open_targets::preferred(
            &targets,
            self.settings.preferred_open_target.as_deref(),
        )?
        .clone();
        let theme = Theme::of(cx).clone();
        let has_menu = targets.len() > 1;
        let open_label = SharedString::from(format!("Open in {}", preferred.label));
        let primary = preferred.clone();

        let mut control = action_control(&theme).child(
            action_segment(&theme, "open-in-main", true)
                .rounded_l(px(ACTION_CONTROL_RADIUS))
                .when(!has_menu, |el| el.rounded_r(px(ACTION_CONTROL_RADIUS)))
                .cursor_pointer()
                .role(gpui::Role::Button)
                .aria_label(open_label.clone())
                .tooltip(crate::settings::widgets::text_tooltip(open_label))
                .on_click(cx.listener(move |this, _, _, cx| this.open_workspace_in(&primary, cx)))
                .child(target_icon(&preferred, 13.0, &theme)),
        );
        if has_menu {
            control = control.child(action_divider(&theme)).child(
                action_chevron(
                    &theme,
                    "open-in-chevron",
                    "Choose app",
                    true,
                    cx.listener(|this, _, _, cx| this.toggle_open_targets_menu(cx)),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, _| {
                        this.open_targets.menu.note_trigger_press();
                    }),
                ),
            );
        }

        if has_menu && self.open_targets.menu.get().is_some() {
            let mut card = popover::popover_card(&theme)
                .w(px(OPEN_IN_MENU_WIDTH))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_open_targets_menu(cx)));
            for target in targets {
                let row_id = SharedString::from(format!("open-in-row-{}", target.id));
                let selected = target.id == preferred.id;
                let open = target.clone();
                card = card.child(
                    popover::menu_row(&theme, false, row_id.clone())
                        .id(row_id)
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.open_workspace_in(&open, cx)),
                        )
                        .child(target_icon(&target, 15.0, &theme))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(SharedString::from(target.label)),
                        )
                        .when(selected, |row| {
                            row.child(
                                icon(icons::CHECK)
                                    .size(px(13.0))
                                    .text_color(theme.text_muted),
                            )
                        }),
                );
            }
            control = control.child(popover::anchored_menu_below_end(
                "open-in-menu",
                card.into_any_element(),
                self.open_targets.menu.closing_since(),
            ));
        }
        Some(control.into_any_element())
    }
}
