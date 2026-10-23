//! On-disk library index: one gzip-compressed JSON-lines file.
//!
//! Line 1 is a [`Header`], every following line is one [`Track`]. The scanner
//! or the librarian writes it; the player reads it once and keeps everything
//! in memory, so browsing never touches the disk or the network.

use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::Path;

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};

pub const INDEX_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct Header {
    pub v: u32,
    /// Unix seconds at which the scan finished.
    pub generated: i64,
    pub count: u32,
    /// Whether the scan extracted artwork (tracks then carry [`Track::art`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub art: bool,
    /// Written by the librarian for its ready catalogue: every album in it is matched with certainty, and one that lists
    /// no missing track is whole.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ready: bool,
    /// The folder the tracks' paths are relative to, where the scan read it on this machine; the player plays the
    /// files from there. Empty in an index made elsewhere, whose files the player fetches from a server.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub root: String,
}

/// One audio file. Field names are single letters on disk to keep the index small.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct Track {
    /// Path relative to the library root, `/`-separated.
    #[serde(rename = "p")]
    pub path: String,
    #[serde(rename = "s")]
    pub size: u64,
    #[serde(rename = "m")]
    pub mtime: i64,
    #[serde(rename = "d", default, skip_serializing_if = "is_zero")]
    pub dur_ms: u32,
    #[serde(rename = "t", default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(rename = "a", default, skip_serializing_if = "String::is_empty")]
    pub artist: String,
    #[serde(rename = "A", default, skip_serializing_if = "String::is_empty")]
    pub album_artist: String,
    #[serde(rename = "l", default, skip_serializing_if = "String::is_empty")]
    pub album: String,
    #[serde(rename = "g", default, skip_serializing_if = "String::is_empty")]
    pub genre: String,
    /// The subgenre under the genre, where the catalogue gives one.
    #[serde(rename = "j", default, skip_serializing_if = "String::is_empty")]
    pub subgenre: String,
    #[serde(rename = "y", default, skip_serializing_if = "is_zero")]
    pub year: u32,
    /// The day the release first came out, where the librarian knows it: "2003-09-23", or "2003-09" or "2003" when the
    /// reference gives no more.
    #[serde(rename = "Y", default, skip_serializing_if = "String::is_empty")]
    pub date: String,
    #[serde(rename = "n", default, skip_serializing_if = "is_zero")]
    pub track: u32,
    #[serde(rename = "c", default, skip_serializing_if = "is_zero")]
    pub disc: u32,
    /// What the disc is, where the librarian names it: the format of a single shown as one release with its other
    /// formats, "CD · SKINT 100CDP", a disc each.
    #[serde(rename = "D", default, skip_serializing_if = "String::is_empty")]
    pub disc_name: String,
    /// Average bitrate in kbit/s.
    #[serde(rename = "b", default, skip_serializing_if = "is_zero")]
    pub bitrate: u32,
    /// Id of the cover thumbnail (`<id>.jpg` in the artwork folder); empty when there is none.
    #[serde(rename = "i", default, skip_serializing_if = "String::is_empty")]
    pub art: String,
    /// Number of tracks the tags say the album has.
    #[serde(rename = "N", default, skip_serializing_if = "is_zero")]
    pub track_total: u32,
    /// Tagged as part of a compilation.
    #[serde(rename = "k", default, skip_serializing_if = "std::ops::Not::not")]
    pub compilation: bool,
    /// Fields the tags lacked and the scanner took from the file path
    /// instead: `t` title, `a` artist, `l` album, `n` track number.
    #[serde(rename = "f", default, skip_serializing_if = "String::is_empty")]
    pub guessed: String,
    /// Kinds of tag present: `2` ID3v2, `1` ID3v1, `a` APE, `m` MP4, `v` Vorbis comments, `r` RIFF.
    #[serde(rename = "x", default, skip_serializing_if = "String::is_empty")]
    pub tags: String,
    /// MusicBrainz release id, when a tagger stored one.
    #[serde(rename = "R", default, skip_serializing_if = "String::is_empty")]
    pub mb_release: String,
    /// MusicBrainz recording id, when a tagger stored one.
    #[serde(rename = "T", default, skip_serializing_if = "String::is_empty")]
    pub mb_recording: String,
    /// Kind of release the track is on, as the librarian names it: Album, EP, Single, Compilation and so on.
    #[serde(rename = "K", default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// A track of the release that the collection does not have. It is listed with its album to show what is
    /// lacking, and never played; it has no file, so its path is empty.
    #[serde(rename = "w", default, skip_serializing_if = "std::ops::Not::not")]
    pub missing: bool,
    /// What the file still wants before it is finished; empty for everything in the ready catalogue.
    #[serde(rename = "z", default, skip_serializing_if = "String::is_empty")]
    pub state: String,
    /// Why the file is in its state, or why an unconfirmed track of the ready catalogue is not confirmed, in a few words.
    #[serde(rename = "e", default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// A track of the ready catalogue placed by its name or its length alone: its album is not verified.
    #[serde(rename = "u", default, skip_serializing_if = "std::ops::Not::not")]
    pub unconfirmed: bool,
    /// Hash of a slice of the audio data, the same for copies of a file
    /// that differ only in their tags; empty unless the scan computed it.
    #[serde(rename = "h", default, skip_serializing_if = "String::is_empty")]
    pub audio_key: String,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl Track {
    /// Artist used for grouping albums: the album artist when tagged, else the track artist.
    pub fn group_artist(&self) -> &str {
        if self.album_artist.is_empty() { &self.artist } else { &self.album_artist }
    }

    /// Whether the file keeps the sound exactly, going by its extension.
    pub fn lossless(&self) -> bool {
        matches!(self.ext().as_str(), "flac" | "wav" | "ape" | "aiff" | "aif" | "wv")
    }

    /// When the release came out, as a number that sorts: 20030923, with 0 for a month or day not known; the year alone
    /// when the date says nothing or another year. 0 when even the year is not known.
    pub fn released(&self) -> u32 {
        let mut parts = self.date.splitn(3, '-').map(|part| part.parse::<u32>().ok());
        match (parts.next().flatten(), parts.next(), parts.next()) {
            (Some(year), month, day) if year == self.year => {
                let (month, day) = (month.flatten().unwrap_or(0).min(12), day.flatten().unwrap_or(0).min(31));
                year * 10_000 + month * 100 + day
            }
            _ => self.year * 10_000,
        }
    }

    /// Larger for better sound: any lossless file above every lossy one, and
    /// within each kind the higher bitrate. 0 when a lossy file's bitrate is unknown.
    pub fn quality(&self) -> u32 {
        if self.lossless() { 1_000_000 + self.bitrate } else { self.bitrate }
    }

    /// Lowercased file extension without the dot.
    pub fn ext(&self) -> String {
        match self.path.rsplit_once('.') {
            Some((_, e)) if !e.contains('/') => e.to_ascii_lowercase(),
            _ => String::new(),
        }
    }
}

