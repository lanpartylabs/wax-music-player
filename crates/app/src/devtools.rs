//! Hooks for driving the window without a person at the keyboard, used to
//! check the UI from scripts:
//!
//! - `WAX_KEYS="Slash text:moon Enter Ctrl+D"` replays one step per frame.
//!   A step is an egui key name with optional `Ctrl+`/`Shift+`/`Alt+`
//!   prefixes, or `text:` followed by characters to type.
//!   `wait:1500` lets that many milliseconds pass before the next step.
//!   `move:X,Y`, `press:X,Y` and `release:X,Y` move the pointer to a point
//!   of the window, and press or let go of its primary button there;
//!   `rclick:X,Y` clicks the other button there.
//! - `WAX_SCREENSHOT=out.png` saves the window once the steps are done, then quits.
//! - `WAX_MUTE=1` plays at no volume, so playback can be checked without a sound.
//! - `WAX_TIMING=1` prints how long the first frame took to appear.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, ColorImage, Event, Key, Modifiers, PointerButton, Pos2, ViewportCommand, pos2};

/// Frames to let the window settle before the first step.
const WARM_UP: u32 = 3;
/// Pause between the last step and the screenshot, so covers can load.
const SETTLE: Duration = Duration::from_millis(700);

enum Step {
    Key(Key, Modifiers),
    Text(String),
    Wait(Duration),
    /// Where the pointer goes, and whether the primary button goes down (true) or up (false) there.
    Pointer(Pos2, Option<bool>),
    /// A click of the other button.
    OtherClick(Pos2),
}

pub struct Script {
    steps: VecDeque<Step>,
    screenshot: Option<PathBuf>,
    frame: u32,
    /// When the last step ran.
    idle_since: Option<Instant>,
    /// No step runs before this.
    resume_at: Option<Instant>,
    requested: bool,
    /// Process start, when the time to the first frame should be reported.
    timing: Option<Instant>,
}

impl Script {
    pub fn from_env(started: Instant) -> Self {
        Script {
            steps: std::env::var("WAX_KEYS").unwrap_or_default().split_whitespace().filter_map(parse_step).collect(),
            screenshot: std::env::var_os("WAX_SCREENSHOT").map(PathBuf::from),
            frame: 0,
            idle_since: None,
            resume_at: None,
            requested: false,
            timing: std::env::var_os("WAX_TIMING").map(|_| started),
        }
    }

    /// Runs before every frame and feeds the next step into its input.
    pub fn inject(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if let Some(started) = self.timing.take() {
            eprintln!("first frame after {} ms", started.elapsed().as_millis());
        }
        if self.steps.is_empty() && self.screenshot.is_none() {
            return;
        }
        // The screenshot comes back in a later frame, which a still window would never draw.
        if self.requested {
            ctx.request_repaint();
            return;
        }
        ctx.request_repaint();
        self.frame += 1;
        if self.frame <= WARM_UP {
            return;
        }
        if self.resume_at.is_some_and(|at| Instant::now() < at) {
            return;
        }
        match self.steps.pop_front() {
            Some(Step::Wait(time)) => self.resume_at = Some(Instant::now() + time),
            Some(Step::Key(key, modifiers)) => {
                raw.events.push(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
                // A real keyboard reports the space bar as a key and as text.
                if key == Key::Space {
                    raw.events.push(Event::Text(" ".to_owned()));
                }
            }
            Some(Step::Text(text)) => raw.events.push(Event::Text(text)),
            Some(Step::OtherClick(pos)) => {
                raw.events.push(Event::PointerMoved(pos));
                for pressed in [true, false] {
                    raw.events.push(Event::PointerButton { pos, button: PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
                }
            }
            Some(Step::Pointer(pos, button)) => {
                raw.events.push(Event::PointerMoved(pos));
                if let Some(pressed) = button {
                    raw.events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
                }
            }
            None if self.idle_since.get_or_insert_with(Instant::now).elapsed() >= SETTLE => {
                self.requested = true;
                ctx.send_viewport_cmd(ViewportCommand::Screenshot(egui::UserData::default()));
            }
            None => {}
        }
    }

    pub fn save(&self, image: &ColorImage, ctx: &egui::Context) {
        let Some(path) = &self.screenshot else { return };
        let [width, height] = image.size;
        let saved = image::save_buffer(path, image.as_raw(), width as u32, height as u32, image::ExtendedColorType::Rgba8);
        if let Err(e) = saved {
            eprintln!("could not save screenshot to {}: {e}", path.display());
        }
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }
}

fn parse_step(token: &str) -> Option<Step> {
    if let Some(text) = token.strip_prefix("text:") {
        return Some(Step::Text(text.to_owned()));
    }
    if let Some(ms) = token.strip_prefix("wait:") {
        return ms.parse().ok().map(|ms| Step::Wait(Duration::from_millis(ms)));
    }
    if let Some((x, y)) = token.strip_prefix("rclick:").and_then(|point| point.split_once(',')) {
        return Some(Step::OtherClick(pos2(x.parse().ok()?, y.parse().ok()?)));
    }
    for (prefix, button) in [("move:", None), ("press:", Some(true)), ("release:", Some(false))] {
        if let Some((x, y)) = token.strip_prefix(prefix).and_then(|point| point.split_once(',')) {
            return Some(Step::Pointer(pos2(x.parse().ok()?, y.parse().ok()?), button));
        }
    }
    let mut modifiers = Modifiers::NONE;
    let mut parts = token.split('+').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            return Key::from_name(part).map(|key| Step::Key(key, modifiers));
        }
        match part {
            "Ctrl" => modifiers |= Modifiers::CTRL | Modifiers::COMMAND,
            "Shift" => modifiers |= Modifiers::SHIFT,
            "Alt" => modifiers |= Modifiers::ALT,
            _ => return None,
        }
    }
    None
}
