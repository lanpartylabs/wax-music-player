//! The window: state, geometry and input. Painting lives in [`paint`].
//!
//! No stock widgets are used; everything is drawn directly, and every key
//! press is a plain state change followed by one sub-millisecond query.

mod actions;
mod carry;
mod filters;
mod paint;
mod scanning;
mod scroll;
mod settings;
mod volume;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Event, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, Rect, pos2, vec2};
use serde::{Deserialize, Serialize};
use wax_core::index::read_index_file;
use wax_core::{Column, Library, Query, View};

use crate::art::Art;
use crate::devtools::Script;
use crate::discard;
use crate::list::{Cells, Grid, List, Move};
use crate::player::{self, Origin, Player, Request};
use crate::playlists::{Places, Store};
use crate::theme::{self, Fonts};

use self::actions::Actions;
use self::filters::{Filters, Menu};
pub use self::scanning::{data_dir, folder_index};
pub use self::settings::Settings;

const BAR_HEIGHT: f32 = 60.0;
const STATUS_HEIGHT: f32 = 26.0;
const SIDEBAR_WIDTH: f32 = 180.0;
const SIDEBAR_ROW: f32 = 30.0;
const ARTIST_LIST_WIDTH: f32 = 240.0;
const ARTIST_ROW: f32 = 28.0;
/// Height of the artist name block above an artist's albums.
const ARTIST_HEADER: f32 = 74.0;
const SONG_HEAD: f32 = 28.0;
const SONG_ROW: f32 = 26.0;
/// Rows of an opened album's track list.
const TRACK_ROW: f32 = 30.0;
const DETAIL_COVER: f32 = 220.0;
/// Pixels scrolled per wheel notch.
const WHEEL_LINE: f32 = 60.0;

/// Order in which `s` steps through the sort columns of the song list.
const SORT_CYCLE: [Column; 8] =
    [Column::Artist, Column::Title, Column::Album, Column::Genre, Column::Year, Column::Time, Column::Added, Column::Quality];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Albums,
    Artists,
    Songs,
    /// The songs liked, the latest first.
    Liked,
    /// A playlist, by its id in the store.
    Playlist(u64),
    /// Not a place: choosing it makes a playlist and goes there.
    NewPlaylist,
    /// One state of the unfinished part of the collection, by its id there. Developer mode only.
    Rest(u32),
    /// Not part of the library: sits apart, at the foot of the sidebar.
    Settings,
}

/// How a section lays out what it shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Form {
    Grid,
    Artists,
    Table,
    Settings,
}

/// The states of unfinished files the developer view knows, in the order it
/// lists them, and whether each is made of albums (shown as a grid) or of
/// single files (shown as a table). A state it does not know is a table.
const REST_ORDER: [(&str, bool); 11] = [
    ("Missing artwork", true),
    ("No genre", true),
    ("Incomplete", true),
    // Songs of unfinished records that the ready catalogue already has, as well or better: a list, to drop them from.
    ("Can go", false),
    ("Duplicates", true),
    ("Problems", true),
    ("Loose songs", false),
    // Songs that are known, in folders that are no album MusicBrainz knows: mixes, home compilations, iPod dumps.
    ("Unknown albums", false),
    ("Likely", false),
    // Folders that look like one album, matched to no one release of it.
    ("Unclear", false),
    ("Unknown", false),
];

/// One state of the unfinished part of the collection, as the sidebar lists it.
struct RestState {
    /// Its id in the unfinished library.
    id: u32,
    name: String,
    tracks: u32,
    albums: bool,
}

/// An index that is loaded but is not the one on screen.
struct Shelved {
    lib: Library,
    info: Vec<AlbumInfo>,
    art: Art,
    source: Source,
}

/// Where the library was read from; shown on the settings page.
pub struct Source {
    pub index: PathBuf,
    pub art: PathBuf,
    /// Unix seconds at which the index was written.
    pub generated: i64,
    /// Whether the index is the librarian's ready catalogue, whose albums can be verified.
    pub ready: bool,
    /// The folder on this machine the index's paths are relative to; empty when its files are on a server.
    pub root: PathBuf,
}

/// Reads an index, leaving out the songs dropped by hand. Its covers are in `art`, or in the folder beside it.
pub fn open_index(index: &Path, art: Option<PathBuf>, gone: &HashSet<String>) -> Result<(Library, Source), String> {
    let (header, mut tracks) = read_index_file(index).map_err(|e| format!("cannot read the index at {}: {e}", index.display()))?;
    tracks.retain(|track| !gone.contains(&track.path));
    let art = art.unwrap_or_else(|| index.with_file_name("art"));
    let source = Source { index: index.to_owned(), art, generated: header.generated, ready: header.ready, root: PathBuf::from(header.root) };
    Ok((Library::new(tracks), source))
}

/// Where the files of an index are: in the folder it was made from, when it was made on this machine; else on the
/// server of the settings, if one is set.
fn origin_of(root: &Path, settings: &Settings) -> Origin {
    if !root.as_os_str().is_empty() {
        Origin::Folder(root.to_owned())
    } else if !settings.server.is_empty() {
        Origin::Server { host: settings.server.clone(), home: settings.home.clone() }
    } else {
        Origin::Nowhere
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Sidebar,
    Main,
    /// The filter bar.
    Filters,
}

/// The orders the album grids can be put in.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AlbumOrder {
    /// The latest release first.
    #[default]
    Newest,
    Oldest,
    /// The album whose files came into the collection last first.
    Added,
    /// By artist, each artist's albums by year.
    Artist,
    Title,
    /// Lossless first, then by bitrate: how most of an album's songs sound.
    Sound,
}

impl AlbumOrder {
    pub const ALL: [AlbumOrder; 6] =
        [AlbumOrder::Newest, AlbumOrder::Oldest, AlbumOrder::Added, AlbumOrder::Artist, AlbumOrder::Title, AlbumOrder::Sound];

    pub fn label(self) -> &'static str {
        match self {
            AlbumOrder::Newest => "Newest releases",
            AlbumOrder::Oldest => "Oldest releases",
            AlbumOrder::Added => "Recently added",
            AlbumOrder::Artist => "Artist",
            AlbumOrder::Title => "Album title",
            AlbumOrder::Sound => "Best sound",
        }
    }

    /// The one `steps` along [`AlbumOrder::ALL`], going round.
    fn step(self, steps: isize) -> AlbumOrder {
        let at = AlbumOrder::ALL.iter().position(|&order| order == self).unwrap_or(0) as isize;
        AlbumOrder::ALL[(at + steps).rem_euclid(AlbumOrder::ALL.len() as isize) as usize]
    }
}

/// Per-album facts that the track index does not store directly.
#[derive(Clone, Copy, Default)]
struct AlbumInfo {
    year: u32,
    /// When it came out, as [`wax_core::index::Track::released`] gives it; 0 when not even the year is known.
    released: u32,
    /// The newest of its files' modification times: when it came into the collection, as near as the files tell.
    added: i64,
    /// How most of its songs sound, as [`wax_core::index::Track::quality`] gives it: the middle of its songs' qualities.
    quality: u32,
    /// Cover id, 0 when no track of the album has artwork.
    art: u64,
    /// Songs held; tracks of the release the collection lacks are in `missing`.
    tracks: u32,
    missing: u32,
    /// Whole and confirmed: in the ready catalogue, with every track of the release held.
    verified: bool,
    /// What it is, as an index into the library's kinds: Album, Single, EP and so on.
    kind: u32,
    ms: u64,
    /// Position in the display order: by artist, then year, then title.
    rank: u32,
}

/// An opened album: its tracks in disc and track order.
struct Detail {
    album: u32,
    /// The order they play in.
    tracks: Vec<u32>,
    /// What the page lists: the tracks, under a heading for each disc where there is more than one.
    lines: Vec<Line>,
    list: List,
    /// Tracks picked together; empty when only the one under the cursor is meant.
    picked: HashSet<u32>,
    /// The line a range of picked tracks is stretched from.
    anchor: usize,
}

/// One line of an opened album.
enum Line {
    Track(u32),
    /// The heading over a disc: its name, or "Disc 2".
    Disc(String),
}

/// A grid of albums in which one album can be opened.
#[derive(Default)]
struct Browser {
    grid: Grid,
    detail: Option<Detail>,
    /// The state its albums were picked by, if they were.
    state: Option<u32>,
}

/// What a key press inside a [`Browser`] asks the app to do.
enum Outcome {
    None,
    Play(u32),
    /// Left was pressed at the left edge.
    Leave,
}

struct ColumnSpec {
    column: Column,
    label: &'static str,
    left: f32,
    right: f32,
    align_right: bool,
}

