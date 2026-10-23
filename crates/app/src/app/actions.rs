//! Liking songs and keeping playlists: the hearts, the menu of things to do
//! with songs or with a playlist (right-click, or `p`), naming a playlist in
//! the sidebar, and the playlists' own song lists.
//!
//! Likes and playlists hold for the library only; the unfinished part of the
//! collection, in developer mode, has neither.

use eframe::egui::{Align2, Color32, Painter, Pos2, Rect, Stroke, StrokeKind, pos2, vec2};
use wax_core::index::Track;

use super::*;
use crate::draw;
use crate::format::count;

/// The room a heart takes at the left of the time in a song table.
pub(super) const HEART_CELL: f32 = 24.0;
const MENU_WIDTH: f32 = 264.0;
const MENU_ROW: f32 = 28.0;
/// How long a first Delete on a playlist in the sidebar waits for the second.
const ARMED_FOR: Duration = Duration::from_secs(5);

/// The heart in the now-playing display, which likes the song playing.
pub(super) fn display_heart(display: Rect) -> Rect {
    Rect::from_center_size(pos2(display.right() - 22.0, display.top() + 31.0), vec2(22.0, 20.0))
}

/// The heart of a song table's row, at the left of its time.
pub(super) fn table_heart(time: &ColumnSpec, row: Rect) -> Rect {
    Rect::from_center_size(pos2(time.left + 10.0, row.center().y), vec2(22.0, row.height()))
}

/// The heart of a track on an opened album's page, before its time.
pub(super) fn track_heart(row: Rect) -> Rect {
    Rect::from_center_size(pos2(row.right() - 76.0, row.center().y), vec2(22.0, row.height()))
}

/// A playlist being named in the sidebar.
pub(super) struct Naming {
    pub(super) what: Named,
    pub(super) name: String,
    /// The whole name is selected, as when naming begins: what is typed replaces it.
    pub(super) selected: bool,
}

/// What is being named.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Named {
    Playlist(u64),
    /// The music folder, on the settings page.
    Folder,
}

impl Naming {
    pub(super) fn type_in(&mut self, text: &str) {
        if self.selected {
            self.name.clear();
            self.selected = false;
        }
        self.name.extend(text.chars().filter(|c| !c.is_control()));
    }
}

/// The list a playlist section shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Listing {
    Liked,
    Playlist(u64),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    /// Like the songs (true), or unlike them.
    Like(bool),
    /// A line naming what the rows under it do; not chosen itself.
    Heading(&'static str),
    AddTo(u64),
    AddToNew,
    /// Take the songs out of the playlist on screen.
    Remove(u64),
    /// Drop the songs from the library.
    Drop,
    Rename(u64),
    DeletePlaylist(u64),
}

/// A menu of things to do, opened on songs or on a playlist.
pub(super) struct Actions {
    at: Pos2,
    /// The songs it acts on, by track; none for a playlist's own menu.
    tracks: Vec<u32>,
    items: Vec<Action>,
    cursor: usize,
    /// "Delete playlist" was chosen once: choosing it again deletes.
    armed: bool,
}

impl Actions {
    fn new(at: Pos2, tracks: Vec<u32>, items: Vec<Action>) -> Self {
        let cursor = items.iter().position(|item| !matches!(item, Action::Heading(_))).unwrap_or(0);
        Actions { at, tracks, items, cursor, armed: false }
    }

    fn frame(&self, screen: Rect) -> Rect {
        let size = vec2(MENU_WIDTH, self.items.len() as f32 * MENU_ROW + 12.0);
        let x = self.at.x.min(screen.right() - size.x - 8.0).max(screen.left() + 8.0);
        let y = self.at.y.min(screen.bottom() - size.y - 8.0).max(screen.top() + 8.0);
        Rect::from_min_size(pos2(x, y), size)
    }

    fn row_rect(frame: Rect, row: usize) -> Rect {
        Rect::from_min_size(pos2(frame.left() + 6.0, frame.top() + 6.0 + row as f32 * MENU_ROW), vec2(frame.width() - 12.0, MENU_ROW))
    }

    /// The row of a thing to do under `pos`; None over a heading or outside.
    fn row_at(&self, screen: Rect, pos: Pos2) -> Option<usize> {
        let frame = self.frame(screen);
        (0..self.items.len()).find(|&row| Self::row_rect(frame, row).contains(pos) && !matches!(self.items[row], Action::Heading(_)))
    }