/// Writes `header`, with the version and the count filled in, then the tracks.
pub fn write_index<W: Write>(out: W, header: &Header, tracks: &[Track]) -> io::Result<()> {
    let mut w = BufWriter::new(GzEncoder::new(out, Compression::default()));
    let header = Header { v: INDEX_VERSION, count: tracks.len() as u32, ..header.clone() };
    serde_json::to_writer(&mut w, &header)?;
    w.write_all(b"\n")?;
    for t in tracks {
        serde_json::to_writer(&mut w, t)?;
        w.write_all(b"\n")?;
    }
    w.into_inner().map_err(|e| e.into_error())?.finish()?.flush()
}

pub fn read_index<R: Read>(input: R) -> io::Result<(Header, Vec<Track>)> {
    let mut lines = BufReader::new(GzDecoder::new(input)).lines();
    let first = lines.next().ok_or_else(|| invalid("empty index"))??;
    let header: Header = serde_json::from_str(&first)?;
    if header.v != INDEX_VERSION {
        return Err(invalid(format!("index version {} not supported", header.v)));
    }
    let mut tracks = Vec::with_capacity(header.count as usize);
    for line in lines {
        let line = line?;
        if !line.is_empty() {
            tracks.push(serde_json::from_str(&line)?);
        }
    }
    Ok((header, tracks))
}