/// Fixed parts of the window for one frame.
struct Layout {
    bar: Rect,
    sidebar: Rect,
    main: Rect,
    status: Rect,
    play: Rect,
    previous: Rect,
    next: Rect,
    /// The volume control beside the buttons; zero-sized when the window is too narrow.
    volume: Rect,
    /// The now-playing display; zero-sized when the window is too narrow.
    display: Rect,
    search: Rect,
    /// The filter bar, across the top of the content; zero-sized when it is not shown.
    filters: Rect,
    /// One per section of the sidebar, in the order of [`App::sections`].
    items: Vec<Rect>,
    /// The headings over the playlists and the unfinished states, and where each goes.
    headings: Vec<(Pos2, &'static str)>,
}

impl Layout {
    fn new(screen: Rect, sections: &[Section], filter_bar: bool) -> Self {
        let bar = Rect::from_min_max(screen.min, pos2(screen.right(), screen.top() + BAR_HEIGHT));
        let status = Rect::from_min_max(pos2(screen.left(), screen.bottom() - STATUS_HEIGHT), screen.max);
        let (top, bottom) = (bar.bottom() + 1.0, status.top() - 1.0);
        let sidebar = Rect::from_min_max(pos2(screen.left(), top), pos2(screen.left() + SIDEBAR_WIDTH, bottom));
        let content = Rect::from_min_max(pos2(sidebar.right() + 1.0, top), pos2(screen.right(), bottom));
        let (filters, main) = if filter_bar { content.split_top_bottom_at_y(content.top() + filters::BAR) } else { (Rect::ZERO, content) };

        let mid = bar.center().y;
        let play = Rect::from_center_size(pos2(screen.left() + 90.0, mid), vec2(36.0, 36.0));
        let skip = |side: f32| Rect::from_center_size(pos2(play.center().x + side * 46.0, mid), vec2(30.0, 30.0));
        let (previous, next) = (skip(-1.0), skip(1.0));
        let search = Rect::from_min_max(pos2(screen.right() - 246.0, mid - 14.0), pos2(screen.right() - 16.0, mid + 14.0));
        let volume = Rect::from_min_size(pos2(next.right() + 20.0, mid - 12.0), vec2(volume::WIDTH, 24.0));
        let volume = if volume.right() + 24.0 > search.left() { Rect::ZERO } else { volume };

        // Centred in the window when the buttons and search field leave room.
        let (left, right) = (next.right().max(volume.right()) + 24.0, search.left() - 24.0);
        let width = (right - left).clamp(0.0, 540.0);
        let display = if width < 160.0 {
            Rect::ZERO
        } else {
            let center = screen.center().x.clamp(left + width / 2.0, right - width / 2.0);
            Rect::from_center_size(pos2(center, mid), vec2(width, 46.0))
        };

        let item = |top: f32| Rect::from_min_size(pos2(sidebar.left() + 8.0, top), vec2(SIDEBAR_WIDTH - 16.0, SIDEBAR_ROW));
        let (mut at, mut headings, mut items) = (sidebar.top() + 38.0, Vec::new(), Vec::with_capacity(sections.len()));
        let mut group = None;
        for section in sections {
            if *section == Section::Settings {
                items.push(item(sidebar.bottom() - SIDEBAR_ROW - 10.0));
                continue;
            }
            let heading = match section {
                Section::Liked | Section::Playlist(_) | Section::NewPlaylist => Some("PLAYLISTS"),
                Section::Rest(_) => Some("UNFINISHED"),
                _ => None,
            };
            if heading.is_some() && heading != group {
                headings.push((pos2(sidebar.left() + 18.0, at + 18.0), heading.unwrap_or_default()));
                at += 40.0;
            }
            group = heading;
            items.push(item(at));
            at += SIDEBAR_ROW;
        }
        Layout { bar, sidebar, main, status, play, previous, next, volume, display, search, filters, items, headings }
    }
}

/// The artist list and, to its right, the pane with that artist's albums.
fn artists_rects(main: Rect) -> (Rect, Rect) {
    let split = main.left() + ARTIST_LIST_WIDTH.min(main.width() / 2.0);
    (
        Rect::from_min_max(pos2(main.left(), main.top() + 6.0), pos2(split, main.bottom())),
        Rect::from_min_max(pos2(split + 1.0, main.top()), main.max),
    )
}

/// The artist pane split into its name header and the albums below.
fn pane_rects(pane: Rect) -> (Rect, Rect) {
    pane.split_top_bottom_at_y(pane.top() + ARTIST_HEADER)
}

/// The song table split into its column header and rows.
fn songs_rects(main: Rect) -> (Rect, Rect) {
    let head = Rect::from_min_max(main.min, pos2(main.right(), main.top() + SONG_HEAD));
    (head, Rect::from_min_max(pos2(main.left(), head.bottom() + 1.0), main.max))
}

/// Columns of the song table. Narrow windows drop genre and year; title,
/// artist and album share what is left. A list of unfinished songs (`why`)
/// says why each is there in place of the genre.
fn song_columns(area: Rect, why: bool) -> Vec<ColumnSpec> {
    const GAP: f32 = 18.0;
    let (left, right) = (area.left() + 34.0, area.right() - 22.0);
    let width = (right - left).max(0.0);

    // (column, label, fixed width, share of the flexible width, right-aligned)
    let mut spec = if why {
        vec![
            (Column::Title, "Title", 0.0, 0.26, false),
            (Column::Artist, "Artist", 0.0, 0.16, false),
            (Column::Album, "Album", 0.0, 0.20, false),
            (Column::Why, "Why", 0.0, 0.38, false),
        ]
    } else {
        vec![(Column::Title, "Title", 0.0, 0.40, false), (Column::Artist, "Artist", 0.0, 0.28, false), (Column::Album, "Album", 0.0, 0.32, false)]
    };
    if width >= 900.0 && !why {
        spec.push((Column::Genre, "Genre", 120.0, 0.0, false));
    }
    if width >= 1040.0 {
        spec.push((Column::Quality, "Quality", 74.0, 0.0, false));
    }
    if width >= 700.0 {
        spec.push((Column::Year, "Year", 40.0, 0.0, true));
    }
    // The heart sits at the left of the time's cell.
    spec.push((Column::Time, "Time", 48.0 + actions::HEART_CELL, 0.0, true));

    let fixed = spec.iter().map(|s| s.2).sum::<f32>() + GAP * (spec.len() - 1) as f32;
    let flexible = (width - fixed).max(0.0);
    let mut x = left;
    spec.into_iter()
        .map(|(column, label, fixed, share, align_right)| {
            let width = if fixed > 0.0 { fixed } else { (flexible * share).floor() };
            let spec = ColumnSpec { column, label, left: x, right: x + width, align_right };
            x = spec.right + GAP;
            spec
        })
        .collect()
}

struct DetailRects {
    back: Rect,
    cover: Rect,
    /// Album title, artist and summary line.
    heading: Rect,
    rows: Rect,
}

fn detail_rects(area: Rect) -> DetailRects {
    const PAD: f32 = 28.0;
    let back = Rect::from_min_size(pos2(area.left() + PAD, area.top() + 14.0), vec2(220.0, 24.0));
    let cover = Rect::from_min_size(pos2(area.left() + PAD, back.bottom() + 12.0), vec2(DETAIL_COVER, DETAIL_COVER));
    let (left, right) = (cover.right() + PAD, (area.right() - PAD).max(cover.right() + PAD));
    let heading = Rect::from_min_max(pos2(left, cover.top()), pos2(right, cover.top() + 84.0));
    let rows = Rect::from_min_max(pos2(left, heading.bottom() + 6.0), pos2(right, area.bottom()));
    DetailRects { back, cover, heading, rows }
}

fn row_rect(area: Rect, slot: usize, height: f32) -> Rect {
    let top = area.top() + slot as f32 * height;
    Rect::from_min_max(pos2(area.left(), top), pos2(area.right(), top + height))
}

/// Whole rows of `height` that fit in `area`.
fn rows_in(area: Rect, height: f32) -> usize {
    (area.height() / height + 0.01).floor().max(0.0) as usize
}

impl Detail {
    /// `state` keeps to the tracks in that state where the grid shows only those.
    fn open(lib: &Library, album: u32, state: Option<u32>) -> Self {
        // Sorting by album is constant here, which leaves disc and track order.
        let view = lib.view(&Query { album: Some(album), state, ..Query::default() }, Column::Album, false);
        let tracks = view.rows;
        let discs = |t: &u32| lib.tracks[*t as usize].disc;
        let several = tracks.first().is_some_and(|first| tracks.iter().any(|t| discs(t) != discs(first)));
        let mut lines = Vec::with_capacity(tracks.len());
        for (n, &t) in tracks.iter().enumerate() {
            let track = &lib.tracks[t as usize];
            if several && (n == 0 || discs(&tracks[n - 1]) != track.disc) {
                lines.push(Line::Disc(if track.disc_name.is_empty() { format!("Disc {}", track.disc.max(1)) } else { track.disc_name.clone() }));
            }
            lines.push(Line::Track(t));
        }
        let mut detail = Detail { album, tracks, lines, list: List::default(), picked: HashSet::new(), anchor: 0 };
        detail.go(Move::Top);
        detail.anchor = detail.list.cursor;
        detail
    }

    /// The tracks an action means: those picked, in the order they play, or else the one under the cursor.
    fn chosen(&self) -> Vec<u32> {
        if self.picked.is_empty() {
            return self.track(self.list.cursor).into_iter().collect();
        }
        self.tracks.iter().copied().filter(|id| self.picked.contains(id)).collect()
    }

    /// Picks every track from the anchor's line to `row`.
    fn pick_to(&mut self, row: usize) {
        let (from, to) = (self.anchor.min(row), self.anchor.max(row).min(self.lines.len().saturating_sub(1)));
        self.picked = (from..=to).filter_map(|line| self.track(line)).collect();
    }

    /// A click on line `row`: Shift stretches the picked tracks to it, Ctrl picks it or lets it go, a plain click picks
    /// it alone. A plain click on one of several picked tracks leaves them picked, so that they can be dragged together;
    /// it returns the row, for the pick to narrow to it if the button comes up without a drag.
    fn click(&mut self, row: usize, m: Modifiers) -> Option<usize> {
        let (was, id) = (self.list.cursor, self.track(row)?);
        self.list.set(row, self.lines.len());
        if m.shift {
            self.pick_to(row);
        } else if m.command {
            if self.picked.is_empty() {
                self.picked.extend(self.track(was));
            }
            if !self.picked.remove(&id) {
                self.picked.insert(id);
            }
            self.anchor = row;
        } else if self.picked.len() > 1 && self.picked.contains(&id) {
            return Some(row);
        } else {
            self.picked.clear();
            self.anchor = row;
        }
        None
    }