    /// Moves the cursor a row up (-1) or down (1), over headings.
    fn step(&mut self, by: isize) {
        let mut at = self.cursor as isize;
        loop {
            at += by;
            match self.items.get(at as usize) {
                Some(Action::Heading(_)) => continue,
                Some(_) if at >= 0 => {
                    self.cursor = at as usize;
                    self.armed = false;
                    return;
                }
                _ => return,
            }
        }
    }
}

/// "1 song" or "3 songs".
fn songs(n: usize) -> String {
    if n == 1 { "1 song".to_owned() } else { format!("{} songs", count(n)) }
}

impl Browser {
    /// The songs a right-click at `pos` means, moving the cursor there: the picked tracks of the opened album when it
    /// is on one of them, else the track it is on; on a cover, every track of that album.
    fn songs_at(&mut self, albums: &[u32], lib: &Library, area: Rect, pos: Pos2, tile: f32) -> Vec<u32> {
        match &mut self.detail {
            Some(detail) => {
                let hit = detail.list.hit(detail_rects(area).rows, TRACK_ROW, pos, detail.lines.len());
                match hit.and_then(|row| detail.track(row).map(|track| (row, track))) {
                    Some((_, track)) if detail.picked.contains(&track) => detail.chosen(),
                    Some((row, track)) => {
                        detail.list.set(row, detail.lines.len());
                        detail.pick_only(row);
                        vec![track]
                    }
                    None => Vec::new(),
                }
            }
            None => match Cells::new(area, tile).hit(pos, self.grid.scroll, albums.len()) {
                Some(index) => {
                    self.grid.set(index, albums.len());
                    Detail::open(lib, albums[index], self.state).tracks
                }
                None => Vec::new(),
            },
        }
    }

    /// The songs the keyboard is on: the picked tracks of the opened album, or the one under its cursor; or the album
    /// under the cursor.
    fn songs_here(&self, albums: &[u32], lib: &Library) -> Vec<u32> {
        match &self.detail {
            Some(detail) => detail.chosen(),
            None => albums.get(self.grid.cursor).map_or_else(Vec::new, |&album| Detail::open(lib, album, self.state).tracks),
        }
    }
}

impl App {
    /// The list the section on screen is, if it is a playlist's.
    pub(super) fn listing(&self) -> Option<Listing> {
        match self.section {
            Section::Liked => Some(Listing::Liked),
            Section::Playlist(id) => Some(Listing::Playlist(id)),
            _ => None,
        }
    }

    /// After the index on screen has changed: where its tracks are, which are liked, and how many songs each playlist has.
    pub(super) fn library_changed(&mut self) {
        self.places = Places::new(&self.lib);
        self.likes_changed();
    }

    fn likes_changed(&mut self) {
        let paths: HashSet<&str> = self.store.liked.iter().map(|entry| entry.path.as_str()).collect();
        let recordings: HashSet<&str> = self.store.liked.iter().map(|entry| entry.recording.as_str()).filter(|r| !r.is_empty()).collect();
        self.liked = self
            .lib
            .tracks
            .iter()
            .map(|t| !t.missing && (paths.contains(t.path.as_str()) || (!t.mb_recording.is_empty() && recordings.contains(t.mb_recording.as_str()))))
            .collect();
        // The playlists are of the library: counted only while it is on screen.
        if !self.rest_shown {
            self.counts = std::iter::once((0, self.places.resolve(&self.store.liked, &self.lib).len()))
                .chain(self.store.playlists.iter().map(|playlist| (playlist.id, self.places.resolve(&playlist.entries, &self.lib).len())))
                .collect();
        }
    }