/// Writes to a temporary sibling first so readers never see a half-written index.
pub fn write_index_file(path: &Path, header: &Header, tracks: &[Track]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    write_index(File::create(&tmp)?, header, tracks)?;
    std::fs::rename(&tmp, path)
}

pub fn read_index_file(path: &Path) -> io::Result<(Header, Vec<Track>)> {
    read_index(File::open(path)?)
}

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_omits_empty_fields() {
        let tracks = vec![
            Track {
                path: "music/Åsa Vinter/Ljus/01 Morgon.mp3".into(),
                size: 9_000_000,
                mtime: 1_700_000_000,
                dur_ms: 255_000,
                title: "Morgon".into(),
                artist: "Åsa Vinter".into(),
                album: "Ljus".into(),
                year: 1997,
                track: 1,
                ..Track::default()
            },
            Track { path: "music/loose.wma".into(), size: 1, mtime: 2, ..Track::default() },
        ];
        let mut buf = Vec::new();
        write_index(&mut buf, &Header { generated: 42, root: "/srv/music".into(), ..Header::default() }, &tracks).unwrap();
        let (header, back) = read_index(&buf[..]).unwrap();
        assert_eq!(header, Header { v: INDEX_VERSION, generated: 42, count: 2, art: false, ready: false, root: "/srv/music".into() });
        assert_eq!(back, tracks);
        assert_eq!(serde_json::to_string(&tracks[1]).unwrap(), r#"{"p":"music/loose.wma","s":1,"m":2}"#);
        assert_eq!(tracks[0].ext(), "mp3");
    }

    #[test]
    fn reads_the_ready_mark_and_a_reason_as_the_librarian_writes_them() {
        let header: Header = serde_json::from_str(r#"{"v":1,"generated":7,"count":1,"art":true,"ready":true}"#).unwrap();
        assert!(header.ready);
        let line = r#"{"p":"a/1.mp3","s":1,"m":2,"t":"Song","c":2,"D":"12\" Vinyl · SKINT 100","e":"placed by its name; 17 s longer than the release's","u":true}"#;
        let track: Track = serde_json::from_str(line).unwrap();
        assert!(track.unconfirmed);
        assert_eq!(track.reason, "placed by its name; 17 s longer than the release's");
        assert_eq!((track.disc, track.disc_name.as_str()), (2, "12\" Vinyl · SKINT 100"));
        assert_eq!(serde_json::to_string(&track).unwrap(), line);
    }

    #[test]
    fn a_release_date_sorts_by_day_and_falls_back_to_the_year() {
        let line = r#"{"p":"a/1.mp3","s":1,"m":2,"y":2003,"Y":"2003-09-23"}"#;
        let track: Track = serde_json::from_str(line).unwrap();
        assert_eq!(track.released(), 20030923);
        assert_eq!(serde_json::to_string(&track).unwrap(), line);
        let on = |year: u32, date: &str| Track { year, date: date.into(), ..Track::default() }.released();
        assert_eq!(on(2003, "2003-09"), 20030900, "the month alone");
        assert_eq!(on(2003, ""), 20030000, "the year alone");
        assert_eq!(on(2003, "1999-01-01"), 20030000, "a date of another year is not believed");
        assert_eq!(on(0, ""), 0);
    }

    #[test]
    fn reads_a_missing_track_as_the_librarian_writes_it() {
        let line = r#"{"p":"","s":0,"m":0,"d":225253,"t":"Third Song","a":"The Example Band","l":"Second Album","n":3,"w":true}"#;
        let track: Track = serde_json::from_str(line).unwrap();
        assert!(track.missing && track.path.is_empty());
        assert_eq!(serde_json::to_string(&track).unwrap(), line);
    }
}
