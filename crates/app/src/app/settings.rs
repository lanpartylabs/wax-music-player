//! The settings page, and what it keeps between runs.
//!
//! A narrow column naming the sections, then one card per section, each a
//! title over rows that carry a label and a hint on the left and a value or a
//! control on the right.

use std::path::PathBuf;

use eframe::egui::{Align2, Painter, Pos2, Rect, Stroke, StrokeKind, pos2, vec2};
use serde::{Deserialize, Serialize};

use super::*;
use crate::draw;
use crate::format::{count, stamp};

use super::actions::Named;
use super::scanning::shown;

/// Cover sizes the slider offers. Thumbnails are 320 pixels, so nothing larger stays sharp.
const COVER_RANGE: (f32, f32) = (100.0, 320.0);
const COVER_STEP: f32 = 10.0;

const NAV_WIDTH: f32 = 200.0;
/// The cards' measure.
const MEASURE: f32 = 760.0;
const CARD_HEAD: f32 = 58.0;
const ROW: f32 = 56.0;
const CARD_GAP: f32 = 16.0;
const INSET: f32 = 20.0;

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Shows the tools for working on the library and on the app.
    pub developer: bool,
    /// The smallest a cover is drawn in the album grids, in points.
    pub cover: f32,
    /// The folder on this machine the music is read from; empty until it is chosen.
    pub music: String,
    /// A server the music of a librarian's catalogue is fetched from to be played, as `ssh` knows it; empty for none.
    pub server: String,
    /// The librarian's folder on the server; it hands over the files.
    pub home: String,
    /// The order the album grids are put in.
    pub album_sort: AlbumOrder,
    /// Where the volume slider stands, from 0 to 1.
    pub volume: f32,
    /// Whether the sound is off; the slider keeps its place for when it is turned back on.
    pub mute: bool,
    /// What the shuffle button shuffles, if anything.
    pub shuffle: crate::shuffle::Mode,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            developer: false,
            cover: 150.0,
            music: String::new(),
            server: String::new(),
            home: "wax".to_owned(),
            album_sort: AlbumOrder::default(),
            volume: 1.0,
            mute: false,
            shuffle: crate::shuffle::Mode::Off,
        }
    }
}

impl Settings {
    fn file() -> PathBuf {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .unwrap_or_default();
        config.join("wax/settings.json")
    }

    /// What was saved last, or the defaults when nothing readable was.
    pub fn load() -> Self {
        let mut settings: Settings =
            std::fs::read(Self::file()).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        settings.cover = settings.cover.clamp(COVER_RANGE.0, COVER_RANGE.1);
        settings.volume = settings.volume.clamp(0.0, 1.0);
        settings
    }