    /// After a search: a playlist's section keeps to its songs, in its order; the liked songs go the latest first.
    pub(super) fn keep_to_playlist(&mut self) {
        let order: Vec<u32> = match self.listing() {
            Some(Listing::Liked) => {
                let latest_first: Vec<_> = self.store.liked.iter().rev().cloned().collect();
                self.places.resolve(&latest_first, &self.lib)
            }
            Some(Listing::Playlist(id)) => self.store.playlist(id).map_or_else(Vec::new, |playlist| self.places.resolve(&playlist.entries, &self.lib)),
            None => return,
        };
        let place: HashMap<u32, usize> = order.into_iter().enumerate().map(|(at, id)| (id, at)).collect();
        self.result.rows.retain(|id| place.contains_key(id));
        self.result.rows.sort_by_key(|id| place[id]);
        let tracks = &self.lib.tracks;
        self.result.total_ms = self.result.rows.iter().map(|&id| tracks[id as usize].dur_ms as u64).sum();
        self.result.total_bytes = self.result.rows.iter().map(|&id| tracks[id as usize].size).sum();
    }

    /// Saves the store and brings everything that shows it up to date; `said` goes in the status bar.
    pub(super) fn store_changed(&mut self, said: String) {
        self.notice = Some((self.store.save().err().unwrap_or(said), Instant::now()));
        self.likes_changed();
        if self.listing().is_some() {
            let (cursor, top) = (self.songs.cursor, self.songs.top);
            self.search_changed();
            self.songs.top = top;
            self.songs.set(cursor, self.result.rows.len());
            self.anchor = self.songs.cursor;
        }
    }

