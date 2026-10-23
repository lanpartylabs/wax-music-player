//! The volume: a speaker and a slider beside the play buttons. A press on the
//! speaker turns the sound off and back on; a press or a drag on the slider
//! sets how loud, and so does the wheel over either; `-` and `=` step it down
//! and up, and `m` turns the sound off and on.
//!
//! Where the slider stands is kept in the settings, written once it has been
//! still for a moment, so that a drag or a turn of the wheel writes it once.

use eframe::egui::{Painter, Pos2, Rect, Stroke, pos2, vec2};

use super::*;
use crate::draw;

/// The control across: the speaker, then the slider.
pub(super) const WIDTH: f32 = 112.0;
const SPEAKER: f32 = 22.0;
/// How far a key press, or a notch of the wheel, moves the slider.
const STEP: f32 = 0.05;
/// How long the slider is still before where it stands is saved.
const SETTLE: Duration = Duration::from_millis(500);
/// Where the slider is put when the sound is turned back on at the bottom of it.
const BACK_ON: f32 = 0.5;

fn speaker_rect(control: Rect) -> Rect {
    Rect::from_min_size(control.min, vec2(SPEAKER, control.height()))
}

fn slider_rect(control: Rect) -> Rect {
    Rect::from_min_max(pos2(control.left() + SPEAKER + 6.0, control.top()), control.max)
}

/// Where the slider's thumb may travel.
fn travel(slider: Rect) -> (f32, f32) {
    (slider.left() + 5.0, slider.right() - 5.0)
}

impl App {
    /// How loud the player is told to play: nothing while the sound is off.
    pub(super) fn heard(&self) -> f32 {
        if self.settings.mute { 0.0 } else { self.settings.volume }
    }

    /// Moves the slider to `level`, from 0 to 1, and turns the sound back on if it was off.
    fn set_volume(&mut self, level: f32) {
        // Kept to a thousandth, which no ear tells apart, so that the settings say 0.7 and not 0.6999999.
        self.settings.volume = (level.clamp(0.0, 1.0) * 1000.0).round() / 1000.0;
        self.settings.mute = false;
        self.player.set_volume(self.heard());
        self.volume_moved = Some(Instant::now());
    }

    /// Moves the slider `steps` steps up, or down when negative.
    pub(super) fn step_volume(&mut self, steps: f32) {
        self.set_volume(self.settings.volume + steps * STEP);
    }

    /// Turns the sound off, or back on where the slider stands.
    pub(super) fn toggle_mute(&mut self) {
        if self.heard() > 0.0 {
            self.settings.mute = true;
            self.player.set_volume(0.0);
            self.volume_moved = Some(Instant::now());
        } else {
            // Back on at the bottom of the slider would still be silent.
            self.set_volume(if self.settings.volume > 0.0 { self.settings.volume } else { BACK_ON });
        }
    }

    /// A press on the volume: the speaker turns the sound off or on, the slider is taken hold of.
    pub(super) fn volume_press(&mut self, pos: Pos2, control: Rect) {
        if speaker_rect(control).contains(pos) {
            self.toggle_mute();
        } else if slider_rect(control).contains(pos) {
            self.volume_held = true;
            self.slide_volume(slider_rect(control), pos.x);
        }
    }

    /// Follows the pointer while the slider is held.
    pub(super) fn volume_drag(&mut self, pos: Option<Pos2>, down: bool, control: Rect) {
        match (self.volume_held, down, pos) {
            (true, true, Some(pos)) => self.slide_volume(slider_rect(control), pos.x),
            (true, false, _) => self.volume_held = false,
            _ => {}
        }
    }

    /// The wheel over the volume: a step a notch, or smoothly from a trackpad.
    pub(super) fn volume_wheel(&mut self, pixels: f32) {
        self.step_volume(pixels / WHEEL_LINE);
    }

    fn slide_volume(&mut self, slider: Rect, x: f32) {
        let (from, to) = travel(slider);
        self.set_volume((x - from) / (to - from));
    }

    /// Saves where the volume stands once it has been still for a moment, or at once when `now`.
    pub(super) fn keep_volume(&mut self, now: bool) {
        let Some(moved) = self.volume_moved else { return };
        let still = moved.elapsed();
        if now || (still >= SETTLE && !self.volume_held) {
            self.volume_moved = None;
            self.settings.save();
        } else {
            self.ctx.request_repaint_after(SETTLE.saturating_sub(still));
        }
    }
}

pub(super) fn paint(app: &App, p: &Painter, control: Rect) {
    if control.width() <= 0.0 {
        return;
    }
    let level = app.settings.volume;
    let on = app.heard() > 0.0;
    let waves = match level {
        _ if !on => None,
        level if level < 0.5 => Some(1),
        _ => Some(2),
    };
    draw::speaker(p, speaker_rect(control).center(), waves, theme::MUTED);

    // A thin track filled up to the thumb; lit while the pointer is on it. With the sound off it stays where it was, pale.
    let lit = app.volume_held || app.pointer.is_some_and(|at| control.contains(at));
    let slider = slider_rect(control);
    let ((from, to), mid) = (travel(slider), slider.center().y);
    let x = from + level * (to - from);
    let fill = match (on, lit) {
        (false, _) => theme::FAINT,
        (true, true) => theme::ACCENT,
        (true, false) => theme::MUTED,
    };
    p.rect_filled(Rect::from_min_max(pos2(from, mid - 1.5), pos2(to, mid + 1.5)), 1.5, theme::LINE);
    p.rect_filled(Rect::from_min_max(pos2(from, mid - 1.5), pos2(x, mid + 1.5)), 1.5, fill);
    p.circle_filled(pos2(x, mid), 5.5, theme::WELL);
    p.circle_stroke(pos2(x, mid), 5.5, Stroke::new(1.0, if lit { theme::ACCENT } else { theme::FAINT }));
}
