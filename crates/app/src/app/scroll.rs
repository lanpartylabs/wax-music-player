//! Scrollbars: one along the right edge of every list and grid that scrolls.
//! Taking hold of the thumb drags the view with it; a press elsewhere on the
//! bar brings the thumb there, centred on the pointer, and holds it from
//! there. A bar widens while the pointer is over it or holds it.

use eframe::egui::{Color32, Painter, Pos2, Rect, pos2};

use super::*;

/// The strip along the right edge of a scrolling area that belongs to its bar.
const STRIP: f32 = 14.0;
const MIN_THUMB: f32 = 24.0;

/// What a scrollbar moves.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Target {
    AlbumGrid,
    /// The track list of the album opened from the album grid.
    AlbumTracks,
    ArtistList,
    /// The artist's albums.
    ArtistGrid,
    ArtistTracks,
    Songs,
}

/// A scrollbar held by the pointer: what it moves, and how far below the thumb's top it was taken hold of.
#[derive(Clone, Copy)]
pub(super) struct Drag {
    target: Target,
    grab: f32,
}

/// Where a scrollbar is this frame.
pub(super) struct Bar {
    area: Rect,
    track: Rect,
    thumb: Rect,
    /// The share of the whole in view.
    size: f32,
    /// The whole, in what the view scrolls by: pixels for a grid, rows for a list.
    whole: f32,
}

impl Bar {
    /// The bar of `area` when `start` of the whole is above the view and `size` of it in view; None when it all fits.
    fn new(area: Rect, (start, size, whole): (f32, f32, f32)) -> Option<Bar> {
        if size >= 1.0 || area.height() <= MIN_THUMB {
            return None;
        }
        let height = (area.height() * size).max(MIN_THUMB);
        let top = area.top() + (start / (1.0 - size)).clamp(0.0, 1.0) * (area.height() - height);
        let track = Rect::from_min_max(pos2(area.right() - STRIP, area.top()), area.max);
        let thumb = Rect::from_min_max(pos2(track.left(), top), pos2(track.right(), top + height));
        Some(Bar { area, track, thumb, size, whole })
    }

    /// How far into the whole the view starts, in what it scrolls by, with the thumb's top at `y`.
    fn offset_at(&self, y: f32) -> f32 {
        let travel = self.area.height() - self.thumb.height();
        let share = if travel > 0.0 { ((y - self.area.top()) / travel).clamp(0.0, 1.0) } else { 0.0 };
        share * (1.0 - self.size) * self.whole
    }
}

/// How far a grid is scrolled, how much of it is in view, and how tall it is.
fn of_grid(grid: &Grid, area: Rect, tile: f32, len: usize) -> (f32, f32, f32) {
    let content = Cells::new(area, tile).content_height(len).max(1.0);
    (grid.scroll / content, area.height() / content, content)
}

/// The same of a list: by rows.
fn of_list(list: &List, len: usize) -> (f32, f32, f32) {
    let shown = list.shown(len);
    match len {
        0 => (0.0, 1.0, 0.0),
        _ => (shown.start as f32 / len as f32, shown.len() as f32 / len as f32, len as f32),
    }
}

impl App {
    /// Every scrollbar on screen, with what it moves.
    fn bars(&self, l: &Layout) -> Vec<(Target, Bar)> {
        let tile = self.settings.cover;
        let mut bars = Vec::new();
        let mut add = |target, area, measure| bars.extend(Bar::new(area, measure).map(|bar| (target, bar)));
        match self.form() {
            Form::Grid => match &self.albums_browser.detail {
                Some(detail) => add(Target::AlbumTracks, detail_rects(l.main).rows, of_list(&detail.list, detail.lines.len())),
                None => add(Target::AlbumGrid, l.main, of_grid(&self.albums_browser.grid, l.main, tile, self.albums.len())),
            },
            Form::Artists => {
                let (list, pane) = artists_rects(l.main);
                add(Target::ArtistList, list, of_list(&self.artist_list, self.result.artists.len()));
                if self.result.artists.get(self.artist_list.cursor).is_some() {
                    let body = pane_rects(pane).1;
                    match &self.artist_browser.detail {
                        Some(detail) => add(Target::ArtistTracks, detail_rects(body).rows, of_list(&detail.list, detail.lines.len())),
                        None => add(Target::ArtistGrid, body, of_grid(&self.artist_browser.grid, body, tile, self.artist_albums.len())),
                    }
                }
            }
            Form::Table => add(Target::Songs, songs_rects(l.main).1, of_list(&self.songs, self.result.rows.len())),
            Form::Settings => {}
        }
        bars
    }

