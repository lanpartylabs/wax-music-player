//! Liked songs and playlists: kept on this machine beside the list of
//! dropped songs, in `playlists.json`. A song is remembered by its file and by
//! the MusicBrainz recording it is, so that it is found again when its file
//! moves, as it will when the clean library is built; its title, artist and
//! album are kept too, for whoever reads the file.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use wax_core::Library;
use wax_core::index::Track;

/// One song of a playlist, or one liked.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Entry {
    /// The file, as the index names it.
    #[serde(rename = "p")]
    pub path: String,
    /// The MusicBrainz recording and release, where the index knows them.
    #[serde(rename = "T", default, skip_serializing_if = "String::is_empty")]
    pub recording: String,
    #[serde(rename = "R", default, skip_serializing_if = "String::is_empty")]
    pub release: String,
    #[serde(rename = "t", default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(rename = "a", default, skip_serializing_if = "String::is_empty")]
    pub artist: String,
    #[serde(rename = "l", default, skip_serializing_if = "String::is_empty")]
    pub album: String,
    /// Unix seconds at which it was added.
    pub added: i64,
}

impl Entry {
    pub fn of(track: &Track) -> Self {
        Entry {
            path: track.path.clone(),
            recording: track.mb_recording.clone(),
            release: track.mb_release.clone(),
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            added: now(),
        }
    }

