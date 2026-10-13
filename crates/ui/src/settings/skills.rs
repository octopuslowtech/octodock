use std::collections::BTreeSet;

use gpui::{
    AnyElement, Context, Entity, SharedString, Subscription, Task, Window, div, prelude::*, px,
};
use zeron_proto::HarnessId;
use zeron_proto::skill_library::{
    LibrarySkill, SkillDeploymentState, SkillLibraryOp, SkillLibraryView, SkillPreset, SkillSource,
    SkillSyncMode, UnmanagedSkill,
};
use zeron_rpc::methods;

use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::popover::{self, Loadable};
use crate::settings::widgets::{self, ActionTone};
use crate::state::AppState;
use crate::theme::Theme;

const MODES: [(SkillSyncMode, &str); 2] = [
    (SkillSyncMode::Symlink, "Link"),
    (SkillSyncMode::Copy, "Copy"),
];

fn target_label(harness: Option<HarnessId>) -> &'static str {
    let Some(harness) = harness else {
        return "Shared";
    };
    crate::settings::SKILL_COMPLETION_HARNESSES
        .iter()
        .find(|(id, _)| *id == harness)
        .map_or("Agent", |(_, name)| *name)
}

fn install_op(text: &str) -> Option<SkillLibraryOp> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let bytes = text.as_bytes();
    let drive = bytes.len() > 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    Some(if drive || text.starts_with(['/', '~', '\\']) {
        SkillLibraryOp::InstallPath { path: text.into() }
    } else {
        SkillLibraryOp::InstallGit {
            source: text.into(),
        }
    })
}

