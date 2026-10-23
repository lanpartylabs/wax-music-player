//! Dragging songs onto a playlist. A press on a song, on picked songs, or on
//! an album's cover takes them up; once the pointer has moved a little with
//! the button held, they are being dragged, and letting go over a playlist
//! in the sidebar adds them there. Over Liked Songs it likes them, over "New
//! Playlist" it makes a playlist of them. Esc puts them back down.

use eframe::egui::{Align2, Color32, Painter, Pos2, Rect, Stroke, StrokeKind, pos2, vec2};

use super::*;
use crate::draw;
use crate::format::count;

/// How far the pointer moves with the button held before a press becomes a drag.
const DRAG_AFTER: f32 = 6.0;

/// Where a plain press on one of several picked songs fell: if no drag follows, the pick narrows to that song.
#[derive(Clone, Copy)]
pub(super) enum Narrow {
    /// A row of a song table.
    Table(usize),
    /// A line of the album opened in the album grid.
    Albums(usize),
    /// A line of the album opened in the artist pane.
    Artist(usize),
}

/// Songs taken up by a press.
pub(super) struct Carry {
    start: Pos2,
    tracks: Vec<u32>,
    moving: bool,
    narrow: Option<Narrow>,
}

impl Carry {
    pub(super) fn moving(&self) -> bool {
        self.moving
    }
}

impl Browser {
    /// The songs under `pos`, as a drag would take them: the picked tracks when it is one of them, else the track; on
    /// a cover, the album's.
    fn songs_under(&self, albums: &[u32], lib: &Library, area: Rect, pos: Pos2, tile: f32) -> Vec<u32> {
        match &self.detail {
            Some(detail) => {
                let row = detail.list.hit(detail_rects(area).rows, TRACK_ROW, pos, detail.lines.len());
                match row.and_then(|row| detail.track(row)) {
                    Some(id) if detail.picked.contains(&id) => detail.chosen(),
                    Some(id) => vec![id],
                    None => Vec::new(),
                }
            }
            None => match Cells::new(area, tile).hit(pos, self.grid.scroll, albums.len()) {
                Some(index) => Detail::open(lib, albums[index], self.state).tracks,
                None => Vec::new(),
            },
        }
    }
}

impl App {
    /// After a press in the content: what is under it is taken up, ready to be dragged.
    pub(super) fn take_up(&mut self, pos: Pos2, l: &Layout, narrow: Option<Narrow>) {
        // Songs of the unfinished part go in no playlist.
        let under = if self.rest_shown { Vec::new() } else { self.songs_under(pos, l) };
        let tracks: Vec<u32> = under.into_iter().filter(|&id| !self.lib.tracks[id as usize].missing).collect();
        self.carry = (!tracks.is_empty() || narrow.is_some()).then_some(Carry { start: pos, tracks, moving: false, narrow });
    }

    fn songs_under(&self, pos: Pos2, l: &Layout) -> Vec<u32> {
        let tile = self.settings.cover;
        match self.form() {
            Form::Table => match self.songs.hit(songs_rects(l.main).1, SONG_ROW, pos, self.result.rows.len()) {
                Some(row) if self.picked.contains(&self.result.rows[row]) => self.chosen(),
                Some(row) => vec![self.result.rows[row]],
                None => Vec::new(),
            },
            Form::Grid => self.albums_browser.songs_under(&self.albums, &self.lib, l.main, pos, tile),
            Form::Artists => {
                let pane = artists_rects(l.main).1;
                if pane.contains(pos) {
                    self.artist_browser.songs_under(&self.artist_albums, &self.lib, pane_rects(pane).1, pos, tile)
                } else {
                    Vec::new()
                }
            }
            Form::Settings => Vec::new(),
        }
    }

