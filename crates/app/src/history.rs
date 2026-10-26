//! What has been played, and when: a line for each song as it starts, added to `played.jsonl` in wax's state folder.
//! Shuffle deals each list from it, so that a song heard yesterday waits until the rest of its list has had a turn; a
//! radio will want it too.
//!
//! A song is known by its file and by the MusicBrainz recording it is, as a liked one is, so that what was played is
//! still known once the clean library moves the files.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use wax_core::index::Track;

/// One song starting.
#[derive(Serialize, Deserialize)]
struct Line {
    /// Unix milliseconds.
    at: i64,
    #[serde(rename = "p")]
    path: String,
    #[serde(rename = "T", default, skip_serializing_if = "String::is_empty")]
    recording: String,
}

pub struct History {
    file: PathBuf,
    /// When each file, and each recording, last started playing.
    by_path: HashMap<String, i64>,
    by_recording: HashMap<String, i64>,
    /// Whether it was said already that the file cannot be written; it is said once a run.
    complained: bool,
}

/// Unix milliseconds now.
pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as i64)
}

impl History {
    pub fn load() -> Self {
        Self::at(crate::state::file("played.jsonl"))
    }

    /// The history kept in `file`; empty when there is none. A line that cannot be read, as the last one may be after
    /// a crash, is passed over.
    pub fn at(file: PathBuf) -> Self {
        let mut history = History { file, by_path: HashMap::new(), by_recording: HashMap::new(), complained: false };
        let text = std::fs::read_to_string(&history.file).unwrap_or_default();
        for line in text.lines().filter_map(|line| serde_json::from_str::<Line>(line).ok()) {
            history.note(&line);
        }
        history
    }

    fn note(&mut self, line: &Line) {
        let slot = self.by_path.entry(line.path.clone()).or_insert(line.at);
        *slot = (*slot).max(line.at);
        if !line.recording.is_empty() {
            let slot = self.by_recording.entry(line.recording.clone()).or_insert(line.at);
            *slot = (*slot).max(line.at);
        }
    }

    /// Notes that a track started playing at `at`, here and in the file. When the file cannot be written the song is
    /// remembered for this run all the same, and the first time that happens says why.
    pub fn played(&mut self, track: &Track, at: i64) -> Option<String> {
        let line = Line { at, path: track.path.clone(), recording: track.mb_recording.clone() };
        self.note(&line);
        let written = self.file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| {
            let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&self.file)?;
            writeln!(file, "{}", serde_json::to_string(&line).unwrap_or_default())
        });
        match written {
            Err(e) if !self.complained => {
                self.complained = true;
                Some(format!("what is played cannot be written down in {}: {e}", self.file.display()))
            }
            _ => None,
        }
    }

    /// When the track last started playing, if it ever did: its own file, or another copy of the same recording.
    pub fn last(&self, track: &Track) -> Option<i64> {
        let by_path = self.by_path.get(&track.path).copied();
        let by_recording = if track.mb_recording.is_empty() { None } else { self.by_recording.get(&track.mb_recording).copied() };
        by_path.max(by_recording)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(path: &str, recording: &str) -> Track {
        Track { path: path.to_owned(), mb_recording: recording.to_owned(), ..Track::default() }
    }

    #[test]
    fn what_was_played_is_known_again_next_run_by_its_file_or_its_recording() {
        let folder = std::env::temp_dir().join(format!("wax-history-test-{}", std::process::id()));
        let file = folder.join("played.jsonl");
        let _ = std::fs::remove_dir_all(&folder);

        let mut history = History::at(file.clone());
        assert_eq!(history.played(&song("a.flac", "rec-a"), 100), None);
        assert_eq!(history.played(&song("b.flac", ""), 200), None);
        assert_eq!(history.played(&song("a.flac", "rec-a"), 300), None);
        // A crash in the middle of a line leaves half of one.
        std::fs::OpenOptions::new().append(true).open(&file).unwrap().write_all(b"{\"at\":4").unwrap();

        let again = History::at(file);
        assert_eq!(again.last(&song("a.flac", "rec-a")), Some(300), "the latest time it started");
        assert_eq!(again.last(&song("b.flac", "")), Some(200));
        assert_eq!(again.last(&song("Clean/A.flac", "rec-a")), Some(300), "moved, and found by its recording");
        assert_eq!(again.last(&song("c.flac", "")), None);
        let _ = std::fs::remove_dir_all(&folder);
    }
}