    pub(super) fn save(&self) {
        let file = Self::file();
        let written = file
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&file, serde_json::to_vec_pretty(self).unwrap_or_default()));
        if let Err(e) = written {
            eprintln!("cannot save the settings to {}: {e}", file.display());
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Row {
    Folder,
    Rescan,
    Index,
    Covers,
    Server,
    Contents,
    Developer,
    CoverSize,
}

impl Row {
    /// Label and hint.
    fn text(self) -> (&'static str, &'static str) {
        match self {
            Row::Folder => ("Music folder", "The folder your music is in."),
            Row::Rescan => ("Scan again", "Finds songs added, changed or removed; wax also does this as it starts."),
            Row::Index => ("Index", "The list of everything in the library."),
            Row::Covers => ("Covers", "The folder the cover thumbnails are read from."),
            Row::Server => ("Server", "A catalogue's songs are fetched from the librarian on this server, over SSH."),
            Row::Contents => ("Contents", "What the index holds, and when it was written."),
            Row::Developer => ("Developer mode", "Lists the unfinished parts of the collection in the sidebar, and shows the tools below."),
            Row::CoverSize => ("Cover size", "The smallest a cover is drawn; the grid takes as many columns as that allows."),
        }
    }
}

struct Card {
    title: &'static str,
    sub: &'static str,
    frame: Rect,
    rows: Vec<(Row, Rect)>,
}

/// Where everything on the page sits this frame.
struct Page {
    /// The column naming the sections; zero-sized when the window is too narrow for it.
    nav: Rect,
    heading: Pos2,
    cards: Vec<Card>,
}

impl Page {
    fn new(area: Rect, settings: &Settings) -> Self {
        let nav_width = if area.width() >= 720.0 { NAV_WIDTH } else { 0.0 };
        let nav = Rect::from_min_max(area.min, pos2(area.left() + nav_width, area.bottom()));
        let left = nav.right() + 32.0;
        let width = (area.right() - 32.0 - left).clamp(0.0, MEASURE);
        let heading = pos2(left, area.top() + 24.0);

        let mut library = vec![Row::Folder];
        if !settings.music.is_empty() {
            library.push(Row::Rescan);
        }
        library.extend([Row::Index, Row::Covers]);
        if !settings.server.is_empty() {
            library.push(Row::Server);
        }
        library.push(Row::Contents);
        let mut sections = vec![
            ("Library", "Where this window reads the collection from.", library),
            ("Developer", "For working on the library and on the app itself.", vec![Row::Developer]),
        ];
        if settings.developer {
            sections[1].2.push(Row::CoverSize);
        }
        let mut top = heading.y + 48.0;
        let cards = sections
            .into_iter()
            .map(|(title, sub, rows)| {
                let frame = Rect::from_min_size(pos2(left, top), vec2(width, CARD_HEAD + rows.len() as f32 * ROW + 6.0));
                let rows = rows
                    .into_iter()
                    .enumerate()
                    .map(|(i, row)| (row, Rect::from_min_size(pos2(left, frame.top() + CARD_HEAD + i as f32 * ROW), vec2(width, ROW))))
                    .collect();
                top = frame.bottom() + CARD_GAP;
                Card { title, sub, frame, rows }
            })
            .collect();
        Page { nav, heading, cards }
    }

    fn nav_item(&self, card: usize) -> Rect {
        let top = self.nav.top() + 20.0 + card as f32 * SIDEBAR_ROW;
        Rect::from_min_size(pos2(self.nav.left() + 12.0, top), vec2((self.nav.width() - 24.0).max(0.0), SIDEBAR_ROW))
    }

    /// Every row from top to bottom; the cursor is an index into this.
    fn rows(&self) -> Vec<(Row, Rect)> {
        self.cards.iter().flat_map(|card| card.rows.iter().copied()).collect()
    }

    /// The card holding row `index`, and the index of that card's first row.
    fn card_of(&self, index: usize) -> (usize, usize) {
        let mut first = 0;
        for (i, card) in self.cards.iter().enumerate() {
            if index < first + card.rows.len() || i + 1 == self.cards.len() {
                return (i, first);
            }
            first += card.rows.len();
        }
        (0, 0)
    }
}

fn toggle_rect(row: Rect) -> Rect {
    Rect::from_min_size(pos2(row.right() - INSET - 44.0, row.center().y - 13.0), vec2(44.0, 26.0))
}

/// The slider's track; the value is written to its right.
fn slider_rect(row: Rect) -> Rect {
    let right = row.right() - INSET - 62.0;
    Rect::from_min_max(pos2(right - 220.0, row.center().y - 12.0), pos2(right, row.center().y + 12.0))
}

/// Where the slider's thumb may travel.
fn slider_travel(slider: Rect) -> (f32, f32) {
    (slider.left() + 9.0, slider.right() - 9.0)
}

impl App {
    fn setting_rows(&self) -> usize {
        Page::new(Rect::ZERO, &self.settings).rows().len()
    }

    /// Whether `pos` is on the music folder's row of the settings page, laid out in `area`.
    pub(super) fn on_folder_row(&self, pos: Pos2, area: Rect) -> bool {
        self.section == Section::Settings && Page::new(area, &self.settings).rows().iter().any(|(row, rect)| *row == Row::Folder && rect.contains(pos))
    }

    pub(super) fn settings_key(&mut self, key: Key, mv: Option<Move>) {
        let rows = Page::new(Rect::ZERO, &self.settings).rows();
        let last = rows.len() - 1;
        let row = rows[self.setting.min(last)].0;
        match (key, mv) {
            (Key::Enter, _) if row == Row::Folder => self.start_naming_folder(),
            (Key::Enter, _) if row == Row::Rescan => self.scan(true),
            (Key::Enter, _) if row == Row::Developer => self.set_developer(!self.settings.developer),
            (_, Some(Move::Left)) if row == Row::CoverSize => self.set_cover(self.settings.cover - COVER_STEP),
            (_, Some(Move::Right)) if row == Row::CoverSize => self.set_cover(self.settings.cover + COVER_STEP),
            (_, Some(Move::Left)) => self.focus = Focus::Sidebar,
            (_, Some(Move::Up)) => self.setting = self.setting.saturating_sub(1),
            (_, Some(Move::Down)) => self.setting = (self.setting + 1).min(last),
            (_, Some(Move::Top)) => self.setting = 0,
            (_, Some(Move::Bottom)) => self.setting = last,
            _ => {}
        }
    }

    pub(super) fn settings_press(&mut self, pos: Pos2, area: Rect) {
        let page = Page::new(area, &self.settings);
        if let Some(card) = (0..page.cards.len()).find(|&i| page.nav_item(i).contains(pos)) {
            self.setting = (0..page.rows().len()).find(|&row| page.card_of(row).0 == card).unwrap_or(0);
            return;
        }
        let Some((index, (row, rect))) = page.rows().into_iter().enumerate().find(|(_, (_, rect))| rect.contains(pos)) else {
            return;
        };
        self.setting = index;
        let naming_folder = self.renaming.as_ref().is_some_and(|naming| naming.what == Named::Folder);
        match row {
            Row::Folder if !naming_folder => self.start_naming_folder(),
            Row::Rescan => self.scan(true),
            Row::Developer if toggle_rect(rect).expand(6.0).contains(pos) => self.set_developer(!self.settings.developer),
            Row::CoverSize if slider_rect(rect).contains(pos) => {
                self.sliding = true;
                self.slide_to(slider_rect(rect), pos.x);
            }
            _ => {}
        }
    }

    /// Follows the pointer while the slider is held, and saves where it was let go.
    pub(super) fn settings_drag(&mut self, pos: Option<Pos2>, down: bool, area: Rect) {
        if !self.sliding {
            return;
        }
        let slider = Page::new(area, &self.settings).rows().into_iter().find(|(row, _)| *row == Row::CoverSize).map(|(_, rect)| slider_rect(rect));
        match (down, pos, slider) {
            (true, Some(pos), Some(slider)) => self.slide_to(slider, pos.x),
            (true, ..) => {}
            (false, ..) => {
                self.sliding = false;
                self.settings.save();
            }
        }
    }

    fn slide_to(&mut self, slider: Rect, x: f32) {
        let (from, to) = slider_travel(slider);
        let share = ((x - from) / (to - from)).clamp(0.0, 1.0);
        let cover = COVER_RANGE.0 + share * (COVER_RANGE.1 - COVER_RANGE.0);
        self.settings.cover = (cover / COVER_STEP).round() * COVER_STEP;
    }

    fn set_cover(&mut self, cover: f32) {
        self.settings.cover = cover.clamp(COVER_RANGE.0, COVER_RANGE.1);
        self.settings.save();
    }

    fn set_developer(&mut self, on: bool) {
        self.settings.developer = on;
        if on {
            self.load_rest();
        }
        self.setting = self.setting.min(self.setting_rows() - 1);
        self.settings.save();
    }
}

pub(super) fn paint(app: &App, p: &Painter, l: &Layout) {
    let p = &p.with_clip_rect(l.main);
    let page = Page::new(l.main, &app.settings);
    let fonts = &app.fonts;
    let focused = app.focus == Focus::Main;
    let line = Stroke::new(1.0, theme::LINE);
    let (current, _) = page.card_of(app.setting);

    if page.nav.width() > 0.0 {
        p.vline(page.nav.right() + 0.5, page.nav.y_range(), line);
        for (i, card) in page.cards.iter().enumerate() {
            let item = page.nav_item(i);
            if i == current {
                p.rect_filled(item, 6.0, theme::IDLE);
            }
            let ink = if i == current { theme::TEXT } else { theme::MUTED };
            draw::text(p, pos2(item.left() + 10.0, item.center().y), Align2::LEFT_CENTER, item.width() - 20.0, card.title, &fonts.body, ink);
        }
    }
    draw::text(p, page.heading, Align2::LEFT_TOP, MEASURE, "Settings", &fonts.title, theme::TEXT);

    let mut index = 0;
    for card in &page.cards {
        let frame = card.frame;
        p.rect_filled(frame, 12.0, theme::WELL);
        p.rect_stroke(frame, 12.0, line, StrokeKind::Inside);
        let (left, room) = (frame.left() + INSET, frame.width() - 2.0 * INSET);
        draw::text(p, pos2(left, frame.top() + 12.0), Align2::LEFT_TOP, room, card.title, &fonts.strong, theme::TEXT);
        draw::text(p, pos2(left, frame.top() + 32.0), Align2::LEFT_TOP, room, card.sub, &fonts.small, theme::MUTED);
        p.hline(frame.x_range(), frame.top() + CARD_HEAD - 0.5, line);

        for (n, &(row, rect)) in card.rows.iter().enumerate() {
            let selected = index == app.setting;
            let lit = selected && focused;
            index += 1;
            if n > 0 {
                p.hline((rect.left() + INSET)..=(rect.right() - INSET), rect.top() - 0.5, Stroke::new(1.0, theme::STRIPE));
            }
            if selected {
                p.rect_filled(rect.shrink2(vec2(6.0, 4.0)), 8.0, theme::STRIPE);
            }

            // What sits on the right decides how much room the words on the left have.
            let mid = rect.center().y;
            let right = rect.right() - INSET;
            let words_end = match row {
                Row::Developer => {
                    toggle(p, toggle_rect(rect), app.settings.developer, lit);
                    toggle_rect(rect).left()
                }
                Row::CoverSize => {
                    let track = slider_rect(rect);
                    slider(p, track, (app.settings.cover - COVER_RANGE.0) / (COVER_RANGE.1 - COVER_RANGE.0), lit || app.sliding);
                    draw::text(p, pos2(right, mid), Align2::RIGHT_CENTER, 60.0, &format!("{:.0} px", app.settings.cover), &fonts.body, theme::TEXT);
                    track.left()
                }
                Row::Folder if app.renaming.as_ref().is_some_and(|naming| naming.what == Named::Folder) => {
                    let field = folder_field(rect);
                    if let Some(naming) = &app.renaming {
                        field_with(p, field, naming, &fonts.body);
                    }
                    field.left()
                }
                Row::Folder | Row::Rescan | Row::Index | Row::Covers | Row::Server | Row::Contents => {
                    let value = match row {
                        Row::Folder if app.settings.music.is_empty() => "Not chosen yet: press Enter".to_owned(),
                        Row::Folder => shown(Path::new(&app.settings.music)),
                        Row::Rescan => match &app.scanning {
                            Some(scan) => scanning_words(scan),
                            None if app.source.index == folder_index() => format!("Last scanned {}", stamp(app.source.generated)),
                            None => "Press Enter".to_owned(),
                        },
                        Row::Index => shown(&app.source.index),
                        Row::Covers => shown(&app.source.art),
                        Row::Server => format!("{}:{}", app.settings.server, app.settings.home),
                        _ if app.source.generated == 0 => "Nothing yet".to_owned(),
                        _ => format!(
                            "{} albums  ·  {} songs  ·  written {}",
                            count(app.lib.albums.len()),
                            count(app.lib.len()),
                            stamp(app.source.generated)
                        ),
                    };
                    let room = (rect.width() * 0.62).min(rect.width() - 200.0);
                    right - draw::text(p, pos2(right, mid), Align2::RIGHT_CENTER, room, &value, &fonts.small, theme::MUTED)
                }
            };
            let (label, hint) = row.text();
            let room = words_end - 16.0 - left;
            draw::text(p, pos2(left, rect.top() + 11.0), Align2::LEFT_TOP, room, label, &fonts.body, theme::TEXT);
            draw::text(p, pos2(left, rect.top() + 30.0), Align2::LEFT_TOP, room, hint, &fonts.small, theme::MUTED);
        }
    }
}

/// Where the music folder is typed: the right part of its row.
fn folder_field(row: Rect) -> Rect {
    let left = row.left() + (row.width() * 0.38).max(220.0);
    Rect::from_min_max(pos2(left, row.center().y - 16.0), pos2(row.right() - INSET, row.center().y + 16.0))
}

/// A field holding what is typed so far: on the selection's tint while it is all selected, else with a caret at its end.
fn field_with(p: &Painter, field: Rect, naming: &actions::Naming, font: &eframe::egui::FontId) {
    p.rect_filled(field, 6.0, theme::BG);
    p.rect_stroke(field, 6.0, Stroke::new(1.5, theme::ACCENT), StrokeKind::Inside);
    let (left, mid) = (field.left() + 10.0, field.center().y);
    let room = field.width() - 20.0;
    if naming.selected && !naming.name.is_empty() {
        let width = draw::width(p, &naming.name, font).min(room);
        p.rect_filled(Rect::from_min_max(pos2(left - 2.0, mid - 10.0), pos2(left + width + 2.0, mid + 10.0)), 3.0, theme::PICKED);
    }
    let used = draw::text(p, pos2(left, mid), Align2::LEFT_CENTER, room, &naming.name, font, theme::TEXT);
    if !naming.selected {
        let x = left + if naming.name.is_empty() { 0.0 } else { used.min(room) } + 1.0;
        p.rect_filled(Rect::from_min_max(pos2(x, mid - 8.0), pos2(x + 1.5, mid + 8.0)), 0.0, theme::ACCENT);
    }
}

/// How far a scan has come, in a few words.
pub(super) fn scanning_words(scan: &scanning::Scan) -> String {
    let progress = scan.progress();
    match progress.found {
        0 => format!("Looking through {}", shown(scan.folder())),
        found => format!("Scanning {}: {} of {} songs", shown(scan.folder()), count(progress.read), count(found)),
    }
}

/// A pill with a thumb that sits right when on; ringed when it has the keyboard.
fn toggle(p: &Painter, rect: Rect, on: bool, lit: bool) {
    p.rect_filled(rect, 13.0, if on { theme::ACCENT } else { theme::IDLE });
    let x = if on { rect.right() - 13.0 } else { rect.left() + 13.0 };
    p.circle_filled(pos2(x, rect.center().y), 9.0, theme::ON_ACCENT);
    if lit {
        p.rect_stroke(rect.expand(3.0), 16.0, Stroke::new(1.5, theme::ACCENT), StrokeKind::Inside);
    }
}

/// A track filled up to its thumb; `share` runs from 0 at the left end to 1 at the right.
fn slider(p: &Painter, rect: Rect, share: f32, lit: bool) {
    let (from, to) = slider_travel(rect);
    let (x, mid) = (from + share.clamp(0.0, 1.0) * (to - from), rect.center().y);
    p.rect_filled(Rect::from_min_max(pos2(rect.left(), mid - 2.0), pos2(rect.right(), mid + 2.0)), 2.0, theme::LINE);
    p.rect_filled(Rect::from_min_max(pos2(rect.left(), mid - 2.0), pos2(x, mid + 2.0)), 2.0, theme::ACCENT);
    p.circle_filled(pos2(x, mid), 9.0, theme::WELL);
    p.circle_stroke(pos2(x, mid), 9.0, Stroke::new(if lit { 2.0 } else { 1.5 }, if lit { theme::ACCENT } else { theme::FAINT }));
}