    /// Every frame: with the button held, a press that has moved far enough becomes a drag; when the button comes up,
    /// a drag drops what it carries where the pointer is, and a press that was no drag narrows the pick.
    pub(super) fn carry_on(&mut self, pos: Option<Pos2>, down: bool, l: &Layout) {
        let Some(carry) = &mut self.carry else { return };
        if down {
            if let Some(pos) = pos {
                if !carry.moving && !carry.tracks.is_empty() && pos.distance(carry.start) > DRAG_AFTER {
                    carry.moving = true;
                    carry.narrow = None;
                }
            }
            return;
        }
        let Some(carry) = self.carry.take() else { return };
        if carry.moving {
            if let Some(pos) = pos {
                self.drop_on(pos, l, &carry.tracks);
            }
        } else if let Some(narrow) = carry.narrow {
            match narrow {
                Narrow::Table(row) => {
                    self.picked.clear();
                    self.anchor = row;
                }
                Narrow::Albums(row) => self.albums_browser.detail.iter_mut().for_each(|detail| detail.pick_only(row)),
                Narrow::Artist(row) => self.artist_browser.detail.iter_mut().for_each(|detail| detail.pick_only(row)),
            }
        }
    }

    /// The sidebar item under `pos` that songs can be dropped on, by its place in the sidebar.
    fn drop_target(&self, l: &Layout, pos: Pos2) -> Option<usize> {
        let index = l.items.iter().position(|item| item.contains(pos))?;
        matches!(self.sections().get(index), Some(Section::Liked | Section::Playlist(_) | Section::NewPlaylist)).then_some(index)
    }

    fn drop_on(&mut self, pos: Pos2, l: &Layout, tracks: &[u32]) {
        let Some(index) = self.drop_target(l, pos) else { return };
        match self.sections()[index] {
            Section::Liked => {
                let songs = Self::held(&self.lib, tracks);
                let n = self.store.like(&songs);
                let said = match n {
                    0 if songs.len() == 1 => "That song is liked already".to_owned(),
                    0 => "Those songs are liked already".to_owned(),
                    1 => "Liked 1 song".to_owned(),
                    n => format!("Liked {} songs", count(n)),
                };
                self.store_changed(said);
            }
            Section::Playlist(id) => self.add_to(id, tracks),
            Section::NewPlaylist => self.new_playlist(tracks),
            _ => {}
        }
    }
}

/// While songs are dragged: every place they can go outlined, the one under the pointer in the accent, and beside the
/// pointer what is carried.
pub(super) fn paint(app: &App, p: &Painter, l: &Layout) {
    let (Some(carry), Some(at)) = (app.carry.as_ref().filter(|carry| carry.moving), app.pointer) else { return };
    let under = app.drop_target(l, at);
    for (index, section) in app.sections().into_iter().enumerate() {
        if matches!(section, Section::Liked | Section::Playlist(_) | Section::NewPlaylist) {
            let (width, color) = if under == Some(index) { (2.0, theme::ACCENT) } else { (1.0, theme::LINE) };
            p.rect_stroke(l.items[index], 6.0, Stroke::new(width, color), StrokeKind::Inside);
        }
    }
    let fonts = &app.fonts;
    let what = match carry.tracks.as_slice() {
        [one] => app.lib.tracks[*one as usize].title.clone(),
        many => format!("{} songs", count(many.len())),
    };
    let width = draw::width(p, &what, &fonts.body).min(240.0) + 40.0;
    let pill = Rect::from_min_size(at + vec2(14.0, 10.0), vec2(width, 28.0));
    p.rect_filled(pill.translate(vec2(0.0, 2.0)).expand(1.0), 14.0, Color32::from_black_alpha(30));
    p.rect_filled(pill, 14.0, theme::ACCENT);
    draw::note(p, pos2(pill.left() + 17.0, pill.center().y), 5.5, theme::ON_ACCENT);
    draw::text(p, pos2(pill.left() + 30.0, pill.center().y), Align2::LEFT_CENTER, pill.width() - 40.0, &what, &fonts.body, theme::ON_ACCENT);
}