    /// Narrows what is picked to line `row` alone.
    fn pick_only(&mut self, row: usize) {
        self.picked.clear();
        self.anchor = row;
    }

    /// The track on a line; None for a heading.
    fn track(&self, row: usize) -> Option<u32> {
        match self.lines.get(row) {
            Some(&Line::Track(t)) => Some(t),
            _ => None,
        }
    }

    /// Moves the cursor, which steps past a heading the way it moved: a heading is never picked.
    fn go(&mut self, mv: Move) {
        let before = self.list.cursor;
        self.list.go(mv, self.lines.len());
        let row = self.list.cursor;
        if self.track(row).is_none() {
            let past = if row < before && row > 0 { row - 1 } else { row + 1 };
            self.list.set(past, self.lines.len());
        }
    }
}

impl Browser {
    fn key(&mut self, albums: &[u32], lib: &Library, key: Key, m: Modifiers, mv: Option<Move>) -> Outcome {
        if let Some(detail) = &mut self.detail {
            match (key, mv) {
                (Key::Enter, _) => {
                    return detail.track(detail.list.cursor).map_or(Outcome::None, Outcome::Play);
                }
                (Key::A, _) if m.command => detail.picked = detail.tracks.iter().copied().collect(),
                (Key::Backspace, _) | (_, Some(Move::Left)) => self.detail = None,
                // Shift with an arrow stretches the picked tracks from where the stretch began; a plain move lets them go.
                (Key::ArrowUp | Key::ArrowDown | Key::PageUp | Key::PageDown, Some(mv)) if m.shift => {
                    detail.go(mv);
                    detail.pick_to(detail.list.cursor);
                }
                (_, Some(mv)) => {
                    detail.go(mv);
                    detail.pick_only(detail.list.cursor);
                }
                _ => {}
            }
            return Outcome::None;
        }
        match (key, mv) {
            (Key::Enter, _) => self.open(albums, lib),
            (_, Some(mv)) if self.grid.go(mv, albums.len()) => return Outcome::Leave,
            _ => {}
        }
        Outcome::None
    }

    fn open(&mut self, albums: &[u32], lib: &Library) {
        if let Some(&album) = albums.get(self.grid.cursor) {
            self.detail = Some(Detail::open(lib, album, self.state));
        }
    }

    /// `tile` is the smallest a cover may be drawn in the grid.
    fn settle(&mut self, albums: usize, area: Rect, tile: f32) {
        match &mut self.detail {
            Some(detail) => detail.list.settle(detail.lines.len(), rows_in(detail_rects(area).rows, TRACK_ROW)),
            None => self.grid.settle(&Cells::new(area, tile), albums),
        }
    }

    fn wheel(&mut self, pixels: f32) {
        match &mut self.detail {
            Some(detail) => detail.list.scroll(-pixels / TRACK_ROW, detail.lines.len()),
            None => self.grid.scroll_by(-pixels),
        }
    }

    /// A press in the browser. On one of several picked tracks it returns that track's line: see [`Detail::click`].
    fn press(&mut self, albums: &[u32], area: Rect, pos: Pos2, tile: f32, m: Modifiers) -> Option<usize> {
        match &mut self.detail {
            Some(detail) => {
                let rects = detail_rects(area);
                if rects.back.contains(pos) {
                    self.detail = None;
                } else if let Some(row) = detail.list.hit(rects.rows, TRACK_ROW, pos, detail.lines.len()) {
                    return detail.click(row, m);
                }
            }
            None => {
                if let Some(index) = Cells::new(area, tile).hit(pos, self.grid.scroll, albums.len()) {
                    self.grid.set(index, albums.len());
                }
            }
        }
        None
    }

    /// A double click opens the album under the pointer, or returns the
    /// track to play when it lands on a track of the opened album.
    fn double_click(&mut self, albums: &[u32], lib: &Library, area: Rect, pos: Pos2, tile: f32) -> Option<u32> {
        match &self.detail {
            Some(detail) => {
                let row = detail.list.hit(detail_rects(area).rows, TRACK_ROW, pos, detail.lines.len())?;
                detail.track(row)
            }
            None => {
                if Cells::new(area, tile).hit(pos, self.grid.scroll, albums.len()).is_some() {
                    self.open(albums, lib);
                }
                None
            }
        }
    }
}

pub struct App {
    lib: Library,
    /// Indexed by album id.
    info: Vec<AlbumInfo>,
    art: Art,
    fonts: Fonts,
    /// For starting another artwork loader when a second index is read.
    ctx: egui::Context,
    source: Source,
    settings: Settings,
    /// Cursor into the rows of the settings page.
    setting: usize,
    /// Whether the pointer is holding the settings page's slider.
    sliding: bool,
    /// Whether the pointer is holding the volume slider.
    volume_held: bool,
    /// When the volume last moved, while that is not saved yet.
    volume_moved: Option<Instant>,
    /// The scrollbar the pointer is holding, if it is.
    drag: Option<scroll::Drag>,
    /// Songs taken up by the pointer, to be dropped on a playlist.
    carry: Option<carry::Carry>,
    /// The scan of the music folder, while one runs.
    scanning: Option<scanning::Scan>,
    /// Where the pointer is, as of this frame.
    pointer: Option<Pos2>,
    /// The index not on screen: the unfinished part of the collection while the
    /// library is shown, and the library while that is. None until developer mode loads it.
    shelved: Option<Shelved>,
    /// Whether the unfinished part is the one on screen.
    rest_shown: bool,
    /// The states of the unfinished part, as the sidebar lists them.
    rest: Vec<RestState>,

    section: Section,
    focus: Focus,
    filters: Filters,
    /// Whether the filter bar was asked for; it also shows while a filter is set.
    filter_bar: bool,
    /// The filter the keyboard is on while the bar has it.
    filter_cursor: usize,
    menu: Option<Menu>,
    searching: bool,
    search: String,
    sort: Column,
    descending: bool,

    /// Everything matching the search: song rows plus the artists and albums they belong to.
    result: View,
    /// Albums shown in the Albums grid, in display order.
    albums: Vec<u32>,
    albums_browser: Browser,
    /// Cursor into `result.artists`.
    artist_list: List,
    /// Albums of the selected artist.
    artist_albums: Vec<u32>,
    artist_browser: Browser,
    /// Whether the keyboard is in the artist's albums rather than the artist list.
    artist_in_grid: bool,
    songs: List,
    /// Songs of the table picked together, by track; empty when only the row under the cursor is meant.
    picked: HashSet<u32>,
    /// The row a range of picked songs is stretched from.
    anchor: usize,
    /// The menu of things to do with songs, or with a playlist, where it is open.
    actions: Option<Actions>,
    /// The songs liked, and the playlists.
    store: Store,
    /// Where the tracks of the index on screen are, for finding the songs of a playlist in it.
    places: Places,
    /// Per track of the index on screen: whether it is liked.
    liked: Vec<bool>,
    /// Songs each playlist has in the library, by playlist id; the liked songs under 0.
    counts: HashMap<u64, usize>,
    /// The playlist being named, and the name so far.
    renaming: Option<actions::Naming>,
    /// A playlist Delete was pressed on once in the sidebar, and when: a second press deletes it.
    arming: Option<(u64, Instant)>,
    /// Paths of every song dropped from the library by hand.
    gone: HashSet<String>,
    /// What the status bar has to say for a few seconds, and since when.
    notice: Option<(String, Instant)>,
    /// What came of telling the librarian about dropped songs, when it is known.
    told: Arc<Mutex<Option<String>>>,

