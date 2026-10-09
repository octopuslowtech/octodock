mod behavior;

use std::{
    cell::Cell,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use gpui::{
    Bounds, Context, DispatchPhase, Empty, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ObjectFit, Pixels, Point, Render, RenderImage, Size, Task,
    Window, canvas, div, img, point, prelude::*, px,
};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use zeron_proto::view::Indicator;

use crate::new_thread_background_mask::SurfaceBounds;
use crate::settings::SavePolicy;
use behavior::{Brain, Face, Mood};

const CHARACTER: &str = "octopus";
const FACE: f32 = 84.0;
const PERCH_SINK: f32 = 0.9;
const PERCH_INSET: f32 = 28.0;
const DRAG_THRESHOLD: f32 = 4.0;
const FLING_DISTANCE: f32 = 240.0;
const SNAP_HOME: f32 = 48.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MascotSettings {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<(f32, f32)>,
}

impl Default for MascotSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            position: None,
        }
    }
}

#[derive(RustEmbed)]
#[folder = "assets/mascots"]
struct Sheets;

fn cells(name: &str) -> Vec<Arc<RenderImage>> {
    let Some(file) = Sheets::get(name) else {
        return Vec::new();
    };
    let Ok(sheet) = image::load_from_memory_with_format(&file.data, image::ImageFormat::WebP)
    else {
        return Vec::new();
    };
    let mut sheet = sheet.into_rgba8();
    for pixel in sheet.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    let cell = sheet.width() / 3;
    (0..9)
        .map(|index| {
            let frame = image::imageops::crop_imm(
                &sheet,
                (index % 3) * cell,
                (index / 3) * cell,
                cell,
                cell,
            )
            .to_image();
            Arc::new(RenderImage::new([image::Frame::new(frame)]))
        })
        .collect()
}

struct Sprites {
    directions: Vec<Arc<RenderImage>>,
    reactions: Vec<Arc<RenderImage>>,
}

impl Sprites {
    fn load() -> Self {
        Self {
            directions: cells(&format!("{CHARACTER}-directions.webp")),
            reactions: cells(&format!("{CHARACTER}-reactions.webp")),
        }
    }

    fn frame(&self, face: Face) -> Option<Arc<RenderImage>> {
        let (reaction, index) = face.frame();
        let sheet = if reaction {
            &self.reactions
        } else {
            &self.directions
        };
        sheet.get(index).cloned()
    }
}

fn distance(a: Point<Pixels>, b: Point<Pixels>) -> f32 {
    f32::from(a.x - b.x).hypot(f32::from(a.y - b.y))
}

fn clamp_to(origin: Point<Pixels>, viewport: Size<Pixels>) -> Point<Pixels> {
    point(
        px(f32::from(origin.x).clamp(0.0, (f32::from(viewport.width) - FACE).max(0.0))),
        px(f32::from(origin.y).clamp(0.0, (f32::from(viewport.height) - FACE).max(0.0))),
    )
}

fn perch_origin(surface: Bounds<Pixels>) -> Point<Pixels> {
    point(
        surface.right() - px(FACE + PERCH_INSET),
        surface.top() - px(FACE * PERCH_SINK),
    )
}

fn mood_for(indicator: Indicator) -> Mood {
    match indicator {
        Indicator::None => Mood::Neutral,
        Indicator::Working => Mood::Focus,
        Indicator::AwaitingInput => Mood::Alert,
        Indicator::Errored => Mood::Sad,
    }
}

struct Press {
    pointer: Point<Pixels>,
    origin: Point<Pixels>,
    dragging: bool,
}

pub struct Mascot {
    brain: Brain,
    epoch: Instant,
    surface: SurfaceBounds,
    page_origin: Rc<Cell<Point<Pixels>>>,
    perchable: bool,
    status: Option<(String, Indicator)>,
    sprites: Option<Sprites>,
    press: Option<Press>,
    carried: Option<Point<Pixels>>,
    origin: Option<Point<Pixels>>,
    perch: Option<Point<Pixels>>,
    viewport: Size<Pixels>,
    shown: Option<Face>,
    armed: Option<u64>,
    wake: Option<Task<()>>,
}

impl Mascot {
    pub fn new(surface: SurfaceBounds) -> Self {
        Self {
            brain: Brain::new(0),
            epoch: Instant::now(),
            surface,
            page_origin: Default::default(),
            perchable: false,
            status: None,
            sprites: None,
            press: None,
            carried: None,
            origin: None,
            perch: None,
            viewport: Default::default(),
            shown: None,
            armed: None,
            wake: None,
        }
    }

