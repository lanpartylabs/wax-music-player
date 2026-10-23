//! The music folder: choosing it, scanning it in the background, and putting
//! what the scan found on screen. The library on screen stays usable while a
//! scan runs; when it is done and found a song added, changed or gone, the
//! new list takes its place, and what is playing plays on.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use wax_scan::{Options, Progress, Summary};

use super::*;
use crate::format::count;

/// Files read at once while scanning; a local disk is not helped by more.
const SCAN_JOBS: usize = 8;

/// A scan under way.
pub(super) struct Scan {
    folder: PathBuf,
    progress: Arc<Mutex<Progress>>,
    done: Arc<Mutex<Option<Result<Summary, String>>>>,
    /// Asked for, rather than the scan at start, which says nothing when it finds nothing new.
    asked: bool,
    /// The music folder was changed while this ran: what it found is of no use, and another scan follows.
    again: bool,
}

impl Scan {
    pub(super) fn progress(&self) -> Progress {
        *self.progress.lock().unwrap()
    }

    pub(super) fn folder(&self) -> &Path {
        &self.folder
    }
}

/// Where wax keeps what it makes: `$XDG_DATA_HOME/wax`.
pub fn data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|home| home.join(".local/share")))
        .unwrap_or_default()
        .join("wax")
}

/// The list made of the music folder; its covers are in `art` beside it. Kept apart from a catalogue a librarian
/// wrote, so that neither overwrites the other.
pub fn folder_index() -> PathBuf {
    data_dir().join("folder/library.jsonl.gz")
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|home| !home.is_empty()).map(PathBuf::from)
}

/// The folder a name typed in means: a leading `~` is the home folder.
pub(super) fn expand(name: &str) -> PathBuf {
    let name = name.trim();
    match (name.strip_prefix('~'), home()) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with('/') => home.join(rest.trim_start_matches('/')),
        _ => PathBuf::from(name),
    }
}

/// A folder as it is shown: the home folder as `~`.
pub(super) fn shown(folder: &Path) -> String {
    match home().and_then(|home| folder.strip_prefix(home).ok().map(Path::to_owned)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~/{}", rest.display()),
        None => folder.display().to_string(),
    }
}

/// Where music usually is on this machine: the desktop's music folder (XDG_MUSIC_DIR), else ~/Music.
fn usual_music_folder() -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| home().map(|home| home.join(".config")));
    let named = config.and_then(|config| std::fs::read_to_string(config.join("user-dirs.dirs")).ok()).and_then(|dirs| {
        dirs.lines().find_map(|line| {
            let value = line.trim().strip_prefix("XDG_MUSIC_DIR=")?.trim_matches('"');
            Some(match (value.strip_prefix("$HOME"), home()) {
                (Some(rest), Some(home)) => home.join(rest.trim_start_matches('/')),
                _ => PathBuf::from(value),
            })
        })
    });
    named.filter(|folder| folder.is_absolute()).or_else(|| home().map(|home| home.join("Music"))).unwrap_or_default()
}

impl App {
    /// What an empty grid or list says: `otherwise` unless the library itself is empty.
    pub(super) fn empty_words(&self, otherwise: &'static str) -> &'static str {
        match (self.lib.is_empty(), &self.scanning) {
            (false, _) => otherwise,
            (true, Some(_)) => "Looking for your music…",
            (true, None) if self.settings.music.is_empty() => "No music yet. Choose your music folder in Settings: Ctrl-,",
            (true, None) => "No music found in the music folder",
        }
    }

    /// The first start: the settings page, with the music folder's field open on where music usually is.
    pub(super) fn welcome(&mut self) {
        self.go(Section::Settings);
        self.focus = Focus::Main;
        self.setting = 0;
        self.start_naming_folder();
        self.notice = Some(("Welcome to wax. Where is your music? Type its folder and press Enter".to_owned(), Instant::now()));
    }

    /// Opens the music folder's field, holding the folder set, or where music usually is; it is selected, so that
    /// typing replaces it.
    pub(super) fn start_naming_folder(&mut self) {
        let folder = if self.settings.music.is_empty() { usual_music_folder() } else { expand(&self.settings.music) };
        self.actions = None;
        self.searching = false;
        self.renaming = Some(actions::Naming { what: actions::Named::Folder, name: shown(&folder), selected: true });
    }

    /// Enter in the music folder's field: a folder that is there becomes the music folder, and is scanned.
    pub(super) fn choose_music_folder(&mut self, name: String) {
        let folder = expand(&name);
        if name.trim().is_empty() {
            return;
        }
        if !folder.is_dir() {
            self.notice = Some((format!("There is no folder at {}", shown(&folder)), Instant::now()));
            self.renaming = Some(actions::Naming { what: actions::Named::Folder, name, selected: false });
            return;
        }
        let folder = std::fs::canonicalize(&folder).unwrap_or(folder);
        self.settings.music = folder.to_string_lossy().into_owned();
        self.settings.save();
        match &mut self.scanning {
            Some(scan) => scan.again = true,
            None => self.scan(true),
        }
    }

    /// Scans the music folder in the background. `asked`: by the person, who is then told what came of it.
    pub(super) fn scan(&mut self, asked: bool) {
        if self.scanning.is_some() || self.settings.music.is_empty() {
            return;
        }
        let folder = expand(&self.settings.music);
        let out = folder_index();
        let options = Options { root: folder.clone(), include: Vec::new(), art: Some(out.with_file_name("art")), out, audio_key: false, reuse: None, jobs: SCAN_JOBS };
        let (progress, done) = (Arc::new(Mutex::new(Progress::default())), Arc::new(Mutex::new(None)));
        let (shared, finished, ctx) = (progress.clone(), done.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let result = wax_scan::scan(&options, &|now| {
                *shared.lock().unwrap() = now;
                ctx.request_repaint();
            });
            *finished.lock().unwrap() = Some(result.map_err(|e| e.to_string()));
            ctx.request_repaint();
        });
        self.scanning = Some(Scan { folder, progress, done, asked, again: false });
    }