    playing: Option<u32>,
    /// Whether the track playing is one of the unfinished part.
    playing_rest: bool,
    paused: bool,
    player: Player,
    /// Where the player stands, as of this frame.
    status: player::Status,
    /// The number the player gave the track now playing.
    serial: u64,
    /// The tracks that play one after another, and which of them is playing.
    queue: Vec<u32>,
    queue_at: usize,
    /// Where the last seek aimed and when, so that quick presses add up before the player has caught up.
    seek_target: Option<(Duration, Instant)>,
    help: bool,
    /// How long the last search took; shown in the status bar.
    query_time: Duration,
    script: Script,
}

/// What the window starts with.
pub struct Start {
    pub lib: Library,
    pub source: Source,
    pub settings: Settings,
    /// Paths of every song dropped from the library by hand.
    pub gone: HashSet<String>,
    /// The search to start with.
    pub search: String,
    pub started: Instant,
    /// There is no library yet: the music folder is asked for.
    pub welcome: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, start: Start) -> Self {
        let Start { lib, source, settings, gone, search, started, welcome } = start;
        theme::install_fonts(&cc.egui_ctx);
        let cache = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
            .unwrap_or_default()
            .join("wax/audio");
        let player = Player::new(origin_of(&source.root, &settings), cache, std::env::var_os("WAX_MUTE").is_some(), cc.egui_ctx.clone());
        let mut app = App {
            info: album_info(&lib, source.ready),
            lib,
            art: Art::new(source.art.clone(), &cc.egui_ctx),
            fonts: Fonts::new(),
            ctx: cc.egui_ctx.clone(),
            source,
            settings,
            setting: 0,
            sliding: false,
            volume_held: false,
            volume_moved: None,
            drag: None,
            carry: None,
            scanning: None,
            pointer: None,
            shelved: None,
            rest_shown: false,
            rest: Vec::new(),
            section: Section::Albums,
            focus: Focus::Main,
            filters: Filters::default(),
            filter_bar: false,
            filter_cursor: 0,
            menu: None,
            searching: false,
            search,
            sort: Column::Artist,
            descending: false,
            result: View::default(),
            albums: Vec::new(),
            albums_browser: Browser::default(),
            artist_list: List::default(),
            artist_albums: Vec::new(),
            artist_browser: Browser::default(),
            artist_in_grid: false,
            songs: List::default(),
            picked: HashSet::new(),
            anchor: 0,
            actions: None,
            store: Store::default(),
            places: Places::default(),
            liked: Vec::new(),
            counts: HashMap::new(),
            renaming: None,
            arming: None,
            gone,
            notice: None,
            told: Arc::new(Mutex::new(None)),
            playing: None,
            playing_rest: false,
            paused: false,
            status: player.status(),
            player,
            serial: 0,
            queue: Vec::new(),
            queue_at: 0,
            seek_target: None,
            help: false,
            query_time: Duration::ZERO,
            script: Script::from_env(started),
        };
        app.player.set_volume(app.heard());
        if app.settings.developer {
            app.load_rest();
        }
        if !app.gone.is_empty() {  // in case the librarian has not heard of all of them
            app.tell_librarian();
        }
        let (store, trouble) = Store::load();
        app.store = store;
        app.notice = trouble.map(|said| (said, Instant::now()));
        app.library_changed();
        app.search_changed();
        // The list of the music folder is brought up to date each time; the first time, the folder is asked for.
        if app.source.index == folder_index() {
            app.scan(app.lib.is_empty());
        } else if welcome {
            app.welcome();
        }
        app
    }

    /// Where the files of the library on screen are, or of the library behind the unfinished part while that is.
    fn origin(&self) -> Origin {
        let source = match &self.shelved {
            Some(other) if self.rest_shown => &other.source,
            _ => &self.source,
        };
        origin_of(&source.root, &self.settings)
    }

    /// Tells the librarian on the server of the songs dropped by hand, when the library is from one.
    fn tell_librarian(&self) {
        if let Origin::Server { host, home } = self.origin() {
            let paths = self.gone.iter().cloned().collect();
            discard::send(host, home, paths, self.told.clone(), self.ctx.clone());
        }
    }

    /// Reads the unfinished part of the collection, which the librarian writes
    /// in a folder beside the index. Without one, developer mode lists no states.
    fn load_rest(&mut self) {
        if self.shelved.is_some() {
            return;
        }
        let index = self.source.index.with_file_name("unfinished").join("library.jsonl.gz");
        if !index.exists() {
            return;
        }
        let (header, mut tracks) = match read_index_file(&index) {
            Ok(read) => read,
            Err(e) => {
                eprintln!("cannot read the unfinished part of the collection at {}: {e}", index.display());
                return;
            }
        };
        tracks.retain(|track| !self.gone.contains(&track.path));
        let lib = Library::new(tracks);
        self.rest = rest_states(&lib);
        let art = index.with_file_name("art");
        let source = Source { art: art.clone(), index, generated: header.generated, ready: header.ready, root: PathBuf::from(header.root) };
        self.shelved = Some(Shelved { info: album_info(&lib, source.ready), lib, art: Art::new(art, &self.ctx), source });
    }

    /// The sections of the sidebar, top to bottom.
    fn sections(&self) -> Vec<Section> {
        let mut sections = vec![Section::Albums, Section::Artists, Section::Songs, Section::Liked];
        sections.extend(self.store.playlists.iter().map(|playlist| Section::Playlist(playlist.id)));
        sections.push(Section::NewPlaylist);
        if self.settings.developer {
            sections.extend(self.rest.iter().map(|state| Section::Rest(state.id)));
        }
        sections.push(Section::Settings);
        sections
    }

    fn label(&self, section: Section) -> &str {
        match section {
            Section::Albums => "Albums",
            Section::Artists => "Artists",
            Section::Songs => "Songs",
            Section::Liked => "Liked Songs",
            Section::Playlist(id) => self.store.playlist(id).map_or("", |playlist| &playlist.name),
            Section::NewPlaylist => "New Playlist",
            Section::Rest(id) => self.rest.iter().find(|state| state.id == id).map_or("", |state| &state.name),
            Section::Settings => "Settings",
        }
    }

    fn form(&self) -> Form {
        match self.section {
            Section::Albums => Form::Grid,
            Section::Artists => Form::Artists,
            Section::Songs | Section::Liked | Section::Playlist(_) | Section::NewPlaylist => Form::Table,
            Section::Rest(id) if self.rest.iter().any(|state| state.id == id && state.albums) => Form::Grid,
            Section::Rest(_) => Form::Table,
            Section::Settings => Form::Settings,
        }
    }

    /// Moves to a section, bringing the index it shows on screen. "New Playlist" makes one, goes there, and has it named.
    fn go(&mut self, section: Section) {
        if section == Section::NewPlaylist {
            self.new_playlist(&[]);
            return;
        }
        if section == self.section {
            return;
        }
        let crossing = matches!(section, Section::Rest(_)) || matches!(self.section, Section::Rest(_));
        // A playlist lists its own songs; the other sections share one search.
        let listing = |section: Section| matches!(section, Section::Liked | Section::Playlist(_));
        let relist = crossing || listing(section) || listing(self.section);
        self.actions = None;
        self.section = section;
        self.menu = None;
        if self.focus == Focus::Filters {
            self.focus = Focus::Main;
        }
        if crossing {
            self.show_rest(matches!(section, Section::Rest(_)));
        }
        if relist {
            self.search_changed();
        }
    }

    /// Puts the unfinished part of the collection on screen, or the library back.
    fn show_rest(&mut self, rest: bool) {
        if rest == self.rest_shown {
            return;
        }
        if let Some(other) = &mut self.shelved {
            std::mem::swap(&mut self.lib, &mut other.lib);
            std::mem::swap(&mut self.info, &mut other.info);
            std::mem::swap(&mut self.art, &mut other.art);
            std::mem::swap(&mut self.source, &mut other.source);
            self.rest_shown = rest;
            self.library_changed();
        }
    }

    /// The state the section on screen keeps to, if it is one of the unfinished part.
    fn state(&self) -> Option<u32> {
        match self.section {
            Section::Rest(id) => Some(id),
            _ => None,
        }
    }

    /// The track playing, if it is in the index on screen.
    fn playing_here(&self) -> Option<u32> {
        self.playing.filter(|_| self.playing_rest == self.rest_shown)
    }

    fn query(&self) -> Query {
        // A name the index on screen does not have matches nothing.
        let id = |names: &[String], name: &Option<String>| {
            name.as_ref().map(|name| names.iter().position(|n| n == name).map_or(u32::MAX, |i| i as u32))
        };
        Query {
            text: self.search.clone(),
            state: self.state(),
            kind: id(&self.lib.kinds, &self.filters.kind),
            sound: self.filters.sound,
            decade: self.filters.decade,
            genre: id(&self.lib.genres, &self.filters.genre),
            ..Query::default()
        }
    }

    /// Album ids by artist, then year: the order of one artist's albums.
    fn in_order(&self, albums: impl Iterator<Item = u32>) -> Vec<u32> {
        let mut ids: Vec<u32> = albums.collect();
        ids.sort_unstable_by_key(|&id| self.info[id as usize].rank);
        ids
    }

    /// Album ids in the order the album grids are put in.
    fn in_grid_order(&self, albums: impl Iterator<Item = u32>) -> Vec<u32> {
        grid_order(&self.info, albums.collect(), self.settings.album_sort)
    }

    /// Puts the album grids in another order, from the top, and keeps the choice for the next run.
    fn sort_albums(&mut self, order: AlbumOrder) {
        self.settings.album_sort = order;
        self.settings.save();
        self.albums = self.in_grid_order(self.albums.iter().copied());
        self.albums_browser.grid = Grid::default();
        self.notice = Some((format!("Albums sorted: {}", order.label()), Instant::now()));
    }

    /// Re-runs the search and sends every cursor back to the top.
    fn search_changed(&mut self) {
        let started = Instant::now();
        self.result = self.lib.view(&self.query(), self.sort, self.descending);
        self.keep_to_playlist();
        self.albums = self.in_grid_order(self.result.albums.iter().map(|f| f.id));
        self.albums_browser = Browser { state: self.state(), ..Browser::default() };
        self.artist_list = List::default();
        self.artist_in_grid = false;
        self.songs = List::default();
        self.picked.clear();
        self.anchor = 0;
        self.actions = None;
        self.artist_changed();
        self.query_time = started.elapsed();
    }

    /// Loads the albums of the artist under the cursor.
    fn artist_changed(&mut self) {
        self.artist_browser = Browser::default();
        self.artist_albums = match self.result.artists.get(self.artist_list.cursor) {
            Some(artist) => {
                let query = Query { artist: Some(artist.id), ..self.query() };
                let view = self.lib.view(&query, Column::Album, false);
                self.in_order(view.albums.iter().map(|f| f.id))
            }
            None => Vec::new(),
        };
    }

    /// Changes the sort order of the song list, which starts again from the top. Songs picked stay picked.
    fn sort_by(&mut self, column: Column) {
        if self.sort == column {
            self.descending = !self.descending;
        } else {
            self.sort = column;
            // "Date added" is only ever wanted newest first, and quality best first.
            self.descending = matches!(column, Column::Added | Column::Quality);
        }
        self.lib.sort(&mut self.result.rows, self.sort, self.descending);
        self.songs = List::default();
        self.anchor = 0;
    }

