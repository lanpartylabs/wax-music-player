//! Cursor and scroll state of the scrolling lists and grids.

use std::ops::Range;

use eframe::egui::{Key, Modifiers, Pos2, Rect, pos2, vec2};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Move {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    HalfUp,
    HalfDown,
    Top,
    Bottom,
}

impl Move {
    /// Arrow keys, paging keys and their vim equivalents.
    pub fn from_key(key: Key, m: Modifiers) -> Option<Move> {
        Some(match key {
            Key::D if m.ctrl => Move::HalfDown,
            Key::U if m.ctrl => Move::HalfUp,
            Key::J | Key::ArrowDown => Move::Down,
            Key::K | Key::ArrowUp => Move::Up,
            Key::H | Key::ArrowLeft => Move::Left,
            // Shift-L likes a song.
            Key::L if !m.shift => Move::Right,
            Key::ArrowRight => Move::Right,
            Key::PageDown => Move::PageDown,
            Key::PageUp => Move::PageUp,
            Key::G if m.shift => Move::Bottom,
            Key::End => Move::Bottom,
            Key::G | Key::Home => Move::Top,
            _ => return None,
        })
    }
}

/// Rows kept visible above and below the cursor while it moves.
const SCROLL_MARGIN: usize = 3;

/// A vertical list of fixed-height rows.
#[derive(Clone, Copy, Default)]
pub struct List {
    pub cursor: usize,
    /// First visible row.
    pub top: usize,
    /// Rows that fitted at the last paint; drives paging.
    page: usize,
    /// Scroll so the cursor is in view at the next paint.
    reveal: bool,
    /// Wheel movement smaller than one row, carried to the next event.
    wheel: f32,
}

impl List {
    /// Moves the cursor and reports whether it changed. Sideways moves are ignored.
    pub fn go(&mut self, mv: Move, len: usize) -> bool {
        let page = self.page.max(1) as isize;
        let delta = match mv {
            Move::Up => -1,
            Move::Down => 1,
            Move::PageUp => -page,
            Move::PageDown => page,
            Move::HalfUp => -(page / 2).max(1),
            Move::HalfDown => (page / 2).max(1),
            Move::Top => isize::MIN,
            Move::Bottom => isize::MAX,
            Move::Left | Move::Right => return false,
        };
        self.set(self.cursor.saturating_add_signed(delta), len)
    }

    pub fn set(&mut self, row: usize, len: usize) -> bool {
        let row = row.min(len.saturating_sub(1));
        let changed = row != self.cursor;
        self.cursor = row;
        self.reveal = true;
        changed
    }

    /// Fits the list to `visible` rows; call once per frame before painting.
    pub fn settle(&mut self, len: usize, visible: usize) {
        self.page = visible.max(1);
        self.cursor = self.cursor.min(len.saturating_sub(1));
        if self.reveal {
            let margin = SCROLL_MARGIN.min(visible.saturating_sub(1) / 2);
            if self.cursor < self.top + margin {
                self.top = self.cursor.saturating_sub(margin);
            } else if self.cursor + margin >= self.top + visible {
                self.top = self.cursor + margin + 1 - visible;
            }
            self.reveal = false;
        }
        self.top = self.top.min(len.saturating_sub(visible));
    }

    /// Wheel scrolling: moves the view by `rows` without moving the cursor.
    pub fn scroll(&mut self, rows: f32, len: usize) {
        self.wheel += rows;
        let whole = self.wheel.trunc();
        self.wheel -= whole;
        let last_top = len.saturating_sub(self.page) as f32;
        self.top = (self.top as f32 + whole).clamp(0.0, last_top) as usize;
    }

    /// Rows to draw.
    pub fn shown(&self, len: usize) -> Range<usize> {
        self.top.min(len)..(self.top + self.page).min(len)
    }

    /// The row under `pos` in a list drawn in `area` with rows `row_height` tall.
    pub fn hit(&self, area: Rect, row_height: f32, pos: Pos2, len: usize) -> Option<usize> {
        let row = self.top + ((pos.y - area.top()) / row_height) as usize;
        (area.contains(pos) && row < len).then_some(row)
    }
}

const PAD: f32 = 26.0;
const GAP: f32 = 24.0;
/// Height of the two text lines under a cover, and the space below them.
pub const CAPTION: f32 = 42.0;
const ROW_GAP: f32 = 16.0;

/// A grid of square tiles with captions, scrolled by pixels.
#[derive(Clone, Copy, Default)]
pub struct Grid {
    pub cursor: usize,
    pub scroll: f32,
    cols: usize,
    rows_visible: usize,
    reveal: bool,
}

/// Geometry of a grid fitted to an area.
#[derive(Clone, Copy)]
pub struct Cells {
    pub area: Rect,
    pub cols: usize,
    /// Edge length of a cover.
    pub tile: f32,
    step_x: f32,
    step_y: f32,
}

impl Cells {
    /// `min_tile` is the smallest a cover may be drawn; the grid takes as many columns as that allows.
    pub fn new(area: Rect, min_tile: f32) -> Self {
        let inner = (area.width() - 2.0 * PAD).max(min_tile);
        let cols = ((inner + GAP) / (min_tile + GAP)).floor().max(1.0) as usize;
        let tile = ((inner - GAP * (cols - 1) as f32) / cols as f32).floor();
        Cells { area, cols, tile, step_x: tile + GAP, step_y: tile + CAPTION + ROW_GAP }
    }