    /// Whether `pos` is on a scrollbar: a press there is the bar's, not the content's under it.
    pub(super) fn on_scrollbar(&self, pos: Pos2, l: &Layout) -> bool {
        self.bars(l).iter().any(|(_, bar)| bar.track.contains(pos))
    }

    /// A press on a scrollbar takes hold of it. Returns whether the press was on one.
    pub(super) fn scrollbar_press(&mut self, pos: Pos2, l: &Layout) -> bool {
        let Some((target, bar)) = self.bars(l).into_iter().find(|(_, bar)| bar.track.contains(pos)) else { return false };
        let grab = if bar.thumb.contains(pos) { pos.y - bar.thumb.top() } else { bar.thumb.height() / 2.0 };
        self.drag = Some(Drag { target, grab });
        self.scroll_to(target, &bar, pos.y - grab);
        true
    }

    /// While a scrollbar is held, the view follows the pointer; letting go lets go of it.
    pub(super) fn scrollbar_drag(&mut self, pos: Option<Pos2>, down: bool, l: &Layout) {
        let Some(drag) = self.drag else { return };
        if !down {
            self.drag = None;
            return;
        }
        if let (Some(pos), Some((_, bar))) = (pos, self.bars(l).into_iter().find(|(target, _)| *target == drag.target)) {
            self.scroll_to(drag.target, &bar, pos.y - drag.grab);
        }
    }

    /// Scrolls what `target` names so that its thumb's top is at `y`. The cursor stays where it is.
    fn scroll_to(&mut self, target: Target, bar: &Bar, y: f32) {
        let offset = bar.offset_at(y);
        let row = offset.round() as usize;
        match target {
            Target::AlbumGrid => self.albums_browser.grid.scroll = offset,
            Target::ArtistGrid => self.artist_browser.grid.scroll = offset,
            Target::AlbumTracks => self.albums_browser.detail.iter_mut().for_each(|detail| detail.list.top = row),
            Target::ArtistTracks => self.artist_browser.detail.iter_mut().for_each(|detail| detail.list.top = row),
            Target::ArtistList => self.artist_list.top = row,
            Target::Songs => self.songs.top = row,
        }
    }
}

/// Every scrollbar on screen, over the content it scrolls.
pub(super) fn paint(app: &App, p: &Painter, l: &Layout) {
    for (target, bar) in app.bars(l) {
        let held = app.drag.is_some_and(|drag| drag.target == target);
        let hot = held || app.pointer.is_some_and(|pos| bar.track.contains(pos));
        let (width, inset, alpha) = if hot { (8.0, 3.0, if held { 110 } else { 80 }) } else { (4.0, 3.0, 48) };
        if hot {
            p.rect_filled(bar.track, 0.0, Color32::from_black_alpha(10));
        }
        let x = bar.track.right() - inset;
        let thumb = Rect::from_min_max(pos2(x - width, bar.thumb.top()), pos2(x, bar.thumb.bottom()));
        p.rect_filled(thumb, width / 2.0, Color32::from_black_alpha(alpha));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_thumb_follows_the_view_and_the_view_follows_the_thumb() {
        let area = Rect::from_min_max(pos2(0.0, 100.0), pos2(500.0, 600.0));
        // A list of 100 rows, 20 in view, the first 40 above it.
        let bar = Bar::new(area, (0.4, 0.2, 100.0)).unwrap();
        assert_eq!(bar.thumb.height(), 100.0);
        assert_eq!(bar.thumb.top(), 100.0 + 0.5 * 400.0, "halfway down the travel");
        assert_eq!(bar.offset_at(bar.thumb.top()).round(), 40.0, "the thumb where it is says the view where it is");
        assert_eq!(bar.offset_at(100.0), 0.0);
        assert_eq!(bar.offset_at(500.0), 80.0, "at the foot: the last rows in view");
        assert_eq!(bar.offset_at(2000.0), 80.0, "never past the foot");
        assert!(bar.track.contains(pos2(495.0, 105.0)) && !bar.track.contains(pos2(480.0, 105.0)), "the bar is the strip along the right edge");
        assert!(Bar::new(area, (0.0, 1.0, 10.0)).is_none(), "no bar when everything fits");
    }
}