    /// Plays a track of what is on screen; the tracks listed after it follow, but for those the collection lacks.
    fn play(&mut self, track: u32) {
        if self.lib.tracks[track as usize].missing {
            self.notice = Some(("This song is not in the collection; it is listed to show the album is missing it".to_owned(), Instant::now()));
            return;
        }
        let listed = match self.form() {
            Form::Grid => self.albums_browser.detail.as_ref().map(|detail| detail.tracks.clone()),
            Form::Artists => self.artist_browser.detail.as_ref().map(|detail| detail.tracks.clone()),
            Form::Table => Some(self.result.rows.clone()),
            Form::Settings => None,
        };
        self.queue = listed.unwrap_or_else(|| vec![track]);
        let lib = &self.lib;
        self.queue.retain(|&id| !lib.tracks[id as usize].missing);
        self.playing_rest = self.rest_shown;
        self.play_at(self.queue.iter().position(|&t| t == track).unwrap_or(0));
    }

    /// Plays the queue's track `at`, and has the one after it fetched meanwhile.
    fn play_at(&mut self, at: usize) {
        let Some(&track) = self.queue.get(at) else { return };
        // The queue's tracks may be in the index that is not on screen.
        let lib = match &self.shelved {
            Some(other) if self.playing_rest != self.rest_shown => &other.lib,
            _ => &self.lib,
        };
        let request = |id: u32| Request { path: lib.tracks[id as usize].path.clone(), size: lib.tracks[id as usize].size };
        let (this, after) = (request(track), self.queue.get(at + 1).map(|&id| request(id)));
        self.queue_at = at;
        self.playing = Some(track);
        self.paused = false;
        self.seek_target = None;
        self.serial = self.player.play(this);
        if let Some(after) = after {
            self.player.prefetch(after);
        }
    }

    /// The songs an action on the table applies to, in the order shown: those picked, or else the one under the cursor.
    fn chosen(&self) -> Vec<u32> {
        if self.picked.is_empty() {
            return self.result.rows.get(self.songs.cursor).copied().into_iter().collect();
        }
        self.result.rows.iter().copied().filter(|id| self.picked.contains(id)).collect()
    }

    /// Esc with songs picked lets go of them: those of the song table, or of the opened album. Returns whether any were.
    fn let_go_of_picks(&mut self) -> bool {
        let detail = match self.form() {
            Form::Grid => self.albums_browser.detail.as_mut(),
            Form::Artists => self.artist_browser.detail.as_mut(),
            Form::Table | Form::Settings => None,
        };
        let had = !self.picked.is_empty() || detail.as_ref().is_some_and(|detail| !detail.picked.is_empty());
        self.picked.clear();
        if let Some(detail) = detail {
            detail.picked.clear();
        }
        had
    }

    /// Picks every row from the anchor to `row`.
    fn pick_to(&mut self, row: usize) {
        let (from, to) = (self.anchor.min(row), self.anchor.max(row).min(self.result.rows.len().saturating_sub(1)));
        self.picked = self.result.rows.get(from..=to).unwrap_or_default().iter().copied().collect();
    }

    /// Drops the chosen songs from the library: they vanish here now, and the librarian is told to leave them out
    /// of the catalogue. Their files are not touched.
    fn drop_chosen(&mut self) {
        self.actions = None;
        let held = self.chosen().into_iter().map(|id| &self.lib.tracks[id as usize]).filter(|track| !track.missing);
        let paths: Vec<String> = held.map(|track| track.path.clone()).collect();
        if paths.is_empty() {
            return;
        }
        if let Err(why) = discard::record(&paths) {
            self.notice = Some((why, Instant::now()));
            return;
        }
        self.gone.extend(paths.iter().cloned());
        self.tell_librarian();
        let dropped: HashSet<&str> = paths.iter().map(String::as_str).collect();

        // Tracks are known by their place in the index, which is about to change: carry what is playing over by path.
        let here = self.playing_rest == self.rest_shown;
        let path_of = |id: u32| self.lib.tracks[id as usize].path.clone();
        let (playing, queue): (Option<String>, Vec<String>) =
            if here { (self.playing.map(path_of), self.queue.iter().map(|&id| path_of(id)).collect()) } else { (None, Vec::new()) };
        let (section, cursor) = (self.label(self.section).to_owned(), self.songs.cursor);

        let mut tracks = std::mem::take(&mut self.lib.tracks);
        tracks.retain(|track| !dropped.contains(track.path.as_str()));
        self.lib = Library::new(tracks);
        self.info = album_info(&self.lib, self.source.ready);
        self.library_changed();
        if here {
            let place: HashMap<&str, u32> = self.lib.tracks.iter().enumerate().map(|(i, track)| (track.path.as_str(), i as u32)).collect();
            self.queue = queue.iter().filter_map(|path| place.get(path.as_str()).copied()).collect();
            self.playing = playing.and_then(|path| place.get(path.as_str()).copied());
            self.queue_at = self.playing.and_then(|id| self.queue.iter().position(|&queued| queued == id)).unwrap_or(0);
        }
        if self.rest_shown {  // the states are numbered afresh too, and one may have emptied
            self.rest = rest_states(&self.lib);
            match self.rest.iter().find(|state| state.name == section) {
                Some(state) => self.section = Section::Rest(state.id),
                None => {
                    self.section = Section::Albums;
                    self.show_rest(false);
                }
            }
        }
        self.search_changed();
        self.songs.set(cursor, self.result.rows.len());
        self.anchor = self.songs.cursor;
        let songs = if paths.len() == 1 { "1 song".to_owned() } else { format!("{} songs", paths.len()) };
        self.notice = Some((format!("{songs} dropped from the library"), Instant::now()));
    }

    /// The next track of the queue (`by` 1) or the one before (-1).
    fn skip(&mut self, by: isize) {
        if self.playing.is_some() {
            self.play_at(self.queue_at.saturating_add_signed(by));
        }
    }

    /// Moves to a share of the way through the track playing.
    fn seek_to_share(&mut self, share: f32) {
        let lib = match &self.shelved {
            Some(other) if self.playing_rest != self.rest_shown => &other.lib,
            _ => &self.lib,
        };
        if let Some(track) = self.playing.map(|id| &lib.tracks[id as usize]) {
            self.player.seek(Duration::from_secs_f32(share * track.dur_ms as f32 / 1000.0));
        }
    }

    fn toggle_pause(&mut self) {
        if self.playing.is_some() {
            self.paused = !self.paused;
            if self.paused { self.player.pause() } else { self.player.resume() }
        }
    }

    /// Moves the place in the track playing by `seconds`.
    fn seek_by(&mut self, seconds: f32) {
        if self.playing.is_none() || self.status.serial != self.serial {
            return;
        }
        let aimed = self.seek_target.filter(|(_, when)| when.elapsed() < Duration::from_millis(600)).map(|(to, _)| to);
        let to = Duration::from_secs_f32((aimed.unwrap_or(self.status.position).as_secs_f32() + seconds).max(0.0));
        self.seek_target = Some((to, Instant::now()));
        self.player.seek(to);
    }

    /// Reads where the player stands; a track that has ended hands over to the next.
    fn follow_player(&mut self) {
        self.status = self.player.status();
        if self.status.serial == self.serial && self.status.state == player::State::Ended && self.playing.is_some() {
            if self.queue_at + 1 < self.queue.len() {
                self.play_at(self.queue_at + 1);
            } else {
                self.playing = None;
            }
        }
    }

    fn play_song_cursor(&mut self) {
        if let Some(&track) = self.result.rows.get(self.songs.cursor) {
            self.play(track);
        }
    }

    fn start_search(&mut self) {
        self.searching = true;
        self.focus = Focus::Main;
        if self.section == Section::Settings {
            self.go(Section::Albums);
        }
    }

    /// Esc: closes an opened album, else clears the search.
    fn back(&mut self) {
        let browser = match self.form() {
            Form::Grid => Some(&mut self.albums_browser),
            Form::Artists => Some(&mut self.artist_browser),
            Form::Table | Form::Settings => None,
        };
        if let Some(browser) = browser.filter(|b| b.detail.is_some()) {
            browser.detail = None;
        } else if !self.search.is_empty() {
            self.search.clear();
            self.search_changed();
        }
    }