    /// The held songs of a set of tracks of the index on screen.
    pub(super) fn held<'a>(lib: &'a Library, tracks: &[u32]) -> Vec<&'a Track> {
        tracks.iter().map(|&id| &lib.tracks[id as usize]).filter(|track| !track.missing).collect()
    }

    /// Whether likes and playlists can be had here; says why not when they cannot.
    fn lists_allowed(&mut self) -> bool {
        if self.rest_shown {
            self.notice = Some(("Only songs of the library can be liked or put in a playlist".to_owned(), Instant::now()));
        }
        !self.rest_shown
    }

    fn toggle_like(&mut self, tracks: &[u32]) {
        if !self.lists_allowed() {
            return;
        }
        let songs = Self::held(&self.lib, tracks);
        if songs.is_empty() {
            return;
        }
        let n = songs.len();
        let liked = self.store.toggle_like(&songs);
        self.store_changed(format!("{} {}", if liked { "Liked" } else { "Unliked" }, if n == 1 { songs_title(&self.lib, tracks) } else { self::songs(n) }));
    }

    /// Shift-L: likes, or unlikes, the songs the keyboard is on; where it is on none, the song playing.
    pub(super) fn like_meant(&mut self) {
        let tracks = match (self.focus, self.form()) {
            (Focus::Main, Form::Table) => self.chosen(),
            (Focus::Main, Form::Grid) => self.albums_browser.detail.as_ref().map_or_else(Vec::new, Detail::chosen),
            (Focus::Main, Form::Artists) if self.artist_in_grid => self.artist_browser.detail.as_ref().map_or_else(Vec::new, Detail::chosen),
            _ => Vec::new(),
        };
        if tracks.is_empty() { self.like_playing() } else { self.toggle_like(&tracks) }
    }

    /// The song playing, liked or unliked: from the heart in the display, or Shift-L where no song is under the keyboard.
    pub(super) fn like_playing(&mut self) {
        if self.playing_rest {
            self.notice = Some(("Only songs of the library can be liked or put in a playlist".to_owned(), Instant::now()));
            return;
        }
        let lib = match &self.shelved {
            Some(other) if self.rest_shown => &other.lib,
            _ => &self.lib,
        };
        let Some(track) = self.playing.map(|id| &lib.tracks[id as usize]) else { return };
        let (title, liked) = (track.title.clone(), self.store.toggle_like(&[track]));
        self.store_changed(format!("{} {title}", if liked { "Liked" } else { "Unliked" }));
    }

    /// A press on a song's heart likes or unlikes it. Returns whether the press was on one.
    pub(super) fn heart_press(&mut self, pos: Pos2, l: &Layout) -> bool {
        if self.rest_shown {
            return false;
        }
        let hit = match self.form() {
            Form::Table => {
                let (head, rows) = songs_rects(l.main);
                let columns = song_columns(head, false);
                self.songs.hit(rows, SONG_ROW, pos, self.result.rows.len()).filter(|&row| {
                    columns.last().is_some_and(|time| table_heart(time, row_rect(rows, row - self.songs.top, SONG_ROW)).contains(pos))
                }).map(|row| self.result.rows[row])
            }
            Form::Grid => Self::track_heart_hit(&self.albums_browser, l.main, pos),
            Form::Artists => Self::track_heart_hit(&self.artist_browser, pane_rects(artists_rects(l.main).1).1, pos),
            Form::Settings => None,
        };
        match hit.filter(|&id| !self.lib.tracks[id as usize].missing) {
            Some(id) => {
                self.toggle_like(&[id]);
                true
            }
            None => false,
        }
    }

    fn track_heart_hit(browser: &Browser, area: Rect, pos: Pos2) -> Option<u32> {
        let detail = browser.detail.as_ref()?;
        let rows = detail_rects(area).rows;
        let row = detail.list.hit(rows, TRACK_ROW, pos, detail.lines.len())?;
        track_heart(row_rect(rows, row - detail.list.top, TRACK_ROW)).contains(pos).then(|| detail.track(row)).flatten()
    }

    /// Makes a playlist holding `tracks`, goes there, and has it named in the sidebar.
    pub(super) fn new_playlist(&mut self, tracks: &[u32]) {
        if !self.lists_allowed() {
            return;
        }
        let id = self.store.create("");
        let songs = Self::held(&self.lib, tracks);
        let added = self.store.add(id, &songs);
        self.go(Section::Playlist(id));
        let said = if added == 0 { "Made a playlist: type its name".to_owned() } else { format!("Made a playlist of {}: type its name", self::songs(added)) };
        self.store_changed(said);
        self.focus = Focus::Sidebar;
        self.renaming = Some(Naming { what: Named::Playlist(id), name: String::new(), selected: false });
    }

    pub(super) fn add_to(&mut self, id: u64, tracks: &[u32]) {
        let songs = Self::held(&self.lib, tracks);
        let (added, name) = (self.store.add(id, &songs), self.label(Section::Playlist(id)).to_owned());
        let said = match added {
            0 if songs.len() == 1 => format!("{name} has this song already"),
            0 => format!("{name} has these songs already"),
            n if n < songs.len() => format!("Added {} to {name}; it had {} already", self::songs(n), songs.len() - n),
            n => format!("Added {} to {name}", self::songs(n)),
        };
        self.store_changed(said);
    }

    fn remove_from(&mut self, id: u64, tracks: &[u32]) {
        let songs = Self::held(&self.lib, tracks);
        let (taken, name) = (self.store.remove(id, &songs), self.label(Section::Playlist(id)).to_owned());
        self.picked.clear();
        self.store_changed(format!("Took {} out of {name}", self::songs(taken)));
    }

    /// Delete in a playlist's song list: takes the songs out of it; in the liked songs, unlikes them.
    pub(super) fn remove_chosen(&mut self) {
        let tracks = self.chosen();
        match self.listing() {
            Some(Listing::Playlist(id)) => self.remove_from(id, &tracks),
            Some(Listing::Liked) => {
                let songs = Self::held(&self.lib, &tracks);
                let taken = self.store.unlike(&songs);
                self.picked.clear();
                self.store_changed(format!("Unliked {}", self::songs(taken.min(songs.len()))));
            }
            None => {}
        }
    }

    /// Begins naming a playlist that has a name: the name is selected, so that typing replaces it.
    pub(super) fn start_renaming(&mut self, id: u64) {
        let name = self.label(Section::Playlist(id)).to_owned();
        self.actions = None;
        self.searching = false;
        self.renaming = Some(Naming { what: Named::Playlist(id), name, selected: true });
    }

    /// Keys while a playlist or the music folder is being named: typing goes into the name; Enter keeps it, Esc leaves
    /// the name as it was.
    /// Backspace on the selected name clears it; an arrow, Home or End lets go of the selection, to add to the name.
    pub(super) fn rename_key(&mut self, key: Key, m: Modifiers) {
        let Some(naming) = &mut self.renaming else { return };
        match key {
            Key::Enter | Key::Tab => self.finish_renaming(true),
            Key::Escape => self.finish_renaming(false),
            Key::U if m.ctrl => naming.name.clear(),
            Key::Backspace if m.ctrl || m.alt || naming.selected => naming.name.clear(),
            Key::Backspace => {
                naming.name.pop();
            }
            Key::ArrowLeft | Key::ArrowRight | Key::Home | Key::End => {}
            _ => return,
        }
        if let Some(naming) = &mut self.renaming {
            naming.selected = false;
        }
    }

    fn finish_renaming(&mut self, keep: bool) {
        let Some(Naming { what, name, .. }) = self.renaming.take() else { return };
        match what {
            Named::Playlist(id) => {
                if keep && !name.trim().is_empty() && name.trim() != self.label(Section::Playlist(id)) {
                    self.store.rename(id, &name);
                    self.store_changed(format!("Named the playlist {}", name.trim()));
                }
            }
            Named::Folder if keep => self.choose_music_folder(name),
            Named::Folder => {}
        }
    }

    /// A press anywhere but on the playlist being named keeps the name typed; one off the music folder's field
    /// leaves the folder as it was.
    pub(super) fn press_while_renaming(&mut self, pos: Pos2, l: &Layout) {
        let Some(what) = self.renaming.as_ref().map(|naming| naming.what) else { return };
        match what {
            Named::Playlist(id) => {
                let on_it =
                    self.sections().iter().position(|&s| s == Section::Playlist(id)).and_then(|i| l.items.get(i)).is_some_and(|item| item.contains(pos));
                if !on_it {
                    self.finish_renaming(true);
                }
            }
            Named::Folder => {
                if !self.on_folder_row(pos, l.main) {
                    self.finish_renaming(false);
                }
            }
        }
    }

    /// Delete on a playlist in the sidebar: the first press asks, a second within a few seconds deletes it.
    pub(super) fn delete_playlist_pressed(&mut self, id: u64) {
        match self.arming {
            Some((armed, since)) if armed == id && since.elapsed() < ARMED_FOR => self.delete_playlist(id),
            _ => {
                self.arming = Some((id, Instant::now()));
                let name = self.label(Section::Playlist(id)).to_owned();
                self.notice = Some((format!("Press Delete again to delete the playlist {name}"), Instant::now()));
            }
        }
    }

    fn delete_playlist(&mut self, id: u64) {
        self.arming = None;
        let Some(playlist) = self.store.delete(id) else { return };
        if self.section == Section::Playlist(id) {
            self.go(Section::Liked);
        }
        self.store_changed(format!("Deleted the playlist {} ({})", playlist.name, songs(playlist.entries.len())));
    }

    /// Opens the menu for songs at `at`. Missing tracks are left out of it; with none left there is no menu.
    fn open_song_actions(&mut self, at: Pos2, tracks: Vec<u32>) {
        let tracks: Vec<u32> = tracks.into_iter().filter(|&id| !self.lib.tracks[id as usize].missing).collect();
        if tracks.is_empty() {
            return;
        }
        let mut items = Vec::new();
        if !self.rest_shown {
            items.push(Action::Like(!tracks.iter().all(|&id| self.liked[id as usize])));
            let here = match self.listing() {
                Some(Listing::Playlist(id)) => Some(id),
                _ => None,
            };
            items.extend(here.map(Action::Remove));
            items.push(Action::Heading("ADD TO PLAYLIST"));
            items.extend(self.store.playlists.iter().filter(|playlist| Some(playlist.id) != here).map(|playlist| Action::AddTo(playlist.id)));
            items.push(Action::AddToNew);
        }
        if self.form() == Form::Table && self.listing().is_none() {
            items.push(Action::Drop);
        }
        if !items.is_empty() {
            self.actions = Some(Actions::new(at, tracks, items));
        }
    }

    fn open_playlist_actions(&mut self, at: Pos2, id: u64) {
        self.actions = Some(Actions::new(at, Vec::new(), vec![Action::Rename(id), Action::DeletePlaylist(id)]));
    }

    /// A press of the other mouse button: on songs it opens their menu, on a playlist in the sidebar the playlist's.
    pub(super) fn right_press(&mut self, pos: Pos2, l: &Layout, _screen: Rect) {
        self.actions = None;
        if self.on_scrollbar(pos, l) {
            return;
        }
        if let Some(index) = l.items.iter().position(|item| item.contains(pos)) {
            if let Some(Section::Playlist(id)) = self.sections().get(index).copied() {
                self.open_playlist_actions(pos, id);
            }
            return;
        }
        if !l.main.contains(pos) {
            return;
        }
        let tile = self.settings.cover;
        let tracks = match self.form() {
            Form::Table => {
                let rows = songs_rects(l.main).1;
                match self.songs.hit(rows, SONG_ROW, pos, self.result.rows.len()) {
                    Some(row) => {
                        if !self.picked.contains(&self.result.rows[row]) {
                            self.picked.clear();
                            self.songs.set(row, self.result.rows.len());
                            self.anchor = row;
                        }
                        self.chosen()
                    }
                    None => Vec::new(),
                }
            }
            Form::Grid => self.albums_browser.songs_at(&self.albums, &self.lib, l.main, pos, tile),
            Form::Artists => {
                let pane = artists_rects(l.main).1;
                if !pane.contains(pos) {
                    return;
                }
                self.artist_in_grid = !self.artist_albums.is_empty();
                self.artist_browser.songs_at(&self.artist_albums, &self.lib, pane_rects(pane).1, pos, tile)
            }
            Form::Settings => Vec::new(),
        };
        self.focus = Focus::Main;
        self.open_song_actions(pos, tracks);
    }

    /// `p`: the menu for the songs the keyboard is on, beside them; in the sidebar, the menu of the playlist it is on.
    pub(super) fn open_actions_by_key(&mut self, l: &Layout) {
        if self.focus == Focus::Sidebar {
            if let Section::Playlist(id) = self.section {
                let index = self.sections().iter().position(|&s| s == self.section).unwrap_or(0);
                let at = l.items.get(index).map_or(l.sidebar.center(), |item| item.right_top() + vec2(-40.0, item.height()));
                self.open_playlist_actions(at, id);
            }
            return;
        }
        let (tracks, at) = match self.form() {
            Form::Table => {
                let rows = songs_rects(l.main).1;
                let row = row_rect(rows, self.songs.cursor.saturating_sub(self.songs.top), SONG_ROW);
                (self.chosen(), pos2(rows.left() + 220.0, row.bottom()))
            }
            Form::Grid => (self.albums_browser.songs_here(&self.albums, &self.lib), self.keyboard_spot(&self.albums_browser, l.main)),
            Form::Artists if self.artist_in_grid => {
                let body = pane_rects(artists_rects(l.main).1).1;
                (self.artist_browser.songs_here(&self.artist_albums, &self.lib), self.keyboard_spot(&self.artist_browser, body))
            }
            _ => return,
        };
        self.open_song_actions(at, tracks);
    }

    /// Where a menu opened from the keyboard on a browser goes: under the track or the cover the cursor is on.
    fn keyboard_spot(&self, browser: &Browser, area: Rect) -> Pos2 {
        match &browser.detail {
            Some(detail) => {
                let rows = detail_rects(area).rows;
                let row = row_rect(rows, detail.list.cursor.saturating_sub(detail.list.top), TRACK_ROW);
                pos2(rows.left() + 160.0, row.bottom())
            }
            None => {
                let cover = Cells::new(area, self.settings.cover).cover(browser.grid.cursor, browser.grid.scroll);
                cover.center()
            }
        }
    }

    /// A press while the menu is open: on a row it does what the row says; anywhere else it closes the menu. Returns
    /// whether the menu was open, and so took the press.
    pub(super) fn actions_press(&mut self, pos: Pos2, screen: Rect) -> bool {
        let Some(menu) = &self.actions else { return false };
        match menu.row_at(screen, pos) {
            Some(row) => self.act(row),
            None if menu.frame(screen).contains(pos) => {}
            None => self.actions = None,
        }
        true
    }

    /// The pointer over a row of the open menu puts its cursor there.
    pub(super) fn hover_actions(&mut self, pos: Option<Pos2>, screen: Rect) {
        if let (Some(menu), Some(pos)) = (&mut self.actions, pos) {
            if let Some(row) = menu.row_at(screen, pos) {
                if row != menu.cursor {
                    menu.cursor = row;
                    menu.armed = false;
                }
            }
        }
    }

    pub(super) fn actions_key(&mut self, key: Key, mv: Option<Move>) {
        let Some(menu) = &mut self.actions else { return };
        match (key, mv) {
            (Key::Enter, _) => {
                let row = menu.cursor;
                self.act(row);
            }
            (Key::Escape, _) | (_, Some(Move::Left)) => self.actions = None,
            (_, Some(Move::Up)) => menu.step(-1),
            (_, Some(Move::Down)) => menu.step(1),
            _ => {}
        }
    }

    /// Does what row `row` of the open menu says.
    fn act(&mut self, row: usize) {
        let Some(menu) = &mut self.actions else { return };
        let Some(&action) = menu.items.get(row) else { return };
        match action {
            Action::Heading(_) => return,
            Action::DeletePlaylist(_) if !menu.armed => {
                menu.armed = true;
                menu.cursor = row;
                return;
            }
            _ => {}
        }
        let tracks = std::mem::take(&mut menu.tracks);
        self.actions = None;
        match action {
            Action::Like(_) => self.toggle_like(&tracks),
            Action::AddTo(id) => self.add_to(id, &tracks),
            Action::AddToNew => self.new_playlist(&tracks),
            Action::Remove(id) => self.remove_from(id, &tracks),
            Action::Drop => self.drop_chosen(),
            Action::Rename(id) => self.start_renaming(id),
            Action::DeletePlaylist(id) => self.delete_playlist(id),
            Action::Heading(_) => {}
        }
    }

    /// What a row of the menu says.
    fn action_label(&self, action: Action, menu: &Actions) -> String {
        let n = menu.tracks.len();
        let name = |id: u64| self.label(Section::Playlist(id)).to_owned();
        match action {
            Action::Like(true) if n == 1 => "Like".to_owned(),
            Action::Like(true) => format!("Like {}", songs(n)),
            Action::Like(false) if n == 1 => "Unlike".to_owned(),
            Action::Like(false) => format!("Unlike {}", songs(n)),
            Action::Heading(text) => text.to_owned(),
            Action::AddTo(id) => name(id),
            Action::AddToNew => "New Playlist…".to_owned(),
            Action::Remove(id) => format!("Remove from {}", name(id)),
            Action::Drop if n == 1 => "Delete from library".to_owned(),
            Action::Drop => format!("Delete {} from library", songs(n)),
            Action::Rename(_) => "Rename".to_owned(),
            Action::DeletePlaylist(_) if menu.armed => "Choose again to delete it".to_owned(),
            Action::DeletePlaylist(_) => "Delete playlist".to_owned(),
        }
    }
}

