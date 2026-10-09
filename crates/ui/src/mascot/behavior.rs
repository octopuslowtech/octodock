use std::f32::consts::FRAC_PI_4;

const DEADZONE: f32 = 70.0;
const TYPING_DEADZONE: f32 = 12.0;
const TYPING_IDLE_MS: u64 = 1500;
const RAPID_WINDOW_MS: u64 = 1600;
const DIZZY_THRESHOLD: u32 = 4;
const BLINK_TO_REACTION_MS: u64 = 120;
const REACTION_CLEAR_MS: u64 = 560;
const DIZZY_CLEAR_MS: u64 = 1100;
pub const SLEEP_AFTER_MS: u64 = 10_000;
const STARE_AFTER_MS: u64 = 2000;
const WAKE_MS: u64 = 700;
const BASHFUL_MS: u64 = 1200;
const DROP_MS: u64 = 900;
const MOOD_FLASH_MS: u64 = 1400;
const GLANCE_EVERY_MS: u64 = 4500;
const GLANCE_MS: u64 = 1100;
const NOTICE_SPEED: f32 = 1.2;
const NOTICE_DISTANCE: f32 = 90.0;
const INTEREST_MS: u64 = 2500;
const WHIM_MIN_MS: u64 = 20_000;
const WHIM_MAX_MS: u64 = 50_000;
const WHIM_MS: u64 = 900;
const BLINK_MS: u64 = 160;
const BLINK_EVERY_MS: u64 = 3600;
const SQUASH_KEYS: [(u64, f32, f32); 5] = [
    (0, 1.0, 1.0),
    (75, 1.1, 0.86),
    (189, 0.95, 1.08),
    (302, 1.03, 0.97),
    (420, 1.0, 1.0),
];

const AMBIENT: u8 = 0;
const MOOD: u8 = 1;
const DIRECT: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    UpLeft,
    Up,
    UpRight,
    Left,
    Center,
    Right,
    DownLeft,
    Down,
    DownRight,
}