    fn now(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    pub fn sync(&mut self, perchable: bool, chat: Option<&str>, indicator: Indicator) {
        self.perchable = perchable;
        let previous = self.status.as_ref().map(|(id, seen)| (id.as_str(), *seen));
        if previous == chat.map(|id| (id, indicator)) {
            return;
        }
        let now = self.now();
        match previous {
            Some((id, seen)) if Some(id) == chat => {
                let finished = seen == Indicator::Working && indicator == Indicator::None;
                self.brain.set_mood(
                    if finished {
                        Mood::Happy
                    } else {
                        mood_for(indicator)
                    },
                    now,
                );
            }
            _ => self.brain.reset_mood(mood_for(indicator), now),
        }
        self.status = chat.map(|id| (id.to_owned(), indicator));
    }

    fn face(&self, now: u64) -> Face {
        let below_middle = self.origin.is_some_and(|origin| {
            f32::from(origin.y) + FACE / 2.0 > f32::from(self.viewport.height) / 2.0
        });
        self.brain.face(now, below_middle)
    }

    fn center(&self) -> Option<(f32, f32)> {
        self.origin.map(|origin| {
            (
                f32::from(origin.x) + FACE / 2.0,
                f32::from(origin.y) + FACE / 2.0,
            )
        })
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let now = self.now();
        let sooner = self
            .brain
            .next_wake(now)
            .is_some_and(|next| self.armed.is_none_or(|armed| next < armed));
        if Some(self.face(now)) != self.shown || sooner || self.brain.squashing() {
            cx.notify();
        }
    }

    fn arm(&mut self, now: u64, cx: &mut Context<Self>) {
        let next = self.brain.next_wake(now);
        if next == self.armed && (next.is_none() || self.wake.is_some()) {
            return;
        }
        self.armed = next;
        self.wake = next.map(|at| {
            let delay = Duration::from_millis(at.saturating_sub(now));
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(delay).await;
                this.update(cx, |this, cx| {
                    this.armed = None;
                    cx.notify();
                })
                .ok();
            })
        });
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let now = self.now();
        if let Some(mut press) = self.press.take() {
            if event.pressed_button != Some(MouseButton::Left) {
                self.press = Some(press);
                self.on_release(cx);
                return;
            }
            let delta = event.position - press.pointer;
            let moved = f32::from(delta.x).hypot(f32::from(delta.y));
            if press.dragging || moved >= DRAG_THRESHOLD {
                if !press.dragging {
                    press.dragging = true;
                    self.brain.drag_started(now);
                }
                self.carried = Some(clamp_to(press.origin + delta, self.viewport));
                cx.notify();
            }
            self.press = Some(press);
            return;
        }
        let Some(center) = self.center() else {
            return;
        };
        self.brain.cursor_moved(
            center,
            f32::from(event.position.x),
            f32::from(event.position.y),
            now,
        );
        self.refresh(cx);
    }

    fn on_release(&mut self, cx: &mut Context<Self>) {
        let Some(press) = self.press.take() else {
            return;
        };
        let now = self.now();
        if !press.dragging {
            self.brain.poke(now);
            cx.notify();
            return;
        }
        let dropped = self.carried.take().unwrap_or(press.origin);
        let home = self
            .perch
            .is_some_and(|perch| distance(dropped, perch) <= SNAP_HOME);
        self.brain
            .dropped(distance(dropped, press.origin) >= FLING_DISTANCE, now);
        let position = (!home).then(|| (f32::from(dropped.x), f32::from(dropped.y)));
        crate::settings::update(SavePolicy::Debounced, cx, |settings| {
            settings.mascot.position = position;
        });
        cx.notify();
    }

    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.platform || modifiers.alt {
            return;
        }
        let (Some(center), Some(surface)) = (
            self.center(),
            self.perchable.then(|| self.surface.get()).flatten(),
        ) else {
            return;
        };
        self.brain.typed(
            center,
            (
                f32::from(surface.origin.x),
                f32::from(surface.origin.y),
                f32::from(surface.size.width),
                f32::from(surface.size.height),
            ),
            self.now(),
        );
        self.refresh(cx);
    }
}