    /// The cover square of tile `index` at scroll offset `scroll`.
    pub fn cover(&self, index: usize, scroll: f32) -> Rect {
        let (row, col) = (index / self.cols, index % self.cols);
        let min = pos2(
            self.area.left() + PAD + col as f32 * self.step_x,
            self.area.top() + PAD + row as f32 * self.step_y - scroll,
        );
        Rect::from_min_size(min, vec2(self.tile, self.tile))
    }

    /// Tiles on rows that are at least partly visible.
    pub fn shown(&self, scroll: f32, len: usize) -> Range<usize> {
        let first_row = ((scroll - PAD) / self.step_y).floor().max(0.0) as usize;
        let last_row = ((scroll + self.area.height() - PAD) / self.step_y).ceil().max(0.0) as usize;
        (first_row * self.cols).min(len)..(last_row * self.cols).min(len)
    }

    pub fn hit(&self, pos: Pos2, scroll: f32, len: usize) -> Option<usize> {
        if !self.area.contains(pos) {
            return None;
        }
        self.shown(scroll, len).find(|&i| {
            let cover = self.cover(i, scroll);
            Rect::from_min_max(cover.min, pos2(cover.right(), cover.bottom() + CAPTION)).contains(pos)
        })
    }

    pub fn content_height(&self, len: usize) -> f32 {
        2.0 * PAD + len.div_ceil(self.cols) as f32 * self.step_y - ROW_GAP
    }
}

impl Grid {
    /// Moves the cursor. Returns true when asked to go left from the first
    /// column, which callers treat as "leave the grid".
    pub fn go(&mut self, mv: Move, len: usize) -> bool {
        if len == 0 {
            return mv == Move::Left;
        }
        let cols = self.cols.max(1);
        let page = cols * self.rows_visible.max(1);
        let last = len - 1;
        let target = match mv {
            Move::Left if self.cursor.is_multiple_of(cols) => return true,
            Move::Left => self.cursor - 1,
            Move::Right => self.cursor + 1,
            Move::Up => self.cursor.checked_sub(cols).unwrap_or(self.cursor),
            // From the row above a short last row, Down still reaches that row.
            Move::Down if self.cursor + cols > last && self.cursor / cols < last / cols => last,
            Move::Down if self.cursor + cols > last => self.cursor,
            Move::Down => self.cursor + cols,
            Move::PageUp => self.cursor.saturating_sub(page),
            Move::PageDown => self.cursor + page,
            Move::HalfUp => self.cursor.saturating_sub((page / 2).max(cols)),
            Move::HalfDown => self.cursor + (page / 2).max(cols),
            Move::Top => 0,
            Move::Bottom => last,
        };
        self.set(target, len);
        false
    }

    pub fn set(&mut self, index: usize, len: usize) {
        self.cursor = index.min(len.saturating_sub(1));
        self.reveal = true;
    }

    pub fn scroll_by(&mut self, pixels: f32) {
        self.scroll += pixels;
    }

    /// Fits the grid to `cells`; call once per frame before painting.
    pub fn settle(&mut self, cells: &Cells, len: usize) {
        self.cols = cells.cols;
        self.rows_visible = (cells.area.height() / cells.step_y).floor().max(1.0) as usize;
        self.cursor = self.cursor.min(len.saturating_sub(1));
        if self.reveal {
            let top = (self.cursor / cells.cols) as f32 * cells.step_y;
            let bottom = top + cells.tile + CAPTION + 2.0 * PAD;
            if top < self.scroll {
                self.scroll = top;
            } else if bottom > self.scroll + cells.area.height() {
                self.scroll = bottom - cells.area.height();
            }
            self.reveal = false;
        }
        let limit = (cells.content_height(len) - cells.area.height()).max(0.0);
        self.scroll = self.scroll.clamp(0.0, limit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(cols: usize, cursor: usize) -> Grid {
        Grid { cursor, cols, rows_visible: 2, ..Grid::default() }
    }

    #[test]
    fn grid_moves_stay_inside_and_report_the_left_edge() {
        let mut g = grid(4, 5);
        assert!(!g.go(Move::Left, 10));
        assert_eq!(g.cursor, 4);
        assert!(g.go(Move::Left, 10), "left from the first column leaves the grid");
        assert_eq!(g.cursor, 4);

        // 10 tiles in 4 columns: the last row holds tiles 8 and 9.
        let mut g = grid(4, 7);
        g.go(Move::Down, 10);
        assert_eq!(g.cursor, 9, "down from above a missing tile lands on the last tile");
        g.go(Move::Down, 10);
        assert_eq!(g.cursor, 9);
        g.go(Move::Right, 10);
        assert_eq!(g.cursor, 9);
        g.go(Move::Top, 10);
        g.go(Move::Up, 10);
        assert_eq!(g.cursor, 0);
        g.go(Move::PageDown, 10);
        assert_eq!(g.cursor, 8);
    }

    #[test]
    fn list_cursor_stays_in_view() {
        let mut list = List::default();
        list.settle(100, 10);
        for _ in 0..20 {
            list.go(Move::Down, 100);
            list.settle(100, 10);
        }
        assert_eq!(list.cursor, 20);
        assert!(list.shown(100).contains(&20));
        assert!(!list.go(Move::Left, 100));
        list.go(Move::Bottom, 100);
        list.settle(100, 10);
        assert_eq!((list.cursor, list.top), (99, 90));
        list.settle(3, 10);
        assert_eq!((list.cursor, list.top), (2, 0));
    }
}