    /// Whether it is this track: the same file, or the same recording.
    fn is(&self, track: &Track) -> bool {
        self.path == track.path || (!self.recording.is_empty() && self.recording == track.mb_recording)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Playlist {
    pub id: u64,
    pub name: String,
    pub created: i64,
    #[serde(default)]
    pub entries: Vec<Entry>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Store {
    /// In the order they were liked.
    #[serde(default)]
    pub liked: Vec<Entry>,
    #[serde(default)]
    pub playlists: Vec<Playlist>,
}

/// Where a library's tracks are, by file and by recording, for finding a remembered song in it.
#[derive(Default)]
pub struct Places {
    by_path: HashMap<String, u32>,
    /// Every copy of a recording, the best sounding first.
    by_recording: HashMap<String, Vec<u32>>,
}

impl Places {
    pub fn new(lib: &Library) -> Self {
        let mut places = Places::default();
        for (i, track) in lib.tracks.iter().enumerate().filter(|(_, track)| !track.missing) {
            places.by_path.insert(track.path.clone(), i as u32);
            if !track.mb_recording.is_empty() {
                places.by_recording.entry(track.mb_recording.clone()).or_default().push(i as u32);
            }
        }
        for copies in places.by_recording.values_mut() {
            copies.sort_by_key(|&i| std::cmp::Reverse(lib.tracks[i as usize].quality()));
        }
        places
    }

    /// The track a remembered song is in this library: its own file; else the same recording on the same release; else
    /// the best copy of the recording anywhere. None when the library has it nowhere.
    pub fn find(&self, entry: &Entry, lib: &Library) -> Option<u32> {
        if let Some(&id) = self.by_path.get(&entry.path) {
            return Some(id);
        }
        let copies = self.by_recording.get(&entry.recording)?;
        copies.iter().copied().find(|&id| lib.tracks[id as usize].mb_release == entry.release).or_else(|| copies.first().copied())
    }

    /// The tracks of a list of songs in that order, each once; songs the library does not have are passed over.
    pub fn resolve(&self, entries: &[Entry], lib: &Library) -> Vec<u32> {
        let mut seen = std::collections::HashSet::new();
        entries.iter().filter_map(|entry| self.find(entry, lib)).filter(|&id| seen.insert(id)).collect()
    }
}

impl Store {
    fn file() -> PathBuf {
        crate::state::file("playlists.json")
    }

    /// What was saved. A file that cannot be read is moved aside, never written over, and said so.
    pub fn load() -> (Self, Option<String>) {
        let file = Self::file();
        let Ok(bytes) = std::fs::read(&file) else { return (Store::default(), None) };
        match serde_json::from_slice(&bytes) {
            Ok(store) => (store, None),
            Err(e) => {
                let aside = file.with_extension(format!("json.unreadable-{}", now()));
                let moved = std::fs::rename(&file, &aside);
                let kept = if moved.is_ok() { format!("kept as {}", aside.display()) } else { "left where it is".to_owned() };
                (Store::default(), Some(format!("the playlists could not be read ({e}); the file is {kept}")))
            }
        }
    }

    /// Writes the whole store, through a temporary file so that a crash never leaves half of one; the version before is
    /// kept beside it as `playlists.json.bak`.
    pub fn save(&self) -> Result<(), String> {
        let file = Self::file();
        let written = (|| -> std::io::Result<()> {
            if let Some(folder) = file.parent() {
                std::fs::create_dir_all(folder)?;
            }
            let tmp = file.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default())?;
            if file.exists() {
                std::fs::copy(&file, file.with_extension("json.bak"))?;
            }
            std::fs::rename(&tmp, &file)
        })();
        written.map_err(|e| format!("the playlists could not be saved to {}: {e}", file.display()))
    }

    pub fn is_liked(&self, track: &Track) -> bool {
        self.liked.iter().any(|entry| entry.is(track))
    }

    /// Likes the songs, or, when every one of them is liked already, unlikes them all. Returns whether they are liked now.
    pub fn toggle_like(&mut self, tracks: &[&Track]) -> bool {
        if tracks.iter().all(|track| self.is_liked(track)) {
            self.liked.retain(|entry| !tracks.iter().any(|track| entry.is(track)));
            false
        } else {
            for track in tracks {
                if !self.is_liked(track) {
                    self.liked.push(Entry::of(track));
                }
            }
            true
        }
    }

    /// Likes those of the songs not liked yet. Returns how many that was.
    pub fn like(&mut self, tracks: &[&Track]) -> usize {
        let fresh: Vec<Entry> = tracks.iter().filter(|track| !self.is_liked(track)).map(|track| Entry::of(track)).collect();
        let n = fresh.len();
        self.liked.extend(fresh);
        n
    }

    /// Unlikes the songs. Returns how many liked songs that took out.
    pub fn unlike(&mut self, tracks: &[&Track]) -> usize {
        let before = self.liked.len();
        self.liked.retain(|entry| !tracks.iter().any(|track| entry.is(track)));
        before - self.liked.len()
    }

    pub fn playlist(&self, id: u64) -> Option<&Playlist> {
        self.playlists.iter().find(|playlist| playlist.id == id)
    }

    fn playlist_mut(&mut self, id: u64) -> Option<&mut Playlist> {
        self.playlists.iter_mut().find(|playlist| playlist.id == id)
    }

    /// Makes an empty playlist and returns its id. With no name it is "New Playlist", numbered if that is taken.
    pub fn create(&mut self, name: &str) -> u64 {
        let name = if name.trim().is_empty() { self.free_name("New Playlist") } else { name.trim().to_owned() };
        let id = self.playlists.iter().map(|playlist| playlist.id).max().unwrap_or(0) + 1;
        self.playlists.push(Playlist { id, name, created: now(), entries: Vec::new() });
        id
    }

    /// `base`, or `base 2`, `base 3`… whichever no playlist has.
    fn free_name(&self, base: &str) -> String {
        (1..).map(|n| if n == 1 { base.to_owned() } else { format!("{base} {n}") }).find(|name| self.playlists.iter().all(|p| &p.name != name)).unwrap()
    }

    /// Renames a playlist; a blank name leaves it as it was.
    pub fn rename(&mut self, id: u64, name: &str) {
        if let (Some(playlist), false) = (self.playlist_mut(id), name.trim().is_empty()) {
            playlist.name = name.trim().to_owned();
        }
    }

    pub fn delete(&mut self, id: u64) -> Option<Playlist> {
        let at = self.playlists.iter().position(|playlist| playlist.id == id)?;
        Some(self.playlists.remove(at))
    }

    /// Adds the songs a playlist does not have yet, at its end. Returns how many were added.
    pub fn add(&mut self, id: u64, tracks: &[&Track]) -> usize {
        let Some(playlist) = self.playlist_mut(id) else { return 0 };
        let mut added = 0;
        for track in tracks {
            if !playlist.entries.iter().any(|entry| entry.is(track)) {
                playlist.entries.push(Entry::of(track));
                added += 1;
            }
        }
        added
    }

    /// Takes the songs out of a playlist. Returns how many were taken out.
    pub fn remove(&mut self, id: u64, tracks: &[&Track]) -> usize {
        let Some(playlist) = self.playlist_mut(id) else { return 0 };
        let before = playlist.entries.len();
        playlist.entries.retain(|entry| !tracks.iter().any(|track| entry.is(track)));
        before - playlist.entries.len()
    }
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(path: &str, recording: &str, release: &str, bitrate: u32) -> Track {
        Track { path: path.into(), mb_recording: recording.into(), mb_release: release.into(), title: path.into(), bitrate, ..Track::default() }
    }

    #[test]
    fn a_liked_song_is_found_again_when_its_file_moves_and_likes_toggle_together() {
        let lib = Library::new(vec![song("old/a.mp3", "rec-a", "rel-1", 192), song("comp/a.mp3", "rec-a", "rel-2", 320), song("old/b.mp3", "rec-b", "rel-1", 192)]);
        let mut store = Store::default();
        let (a, b) = (&lib.tracks[0], &lib.tracks[2]);
        assert!(store.toggle_like(&[a]));
        assert!(store.is_liked(&lib.tracks[1]), "the same recording on another record is liked too");
        assert!(store.toggle_like(&[a, b]), "one of them was not liked: both are now");
        assert!(!store.toggle_like(&[a, b]), "both were: both are not");
        assert!(store.liked.is_empty());

        store.toggle_like(&[a]);
        let places = Places::new(&lib);
        assert_eq!(places.resolve(&store.liked, &lib), [0]);
        // The clean library: the file is elsewhere now, and the compilation is gone.
        let moved = Library::new(vec![song("clean/A/a.flac", "rec-a", "rel-1", 900), song("clean/A/b.flac", "rec-b", "rel-1", 900)]);
        let places = Places::new(&moved);
        assert_eq!(places.resolve(&store.liked, &moved), [0], "found by its recording on its release");
        let gone = Library::new(vec![song("clean/A/b.flac", "rec-b", "rel-1", 900)]);
        assert!(Places::new(&gone).resolve(&store.liked, &gone).is_empty(), "nowhere: passed over, not forgotten");
        assert_eq!(store.liked.len(), 1);
    }

    #[test]
    fn playlists_are_made_named_added_to_and_emptied() {
        let lib = Library::new(vec![song("a.mp3", "rec-a", "rel", 192), song("b.mp3", "", "", 192)]);
        let (a, b) = (&lib.tracks[0], &lib.tracks[1]);
        let mut store = Store::default();
        let first = store.create("");
        let second = store.create("  ");
        assert_eq!((store.playlist(first).unwrap().name.as_str(), store.playlist(second).unwrap().name.as_str()), ("New Playlist", "New Playlist 2"));
        store.rename(first, "Road trip ");
        store.rename(second, "");
        assert_eq!((store.playlist(first).unwrap().name.as_str(), store.playlist(second).unwrap().name.as_str()), ("Road trip", "New Playlist 2"));
        assert_eq!(store.add(first, &[a, b]), 2);
        assert_eq!(store.add(first, &[b]), 0, "a song is in a playlist once");
        assert_eq!(Places::new(&lib).resolve(&store.playlist(first).unwrap().entries, &lib), [0, 1]);
        assert_eq!(store.remove(first, &[a]), 1);
        assert_eq!(store.playlist(first).unwrap().entries.len(), 1);
        assert!(store.delete(second).is_some());
        assert_eq!(store.create(""), first + 1, "ids are not used twice while the highest stands");
        let saved = serde_json::to_string(&store).unwrap();
        assert_eq!(serde_json::from_str::<Store>(&saved).unwrap(), store);
    }
}