    fn handle_input(&mut self, ui: &egui::Ui, l: &Layout) {
        let (events, hover, latest, pressed, double_clicked, held, other, modifiers) = ui.input(|i| {
            let pressed = if i.pointer.primary_pressed() { i.pointer.interact_pos() } else { None };
            let other = if i.pointer.secondary_pressed() { i.pointer.interact_pos() } else { None };
            let double_clicked = i.pointer.button_double_clicked(PointerButton::Primary);
            (i.events.clone(), i.pointer.hover_pos(), i.pointer.latest_pos(), pressed, double_clicked, i.pointer.primary_down(), other, i.modifiers)
        });
        self.pointer = hover;
        self.hover_actions(hover, ui.max_rect());
        if let Some(said) = self.told.lock().unwrap().take() {
            let before = self.notice.take().map_or(String::new(), |(text, _)| text + "; ");
            self.notice = Some((before + &said, Instant::now()));
        }

        // The key that opens the search also arrives as typed text; that
        // one character must not end up in the query.
        let mut swallow_text = false;
        for event in events {
            match event {
                Event::Key { key, pressed: true, modifiers, .. } => {
                    let (was_searching, was_naming) = (self.searching, self.renaming.is_some());
                    self.key(key, modifiers, ui.ctx(), l);
                    swallow_text = (self.searching && !was_searching) || (self.renaming.is_some() && !was_naming);
                }
                Event::Text(_) if swallow_text => swallow_text = false,
                Event::Text(text) | Event::Paste(text) if self.renaming.is_some() => {
                    if let Some(naming) = &mut self.renaming {
                        naming.type_in(&text);
                    }
                }
                Event::Text(text) | Event::Paste(text) if self.searching => {
                    self.search.extend(text.chars().filter(|c| !c.is_control()));
                    self.search_changed();
                }
                Event::MouseWheel { unit, delta, .. } => {
                    let pixels = match unit {
                        MouseWheelUnit::Point => delta.y,
                        MouseWheelUnit::Line => delta.y * WHEEL_LINE,
                        MouseWheelUnit::Page => delta.y * l.main.height(),
                    };
                    if let Some(pos) = hover {
                        self.wheel(pos, pixels, l);
                    }
                }
                Event::Screenshot { image, .. } => self.script.save(&image, ui.ctx()),
                _ => {}
            }
        }

        if let Some(pos) = pressed {
            // An open menu takes the press: on one of its rows it does that; anywhere else it closes.
            if !self.actions_press(pos, ui.max_rect()) {
                self.press(pos, l, modifiers);
            }
        }
        if let Some(pos) = other {
            self.right_press(pos, l, ui.max_rect());
        }
        if let (true, Some(pos)) = (double_clicked, hover) {
            if self.actions.is_none() && !self.on_scrollbar(pos, l) {
                self.double_click(pos, l);
            }
        }
        self.settings_drag(hover, held, l.main);
        self.volume_drag(latest, held, l.volume);
        self.scrollbar_drag(latest, held, l);
        self.carry_on(latest, held, l);
    }

    fn key(&mut self, key: Key, m: Modifiers, ctx: &egui::Context, l: &Layout) {
        if key == Key::Escape && self.carry.take().is_some() {
            return; // the songs being dragged are put down where they were
        }
        if self.help {
            self.help = false;
        } else if self.renaming.is_some() {
            self.rename_key(key, m);
        } else if self.searching {
            self.search_key(key, m);
        } else if self.menu.is_some() {
            self.menu_key(key, Move::from_key(key, m));
        } else if self.actions.is_some() {
            self.actions_key(key, Move::from_key(key, m));
        } else {
            match key {
                Key::Q if m.command => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                Key::N if m.command => self.new_playlist(&[]),
                Key::L if m.shift => self.like_meant(),
                Key::P => self.open_actions_by_key(l),
                Key::F if m.command => self.start_search(),
                Key::F => self.filter_pressed(),
                Key::Questionmark => self.help = true,
                Key::Slash if m.shift => self.help = true,
                Key::Slash => self.start_search(),
                Key::Num1 => self.go(Section::Albums),
                Key::Num2 => self.go(Section::Artists),
                Key::Num3 => self.go(Section::Songs),
                Key::Comma if m.command => self.go(Section::Settings),
                Key::Space => self.toggle_pause(),
                Key::N if m.shift => self.skip(-1),
                Key::N => self.skip(1),
                Key::OpenBracket => self.seek_by(-10.0),
                Key::CloseBracket => self.seek_by(10.0),
                // With Ctrl these zoom the window.
                Key::Minus if !m.command => self.step_volume(-1.0),
                Key::Equals | Key::Plus if !m.command => self.step_volume(1.0),
                Key::M => self.toggle_mute(),
                Key::Tab => self.focus = if self.focus == Focus::Main { Focus::Sidebar } else { Focus::Main },
                Key::Escape if self.let_go_of_picks() => {}
                Key::Escape if self.focus == Focus::Filters => self.leave_filters(),
                Key::Escape => self.back(),
                _ if self.focus == Focus::Sidebar => self.sidebar_key(key, m),
                _ if self.focus == Focus::Filters => self.filter_key(key, Move::from_key(key, m)),
                _ => self.main_key(key, m),
            }
        }
    }

    /// Keys in the sidebar. The arrows pass over "New Playlist", which Ctrl-N chooses; on a playlist, F2 names it
    /// and Delete, pressed twice, deletes it.
    fn sidebar_key(&mut self, key: Key, m: Modifiers) {
        let sections: Vec<Section> = self.sections().into_iter().filter(|&s| s != Section::NewPlaylist).collect();
        let index = sections.iter().position(|&s| s == self.section).unwrap_or(0);
        match (key, Move::from_key(key, m), self.section) {
            (Key::F2, _, Section::Playlist(id)) => self.start_renaming(id),
            (Key::Delete, _, Section::Playlist(id)) => self.delete_playlist_pressed(id),
            (Key::Enter, ..) | (_, Some(Move::Right), _) => self.focus = Focus::Main,
            (_, Some(Move::Up), _) => self.go(sections[index.saturating_sub(1)]),
            (_, Some(Move::Down), _) => self.go(sections[(index + 1).min(sections.len() - 1)]),
            _ => {}
        }
    }

    fn main_key(&mut self, key: Key, m: Modifiers) {
        let mv = Move::from_key(key, m);
        match self.form() {
            // `s` and `S` step through the orders of the grid, forward and back.
            Form::Grid if key == Key::S && self.albums_browser.detail.is_none() => {
                self.sort_albums(self.settings.album_sort.step(if m.shift { -1 } else { 1 }));
            }
            Form::Grid => match self.albums_browser.key(&self.albums, &self.lib, key, m, mv) {
                Outcome::Play(track) => self.play(track),
                Outcome::Leave => self.focus = Focus::Sidebar,
                Outcome::None => {}
            },
            Form::Artists if self.artist_in_grid => {
                match self.artist_browser.key(&self.artist_albums, &self.lib, key, m, mv) {
                    Outcome::Play(track) => self.play(track),
                    Outcome::Leave => self.artist_in_grid = false,
                    Outcome::None => {}
                }
            }
            Form::Artists => match (key, mv) {
                (Key::Enter, _) | (_, Some(Move::Right)) => self.artist_in_grid = !self.artist_albums.is_empty(),
                (_, Some(Move::Left)) => self.focus = Focus::Sidebar,
                (_, Some(mv)) if self.artist_list.go(mv, self.result.artists.len()) => self.artist_changed(),
                _ => {}
            },
            Form::Table => match (key, mv) {
                (Key::Enter, _) => self.play_song_cursor(),
                // A playlist keeps its own order.
                (Key::S, _) if self.listing().is_some() => {}
                (Key::S, _) if m.shift => self.sort_by(self.sort),
                (Key::S, _) => {
                    let at = SORT_CYCLE.iter().position(|&c| c == self.sort).unwrap_or(0);
                    self.sort_by(SORT_CYCLE[(at + 1) % SORT_CYCLE.len()]);
                }
                // In a playlist Delete takes songs out of it, never out of the library.
                (Key::Delete, _) if self.listing().is_some() => self.remove_chosen(),
                (Key::Delete, _) => self.drop_chosen(),
                (Key::A, _) if m.command => {
                    self.picked = self.result.rows.iter().copied().collect();
                }
                (_, Some(Move::Left)) => self.focus = Focus::Sidebar,
                (_, Some(mv)) => {
                    self.songs.go(mv, self.result.rows.len());
                    // Shift with an arrow stretches the picked songs from where the stretch began; a plain move lets them go.
                    if m.shift && matches!(key, Key::ArrowUp | Key::ArrowDown | Key::PageUp | Key::PageDown) {
                        self.pick_to(self.songs.cursor);
                    } else {
                        self.picked.clear();
                        self.anchor = self.songs.cursor;
                    }
                }
                _ => {}
            },
            Form::Settings => self.settings_key(key, mv),
        }
    }

    /// Keys while the search field has the keyboard. Letters arrive as text
    /// events; only editing and navigation are handled here.
    fn search_key(&mut self, key: Key, m: Modifiers) {
        match key {
            Key::Escape => {
                self.searching = false;
                if !self.search.is_empty() {
                    self.search.clear();
                    self.search_changed();
                }
            }
            // Enter acts on the top hit: plays the song, opens the album or artist.
            Key::Enter => {
                self.searching = false;
                self.main_key(Key::Enter, Modifiers::NONE);
            }
            Key::Tab => self.searching = false,
            Key::ArrowDown | Key::ArrowUp | Key::ArrowLeft | Key::ArrowRight | Key::PageDown | Key::PageUp => {
                self.main_key(key, Modifiers::NONE);
                self.focus = Focus::Main;
            }
            Key::U if m.ctrl => {
                self.search.clear();
                self.search_changed();
            }
            Key::W if m.ctrl => self.delete_word(),
            Key::Backspace if m.ctrl || m.alt => self.delete_word(),
            Key::Backspace if self.search.pop().is_some() => self.search_changed(),
            _ => {}
        }
    }

    fn delete_word(&mut self) {
        let text = self.search.trim_end();
        let keep = text.rfind(char::is_whitespace).map_or(0, |i| i + 1);
        self.search.truncate(keep);
        self.search_changed();
    }