fn request_params(op: Option<&SkillLibraryOp>, scope: Option<&str>) -> serde_json::Value {
    let mut params = op
        .and_then(|op| serde_json::to_value(op).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if let (Some(scope), Some(object)) = (scope, params.as_object_mut()) {
        object.insert("spaceId".into(), scope.into());
    }
    params
}

fn source_label(source: &SkillSource) -> String {
    match source {
        SkillSource::Local { path } | SkillSource::Archive { path } => path.clone(),
        SkillSource::Git { url, subpath, .. } => {
            let repository = url.trim_start_matches("https://").trim_end_matches(".git");
            match subpath {
                Some(subpath) => format!("{repository}/{subpath}"),
                None => repository.to_owned(),
            }
        }
    }
}

fn deployment_state(
    skill: &LibrarySkill,
    harness: Option<HarnessId>,
) -> Option<SkillDeploymentState> {
    skill
        .deployments
        .iter()
        .find(|deployment| deployment.harness == harness)
        .map(|deployment| deployment.state)
}

fn preset_coverage(
    preset: &SkillPreset,
    skills: &[LibrarySkill],
    harness: Option<HarnessId>,
) -> (usize, usize) {
    let members = skills
        .iter()
        .filter(|skill| preset.skills.contains(&skill.id));
    members.fold((0, 0), |(deployed, total), skill| {
        let on = deployment_state(skill, harness) == Some(SkillDeploymentState::Deployed);
        (deployed + usize::from(on), total + 1)
    })
}

fn visible_targets(view: &SkillLibraryView) -> Vec<Option<HarnessId>> {
    view.targets
        .iter()
        .filter(|target| {
            target.available
                || view
                    .skills
                    .iter()
                    .any(|skill| deployment_state(skill, target.harness).is_some())
        })
        .map(|target| target.harness)
        .collect()
}

enum Badge {
    Off,
    On,
    Partial(usize, usize),
    Problem(&'static str),
}

struct PresetDraft {
    id: Option<String>,
    input: Entity<ComposerInput>,
    skills: BTreeSet<String>,
    _events: Subscription,
}

pub struct SkillsPage {
    state: Entity<AppState>,
    scroll: widgets::PageScroll,
    library: Loadable<SkillLibraryView>,
    scope: Option<String>,
    scope_select: widgets::SelectState,
    mode_select: widgets::SelectState,
    source: Entity<ComposerInput>,
    busy: Option<&'static str>,
    confirm_remove: Option<String>,
    preset: Option<PresetDraft>,
    error: Option<String>,
    task: Option<Task<()>>,
    _source: Subscription,
    _state: Subscription,
}

impl SkillsPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let source = cx.new(|cx| ComposerInput::new("Folder, archive, Git URL or owner/repo", cx));
        let source_events = cx.subscribe(&source, |page: &mut Self, _, event, cx| {
            if matches!(event, ComposerInputEvent::Submitted) {
                page.install_typed(cx);
            }
        });
        let state_events = cx.observe(&state, |page: &mut Self, _, cx| {
            if matches!(page.library, Loadable::Idle) {
                page.run(None, "Loading…", cx);
            }
        });
        let mut page = Self {
            state,
            scroll: widgets::PageScroll::default(),
            library: Loadable::Idle,
            scope: None,
            scope_select: widgets::SelectState::default(),
            mode_select: widgets::SelectState::default(),
            source,
            busy: None,
            confirm_remove: None,
            preset: None,
            error: None,
            task: None,
            _source: source_events,
            _state: state_events,
        };
        page.run(None, "Loading…", cx);
        page
    }

    pub(crate) fn dismiss_on_escape(&mut self, cx: &mut Context<Self>) -> bool {
        if self.preset.take().is_none() && self.confirm_remove.take().is_none() {
            return false;
        }
        cx.notify();
        true
    }

    fn run(&mut self, op: Option<SkillLibraryOp>, busy: &'static str, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let mutation = op.is_some();
        let method = if mutation {
            methods::MUTATE_SKILL_LIBRARY
        } else {
            methods::LIST_SKILL_LIBRARY
        };
        let params = request_params(op.as_ref(), self.scope.as_deref());
        self.busy = Some(busy);
        self.confirm_remove = None;
        if !matches!(self.library, Loadable::Ready(_)) {
            self.library = Loadable::Loading;
        }
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(method, params)
                .await
                .map_err(|error| error.to_string())
                .and_then(|value| {
                    serde_json::from_value::<SkillLibraryView>(value)
                        .map_err(|error| error.to_string())
                });
            this.update(cx, |page, cx| {
                page.busy = None;
                match result {
                    Ok(view) => page.library = Loadable::Ready(view),
                    Err(error) if matches!(page.library, Loadable::Ready(_)) => {
                        page.error = Some(error);
                        if mutation {
                            page.run(None, "Refreshing…", cx);
                        }
                    }
                    Err(error) => page.library = Loadable::Error(error),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn set_scope(&mut self, scope: Option<String>, cx: &mut Context<Self>) {
        if self.scope == scope {
            return;
        }
        self.scope = scope;
        self.busy = None;
        self.task = None;
        self.library = Loadable::Idle;
        self.scroll.reset();
        self.run(None, "Loading…", cx);
    }

    fn install_typed(&mut self, cx: &mut Context<Self>) {
        let Some(op) = install_op(self.source.read(cx).text()) else {
            return;
        };
        if self.busy.is_some() {
            return;
        }
        self.source.update(cx, |input, cx| input.set_text("", cx));
        self.run(Some(op), "Adding…", cx);
    }

    fn browse(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: true,
            multiple: false,
            prompt: Some("Choose Skill Folder or Archive".into()),
        });
        cx.spawn(async move |this, cx| {
            let path = match receiver.await {
                Ok(Ok(Some(mut paths))) => paths.pop(),
                _ => None,
            };
            let Some(path) = path else {
                return;
            };
            this.update(cx, |page, cx| {
                let op = SkillLibraryOp::InstallPath {
                    path: path.to_string_lossy().into_owned(),
                };
                page.run(Some(op), "Adding…", cx);
            })
            .ok();
        })
        .detach();
    }

    fn edit_preset(&mut self, preset: Option<&SkillPreset>, cx: &mut Context<Self>) {
        let input = cx.new(|cx| ComposerInput::new("Preset name", cx));
        if let Some(preset) = preset {
            input.update(cx, |input, cx| input.set_text(preset.name.clone(), cx));
        }
        let events = cx.subscribe(&input, |page: &mut Self, _, event, cx| {
            if matches!(event, ComposerInputEvent::Submitted) {
                page.save_preset(cx);
            }
        });
        self.preset = Some(PresetDraft {
            id: preset.map(|preset| preset.id.clone()),
            input,
            skills: preset
                .map(|preset| preset.skills.iter().cloned().collect())
                .unwrap_or_default(),
            _events: events,
        });
        cx.notify();
    }

    fn save_preset(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = &self.preset else {
            return;
        };
        let name = draft.input.read(cx).text().trim().to_owned();
        if name.is_empty() || self.busy.is_some() {
            return;
        }
        let op = SkillLibraryOp::SavePreset {
            id: draft.id.clone(),
            name,
            skills: draft.skills.iter().cloned().collect(),
        };
        self.preset = None;
        self.run(Some(op), "Saving…", cx);
    }

    fn toggle_draft_skill(&mut self, skill: &str, cx: &mut Context<Self>) {
        if let Some(draft) = &mut self.preset
            && !draft.skills.remove(skill)
        {
            draft.skills.insert(skill.to_owned());
        }
        cx.notify();
    }

    fn remove(&mut self, skill: String, cx: &mut Context<Self>) {
        if self.confirm_remove.as_deref() == Some(skill.as_str()) {
            self.run(Some(SkillLibraryOp::Remove { skill }), "Removing…", cx);
        } else {
            self.confirm_remove = Some(skill);
            cx.notify();
        }
    }

    fn scopes(&self, cx: &Context<Self>) -> Vec<(Option<String>, SharedString)> {
        let state = self.state.read(cx);
        let local = state.local_device_id.as_deref();
        std::iter::once((None, SharedString::from("All projects")))
            .chain(
                state
                    .spaces
                    .iter()
                    .filter(|space| Some(space.device_id.as_str()) == local)
                    .map(|space| {
                        (
                            Some(space.id.clone()),
                            SharedString::from(space.display_name().to_owned()),
                        )
                    }),
            )
            .collect()
    }

    fn action(
        theme: &Theme,
        tone: ActionTone,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
    ) -> gpui::Stateful<gpui::Div> {
        let label = label.into();
        let accent = theme.accent;
        widgets::text_action(theme, tone, label.clone())
            .id(id.into())
            .flex_none()
            .tab_index(0)
            .role(gpui::Role::Button)
            .aria_label(label)
            .focus_visible(move |style| style.border_2().border_color(accent))
    }

    #[allow(clippy::too_many_arguments)]
    fn target_badge(
        &self,
        id: String,
        harness: Option<HarnessId>,
        badge: Badge,
        subject: &str,
        op: SkillLibraryOp,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = target_label(harness);
        let (icon_path, tint) = match harness {
            Some(harness) => crate::pickers::harness_brand_icon(harness),
            None => (crate::icons::GLOBAL, None),
        };
        let on = matches!(badge, Badge::On);
        let color = match badge {
            Badge::On => theme.text,
            Badge::Problem(_) => theme.warning_muted,
            Badge::Off | Badge::Partial(..) => theme.text_muted,
        };
        let (aria, trailing): (String, Option<AnyElement>) = match badge {
            Badge::Off => (format!("Add {subject} to {label}"), None),
            Badge::On => (
                format!("Remove {subject} from {label}"),
                Some(
                    crate::icons::icon(crate::icons::CHECK)
                        .size(px(11.0))
                        .text_color(theme.success_muted)
                        .into_any_element(),
                ),
            ),
            Badge::Partial(deployed, total) => (
                format!("Add {subject} to {label}, {deployed} of {total} added"),
                Some(
                    div()
                        .child(format!("{deployed}/{total}"))
                        .into_any_element(),
                ),
            ),
            Badge::Problem(problem) => (
                format!("Fix {subject} in {label}, {problem}"),
                Some(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            crate::icons::icon(crate::icons::DANGER_TRIANGLE)
                                .size(px(11.0))
                                .text_color(color),
                        )
                        .child(problem)
                        .into_any_element(),
                ),
            ),
        };
        let accent = theme.accent;
        let hover = theme.glass_hover();
        let text = theme.text;
        div()
            .id(SharedString::from(id))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(5.0))
            .min_h(px(26.0))
            .px(px(9.0))
            .rounded_full()
            .border_1()
            .border_color(if on {
                theme.border_strong
            } else {
                theme.border
            })
            .when(on, |el| el.bg(theme.wash(0.10)))
            .text_size(crate::typography::ui_rems(11.5))
            .text_color(color)
            .cursor_pointer()
            .hover(move |style| style.bg(hover).text_color(text))
            .tab_index(0)
            .role(gpui::Role::Button)
            .aria_label(aria)
            .focus_visible(move |style| style.border_2().border_color(accent))
            .on_click(cx.listener(move |page, _, _, cx| {
                page.run(Some(op.clone()), "Updating…", cx);
            }))
            .child(
                crate::icons::icon(icon_path)
                    .size(px(12.0))
                    .text_color(tint.unwrap_or(color)),
            )
            .child(label)
            .children(trailing)
            .into_any_element()
    }

    fn badge_row(badges: Vec<AnyElement>) -> gpui::Div {
        div()
            .mt(px(8.0))
            .flex()
            .flex_row()
            .flex_wrap()
            .gap(px(6.0))
            .children(badges)
    }

    fn skill_row(
        &self,
        ix: usize,
        skill: &LibrarySkill,
        targets: &[Option<HarnessId>],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut meta: Vec<AnyElement> = Vec::new();
        if !skill.description.is_empty() {
            meta.push(div().child(skill.description.clone()).into_any_element());
        }
        meta.push(
            div()
                .truncate()
                .child(source_label(&skill.source))
                .into_any_element(),
        );
        let badges = targets
            .iter()
            .map(|harness| {
                let deploy = SkillLibraryOp::Deploy {
                    skill: skill.id.clone(),
                    harness: *harness,
                };
                let undeploy = SkillLibraryOp::Undeploy {
                    skill: skill.id.clone(),
                    harness: *harness,
                };
                let (badge, op) = match deployment_state(skill, *harness) {
                    Some(SkillDeploymentState::Deployed) => (Badge::On, undeploy),
                    Some(SkillDeploymentState::Missing) => (Badge::Problem("missing"), deploy),
                    Some(SkillDeploymentState::Conflict) => (Badge::Problem("replaced"), undeploy),
                    None => (Badge::Off, deploy),
                };
                self.target_badge(
                    format!("skill-target-{ix}-{}", target_label(*harness)),
                    *harness,
                    badge,
                    &skill.name,
                    op,
                    theme,
                    cx,
                )
            })
            .collect();
        let controls: AnyElement = match &self.preset {
            Some(draft) => {
                let included = draft.skills.contains(&skill.id);
                let id = skill.id.clone();
                let accent = theme.accent;
                widgets::toggle_switch(theme, included, format!("skill-preset-{}", skill.id))
                    .id(("skill-preset-toggle", ix))
                    .cursor_pointer()
                    .tab_index(0)
                    .role(gpui::Role::Switch)
                    .aria_label(format!("Include {} in the preset", skill.name))
                    .aria_toggled(if included {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .focus_visible(move |style| style.border_2().border_color(accent))
                    .on_click(cx.listener(move |page, _, _, cx| {
                        page.toggle_draft_skill(&id, cx);
                    }))
                    .into_any_element()
            }
            None => {
                let confirming = self.confirm_remove.as_deref() == Some(skill.id.as_str());
                let update_id = skill.id.clone();
                let remove_id = skill.id.clone();
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .when(skill.update_available, |el| {
                        el.child(
                            Self::action(
                                theme,
                                ActionTone::Outlined,
                                format!("skill-update-{ix}"),
                                "Update",
                            )
                            .on_click(cx.listener(
                                move |page, _, _, cx| {
                                    let op = SkillLibraryOp::Update {
                                        skill: update_id.clone(),
                                    };
                                    page.run(Some(op), "Updating…", cx);
                                },
                            )),
                        )
                    })
                    .child(
                        Self::action(
                            theme,
                            ActionTone::Quiet,
                            format!("skill-remove-{ix}"),
                            if confirming {
                                "Remove from library?"
                            } else {
                                "Remove"
                            },
                        )
                        .when(confirming, |el| el.text_color(theme.danger_muted))
                        .on_click(cx.listener(move |page, _, _, cx| {
                            page.remove(remove_id.clone(), cx);
                        })),
                    )
                    .into_any_element()
            }
        };
        widgets::card_row(theme, ix == 0)
            .child(
                div()
                    .flex_1()
                    .min_w(px(220.0))
                    .flex()
                    .flex_col()
                    .child(widgets::row_title(theme, skill.name.clone()))
                    .child(widgets::meta_line(theme, meta))
                    .child(Self::badge_row(badges)),
            )
            .child(controls)
            .into_any_element()
    }

    fn preset_row(
        &self,
        ix: usize,
        preset: &SkillPreset,
        view: &SkillLibraryView,
        targets: &[Option<HarnessId>],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let badges = targets
            .iter()
            .map(|harness| {
                let (deployed, total) = preset_coverage(preset, &view.skills, *harness);
                let complete = total > 0 && deployed == total;
                let badge = if complete {
                    Badge::On
                } else if deployed > 0 {
                    Badge::Partial(deployed, total)
                } else {
                    Badge::Off
                };
                self.target_badge(
                    format!("preset-target-{ix}-{}", target_label(*harness)),
                    *harness,
                    badge,
                    &preset.name,
                    SkillLibraryOp::ApplyPreset {
                        id: preset.id.clone(),
                        harness: *harness,
                        enabled: !complete,
                    },
                    theme,
                    cx,
                )
            })
            .collect();
        let count = preset.skills.len();
        let summary = if count == 1 {
            "1 skill".to_owned()
        } else {
            format!("{count} skills")
        };
        let edited = preset.clone();
        let deleted = preset.id.clone();
        widgets::card_row(theme, ix == 0)
            .child(
                div()
                    .flex_1()
                    .min_w(px(220.0))
                    .flex()
                    .flex_col()
                    .child(widgets::row_title(theme, preset.name.clone()))
                    .child(widgets::meta_line(
                        theme,
                        vec![div().child(summary).into_any_element()],
                    ))
                    .child(Self::badge_row(badges)),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .child(
                        Self::action(
                            theme,
                            ActionTone::Quiet,
                            format!("preset-edit-{ix}"),
                            "Edit",
                        )
                        .on_click(cx.listener(move |page, _, _, cx| {
                            page.edit_preset(Some(&edited), cx);
                        })),
                    )
                    .child(
                        Self::action(
                            theme,
                            ActionTone::Quiet,
                            format!("preset-delete-{ix}"),
                            "Delete",
                        )
                        .on_click(cx.listener(move |page, _, _, cx| {
                            let op = SkillLibraryOp::DeletePreset {
                                id: deleted.clone(),
                            };
                            page.run(Some(op), "Deleting…", cx);
                        })),
                    ),
            )
            .into_any_element()
    }

    fn unmanaged_row(
        &self,
        ix: usize,
        skill: &UnmanagedSkill,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let path = skill.path.clone();
        widgets::card_row(theme, ix == 0)
            .child(
                div()
                    .flex_1()
                    .min_w(px(220.0))
                    .flex()
                    .flex_col()
                    .child(widgets::row_title(theme, skill.name.clone()))
                    .child(widgets::meta_line(
                        theme,
                        vec![
                            div().child(target_label(skill.harness)).into_any_element(),
                            div()
                                .truncate()
                                .child(skill.path.clone())
                                .into_any_element(),
                        ],
                    )),
            )
            .child(
                Self::action(
                    theme,
                    ActionTone::Outlined,
                    format!("skill-adopt-{ix}"),
                    "Add to library",
                )
                .aria_label(format!("Add {} to the library", skill.name))
                .on_click(cx.listener(move |page, _, _, cx| {
                    let op = SkillLibraryOp::Adopt { path: path.clone() };
                    page.run(Some(op), "Adding…", cx);
                })),
            )
            .into_any_element()
    }

    fn empty_row(theme: &Theme, copy: &'static str) -> gpui::Div {
        widgets::card_row(theme, true).child(
            div()
                .text_size(crate::typography::ui_rems(widgets::ROW_DESCRIPTION_SIZE))
                .text_color(theme.text_muted)
                .child(copy),
        )
    }

    fn setting_row(
        theme: &Theme,
        first: bool,
        title: &'static str,
        description: &'static str,
        control: impl IntoElement,
    ) -> gpui::Div {
        widgets::card_row(theme, first)
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.0))
                    .child(widgets::row_title(theme, title))
                    .child(widgets::meta_line(
                        theme,
                        vec![div().child(description).into_any_element()],
                    )),
            )
            .child(control)
    }

    fn render_library(
        &self,
        view: &SkillLibraryView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let targets = visible_targets(view);

        let add_card = widgets::section_card(theme).mt(px(0.0)).child(
            widgets::card_row(theme, true)
                .child(div().flex_1().min_w(px(240.0)).child(popover::dialog_field(
                    self.source.clone().into_any_element(),
                )))
                .child(
                    Self::action(theme, ActionTone::Solid, "skill-add", "Add")
                        .aria_label("Add skill")
                        .on_click(cx.listener(|page, _, _, cx| page.install_typed(cx))),
                )
                .child(
                    Self::action(theme, ActionTone::Outlined, "skill-browse", "Browse…")
                        .on_click(cx.listener(|page, _, _, cx| page.browse(cx))),
                ),
        );

        let scopes = self.scopes(cx);
        let scope_ids: Vec<Option<String>> = scopes.iter().map(|(id, _)| id.clone()).collect();
        let scope_control =
            widgets::select("skills-scope", "Deploy scope", theme, |page: &mut Self| {
                &mut page.scope_select
            })
            .options(
                scopes
                    .iter()
                    .map(|(_, name)| widgets::SelectOption::new(name.clone())),
                scope_ids
                    .iter()
                    .position(|id| *id == self.scope)
                    .unwrap_or_default(),
            )
            .width(200.0)
            .on_select(move |page, ix, _, cx| page.set_scope(scope_ids[ix].clone(), cx))
            .render(&self.scope_select, cx);
        let mode_control =
            widgets::select("skills-mode", "Deploy method", theme, |page: &mut Self| {
                &mut page.mode_select
            })
            .options(
                MODES
                    .iter()
                    .map(|(_, label)| widgets::SelectOption::new(*label)),
                MODES
                    .iter()
                    .position(|(mode, _)| *mode == view.mode)
                    .unwrap_or_default(),
            )
            .width(200.0)
            .on_select(|page, ix, _, cx| {
                let op = SkillLibraryOp::SetMode { mode: MODES[ix].0 };
                page.run(Some(op), "Saving…", cx);
            })
            .render(&self.mode_select, cx);
        let deploy_card = widgets::section_card(theme)
            .mt(px(0.0))
            .child(Self::setting_row(
                theme,
                true,
                "Scope",
                "Agent folders in your home directory, or one project's folders.",
                scope_control,
            ))
            .child(Self::setting_row(
                theme,
                false,
                "Method",
                "Links follow the library. Copies stay put until the skill updates.",
                mode_control,
            ));

        let preset_rows: Vec<AnyElement> = view
            .presets
            .iter()
            .enumerate()
            .map(|(ix, preset)| self.preset_row(ix, preset, view, &targets, theme, cx))
            .collect();
        let preset_card = match &self.preset {
            Some(draft) => widgets::section_card(theme).mt(px(0.0)).child(
                widgets::card_row(theme, true)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(240.0))
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(popover::dialog_field(
                                draft.input.clone().into_any_element(),
                            ))
                            .child(
                                div()
                                    .text_size(crate::typography::ui_rems(
                                        widgets::ROW_DESCRIPTION_SIZE,
                                    ))
                                    .text_color(theme.text_muted)
                                    .child("Switch on the skills this preset includes."),
                            ),
                    )
                    .child(
                        Self::action(theme, ActionTone::Quiet, "preset-cancel", "Cancel").on_click(
                            cx.listener(|page, _, _, cx| {
                                page.preset = None;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        Self::action(theme, ActionTone::Solid, "preset-save", "Save preset")
                            .on_click(cx.listener(|page, _, _, cx| page.save_preset(cx))),
                    ),
            ),
            None => widgets::section_card(theme)
                .mt(px(0.0))
                .when(preset_rows.is_empty(), |card| {
                    card.child(Self::empty_row(
                        theme,
                        "Group skills to add or remove them from an agent together.",
                    ))
                })
                .children(preset_rows)
                .child(
                    div().px(px(6.0)).py(px(6.0)).flex().child(
                        Self::action(theme, ActionTone::Quiet, "preset-new", "New preset")
                            .on_click(cx.listener(|page, _, _, cx| page.edit_preset(None, cx))),
                    ),
                ),
        };

        let skill_rows: Vec<AnyElement> = view
            .skills
            .iter()
            .enumerate()
            .map(|(ix, skill)| self.skill_row(ix, skill, &targets, theme, cx))
            .collect();
        let library_card = widgets::section_card(theme)
            .mt(px(0.0))
            .when(skill_rows.is_empty(), |card| {
                card.child(Self::empty_row(
                    theme,
                    "No skills yet. Add a folder, an archive or a Git repository above.",
                ))
            })
            .children(skill_rows);

        let unmanaged_rows: Vec<AnyElement> = view
            .unmanaged
            .iter()
            .enumerate()
            .map(|(ix, skill)| self.unmanaged_row(ix, skill, theme, cx))
            .collect();

        div()
            .flex()
            .flex_col()
            .child(widgets::section(theme, "Add a skill", add_card))
            .child(widgets::section(theme, "Deploy to", deploy_card))
            .child(widgets::section(theme, "Presets", preset_card))
            .child(widgets::section(theme, "Library", library_card))
            .when(!unmanaged_rows.is_empty(), |el| {
                el.child(widgets::section(
                    theme,
                    "Found in agent folders",
                    widgets::section_card(theme)
                        .mt(px(0.0))
                        .children(unmanaged_rows),
                ))
            })
            .into_any_element()
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }
}

