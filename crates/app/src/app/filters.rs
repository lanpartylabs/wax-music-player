//! The filter bar: narrows everything the window shows by genre, kind of
//! release, sound quality and decade, and over an album grid says, at its
//! right end, what order the albums are in. `f` brings it up and takes the
//! keyboard to it; it stays for as long as a filter is set, and always over a
//! grid.

use eframe::egui::{Align2, Color32, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, pos2, vec2};
use wax_core::Sound;

use super::*;
use crate::draw;

pub(super) const BAR: f32 = 44.0;
const CHIP: (f32, f32) = (178.0, 28.0);
/// The sort chip is wider: its choices are longer.
const SORT_CHIP: f32 = 208.0;
/// The narrowest a filter's chip gets, to leave the sort room in a narrow window.
const MIN_CHIP: f32 = 128.0;
const CLEAR: f32 = 52.0;
const GAP: f32 = 10.0;
/// Lines up with the covers of the grid below.
const PAD: f32 = 26.0;
const MENU_ROW: f32 = 26.0;
/// Rows of a menu shown at once.
const MENU_ROWS: usize = 12;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Filter {
    Genre,
    Kind,
    Sound,
    Decade,
    /// Not a filter: the order of an album grid, at the bar's right end.
    Sort,
}

impl Filter {
    /// The filters proper, left to right.
    const ALL: [Filter; 4] = [Filter::Genre, Filter::Kind, Filter::Sound, Filter::Decade];

    fn label(self) -> &'static str {
        match self {
            Filter::Genre => "Genre",
            Filter::Kind => "Kind",
            Filter::Sound => "Sound",
            Filter::Decade => "Decade",
            Filter::Sort => "Sort",
        }
    }

    /// Whether its menu starts with "All": every filter's does; the sort's does not.
    fn all_row(self) -> usize {
        usize::from(self != Filter::Sort)
    }
}

/// What is asked of each filter. Genres and kinds are kept by name, so that
/// they hold when the window turns to the other index.
#[derive(Clone, Default, PartialEq)]
pub(super) struct Filters {
    pub genre: Option<String>,
    pub kind: Option<String>,
    pub sound: Option<Sound>,
    pub decade: Option<u32>,
}

impl Filters {
    /// The choice made for a filter, as its menu words it.
    fn chosen(&self, filter: Filter) -> Option<String> {
        match filter {
            Filter::Genre => self.genre.clone(),
            Filter::Kind => self.kind.clone(),
            Filter::Sound => self.sound.map(|sound| sound.label().to_owned()),
            Filter::Decade => self.decade.map(|decade| format!("{decade}s")),
            Filter::Sort => None,
        }
    }
}

/// The open list of one filter's choices; row 0 is "All".
pub(super) struct Menu {
    filter: Filter,
    pub list: List,
    rows: usize,
}

/// How wide a filter's chip is: narrower than it would be, down to a point, where the bar has no room otherwise for the
/// filters, "Clear" and the sort.
fn chip_width(bar: Rect) -> f32 {
    let filters = Filter::ALL.len() as f32;
    let room = bar.width() - 2.0 * PAD - (CLEAR + 2.0 * GAP + 6.0) - SORT_CHIP - GAP * (filters - 1.0);
    (room / filters).clamp(MIN_CHIP, CHIP.0)
}

fn chip_rect(bar: Rect, filter: Filter) -> Rect {
    let (top, width) = (bar.center().y - CHIP.1 / 2.0, chip_width(bar));
    match Filter::ALL.iter().position(|&f| f == filter) {
        Some(index) => Rect::from_min_size(pos2(bar.left() + PAD + index as f32 * (width + GAP), top), vec2(width, CHIP.1)),
        // At the right end, but never over "Clear".
        None => Rect::from_min_size(pos2((bar.right() - PAD - SORT_CHIP).max(clear_rect(bar).right() + GAP), top), vec2(SORT_CHIP, CHIP.1)),
    }
}

/// "Clear", after the last filter.
fn clear_rect(bar: Rect) -> Rect {
    let last = chip_rect(bar, Filter::ALL[Filter::ALL.len() - 1]);
    Rect::from_min_size(pos2(last.right() + GAP + 6.0, last.top()), vec2(CLEAR, CHIP.1))
}