    /// Wheel scrolling moves the view under the pointer without moving its cursor.
    fn wheel(&mut self, pos: Pos2, pixels: f32, l: &Layout) {
        if l.volume.contains(pos) {
            return self.volume_wheel(pixels);
        }
        if self.filters_wheel(pos, pixels, l.filters) || !l.main.contains(pos) {
            return;
        }
        match self.form() {
            Form::Grid => self.albums_browser.wheel(pixels),
            Form::Artists if artists_rects(l.main).0.contains(pos) => {
                self.artist_list.scroll(-pixels / ARTIST_ROW, self.result.artists.len());
            }
            Form::Artists => self.artist_browser.wheel(pixels),
            Form::Table => self.songs.scroll(-pixels / SONG_ROW, self.result.rows.len()),
            Form::Settings => {}
        }
    }

    fn press(&mut self, pos: Pos2, l: &Layout, m: Modifiers) {
        self.press_while_renaming(pos, l);
        if l.search.contains(pos) {
            self.start_search();
            return;
        }
        self.searching = false;
        if self.filters_press(pos, l.filters) {
            return;
        }
        if l.play.contains(pos) {
            self.toggle_pause();
        } else if l.previous.contains(pos) {
            self.skip(-1);
        } else if l.next.contains(pos) {
            self.skip(1);
        } else if l.volume.contains(pos) {
            self.volume_press(pos, l.volume);
        } else if actions::display_heart(l.display).contains(pos) && self.playing.is_some() {
            self.like_playing();
        } else if l.display.contains(pos) && pos.y > l.display.bottom() - 12.0 {
            self.seek_to_share(((pos.x - paint::progress_line(l.display).left()) / paint::progress_line(l.display).width()).clamp(0.0, 1.0));
        } else if l.sidebar.contains(pos) {
            if let Some(index) = l.items.iter().position(|item| item.contains(pos)) {
                self.go(self.sections()[index]);
            }
            self.focus = Focus::Sidebar;
        } else if l.main.contains(pos) {
            self.focus = Focus::Main;
            if !self.scrollbar_press(pos, l) && !self.heart_press(pos, l) {
                let narrow = self.press_main(pos, l, m);
                self.take_up(pos, l, narrow);
            }
        }
    }

    /// A press in the content. A plain press on one of several picked songs leaves them picked, so that they can be
    /// dragged together, and returns where it was: letting go without a drag then narrows the pick to that song.
    fn press_main(&mut self, pos: Pos2, l: &Layout, m: Modifiers) -> Option<carry::Narrow> {
        let tile = self.settings.cover;
        match self.form() {
            Form::Grid => return self.albums_browser.press(&self.albums, l.main, pos, tile, m).map(carry::Narrow::Albums),
            Form::Artists => {
                let (list, pane) = artists_rects(l.main);
                let artists = self.result.artists.len();
                if let Some(row) = self.artist_list.hit(list, ARTIST_ROW, pos, artists) {
                    self.artist_in_grid = false;
                    if self.artist_list.set(row, artists) {
                        self.artist_changed();
                    }
                } else if pane.contains(pos) {
                    self.artist_in_grid = !self.artist_albums.is_empty();
                    return self.artist_browser.press(&self.artist_albums, pane_rects(pane).1, pos, tile, m).map(carry::Narrow::Artist);
                }
            }
            Form::Table => {
                let (head, rows) = songs_rects(l.main);
                if head.contains(pos) {
                    // A playlist keeps its own order.
                    let columns = song_columns(head, matches!(self.section, Section::Rest(_)));
                    if let (None, Some(spec)) = (self.listing(), columns.iter().find(|c| (c.left..=c.right).contains(&pos.x))) {
                        self.sort_by(spec.column);
                    }
                } else if let Some(row) = self.songs.hit(rows, SONG_ROW, pos, self.result.rows.len()) {
                    let (was, id) = (self.songs.cursor, self.result.rows[row]);
                    self.songs.set(row, self.result.rows.len());
                    if m.shift {  // everything from where the last plain click was to here
                        self.pick_to(row);
                    } else if m.command {  // this one in or out, keeping the rest
                        if self.picked.is_empty() {
                            self.picked.extend(self.result.rows.get(was).copied());
                        }
                        if !self.picked.remove(&id) {
                            self.picked.insert(id);
                        }
                        self.anchor = row;
                    } else if self.picked.len() > 1 && self.picked.contains(&id) {
                        return Some(carry::Narrow::Table(row));
                    } else {
                        self.picked.clear();
                        self.anchor = row;
                    }
                }
            }
            Form::Settings => self.settings_press(pos, l.main),
        }
        None
    }

    fn double_click(&mut self, pos: Pos2, l: &Layout) {
        // A playlist's name in the sidebar is changed where it stands.
        if let Some(index) = l.items.iter().position(|item| item.contains(pos)) {
            if let Some(Section::Playlist(id)) = self.sections().get(index) {
                self.start_renaming(*id);
            }
            return;
        }
        let tile = self.settings.cover;
        let track = match self.form() {
            Form::Grid => self.albums_browser.double_click(&self.albums, &self.lib, l.main, pos, tile),
            Form::Artists => {
                let body = pane_rects(artists_rects(l.main).1).1;
                self.artist_browser.double_click(&self.artist_albums, &self.lib, body, pos, tile)
            }
            Form::Settings => None,
            Form::Table => {
                let rows = songs_rects(l.main).1;
                self.songs.hit(rows, SONG_ROW, pos, self.result.rows.len()).map(|row| self.result.rows[row])
            }
        };
        if let Some(track) = track {
            self.play(track);
        }
    }

    /// Fits every list of the current section to the window; runs after
    /// input so a moved cursor is scrolled into view in the same frame.
    fn settle(&mut self, l: &Layout) {
        self.settle_menu();
        let tile = self.settings.cover;
        match self.form() {
            Form::Grid => self.albums_browser.settle(self.albums.len(), l.main, tile),
            Form::Artists => {
                let (list, pane) = artists_rects(l.main);
                self.artist_list.settle(self.result.artists.len(), rows_in(list, ARTIST_ROW));
                self.artist_browser.settle(self.artist_albums.len(), pane_rects(pane).1, tile);
            }
            Form::Settings => {}
            Form::Table => {
                let rows = songs_rects(l.main).1;
                self.songs.settle(self.result.rows.len(), rows_in(rows, SONG_ROW));
            }
        }
    }
}

/// The states of an unfinished index as the sidebar lists them.
fn rest_states(lib: &Library) -> Vec<RestState> {
    let mut tracks_in = vec![0u32; lib.states.len()];
    for &state in &lib.state_of {
        tracks_in[state as usize] += 1;
    }
    let place = |name: &str| REST_ORDER.iter().position(|(known, _)| *known == name).unwrap_or(REST_ORDER.len());
    let mut states: Vec<RestState> = (0..lib.states.len())
        .filter(|&id| tracks_in[id] > 0)
        .map(|id| RestState {
            id: id as u32,
            name: lib.states[id].clone(),
            tracks: tracks_in[id],
            albums: REST_ORDER.get(place(&lib.states[id])).is_some_and(|(_, albums)| *albums),
        })
        .collect();
    states.sort_by_key(|state| (place(&state.name), state.name.clone()));
    states
}

/// What the album views show of each album. In the ready catalogue (`ready`) an album is verified when it holds every
/// track of its release, each confirmed: none listed as missing, none dropped since, none placed by its name or length
/// alone, none still unfinished.
fn album_info(lib: &Library, ready: bool) -> Vec<AlbumInfo> {
    let mut info = vec![AlbumInfo::default(); lib.albums.len()];
    let mut whole = vec![(0u32, ready); lib.albums.len()]; // per album: the release's track count, and whether every track is finished
    let mut qualities = vec![Vec::new(); lib.albums.len()];
    for (i, track) in lib.tracks.iter().enumerate() {
        let album = &mut info[lib.album_of[i] as usize];
        let (count, finished) = &mut whole[lib.album_of[i] as usize];
        *count = (*count).max(track.track_total);
        *finished &= track.state.is_empty() && !track.unconfirmed;
        if track.missing {
            album.missing += 1;
            continue;
        }
        album.tracks += 1;
        album.ms += track.dur_ms as u64;
        album.year = album.year.max(track.year);
        album.released = album.released.max(track.released());
        album.added = album.added.max(track.mtime);
        qualities[lib.album_of[i] as usize].push(track.quality());
        album.kind = lib.kind_of[i];
        if album.art == 0 {
            album.art = cover_id(&track.art);
        }
    }
    for ((album, (count, finished)), mut sounds) in info.iter_mut().zip(whole).zip(qualities) {
        album.verified = finished && album.missing == 0 && album.tracks > 0 && album.tracks >= count;
        if !sounds.is_empty() {
            let middle = sounds.len() / 2;
            album.quality = *sounds.select_nth_unstable(middle).1;
        }
    }
    // Album ids already ascend by title, so they break ties within a year.
    let mut order: Vec<u32> = (0..info.len() as u32).collect();
    order.sort_unstable_by_key(|&id| {
        let year = info[id as usize].year;
        (lib.albums[id as usize].artist, if year == 0 { u32::MAX } else { year }, id)
    });
    for (rank, id) in order.into_iter().enumerate() {
        info[id as usize].rank = rank as u32;
    }
    info
}

/// Puts albums in an order. Albums alike in it stay by artist, then year; one whose date is not known comes last either way.
fn grid_order(info: &[AlbumInfo], mut ids: Vec<u32>, order: AlbumOrder) -> Vec<u32> {
    ids.sort_unstable_by_key(|&id| {
        let album = &info[id as usize];
        match order {
            AlbumOrder::Newest => (album.released == 0, -i64::from(album.released), album.rank),
            AlbumOrder::Oldest => (album.released == 0, i64::from(album.released), album.rank),
            AlbumOrder::Added => (false, -album.added, album.rank),
            AlbumOrder::Artist => (false, 0, album.rank),
            AlbumOrder::Title => (false, 0, id), // ids ascend by title
            AlbumOrder::Sound => (false, -i64::from(album.quality), album.rank),
        }
    });
    ids
}