const RING: [Direction; 8] = [
    Direction::Right,
    Direction::DownRight,
    Direction::Down,
    Direction::DownLeft,
    Direction::Left,
    Direction::UpLeft,
    Direction::Up,
    Direction::UpRight,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reaction {
    Blink,
    Heart,
    Sparkle,
    Surprised,
    Wink,
    Bashful,
    Sleepy,
    Dizzy,
    Delighted,
}

const POKE_BONUS: [Reaction; 3] = [Reaction::Heart, Reaction::Sparkle, Reaction::Delighted];
const WHIMS: [Reaction; 5] = [
    Reaction::Sparkle,
    Reaction::Wink,
    Reaction::Delighted,
    Reaction::Heart,
    Reaction::Surprised,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mood {
    Neutral,
    Focus,
    Alert,
    Happy,
    Sad,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    Gaze(Direction),
    Reaction(Reaction),
}

impl Face {
    pub fn frame(self) -> (bool, usize) {
        match self {
            Face::Gaze(direction) => (false, direction as usize),
            Face::Reaction(reaction) => (true, reaction as usize),
        }
    }
}

fn direction_to(center: (f32, f32), x: f32, y: f32, deadzone: f32) -> Direction {
    let (dx, dy) = (x - center.0, y - center.1);
    if dx.hypot(dy) < deadzone {
        return Direction::Center;
    }
    let step = (dy.atan2(dx) / FRAC_PI_4).round() as i32;
    RING[step.rem_euclid(8) as usize]
}

#[derive(Debug, Clone, Copy)]
struct Showing {
    reaction: Reaction,
    priority: u8,
    until: u64,
    hold_from: Option<u64>,
    swap: Option<(u64, Reaction)>,
}

#[derive(Debug)]
pub struct Brain {
    direction: Direction,
    showing: Option<Showing>,
    asleep: bool,
    dragging: bool,
    typing: Option<(Direction, u64)>,
    mood: Mood,
    focus_since: u64,
    idle_since: u64,
    hovered: bool,
    stared: bool,
    noticed: (f32, f32),
    noticed_at: Option<u64>,
    interest_until: u64,
    pokes: u32,
    poked_at: u64,
    squash_at: Option<u64>,
    next_blink: u64,
    next_whim: u64,
    seed: u32,
}

impl Brain {
    pub fn new(now: u64) -> Self {
        let mut brain = Self {
            direction: Direction::Center,
            showing: None,
            asleep: false,
            dragging: false,
            typing: None,
            mood: Mood::Neutral,
            focus_since: now,
            idle_since: now,
            hovered: false,
            stared: false,
            noticed: (0.0, 0.0),
            noticed_at: None,
            interest_until: 0,
            pokes: 0,
            poked_at: 0,
            squash_at: None,
            next_blink: now + BLINK_EVERY_MS,
            next_whim: 0,
            seed: 0x9E37_79B9,
        };
        brain.next_whim = now + brain.whim_delay();
        brain
    }

    fn random(&mut self) -> u32 {
        self.seed = self
            .seed
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        self.seed >> 8
    }

    fn whim_delay(&mut self) -> u64 {
        WHIM_MIN_MS + u64::from(self.random()) % (WHIM_MAX_MS - WHIM_MIN_MS)
    }

    fn show(
        &mut self,
        reaction: Reaction,
        ms: u64,
        priority: u8,
        hold_from: Option<u64>,
        now: u64,
    ) -> bool {
        if self
            .showing
            .is_some_and(|current| current.until > now && current.priority > priority)
        {
            return false;
        }
        self.showing = Some(Showing {
            reaction,
            priority,
            until: now + ms,
            hold_from,
            swap: None,
        });
        true
    }

    fn wake(&mut self, by_user: bool, now: u64) {
        self.idle_since = now;
        if by_user
            && self
                .showing
                .is_some_and(|held| held.hold_from.is_some_and(|from| now >= from))
        {
            self.showing = None;
        }
        if !self.asleep {
            return;
        }
        self.asleep = false;
        self.next_blink = now + BLINK_EVERY_MS;
        self.next_whim = now + self.whim_delay();
        if by_user {
            self.show(Reaction::Surprised, WAKE_MS, DIRECT, None, now);
        }
    }

    pub fn cursor_moved(&mut self, center: (f32, f32), x: f32, y: f32, now: u64) {
        let next = direction_to(center, x, y, DEADZONE);
        self.hovered = next == Direction::Center;
        if !self.hovered {
            self.stared = false;
        }
        self.wake(true, now);
        let moved = (x - self.noticed.0).hypot(y - self.noticed.1);
        let speed = self
            .noticed_at
            .map_or(0.0, |at| moved / now.saturating_sub(at).max(1) as f32);
        self.noticed_at = Some(now);
        let notable = self.hovered || speed >= NOTICE_SPEED || moved >= NOTICE_DISTANCE;
        if !notable && now >= self.interest_until {
            return;
        }
        if notable {
            self.noticed = (x, y);
            self.interest_until = now + INTEREST_MS;
        }
        self.direction = next;
    }

    pub fn typed(&mut self, center: (f32, f32), field: (f32, f32, f32, f32), now: u64) {
        let (x, y, width, height) = field;
        let look = (
            center.0.clamp(x, x + width.max(0.0)),
            center.1.clamp(y, y + height.max(0.0)),
        );
        self.wake(true, now);
        self.typing = Some((
            direction_to(center, look.0, look.1, TYPING_DEADZONE),
            now + TYPING_IDLE_MS,
        ));
    }

    pub fn set_mood(&mut self, mood: Mood, now: u64) {
        if mood == self.mood {
            return;
        }
        self.reset_mood(mood, now);
        self.wake(false, now);
        match mood {
            Mood::Neutral => {}
            Mood::Focus => {
                self.show(Reaction::Sparkle, MOOD_FLASH_MS, MOOD, None, now);
            }
            Mood::Alert => {
                self.show(Reaction::Surprised, MOOD_FLASH_MS, MOOD, None, now);
            }
            Mood::Sad => {
                self.show(Reaction::Dizzy, MOOD_FLASH_MS, MOOD, None, now);
            }
            Mood::Happy => {
                self.show(
                    Reaction::Delighted,
                    SLEEP_AFTER_MS,
                    MOOD,
                    Some(now + MOOD_FLASH_MS),
                    now,
                );
            }
        }
    }

    pub fn reset_mood(&mut self, mood: Mood, now: u64) {
        if mood == Mood::Focus && self.mood != Mood::Focus {
            self.focus_since = now;
        }
        self.mood = mood;
    }

    pub fn drag_started(&mut self, now: u64) {
        self.dragging = true;
        self.wake(false, now);
    }

    pub fn dropped(&mut self, flung: bool, now: u64) {
        self.dragging = false;
        if flung {
            self.show(Reaction::Dizzy, DIZZY_CLEAR_MS, DIRECT, None, now);
        } else {
            self.show(Reaction::Bashful, DROP_MS, DIRECT, None, now);
        }
        self.wake(false, now);
    }

    pub fn poke(&mut self, now: u64) {
        self.wake(false, now);
        self.pokes = if now.saturating_sub(self.poked_at) < RAPID_WINDOW_MS {
            self.pokes + 1
        } else {
            1
        };
        self.poked_at = now;
        if self.pokes >= DIZZY_THRESHOLD {
            self.pokes = 0;
            self.show(Reaction::Dizzy, DIZZY_CLEAR_MS, DIRECT, None, now);
        } else if self.show(Reaction::Blink, REACTION_CLEAR_MS, DIRECT, None, now) {
            let bonus = POKE_BONUS[(self.pokes as usize - 1) % POKE_BONUS.len()];
            if let Some(showing) = &mut self.showing {
                showing.swap = Some((now + BLINK_TO_REACTION_MS, bonus));
            }
        }
        self.squash_at = Some(now);
    }

    pub fn tick(&mut self, now: u64) {
        if self.showing.is_some_and(|showing| now >= showing.until) {
            self.showing = None;
        }
        if let Some(showing) = &mut self.showing
            && let Some((at, reaction)) = showing.swap
            && now >= at
        {
            showing.reaction = reaction;
            showing.swap = None;
        }
        if self.typing.is_some_and(|(_, until)| now >= until) {
            self.typing = None;
        }
        if self
            .squash_at
            .is_some_and(|at| now >= at + SQUASH_KEYS[4].0)
        {
            self.squash_at = None;
        }
        if self.dragging {
            return;
        }
        if now >= self.interest_until {
            self.direction = Direction::Center;
        }
        let still = now.saturating_sub(self.idle_since);
        if still >= SLEEP_AFTER_MS {
            self.asleep = true;
            return;
        }
        if self.hovered && !self.stared && still >= STARE_AFTER_MS {
            self.stared = true;
            self.show(Reaction::Bashful, BASHFUL_MS, DIRECT, None, now);
        }
        if now >= self.next_blink {
            self.next_blink = now + BLINK_EVERY_MS;
            self.show(Reaction::Blink, BLINK_MS, AMBIENT, None, now);
        }
        if now >= self.next_whim {
            self.next_whim = now + self.whim_delay();
            let whim = WHIMS[self.random() as usize % WHIMS.len()];
            self.show(whim, WHIM_MS, AMBIENT, None, now);
        }
    }

    fn glancing(&self, now: u64) -> bool {
        let elapsed = now.saturating_sub(self.focus_since);
        self.mood == Mood::Focus
            && elapsed >= GLANCE_EVERY_MS
            && elapsed % GLANCE_EVERY_MS < GLANCE_MS
    }

    pub fn face(&self, now: u64, below_middle: bool) -> Face {
        if self.dragging {
            return Face::Reaction(Reaction::Surprised);
        }
        if let Some(showing) = self.showing.filter(|showing| now < showing.until) {
            return Face::Reaction(showing.reaction);
        }
        if self.asleep {
            return Face::Reaction(Reaction::Sleepy);
        }
        if let Some((direction, _)) = self.typing.filter(|(_, until)| now < *until) {
            return Face::Gaze(direction);
        }
        if self.glancing(now) {
            return Face::Gaze(if below_middle {
                Direction::Up
            } else {
                Direction::Down
            });
        }
        Face::Gaze(self.direction)
    }

    pub fn next_wake(&self, now: u64) -> Option<u64> {
        if self.dragging {
            return None;
        }
        let mut deadlines = Vec::with_capacity(10);
        if let Some(showing) = self.showing {
            deadlines.push(showing.until);
            deadlines.extend(showing.swap.map(|(at, _)| at));
        }
        deadlines.extend(self.typing.map(|(_, until)| until));
        if !self.asleep {
            deadlines.push(self.idle_since + SLEEP_AFTER_MS);
            deadlines.push(self.next_blink);
            deadlines.push(self.next_whim);
            if self.hovered && !self.stared {
                deadlines.push(self.idle_since + STARE_AFTER_MS);
            }
            if self.direction != Direction::Center {
                deadlines.push(self.interest_until);
            }
            if self.mood == Mood::Focus {
                let phase = now.saturating_sub(self.focus_since) % GLANCE_EVERY_MS;
                deadlines.push(if self.glancing(now) {
                    now + GLANCE_MS - phase
                } else {
                    now + GLANCE_EVERY_MS - phase
                });
            }
        }
        deadlines.into_iter().map(|at| at.max(now + 1)).min()
    }

    #[cfg(test)]
    pub fn asleep(&self) -> bool {
        self.asleep
    }

    pub fn squashing(&self) -> bool {
        self.squash_at.is_some()
    }

    pub fn squash(&self, now: u64) -> (f32, f32) {
        let Some(elapsed) = self.squash_at.map(|at| now.saturating_sub(at)) else {
            return (1.0, 1.0);
        };
        for pair in SQUASH_KEYS.windows(2) {
            let ((from, x0, y0), (to, x1, y1)) = (pair[0], pair[1]);
            if elapsed < to {
                let t = (elapsed - from) as f32 / (to - from) as f32;
                return (x0 + (x1 - x0) * t, y0 + (y1 - y0) * t);
            }
        }
        (1.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CENTER: (f32, f32) = (500.0, 500.0);

    #[test]
    fn gaze_snaps_to_the_eight_way_ring_outside_the_deadzone() {
        assert_eq!(
            direction_to(CENTER, 510.0, 510.0, DEADZONE),
            Direction::Center
        );
        assert_eq!(
            direction_to(CENTER, 900.0, 500.0, DEADZONE),
            Direction::Right
        );
        assert_eq!(direction_to(CENTER, 500.0, 100.0, DEADZONE), Direction::Up);
        assert_eq!(
            direction_to(CENTER, 100.0, 900.0, DEADZONE),
            Direction::DownLeft
        );
        assert_eq!(Face::Gaze(Direction::Center).frame(), (false, 4));
        assert_eq!(Face::Reaction(Reaction::Dizzy).frame(), (true, 7));
    }

    #[test]
    fn poke_blinks_then_cycles_bonus_reactions_and_clears() {
        let mut brain = Brain::new(0);
        brain.poke(1000);
        assert_eq!(brain.face(1000, false), Face::Reaction(Reaction::Blink));
        assert!(brain.squashing());
        brain.tick(1130);
        assert_eq!(brain.face(1130, false), Face::Reaction(Reaction::Heart));
        brain.tick(1600);
        assert_eq!(brain.face(1600, false), Face::Gaze(Direction::Center));
        assert!(!brain.squashing());
        brain.poke(1700);
        brain.tick(1830);
        assert_eq!(brain.face(1830, false), Face::Reaction(Reaction::Sparkle));
    }

    #[test]
    fn four_rapid_pokes_make_it_dizzy() {
        let mut brain = Brain::new(0);
        for at in [100, 300, 500, 700] {
            brain.poke(at);
        }
        assert_eq!(brain.face(700, false), Face::Reaction(Reaction::Dizzy));
        brain.tick(1900);
        assert_eq!(brain.face(1900, false), Face::Gaze(Direction::Center));
    }

    #[test]
    fn naps_when_left_alone_and_wakes_startled() {
        let mut brain = Brain::new(0);
        brain.tick(SLEEP_AFTER_MS);
        assert!(brain.asleep());
        assert_eq!(
            brain.face(SLEEP_AFTER_MS + 500, false),
            Face::Reaction(Reaction::Sleepy)
        );
        assert_eq!(brain.next_wake(SLEEP_AFTER_MS + 500), None);
        brain.cursor_moved(CENTER, 900.0, 500.0, 20_000);
        assert!(!brain.asleep());
        assert_eq!(
            brain.face(20_000, false),
            Face::Reaction(Reaction::Surprised)
        );
        brain.tick(20_000 + WAKE_MS);
        assert_eq!(
            brain.face(20_000 + WAKE_MS, false),
            Face::Gaze(Direction::Right)
        );
    }

    #[test]
    fn workspace_mood_never_cuts_a_direct_reaction_short() {
        let mut brain = Brain::new(0);
        for at in [100, 300, 500, 700] {
            brain.poke(at);
        }
        brain.set_mood(Mood::Alert, 800);
        assert_eq!(brain.face(800, false), Face::Reaction(Reaction::Dizzy));
    }

    #[test]
    fn finished_run_stays_delighted_until_the_user_returns() {
        let mut brain = Brain::new(0);
        brain.set_mood(Mood::Focus, 100);
        assert_eq!(brain.face(100, false), Face::Reaction(Reaction::Sparkle));
        brain.set_mood(Mood::Happy, 5000);
        brain.cursor_moved(CENTER, 900.0, 500.0, 5200);
        assert_eq!(brain.face(5200, false), Face::Reaction(Reaction::Delighted));
        brain.tick(9000);
        assert_eq!(brain.face(9000, false), Face::Reaction(Reaction::Delighted));
        brain.cursor_moved(CENTER, 100.0, 500.0, 9100);
        assert_eq!(brain.face(9100, false), Face::Gaze(Direction::Left));
    }

    #[test]
    fn running_agent_draws_periodic_glances_at_the_transcript() {
        let mut brain = Brain::new(0);
        brain.set_mood(Mood::Focus, 0);
        brain.tick(MOOD_FLASH_MS);
        assert_eq!(
            brain.face(MOOD_FLASH_MS, true),
            Face::Gaze(Direction::Center)
        );
        assert_eq!(
            brain.face(GLANCE_EVERY_MS + 10, true),
            Face::Gaze(Direction::Up)
        );
        assert_eq!(
            brain.face(GLANCE_EVERY_MS + 10, false),
            Face::Gaze(Direction::Down)
        );
        assert_eq!(
            brain.face(GLANCE_EVERY_MS + GLANCE_MS, true),
            Face::Gaze(Direction::Center)
        );
    }

    #[test]
    fn creeping_cursor_is_ignored_but_a_dart_is_followed() {
        let mut brain = Brain::new(0);
        brain.cursor_moved(CENTER, 900.0, 500.0, 100);
        assert_eq!(brain.face(100, false), Face::Gaze(Direction::Right));
        brain.tick(100 + INTEREST_MS);
        assert_eq!(
            brain.face(100 + INTEREST_MS, false),
            Face::Gaze(Direction::Center)
        );
        brain.cursor_moved(CENTER, 905.0, 500.0, 3000);
        assert_eq!(brain.face(3000, false), Face::Gaze(Direction::Center));
        brain.cursor_moved(CENTER, 500.0, 100.0, 3010);
        assert_eq!(brain.face(3010, false), Face::Gaze(Direction::Up));
    }

    #[test]
    fn typing_outranks_the_parked_cursor_and_a_stare_makes_it_bashful() {
        let mut brain = Brain::new(0);
        brain.cursor_moved(CENTER, 900.0, 500.0, 100);
        brain.typed(CENTER, (100.0, 700.0, 800.0, 120.0), 200);
        assert_eq!(brain.face(200, false), Face::Gaze(Direction::Down));
        brain.tick(200 + TYPING_IDLE_MS);
        assert_eq!(
            brain.face(200 + TYPING_IDLE_MS, false),
            Face::Gaze(Direction::Right)
        );
        brain.cursor_moved(CENTER, 505.0, 505.0, 3000);
        brain.tick(3000 + STARE_AFTER_MS);
        assert_eq!(
            brain.face(3000 + STARE_AFTER_MS, false),
            Face::Reaction(Reaction::Bashful)
        );
    }

    #[test]
    fn carrying_surprises_and_a_long_throw_lands_dizzy() {
        let mut brain = Brain::new(0);
        brain.drag_started(100);
        assert_eq!(brain.face(150, false), Face::Reaction(Reaction::Surprised));
        assert_eq!(brain.next_wake(150), None);
        brain.dropped(true, 400);
        assert_eq!(brain.face(400, false), Face::Reaction(Reaction::Dizzy));
        brain.drag_started(2000);
        brain.dropped(false, 2100);
        assert_eq!(brain.face(2100, false), Face::Reaction(Reaction::Bashful));
        assert_eq!(brain.next_wake(2100), Some(2100 + DROP_MS));
    }

    #[test]
    fn squash_follows_the_keyframes_and_settles() {
        let mut brain = Brain::new(0);
        assert_eq!(brain.squash(50), (1.0, 1.0));
        brain.poke(1000);
        let (x, y) = brain.squash(1075);
        assert!((x - 1.1).abs() < 1e-4 && (y - 0.86).abs() < 1e-4);
        assert_eq!(brain.squash(1420), (1.0, 1.0));
    }
}
