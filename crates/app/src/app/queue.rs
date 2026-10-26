//! The queue: the songs that play one after another. It keeps the list they came from, in that list's own order, and
//! the order they play in, which is the list's own or one that shuffle dealt from it. Enter plays from the list on
//! screen; the shuffle button (and `x`) steps through what shuffle shuffles, deals afresh what follows the song
//! playing, and starts the list on screen when nothing is playing.

use super::*;
use crate::history;
use crate::shuffle::{self, Mode};

#[derive(Default)]
pub(super) struct Queue {
    /// Which list the songs came from, as shuffle keeps its rounds: "library?jazz||||", "playlist/3?||||", "album/…".
    list: String,
    /// The list's songs in the order it shows them: what shuffle deals from, and what turning it off goes back to.
    source: Vec<u32>,
    /// The order they play in.
    order: Vec<u32>,
    at: usize,
    /// When the song at `at` started, in Unix milliseconds.
    started: i64,
}

/// A queue known by its songs' files, to be found again in another index.
pub(super) struct Carried {
    list: String,
    source: Vec<String>,
    order: Vec<String>,
    at: usize,
    started: i64,
}

impl Queue {
    fn new(list: String, source: Vec<u32>) -> Self {
        Queue { list, source, ..Queue::default() }
    }

    pub(super) fn carry(&self, lib: &Library) -> Carried {
        let paths = |songs: &[u32]| songs.iter().map(|&id| lib.tracks[id as usize].path.clone()).collect();
        Carried { list: self.list.clone(), source: paths(&self.source), order: paths(&self.order), at: self.at, started: self.started }
    }
}

impl Carried {
    /// The queue in `lib`: songs it does not have drop out, and the place moves with them.
    pub(super) fn find(self, lib: &Library) -> Queue {
        let place: HashMap<&str, u32> = lib.tracks.iter().enumerate().map(|(i, track)| (track.path.as_str(), i as u32)).collect();
        let source = self.source.iter().filter_map(|path| place.get(path.as_str()).copied()).collect();
        let (mut order, mut at) = (Vec::with_capacity(self.order.len()), 0);
        for (i, path) in self.order.iter().enumerate() {
            if let Some(&id) = place.get(path.as_str()) {
                at += usize::from(i < self.at);
                order.push(id);
            }
        }
        Queue { list: self.list, source, order, at, started: self.started }
    }
}

impl App {
    /// The list on screen, which Enter plays from and the shuffle button starts: the name shuffle keeps its round by,
    /// and the songs in the order shown, but for those the collection lacks.
    fn listed(&self) -> Option<(String, Vec<u32>)> {
        let lib = &self.lib;
        let opened = match self.form() {
            Form::Grid => self.albums_browser.detail.as_ref(),
            Form::Artists => self.artist_browser.detail.as_ref(),
            Form::Table => None,
            Form::Settings => return None,
        };
        let f = &self.filters;
        let narrowed = format!(
            "?{}|{}|{}|{}|{}",
            self.search,
            f.genre.as_deref().unwrap_or(""),
            f.kind.as_deref().unwrap_or(""),
            f.sound.map_or("", |sound| sound.label()),
            f.decade.map_or(String::new(), |decade| decade.to_string()),
        );
        let (name, songs) = if let Some(detail) = opened {
            let album = &lib.albums[detail.album as usize];
            let release = detail.tracks.first().map_or("", |&id| lib.tracks[id as usize].mb_release.as_str());
            (format!("album/{}/{}/{release}", lib.artists[album.artist as usize], album.name), detail.tracks.clone())
        } else if self.form() == Form::Artists && self.artist_in_grid {
            let artist = self.result.artists.get(self.artist_list.cursor)?;
            let view = lib.view(&Query { artist: Some(artist.id), ..self.query() }, Column::Album, false);
            (format!("artist/{}{narrowed}", lib.artists[artist.id as usize]), view.rows)
        } else {
            // Albums, Artists and Songs all list the library, as far as the search and the filters let them.
            let name = match self.section {
                Section::Albums | Section::Artists | Section::Songs => "library".to_owned(),
                Section::Liked => "liked".to_owned(),
                Section::Playlist(id) => format!("playlist/{id}"),
                section => self.label(section).to_owned(),
            };
            (name + &narrowed, self.result.rows.clone())
        };
        let part = if self.rest_shown { "unfinished/" } else { "" };
        let songs = songs.into_iter().filter(|&id| !lib.tracks[id as usize].missing).collect();
        Some((format!("{part}{name}"), songs))
    }

    /// Plays a song of what is on screen. The songs listed after it follow; with shuffle on, the list is dealt from it.
    pub(super) fn play(&mut self, track: u32) {
        if self.lib.tracks[track as usize].missing {
            self.notice = Some(("This song is not in the collection; it is listed to show the album is missing it".to_owned(), Instant::now()));
            return;
        }
        let (list, songs) = self
            .listed()
            .filter(|(_, songs)| songs.contains(&track))
            .unwrap_or_else(|| (format!("song/{}", self.lib.tracks[track as usize].path), vec![track]));
        self.queue = Queue::new(list, songs);
        self.playing_rest = self.rest_shown;
        let over = self.arrange(Some(track), 0, history::now());
        self.play_at(self.queue.at);
        if over {
            self.notice = Some(("Everything here has played this round, so the shuffle begins a new one".to_owned(), Instant::now()));
        }
    }