/// Parses the cover id stored with a track; 0 when it has none.
fn cover_id(art: &str) -> u64 {
    u64::from_str_radix(art, 16).unwrap_or(0)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let screen = ui.max_rect();
        let layout = Layout::new(screen, &self.sections(), self.filters_shown());
        self.art.begin_frame(ui.ctx());
        if let Some(other) = &mut self.shelved {
            other.art.begin_frame(ui.ctx());
        }
        self.follow_player();
        self.follow_scan();
        self.handle_input(ui, &layout);
        self.keep_volume(false);
        // Input may have changed the sections, the index on screen or whether the filter bar shows.
        let layout = Layout::new(screen, &self.sections(), self.filters_shown());
        self.settle(&layout);
        paint::all(self, ui.painter(), &layout, screen);
        if self.carry.as_ref().is_some_and(carry::Carry::moving) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        self.art.end_frame();
        if let Some(other) = &mut self.shelved {
            other.art.end_frame();
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.keep_volume(true);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::BG.to_normalized_gamma_f32()
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.script.inject(ctx, raw_input);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wax_core::index::Track;

    fn track(album: &str, n: u32, of: u32) -> Track {
        Track { path: format!("{album}/{n}.mp3"), album: album.into(), album_artist: "A".into(), title: format!("t{n}"), track: n, track_total: of, ..Track::default() }
    }

    #[test]
    fn songs_play_from_the_folder_the_index_was_made_of_else_from_the_server() {
        let mut settings = Settings::default();
        assert_eq!(origin_of(Path::new(""), &settings), Origin::Nowhere, "nothing set up yet");
        settings.server = "music-box".into();
        assert_eq!(origin_of(Path::new(""), &settings), Origin::Server { host: "music-box".into(), home: "wax".into() });
        let folder = Path::new("/srv/music");
        assert_eq!(origin_of(folder, &settings), Origin::Folder(folder.to_owned()), "a list of a folder here plays from it");
    }

    #[test]
    fn the_volume_sits_beside_the_buttons_until_the_window_is_too_narrow() {
        let layout = |width: f32| Layout::new(Rect::from_min_size(Pos2::ZERO, vec2(width, 700.0)), &[Section::Albums, Section::Settings], false);
        let wide = layout(1280.0);
        assert!(wide.volume.left() > wide.next.right() && wide.volume.right() < wide.display.left(), "between the buttons and the display");
        let narrow = layout(560.0);
        assert!(narrow.volume.width() > 0.0 && narrow.volume.right() < narrow.search.left(), "kept while the search field leaves room");
        assert_eq!(narrow.display, Rect::ZERO, "the display gives way first");
        assert_eq!(layout(400.0).volume, Rect::ZERO);
    }

    #[test]
    fn an_album_is_verified_only_when_whole_and_in_the_ready_catalogue() {
        let mut tracks = vec![track("Whole", 1, 2), track("Whole", 2, 2), track("Gap", 1, 2), track("Gap", 2, 2), track("Short", 1, 3), track("Short", 2, 3)];
        tracks[3].missing = true;
        tracks[3].path.clear();
        let lib = Library::new(tracks.clone());
        let verified = |lib: &Library, info: &[AlbumInfo], name: &str| info[lib.albums.iter().position(|a| a.name == name).unwrap()].verified;
        let info = album_info(&lib, true);
        assert!(verified(&lib, &info, "Whole"));
        assert!(!verified(&lib, &info, "Gap"), "a track listed as missing");
        assert!(!verified(&lib, &info, "Short"), "a track dropped since");
        assert!(!verified(&lib, &album_info(&lib, false), "Whole"), "an index the librarian did not mark");
        tracks[1].unconfirmed = true;
        let lib = Library::new(tracks.clone());
        assert!(!verified(&lib, &album_info(&lib, true), "Whole"), "a track placed by its name alone");
        tracks[1].unconfirmed = false;
        tracks[0].state = "Likely".into();
        let lib = Library::new(tracks);
        assert!(!verified(&lib, &album_info(&lib, true), "Whole"), "an unfinished track");
    }

    #[test]
    fn the_album_grid_goes_by_release_day_newest_first_and_can_be_put_in_other_orders() {
        let on = |album: &str, artist: &str, year: u32, date: &str, bitrate: u32, mtime: i64| Track {
            album_artist: artist.into(),
            year,
            date: date.into(),
            bitrate,
            mtime,
            ..track(album, 1, 1)
        };
        let lib = Library::new(vec![
            on("First Light", "The Night Owls", 2004, "2004-06-07", 192, 30),
            on("Salt Meridian", "The Night Owls", 2006, "2006-10-02", 320, 10),
            on("Bright Harbor", "Amber Coast", 2004, "2004-09-21", 256, 20),
            on("Dust Ledger", "Amber Coast", 0, "", 128, 40),
            on("Window Atlas", "Amber Coast", 2000, "", 0, 5),
        ]);
        let info = album_info(&lib, true);
        let names = |order: AlbumOrder| -> Vec<String> {
            grid_order(&info, (0..lib.albums.len() as u32).collect(), order).into_iter().map(|id| lib.albums[id as usize].name.clone()).collect()
        };
        assert_eq!(AlbumOrder::default(), AlbumOrder::Newest);
        assert_eq!(names(AlbumOrder::Newest), ["Salt Meridian", "Bright Harbor", "First Light", "Window Atlas", "Dust Ledger"], "by the day, the undated last");
        assert_eq!(names(AlbumOrder::Oldest), ["Window Atlas", "First Light", "Bright Harbor", "Salt Meridian", "Dust Ledger"], "a year alone before its days");
        assert_eq!(names(AlbumOrder::Added), ["Dust Ledger", "First Light", "Bright Harbor", "Salt Meridian", "Window Atlas"]);
        assert_eq!(names(AlbumOrder::Artist), ["Window Atlas", "Bright Harbor", "Dust Ledger", "First Light", "Salt Meridian"]);
        assert_eq!(names(AlbumOrder::Title), ["Bright Harbor", "Dust Ledger", "First Light", "Salt Meridian", "Window Atlas"]);
        assert_eq!(names(AlbumOrder::Sound), ["Salt Meridian", "Bright Harbor", "First Light", "Dust Ledger", "Window Atlas"]);
        assert_eq!(AlbumOrder::Newest.step(-1), AlbumOrder::Sound, "S goes back round");
    }

    #[test]
    fn tracks_of_an_album_page_are_picked_by_shift_and_ctrl_and_a_press_on_them_keeps_them_for_a_drag() {
        let lib = Library::new((1..=5).map(|n| track("Five", n, 5)).collect());
        let mut detail = Detail::open(&lib, 0, None);
        let (plain, shift, ctrl) = (Modifiers::NONE, Modifiers::SHIFT, Modifiers { ctrl: true, command: true, ..Modifiers::NONE });
        assert_eq!(detail.chosen(), [detail.tracks[0]], "nothing picked: the track under the cursor");
        assert_eq!(detail.click(2, shift), None);
        assert_eq!(detail.chosen(), detail.tracks[..3], "from the first to the third");
        assert_eq!(detail.click(4, ctrl), None);
        assert_eq!(detail.chosen(), [detail.tracks[0], detail.tracks[1], detail.tracks[2], detail.tracks[4]]);
        assert_eq!(detail.click(1, plain), Some(1), "a plain press on a picked track keeps them all picked");
        assert_eq!(detail.chosen().len(), 4);
        detail.pick_only(1);
        assert_eq!(detail.chosen(), [detail.tracks[1]], "until the button comes up without a drag");
        assert_eq!(detail.click(3, plain), None);
        assert_eq!(detail.chosen(), [detail.tracks[3]]);
    }

    #[test]
    fn an_album_of_several_discs_lists_a_heading_over_each_that_the_cursor_steps_past() {
        let mut tracks = vec![track("Single", 1, 2), track("Single", 2, 2), track("Single", 1, 1), track("One", 1, 1)];
        tracks[0].disc_name = "CD · SKINT 100CDP".into();
        tracks[1].disc_name = "CD · SKINT 100CDP".into();
        tracks[2].disc = 2;
        tracks[2].path = "Single/2-1.mp3".into();
        for t in &mut tracks[..2] {
            t.disc = 1;
        }
        let lib = Library::new(tracks);
        let open = |name: &str| Detail::open(&lib, lib.albums.iter().position(|a| a.name == name).unwrap() as u32, None);
        let mut detail = open("Single");
        let headings: Vec<&str> = detail.lines.iter().filter_map(|line| if let Line::Disc(name) = line { Some(name.as_str()) } else { None }).collect();
        assert_eq!(headings, ["CD · SKINT 100CDP", "Disc 2"]);
        assert_eq!((detail.lines.len(), detail.tracks.len(), detail.list.cursor), (5, 3, 1), "opened on the first track, under its heading");
        detail.go(Move::Down);
        detail.go(Move::Down);
        assert_eq!(detail.list.cursor, 4, "over the second heading to the track under it");
        detail.go(Move::Up);
        assert_eq!(detail.list.cursor, 2, "and back over it to the last track above");
        detail.go(Move::Top);
        detail.go(Move::Up);
        assert_eq!(detail.list.cursor, 1, "never up onto the first heading");
        assert!(open("One").lines.iter().all(|line| matches!(line, Line::Track(_))), "one disc: no heading");
    }
}