/// A filter's menu hangs from the left of its chip; the sort's, at the right end, from its right.
fn menu_rect(bar: Rect, menu: &Menu) -> Rect {
    let chip = chip_rect(bar, menu.filter);
    let height = menu.rows.min(MENU_ROWS) as f32 * MENU_ROW + 12.0;
    let width = chip.width() + 44.0;
    let left = if menu.filter == Filter::Sort { chip.right() - width } else { chip.left() };
    Rect::from_min_size(pos2(left, chip.bottom() + 4.0), vec2(width, height))
}

/// The rows inside a menu's frame.
fn menu_rows(frame: Rect) -> Rect {
    frame.shrink2(vec2(6.0, 6.0))
}

impl App {
    /// Whether the bar is on screen: over an album grid, asked for, or holding a filter.
    pub(super) fn filters_shown(&self) -> bool {
        self.form() == Form::Grid || (self.form() != Form::Settings && (self.filter_bar || self.filters != Filters::default()))
    }

    /// The chips on the bar, left to right: the filters, and over an album grid its order.
    fn chips(&self) -> Vec<Filter> {
        let mut chips = Filter::ALL.to_vec();
        if self.form() == Form::Grid {
            chips.push(Filter::Sort);
        }
        chips
    }

    /// What a chip shows as chosen: a filter's choice, or the grid's order; None for a filter left at "All".
    fn shown_choice(&self, filter: Filter) -> Option<String> {
        match filter {
            Filter::Sort => Some(self.settings.album_sort.label().to_owned()),
            _ => self.filters.chosen(filter),
        }
    }

    /// What a filter can be set to, going by the index on screen.
    fn choices(&self, filter: Filter) -> Vec<String> {
        let named = |names: &[String]| names.iter().filter(|name| !name.is_empty()).cloned().collect();
        match filter {
            Filter::Genre => named(&self.lib.genres),
            Filter::Kind => named(&self.lib.kinds),
            Filter::Sound => Sound::ALL.iter().map(|sound| sound.label().to_owned()).collect(),
            Filter::Decade => self.lib.decades.iter().map(|decade| format!("{decade}s")).collect(),
            Filter::Sort => AlbumOrder::ALL.iter().map(|order| order.label().to_owned()).collect(),
        }
    }

    /// `f`: brings the bar up and takes the keyboard to it, or hands the keyboard back.
    pub(super) fn filter_pressed(&mut self) {
        if self.form() == Form::Settings {
            return;
        }
        if self.focus == Focus::Filters {
            self.leave_filters();
        } else {
            self.filter_bar = true;
            self.focus = Focus::Filters;
        }
    }

    /// Hands the keyboard back to the content; the bar goes with it unless a filter is set.
    pub(super) fn leave_filters(&mut self) {
        self.menu = None;
        self.filter_bar = false;
        if self.focus == Focus::Filters {
            self.focus = Focus::Main;
        }
    }

    fn open_menu(&mut self, filter: Filter) {
        let (choices, all) = (self.choices(filter), filter.all_row());
        let at = self.shown_choice(filter).and_then(|chosen| choices.iter().position(|choice| *choice == chosen)).map_or(0, |i| i + all);
        let mut list = List::default();
        list.set(at, choices.len() + all);
        self.menu = Some(Menu { filter, list, rows: choices.len() + all });
    }

    /// Sets a filter to row `row` of its menu and looks everything up again; or puts the grid in the order on that row.
    fn choose(&mut self, filter: Filter, row: usize) {
        self.menu = None;
        if filter == Filter::Sort {
            self.sort_albums(AlbumOrder::ALL.get(row).copied().unwrap_or_default());
            return;
        }
        let choice = row.checked_sub(1).and_then(|i| self.choices(filter).into_iter().nth(i));
        match filter {
            Filter::Genre => self.filters.genre = choice,
            Filter::Kind => self.filters.kind = choice,
            Filter::Sound => self.filters.sound = choice.and_then(|label| Sound::ALL.into_iter().find(|sound| sound.label() == label)),
            Filter::Decade => self.filters.decade = choice.and_then(|label| label.trim_end_matches('s').parse().ok()),
            Filter::Sort => {}
        }
        self.search_changed();
    }