    /// Steps what shuffle shuffles: songs, albums, artists, then off. What is playing plays on, and what follows it is
    /// dealt afresh; with nothing playing, the list on screen starts.
    pub(super) fn step_shuffle(&mut self, steps: isize) {
        let was = self.settings.shuffle;
        let mode = was.step(steps);
        self.settings.shuffle = mode;
        self.settings.save();
        let over = if let Some(playing) = self.playing {
            // The songs before the place are kept for going back to, when they were dealt; a list played in its own
            // order has its earlier songs still to come.
            let kept = if was == Mode::Off { 0 } else { self.queue.at };
            let over = self.arrange(Some(playing), kept, self.queue.started);
            self.fetch_next();
            over
        } else if mode != Mode::Off
            && let Some((list, songs)) = self.listed().filter(|(_, songs)| !songs.is_empty())
        {
            self.queue = Queue::new(list, songs);
            self.playing_rest = self.rest_shown;
            let over = self.arrange(None, 0, history::now());
            self.play_at(0);
            over
        } else {
            false
        };
        let said = match mode {
            Mode::Off => "Shuffle off".to_owned(),
            mode => format!("Shuffling {}", mode.word()),
        };
        let said = if over { said + "; everything here has played this round, so a new one begins" } else { said };
        self.notice = Some((said, Instant::now()));
    }

    /// Puts the queue in the order the shuffle mode asks for, from `first` on, keeping the `kept` songs before the place
    /// for going back to; with shuffle off, in the list's own order. A round that has to begin anew begins at `now`:
    /// when the song playing started, or now. Returns whether one did.
    fn arrange(&mut self, first: Option<u32>, kept: usize, now: i64) -> bool {
        let mode = self.settings.shuffle;
        let queue = &mut self.queue;
        if mode == Mode::Off {
            queue.order = queue.source.clone();
            queue.at = first.and_then(|first| queue.order.iter().position(|&id| id == first)).unwrap_or(0);
            return false;
        }
        // The queue's songs may be in the index that is not on screen.
        let lib = match &self.shelved {
            Some(other) if self.playing_rest != self.rest_shown => &other.lib,
            _ => &self.lib,
        };
        let list = format!("{}:{}", mode.word(), queue.list);
        let began = self.rounds.began(&list, now);
        let history = &self.history;
        let heard = |id: u32| history.last(&lib.tracks[id as usize]).is_some_and(|at| at >= began);
        let mut dealt = shuffle::deal(lib, mode, first, &queue.source, heard, &mut self.dice);
        let mut over = false;
        if dealt.fresh == 0 {
            let again = shuffle::deal(lib, mode, first, &queue.source, |_| false, &mut self.dice);
            if again.fresh > 0 {
                self.rounds.renew(&list, now);
                (dealt, over) = (again, true);
            }
        }
        if let Err(why) = self.rounds.save() {
            self.notice = Some((why, Instant::now()));
        }
        let mut order = queue.order[..kept.min(queue.order.len())].to_vec();
        queue.at = order.len();
        order.extend(dealt.order);
        queue.order = order;
        over
    }

    /// Plays the queue's song `at`, and has the one after it fetched meanwhile.
    fn play_at(&mut self, at: usize) {
        let Some(&track) = self.queue.order.get(at) else { return };
        let lib = match &self.shelved {
            Some(other) if self.playing_rest != self.rest_shown => &other.lib,
            _ => &self.lib,
        };
        let track_of = &lib.tracks[track as usize];
        let now = history::now();
        let complaint = self.history.played(track_of, now);
        let request = Request { path: track_of.path.clone(), size: track_of.size };
        self.queue.at = at;
        self.queue.started = now;
        self.playing = Some(track);
        self.paused = false;
        self.seek_target = None;
        self.serial = self.player.play(request);
        self.fetch_next();
        if let Some(complaint) = complaint {
            self.notice = Some((complaint, Instant::now()));
        }
    }

    /// Has the song after the one playing fetched, so that it starts without a wait.
    fn fetch_next(&mut self) {
        let lib = match &self.shelved {
            Some(other) if self.playing_rest != self.rest_shown => &other.lib,
            _ => &self.lib,
        };
        if let Some(&next) = self.queue.order.get(self.queue.at + 1) {
            let track = &lib.tracks[next as usize];
            self.player.prefetch(Request { path: track.path.clone(), size: track.size });
        }
    }

    /// The next song of the queue (`by` 1) or the one before (-1).
    pub(super) fn skip(&mut self, by: isize) {
        if self.playing.is_some() {
            self.play_at(self.queue.at.saturating_add_signed(by));
        }
    }

    /// Reads where the player stands; a song that has ended hands over to the next.
    pub(super) fn follow_player(&mut self) {
        self.status = self.player.status();
        if self.status.serial == self.serial && self.status.state == player::State::Ended && self.playing.is_some() {
            if self.queue.at + 1 < self.queue.order.len() {
                self.play_at(self.queue.at + 1);
            } else {
                self.playing = None;
            }
        }
    }

    pub(super) fn play_song_cursor(&mut self) {
        if let Some(&track) = self.result.rows.get(self.songs.cursor) {
            self.play(track);
        }
    }
}