/// A single song's title, for the status bar.
fn songs_title(lib: &Library, tracks: &[u32]) -> String {
    tracks.first().map_or_else(String::new, |&id| lib.tracks[id as usize].title.clone())
}

/// The open menu, over everything.
pub(super) fn paint(app: &App, p: &Painter, screen: Rect) {
    let Some(menu) = &app.actions else { return };
    let fonts = &app.fonts;
    let frame = menu.frame(screen);
    p.rect_filled(frame.translate(vec2(0.0, 3.0)).expand(2.0), 10.0, Color32::from_black_alpha(26));
    p.rect_filled(frame, 8.0, theme::WELL);
    p.rect_stroke(frame, 8.0, Stroke::new(1.0, theme::LINE), StrokeKind::Inside);
    for (row, &action) in menu.items.iter().enumerate() {
        let rect = Actions::row_rect(frame, row);
        let mid = rect.center().y;
        if let Action::Heading(text) = action {
            draw::text(p, pos2(rect.left() + 10.0, mid + 3.0), Align2::LEFT_CENTER, rect.width() - 20.0, text, &fonts.label, theme::FAINT);
            continue;
        }
        let lit = row == menu.cursor;
        if lit {
            p.rect_filled(rect, 5.0, theme::ACCENT);
        }
        let warn = matches!(action, Action::Drop | Action::DeletePlaylist(_));
        let ink = match (lit, warn) {
            (true, _) => theme::ON_ACCENT,
            (false, true) => theme::WARN,
            (false, false) => theme::TEXT,
        };
        // Playlists to add to sit under their heading.
        let mut left = rect.left() + if matches!(action, Action::AddTo(_) | Action::AddToNew) { 22.0 } else { 10.0 };
        if let Action::Like(like) = action {
            draw::heart(p, pos2(left + 6.0, mid), 11.0, if lit { ink } else { theme::LOVE }, !like);
            left += 22.0;
        }
        draw::text(p, pos2(left, mid), Align2::LEFT_CENTER, rect.right() - 10.0 - left, &app.action_label(action, menu), &fonts.body, ink);
    }
}