    /// Keys while the bar has the keyboard and no menu is open. Backspace sets a filter back to "All", and the order to the first.
    pub(super) fn filter_key(&mut self, key: Key, mv: Option<Move>) {
        let chips = self.chips();
        let last = chips.len() - 1;
        match (key, mv) {
            (Key::Enter, _) | (_, Some(Move::Down)) => self.open_menu(chips[self.filter_cursor.min(last)]),
            (Key::Backspace | Key::Delete, _) => self.choose(chips[self.filter_cursor.min(last)], 0),
            (_, Some(Move::Left)) => self.filter_cursor = self.filter_cursor.saturating_sub(1),
            (_, Some(Move::Right)) => self.filter_cursor = (self.filter_cursor + 1).min(last),
            (_, Some(Move::Up)) => self.leave_filters(),
            _ => {}
        }
    }

    /// Keys while a menu is open.
    pub(super) fn menu_key(&mut self, key: Key, mv: Option<Move>) {
        let Some(menu) = &mut self.menu else { return };
        match (key, mv) {
            (Key::Enter, _) => {
                let (filter, row) = (menu.filter, menu.list.cursor);
                self.choose(filter, row);
            }
            (Key::Escape, _) | (_, Some(Move::Left)) => self.menu = None,
            (_, Some(mv)) => {
                menu.list.go(mv, menu.rows);
            }
            _ => {}
        }
    }

    /// A press on the bar or its menu. Returns whether it landed there, so the content under a menu does not get it too.
    pub(super) fn filters_press(&mut self, pos: Pos2, bar: Rect) -> bool {
        if let Some(menu) = &self.menu {
            let frame = menu_rect(bar, menu);
            let (filter, hit) = (menu.filter, menu.list.hit(menu_rows(frame), MENU_ROW, pos, menu.rows));
            if let Some(row) = hit {
                self.choose(filter, row);
            } else {
                self.menu = None;
            }
            if hit.is_some() || frame.contains(pos) {
                return true;
            }
        }
        if !bar.contains(pos) {
            return false;
        }
        let chips = self.chips();
        if let Some(index) = chips.iter().position(|&chip| chip_rect(bar, chip).contains(pos)) {
            self.focus = Focus::Filters;
            self.filter_cursor = index;
            self.open_menu(chips[index]);
        } else if clear_rect(bar).contains(pos) && self.filters != Filters::default() {
            self.filters = Filters::default();
            self.search_changed();
        }
        true
    }

    /// Wheel over an open menu. Returns whether the pointer was over it.
    pub(super) fn filters_wheel(&mut self, pos: Pos2, pixels: f32, bar: Rect) -> bool {
        let Some(menu) = &mut self.menu else { return false };
        let over = menu_rect(bar, menu).contains(pos);
        if over {
            menu.list.scroll(-pixels / MENU_ROW, menu.rows);
        }
        over
    }

    pub(super) fn settle_menu(&mut self) {
        if let Some(menu) = &mut self.menu {
            menu.list.settle(menu.rows, menu.rows.min(MENU_ROWS));
        }
    }
}

pub(super) fn paint(app: &App, p: &Painter, bar: Rect) {
    let fonts = &app.fonts;
    let line = Stroke::new(1.0, theme::LINE);
    p.rect_filled(bar, 0.0, theme::BG);
    p.hline(bar.x_range(), bar.bottom() - 0.5, line);

    for (i, filter) in app.chips().into_iter().enumerate() {
        let chip = chip_rect(bar, filter);
        let chosen = app.shown_choice(filter);
        let lit = app.focus == Focus::Filters && app.filter_cursor == i;
        let open = app.menu.as_ref().is_some_and(|menu| menu.filter == filter);
        let edge = if lit || open { Stroke::new(1.5, theme::ACCENT) } else { line };
        p.rect_filled(chip, 14.0, theme::WELL);
        p.rect_stroke(chip, 14.0, edge, StrokeKind::Inside);

        let mid = chip.center().y;
        let used = draw::text(p, pos2(chip.left() + 13.0, mid), Align2::LEFT_CENTER, 60.0, filter.label(), &fonts.small, theme::MUTED);
        // A filter that narrows what is shown stands out; the order narrows nothing.
        let (value, ink) = match &chosen {
            Some(value) if filter != Filter::Sort => (value.as_str(), theme::ACCENT),
            Some(value) => (value.as_str(), theme::TEXT),
            None => ("All", theme::TEXT),
        };
        draw::text(p, pos2(chip.left() + 13.0 + used + 8.0, mid), Align2::LEFT_CENTER, chip.width() - used - 46.0, value, &fonts.body, ink);
        caret(p, pos2(chip.right() - 15.0, mid + 1.0), theme::MUTED);
    }
    if app.filters != Filters::default() {
        let clear = clear_rect(bar);
        draw::text(p, clear.left_center(), Align2::LEFT_CENTER, clear.width(), "Clear", &fonts.body, theme::ACCENT);
    }
}