impl popover::ScrollRailHost for SkillsPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

impl Render for SkillsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::toast::take_error(&mut self.error, cx);
        let theme = Theme::of(cx).for_settings_surface();
        let view = self.library.ready().cloned();
        let count = view.as_ref().map(|view| view.skills.len());
        let body: AnyElement = match (&view, &self.library) {
            (Some(view), _) => self.render_library(view, &theme, cx),
            (None, Loadable::Error(message)) => div()
                .child(widgets::error_strip(&theme, message.clone()))
                .child(
                    Self::action(&theme, ActionTone::Quiet, "skills-retry", "Retry")
                        .mt(px(8.0))
                        .on_click(cx.listener(|page, _, _, cx| page.run(None, "Loading…", cx))),
                )
                .into_any_element(),
            _ => widgets::section_card(&theme)
                .p(px(16.0))
                .child(popover::skeleton_rows(
                    "skills-skeleton",
                    &theme,
                    4,
                    cx.entity_id(),
                    cx,
                ))
                .into_any_element(),
        };
        let status: AnyElement = match self.busy {
            Some(busy) if view.is_some() => div()
                .flex_none()
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(theme.text_muted)
                .child(busy)
                .into_any_element(),
            _ => Self::action(
                &theme,
                ActionTone::Quiet,
                "skills-check-updates",
                "Check for updates",
            )
            .when(view.is_none(), |el| el.invisible())
            .on_click(cx.listener(|page, _, _, cx| {
                page.run(Some(SkillLibraryOp::CheckUpdates), "Checking…", cx);
            }))
            .into_any_element(),
        };
        let scrollbar = popover::rail(self, "skills-page-scrollbar", &theme, cx);
        div()
            .id("skills-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                crate::edge_fade::edge_faded(
                    16.0,
                    true,
                    true,
                    div()
                        .id("skills-page")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll.scroll)
                        .child(
                            widgets::page_column()
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .justify_between()
                                        .child(widgets::page_header(&theme, "Skills", count))
                                        .child(status),
                                )
                                .child(widgets::page_subtitle(
                                    &theme,
                                    "One library of agent skills, added to the agents that should use them.",
                                ))
                                .child(body),
                        ),
                )
                .fade_overflow_y(&self.scroll.scroll),
            )
            .children(scrollbar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::skill_library::{SkillDeployment, SkillTarget};

    fn skill(id: &str, deployments: &[(Option<HarnessId>, SkillDeploymentState)]) -> LibrarySkill {
        LibrarySkill {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            source: SkillSource::Local { path: "/s".into() },
            update_available: false,
            deployments: deployments
                .iter()
                .map(|(harness, state)| SkillDeployment {
                    harness: *harness,
                    mode: SkillSyncMode::Symlink,
                    state: *state,
                })
                .collect(),
        }
    }

    #[test]
    fn typed_sources_route_paths_and_git_references() {
        let path = |text: &str| SkillLibraryOp::InstallPath { path: text.into() };
        assert_eq!(install_op(" /tmp/review "), Some(path("/tmp/review")));
        assert_eq!(install_op("~/skills/x.zip"), Some(path("~/skills/x.zip")));
        assert_eq!(install_op(r"C:\skills\x"), Some(path(r"C:\skills\x")));
        for git in [
            "owner/repo",
            "https://github.com/owner/repo",
            "git@host:a/b.git",
        ] {
            assert_eq!(
                install_op(git),
                Some(SkillLibraryOp::InstallGit { source: git.into() })
            );
        }
        assert_eq!(install_op("   "), None);
    }

    #[test]
    fn requests_carry_the_scope_beside_the_operation() {
        assert_eq!(request_params(None, None), serde_json::json!({}));
        assert_eq!(
            request_params(None, Some("space")),
            serde_json::json!({"spaceId": "space"})
        );
        let op = SkillLibraryOp::Deploy {
            skill: "review".into(),
            harness: Some(HarnessId::Codex),
        };
        assert_eq!(
            request_params(Some(&op), Some("space")),
            serde_json::json!({
                "op": "deploy",
                "skill": "review",
                "harness": "codex",
                "spaceId": "space",
            })
        );
    }

    #[test]
    fn presets_report_coverage_and_hidden_targets_with_deployments_stay_visible() {
        use SkillDeploymentState::{Deployed, Missing};
        let cursor = Some(HarnessId::Cursor);
        let view = SkillLibraryView {
            skills: vec![
                skill("a", &[(cursor, Deployed), (None, Deployed)]),
                skill("b", &[(cursor, Missing)]),
                skill("c", &[(Some(HarnessId::Grok), Deployed)]),
            ],
            targets: [None, cursor, Some(HarnessId::Grok), Some(HarnessId::Pi)]
                .into_iter()
                .map(|harness| SkillTarget {
                    harness,
                    path: String::new(),
                    available: harness.is_none(),
                })
                .collect(),
            ..Default::default()
        };
        let preset = SkillPreset {
            id: "p".into(),
            name: "Pair".into(),
            skills: vec!["a".into(), "b".into(), "gone".into()],
        };
        assert_eq!(preset_coverage(&preset, &view.skills, cursor), (1, 2));
        assert_eq!(preset_coverage(&preset, &view.skills, None), (1, 2));
        assert_eq!(
            preset_coverage(&preset, &view.skills, Some(HarnessId::Pi)),
            (0, 2)
        );
        assert_eq!(
            visible_targets(&view),
            [None, cursor, Some(HarnessId::Grok)]
        );
        assert_eq!(target_label(None), "Shared");
        assert_eq!(target_label(Some(HarnessId::ClaudeCode)), "Claude Code");
    }

    #[test]
    fn git_sources_read_as_repository_paths() {
        let source = SkillSource::Git {
            url: "https://github.com/owner/repo.git".into(),
            branch: None,
            subpath: Some("skills/review".into()),
            revision: None,
        };
        assert_eq!(source_label(&source), "github.com/owner/repo/skills/review");
    }

    #[gpui::test]
    fn every_page_state_renders_and_escape_closes_inline_edits(cx: &mut gpui::TestAppContext) {
        use SkillDeploymentState::{Conflict, Deployed, Missing};
        use gpui::AppContext;
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            crate::settings::init(Default::default(), dir.path(), cx);
            gpui_base::init(cx);
            cx.set_global(crate::theme::Theme::default());
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| crate::state::AppState::new());
            SkillsPage::new(state, cx)
        });
        let draw = |cx: &mut gpui::TestAppContext| {
            cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear())
                .unwrap();
        };
        draw(cx);
        window
            .update(cx, |page, _, cx| {
                page.library = Loadable::Error("engine unavailable".into());
                cx.notify();
            })
            .unwrap();
        draw(cx);

        let cursor = Some(HarnessId::Cursor);
        let mut updatable = skill("review", &[(cursor, Deployed), (None, Missing)]);
        updatable.update_available = true;
        updatable.description = "Reviews a change".into();
        let view = SkillLibraryView {
            mode: SkillSyncMode::Copy,
            skills: vec![updatable, skill("notes", &[(cursor, Conflict)])],
            targets: [None, cursor, Some(HarnessId::ClaudeCode)]
                .into_iter()
                .map(|harness| SkillTarget {
                    harness,
                    path: "/home/user/skills".into(),
                    available: true,
                })
                .collect(),
            unmanaged: vec![UnmanagedSkill {
                harness: cursor,
                name: "stray".into(),
                description: String::new(),
                path: "/home/user/.cursor/skills/stray".into(),
            }],
            presets: vec![SkillPreset {
                id: "p".into(),
                name: "Pair".into(),
                skills: vec!["review".into(), "notes".into()],
            }],
        };
        window
            .update(cx, |page, _, cx| {
                page.library = Loadable::Ready(view.clone());
                assert!(!page.dismiss_on_escape(cx));
            })
            .unwrap();
        draw(cx);

        window
            .update(cx, |page, _, cx| {
                page.remove("review".into(), cx);
                assert_eq!(page.confirm_remove.as_deref(), Some("review"));
            })
            .unwrap();
        draw(cx);
        window
            .update(cx, |page, _, cx| {
                assert!(page.dismiss_on_escape(cx));
                assert!(page.confirm_remove.is_none());
                page.edit_preset(Some(&view.presets[0]), cx);
                page.toggle_draft_skill("notes", cx);
                let draft = page.preset.as_ref().unwrap();
                assert_eq!(draft.input.read(cx).text(), "Pair");
                assert_eq!(draft.skills.iter().collect::<Vec<_>>(), ["review"]);
            })
            .unwrap();
        draw(cx);
        window
            .update(cx, |page, _, cx| {
                assert!(page.dismiss_on_escape(cx));
                assert!(page.preset.is_none());
                page.library = Loadable::Ready(SkillLibraryView::default());
                cx.notify();
            })
            .unwrap();
        draw(cx);
    }
}