    /// Every frame: when the scan is done, what it found goes on screen.
    pub(super) fn follow_scan(&mut self) {
        let Some(result) = self.scanning.as_ref().and_then(|scan| scan.done.lock().unwrap().take()) else { return };
        let Some(scan) = self.scanning.take() else { return };
        if scan.again {
            return self.scan(true);
        }
        let folder = shown(&scan.folder);
        let summary = match result {
            Ok(summary) => summary,
            Err(why) => {
                self.notice = Some((format!("Cannot scan {folder}: {why}"), Instant::now()));
                return;
            }
        };
        let index = folder_index();
        let fresh = summary.changed() || self.source.index != index;
        if fresh {
            if let Err(why) = self.take_library(&index) {
                self.notice = Some((why, Instant::now()));
                return;
            }
        }
        if scan.asked || fresh {
            let unread = match summary.unreadable {
                0 => String::new(),
                n => format!(", {} without tags that could be read", count(n)),
            };
            let said = match summary.tracks {
                0 => format!("No music found in {folder}"),
                1 => format!("Scanned {folder}: 1 song{unread}"),
                n => format!("Scanned {folder}: {} songs{unread}", count(n)),
            };
            self.notice = Some((said, Instant::now()));
        }
    }

    /// Puts the index at `index` on screen in place of the library. The views start afresh; what is playing plays on
    /// and stays where it is in its queue, found again by its path.
    pub(super) fn take_library(&mut self, index: &Path) -> Result<(), String> {
        let (lib, source) = open_index(index, None, &self.gone)?;
        let path_of = |id: &u32| self.lib.tracks[*id as usize].path.clone();
        let (playing, queue): (Option<String>, Vec<String>) = if self.playing_rest == self.rest_shown {
            (self.playing.as_ref().map(path_of), self.queue.iter().map(path_of).collect())
        } else {
            (None, Vec::new())
        };

        self.info = album_info(&lib, source.ready);
        self.art = Art::new(source.art.clone(), &self.ctx);
        self.lib = lib;
        self.source = source;
        self.shelved = None;
        self.rest.clear();
        self.rest_shown = false;
        self.playing_rest = false;
        if matches!(self.section, Section::Rest(_)) {
            self.section = Section::Albums;
        }
        self.player.set_origin(self.origin());

        let place: HashMap<&str, u32> = self.lib.tracks.iter().enumerate().map(|(i, track)| (track.path.as_str(), i as u32)).collect();
        self.queue = queue.iter().filter_map(|path| place.get(path.as_str()).copied()).collect();
        self.playing = playing.and_then(|path| place.get(path.as_str()).copied());
        self.queue_at = self.playing.and_then(|id| self.queue.iter().position(|&queued| queued == id)).unwrap_or(0);

        (self.albums_browser, self.artist_browser) = (Browser::default(), Browser::default());
        (self.artist_list, self.songs) = (List::default(), List::default());
        self.artist_in_grid = false;
        self.picked.clear();
        self.carry = None;
        self.actions = None;
        if self.settings.developer {
            self.load_rest();
        }
        self.library_changed();
        self.search_changed();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_is_typed_and_shown_with_the_home_folder_as_a_tilde() {
        let Some(home) = home() else { return };
        assert_eq!(expand("~/Music"), home.join("Music"));
        assert_eq!(expand(" ~ "), home);
        assert_eq!(expand("/srv/music"), PathBuf::from("/srv/music"));
        assert_eq!(expand("~other/Music"), PathBuf::from("~other/Music"), "another account's home is not guessed at");
        assert_eq!(shown(&home.join("Music/Old")), "~/Music/Old");
        assert_eq!(shown(&home), "~");
        assert_eq!(shown(Path::new("/srv/music")), "/srv/music");
    }
}