/// The open menu, drawn over the content.
pub(super) fn paint_menu(app: &App, p: &Painter, bar: Rect) {
    let Some(menu) = &app.menu else { return };
    let fonts = &app.fonts;
    let frame = menu_rect(bar, menu);
    p.rect_filled(frame.translate(vec2(0.0, 3.0)).expand(2.0), 12.0, Color32::from_black_alpha(26));
    p.rect_filled(frame, 10.0, theme::WELL);
    p.rect_stroke(frame, 10.0, Stroke::new(1.0, theme::LINE), StrokeKind::Inside);

    let rows = menu_rows(frame);
    let (choices, all) = (app.choices(menu.filter), menu.filter.all_row());
    let chosen = app.shown_choice(menu.filter);
    let p = &p.with_clip_rect(rows);
    for row in menu.list.shown(menu.rows) {
        let rect = row_rect(rows, row - menu.list.top, MENU_ROW);
        let name = if row < all { "All" } else { choices[row - all].as_str() };
        let selected = row == menu.list.cursor;
        let current = if row < all { chosen.is_none() } else { chosen.as_deref() == Some(name) };
        if selected {
            p.rect_filled(rect, 6.0, theme::ACCENT);
        }
        let ink = if selected { theme::ON_ACCENT } else { theme::TEXT };
        draw::text(p, pos2(rect.left() + 12.0, rect.center().y), Align2::LEFT_CENTER, rect.width() - 40.0, name, &fonts.body, ink);
        if current {
            p.circle_filled(pos2(rect.right() - 14.0, rect.center().y), 3.0, if selected { theme::ON_ACCENT } else { theme::ACCENT });
        }
    }
}

/// A small triangle pointing down.
fn caret(p: &Painter, center: Pos2, color: Color32) {
    let points = vec![pos2(center.x - 4.0, center.y - 2.5), pos2(center.x + 4.0, center.y - 2.5), pos2(center.x, center.y + 2.5)];
    p.add(Shape::convex_polygon(points, color, Stroke::NONE));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sort_sits_at_the_right_end_clear_of_the_filters_however_wide_the_bar() {
        for width in [700.0, 900.0, 1100.0, 1740.0] {
            let bar = Rect::from_min_size(pos2(181.0, 61.0), vec2(width, BAR));
            let (sort, clear) = (chip_rect(bar, Filter::Sort), clear_rect(bar));
            assert!(sort.left() >= clear.right() + GAP - 0.5, "{width}: the sort clears \"Clear\"");
            for pair in Filter::ALL.windows(2) {
                assert!(chip_rect(bar, pair[1]).left() >= chip_rect(bar, pair[0]).right() + GAP - 0.5, "{width}: filters apart");
            }
            if width >= 900.0 {
                assert!(sort.right() <= bar.right() - PAD + 0.5, "{width}: the sort inside the bar");
                assert!((sort.right() - (bar.right() - PAD)).abs() < 0.5, "{width}: flush with the right end");
            }
            let menu = menu_rect(bar, &Menu { filter: Filter::Sort, list: List::default(), rows: AlbumOrder::ALL.len() });
            assert!((menu.right() - sort.right()).abs() < 0.5, "{width}: the sort's menu hangs from its right edge");
        }
        let wide = Rect::from_min_size(pos2(0.0, 0.0), vec2(1740.0, BAR));
        assert_eq!(chip_rect(wide, Filter::Genre).width(), CHIP.0, "full width where there is room");
    }
}