impl Render for Mascot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = crate::settings::mascot(cx);
        let viewport = window.viewport_size();
        let surface = self.perchable.then(|| self.surface.get()).flatten();
        let perch = surface.map(|surface| clamp_to(perch_origin(surface), viewport));
        let pinned = settings
            .position
            .filter(|(x, y)| x.is_finite() && y.is_finite())
            .map(|(x, y)| clamp_to(point(px(x), px(y)), viewport));
        let origin = settings
            .enabled
            .then(|| self.carried.or(pinned).or(perch))
            .flatten();
        self.origin = origin;
        self.perch = perch;
        self.viewport = viewport;
        let Some(origin) = origin else {
            self.wake = None;
            self.armed = None;
            self.press = None;
            return Empty.into_any_element();
        };

        let now = self.now();
        self.brain.tick(now);
        let face = self.face(now);
        self.shown = Some(face);
        self.arm(now, cx);
        let (scale_x, scale_y) = if crate::motion::reduced_motion(cx) {
            (1.0, 1.0)
        } else {
            self.brain.squash(now)
        };
        if self.brain.squashing() {
            window.request_animation_frame();
        }

        let local = origin - self.page_origin.get();
        let dragging = self.press.as_ref().is_some_and(|press| press.dragging);
        let (width, height) = (FACE * scale_x, FACE * scale_y);
        let image = self.sprites.get_or_insert_with(Sprites::load).frame(face);

        let entity = cx.entity();
        let measured_origin = self.page_origin.clone();
        let measured_surface = self.surface.clone();
        let probe = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let actual = bounds.origin - local;
                if distance(actual, measured_origin.get()) > 0.5 {
                    measured_origin.set(actual);
                    window.request_animation_frame();
                }
                if surface.is_some() && measured_surface.get() != surface {
                    window.request_animation_frame();
                }
                let mascot = entity.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        mascot.update(cx, |mascot, cx| mascot.on_mouse_move(event, cx));
                    }
                });
                let mascot = entity.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                        mascot.update(cx, |mascot, cx| mascot.on_release(cx));
                    }
                });
                let mascot = entity.clone();
                window.on_key_event(move |event: &KeyDownEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        mascot.update(cx, |mascot, cx| mascot.on_key(event, cx));
                    }
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_0();

        div()
            .absolute()
            .left(local.x)
            .top(local.y)
            .w(px(FACE))
            .h(px(FACE))
            .child(probe)
            .children(image.map(|image| {
                img(image)
                    .absolute()
                    .left(px((FACE - width) / 2.0))
                    .top(px(FACE - height))
                    .w(px(width))
                    .h(px(height))
                    .object_fit(ObjectFit::Fill)
            }))
            .child(
                div()
                    .id("mascot")
                    .absolute()
                    .left(px(FACE * 0.14))
                    .top(px(FACE * 0.06))
                    .w(px(FACE * 0.72))
                    .h(px(FACE * 0.76))
                    .occlude()
                    .map(|hit| {
                        if dragging {
                            hit.cursor_grabbing()
                        } else {
                            hit.cursor_grab()
                        }
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            let Some(origin) = this.origin else {
                                return;
                            };
                            this.press = Some(Press {
                                pointer: event.position,
                                origin,
                                dragging: false,
                            });
                            window.prevent_default();
                            cx.stop_propagation();
                        }),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_sheets_decode_into_nine_frames() {
        let sprites = Sprites::load();
        assert_eq!((sprites.directions.len(), sprites.reactions.len()), (9, 9));
        assert!(
            sprites
                .frame(Face::Reaction(behavior::Reaction::Dizzy))
                .is_some()
        );
    }

    #[test]
    fn settings_ignore_retired_fields_and_keep_the_position() {
        let restored: MascotSettings =
            serde_json::from_str(r#"{"character":"owl","name":"Bo","position":[12.0,34.0]}"#)
                .unwrap();
        assert_eq!(restored.position, Some((12.0, 34.0)));
        assert!(restored.enabled);
    }

    #[test]
    fn perch_sits_on_the_composer_edge_and_stays_inside_the_window() {
        let viewport = gpui::size(px(1200.0), px(800.0));
        let surface = Bounds::new(
            point(px(300.0), px(650.0)),
            gpui::size(px(700.0), px(120.0)),
        );
        assert_eq!(
            perch_origin(surface),
            point(px(1000.0 - FACE - PERCH_INSET), px(650.0 - FACE * 0.9))
        );
        assert_eq!(
            clamp_to(point(px(5000.0), px(-40.0)), viewport),
            point(px(1200.0 - FACE), px(0.0))
        );
    }

    #[test]
    fn finished_run_reads_as_happy_and_a_chat_switch_is_silent() {
        let mut mascot = Mascot::new(Default::default());
        mascot.sync(true, Some("a"), Indicator::Working);
        assert_eq!(
            mascot.face(mascot.now()),
            Face::Gaze(behavior::Direction::Center)
        );
        mascot.sync(true, Some("a"), Indicator::None);
        assert_eq!(
            mascot.face(mascot.now()),
            Face::Reaction(behavior::Reaction::Delighted)
        );
        let mut switched = Mascot::new(Default::default());
        switched.sync(true, Some("a"), Indicator::None);
        switched.sync(true, Some("b"), Indicator::Errored);
        assert_eq!(
            switched.face(switched.now()),
            Face::Gaze(behavior::Direction::Center)
        );
        switched.sync(true, Some("b"), Indicator::AwaitingInput);
        assert_eq!(
            switched.face(switched.now()),
            Face::Reaction(behavior::Reaction::Surprised)
        );
    }
}
