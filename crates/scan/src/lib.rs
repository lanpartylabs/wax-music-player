//! Scanning a music folder: walk it, read the tags, write the library index.
//!
//! Used by the `wax-scan` command and by the player, which scans the music
//! folder it is given. Re-runs are incremental: a file whose size and
//! modification time match the previous index is not opened again. A long
//! first scan saves its progress as it goes, so an interrupted run resumes.

mod art;
mod audio_key;

use std::collections::HashMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use lofty::config::{ParseOptions, ParsingMode};
use lofty::file::{AudioFile, TaggedFile, TaggedFileExt};
use lofty::picture::PictureType;
use lofty::probe::Probe;
use lofty::tag::{Accessor, ItemKey, Tag, TagType};
use rayon::prelude::*;
use wax_core::index::{read_index_file, write_index_file};
use wax_core::{Header, Track};

use crate::art::ArtStore;

const AUDIO_EXTS: &[&str] =
    &["mp3", "m4a", "flac", "ogg", "opus", "wav", "aac", "aiff", "aif", "wma", "ape", "wv", "mpc"];

/// Files tagged between two reports of progress.
const BATCH: usize = 250;
/// Files tagged between two saves of the progress file.
const CHECKPOINT_EVERY: usize = 1000;

pub type Failure = Box<dyn Error + Send + Sync>;

/// What to scan and where to write what was found.
#[derive(Clone, Debug)]
pub struct Options {
    /// The library root; paths in the index are relative to it.
    pub root: PathBuf,
    /// Only these folders under the root; all of it when empty.
    pub include: Vec<String>,
    /// The index to write.
    pub out: PathBuf,
    /// Also save a cover thumbnail per album into this folder.
    pub art: Option<PathBuf>,
    /// Also hash a slice of each file's audio, to find duplicates.
    pub audio_key: bool,
    /// A previous index to take unchanged files from; `out` when None.
    pub reuse: Option<PathBuf>,
    /// Parallel readers.
    pub jobs: usize,
}

/// How far a scan has come.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// Audio files found by the walk; 0 while it is still walking.
    pub found: usize,
    /// Of those, the ones whose tags are read or taken from the previous index.
    pub read: usize,
}

/// What a finished scan did.
#[derive(Clone, Copy, Debug, Default)]
pub struct Summary {
    pub tracks: usize,
    /// Taken unchanged from the previous index.
    pub reused: usize,
    /// In the previous index.
    pub before: usize,
    pub unreadable: usize,
    pub with_art: usize,
    pub hours: u64,
    pub seconds: f32,
}

impl Summary {
    /// Whether the index now differs from the one before: a file added, changed or gone.
    pub fn changed(&self) -> bool {
        self.reused != self.tracks || self.before != self.tracks
    }
}

/// A file found by the walk, before its tags are read.
struct Found {
    rel: String,
    size: u64,
    mtime: i64,
    /// How many leading path components belong to the `include` prefix.
    base: usize,
    /// Best cover image file in the same folder, if any.
    cover: Option<Arc<PathBuf>>,
}

/// Scans as `options` say, telling `progress` how far it has come now and then, and writes the index.
pub fn scan(options: &Options, progress: &(dyn Fn(Progress) + Sync)) -> Result<Summary, Failure> {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(options.jobs.max(1)).build()?;
    pool.install(|| run(options, progress))
}

fn run(options: &Options, progress: &(dyn Fn(Progress) + Sync)) -> Result<Summary, Failure> {
    let started = Instant::now();
    if !options.root.is_dir() {
        return Err(format!("{} is not a folder", options.root.display()).into());
    }
    let root = std::fs::canonicalize(&options.root)?;
    let starts: Vec<(PathBuf, usize)> = if options.include.is_empty() {
        vec![(root.clone(), 0)]
    } else {
        options.include.iter().map(|s| (root.join(s), Path::new(s).components().count())).collect()
    };
    progress(Progress::default());
    let found = walk(&root, starts);
    progress(Progress { found: found.len(), read: 0 });

    let art = options.art.clone().map(ArtStore::new).transpose()?;

    // Known tracks come from the previous index and from the progress file
    // of an interrupted run. An index made without artwork is of no use to
    // a run that wants artwork.
    let partial = options.out.with_extension("partial");
    let mut previous: HashMap<String, Track> = HashMap::new();
    let mut before = 0;
    for (n, source) in [options.reuse.as_deref().unwrap_or(&options.out), &partial].into_iter().enumerate() {
        match read_index_file(source) {
            Ok((header, tracks)) if header.art || art.is_none() => {
                if n == 0 {
                    before = tracks.len();
                }
                previous.extend(tracks.into_iter().map(|t| (t.path.clone(), t)));
            }
            _ => {}
        }
    }

    let (reused, unreadable) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let now = || SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let header = |tracks: &[Track]| Header {
        generated: now(),
        art: art.is_some(),
        root: root.to_string_lossy().into_owned(),
        count: tracks.len() as u32,
        ..Header::default()
    };
    let mut tracks: Vec<Track> = Vec::with_capacity(found.len());
    let mut unsaved = 0;
    for batch in found.chunks(BATCH) {
        let read: Vec<Track> = batch
            .par_iter()
            .map(|f| match previous.get(&f.rel) {
                Some(old) if old.size == f.size && old.mtime == f.mtime => {
                    reused.fetch_add(1, Ordering::Relaxed);
                    old.clone()
                }
                _ => {
                    let (mut track, readable) = read_track(&root, f, art.as_ref());
                    if options.audio_key {
                        track.audio_key = audio_key::audio_key(&root.join(&f.rel), f.size).unwrap_or_default();
                    }
                    if !readable {
                        unreadable.fetch_add(1, Ordering::Relaxed);
                    }
                    track
                }
            })
            .collect();
        tracks.extend(read);
        unsaved += batch.len();
        if unsaved >= CHECKPOINT_EVERY && tracks.len() < found.len() {
            write_index_file(&partial, &header(&tracks), &tracks)?;
            unsaved = 0;
        }
        progress(Progress { found: found.len(), read: tracks.len() });
    }
    tracks.sort_unstable_by(|a, b| a.path.cmp(&b.path));

    if let Some(folder) = options.out.parent().filter(|folder| !folder.as_os_str().is_empty()) {
        std::fs::create_dir_all(folder)?;
    }
    write_index_file(&options.out, &header(&tracks), &tracks)?;
    let _ = std::fs::remove_file(&partial);

    Ok(Summary {
        tracks: tracks.len(),
        reused: reused.into_inner(),
        before,
        unreadable: unreadable.into_inner(),
        with_art: tracks.iter().filter(|t| !t.art.is_empty()).count(),
        hours: tracks.iter().map(|t| t.dur_ms as u64).sum::<u64>() / 3_600_000,
        seconds: started.elapsed().as_secs_f32(),
    })
}

/// Breadth-first walk that lists every directory of a level in parallel, so a
/// slow network filesystem costs one round of latency per level, not per folder.
fn walk(root: &Path, starts: Vec<(PathBuf, usize)>) -> Vec<Found> {
    let mut dirs = starts;
    let mut files = Vec::new();
    while !dirs.is_empty() {
        let level: Vec<_> = dirs.par_iter().map(|(dir, base)| list_dir(root, dir, *base)).collect();
        dirs.clear();
        for (subdirs, found) in level {
            dirs.extend(subdirs);
            files.extend(found);
        }
    }
    files
}

fn list_dir(root: &Path, dir: &Path, base: usize) -> (Vec<(PathBuf, usize)>, Vec<Found>) {
    let (mut subdirs, mut files) = (Vec::new(), Vec::new());
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("skipping {}: {e}", dir.display());
            return (subdirs, files);
        }
    };
    let mut cover: Option<(u8, PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        // Hidden entries are metadata folders, never music.
        if entry.file_name().as_encoded_bytes().starts_with(b".") {
            continue;
        }
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            subdirs.push((path, base));
        } else if let Some(rank) = entry.file_name().to_str().and_then(art::cover_rank) {
            if kind.is_file() && cover.as_ref().is_none_or(|(best, _)| rank < *best) {
                cover = Some((rank, path));
            }
        } else if kind.is_file() && is_audio(&path) {
            let (Ok(meta), Some(rel)) = (entry.metadata(), path.strip_prefix(root).ok().and_then(Path::to_str))
            else {
                eprintln!("skipping {}", path.display());
                continue;
            };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            // Paths in the index are `/`-separated wherever the scan ran.
            let rel = rel.replace(std::path::MAIN_SEPARATOR, "/");
            files.push(Found { rel, size: meta.len(), mtime, base, cover: None });
        }
    }
    let cover = cover.map(|(_, path)| Arc::new(path));
    for file in &mut files {
        file.cover = cover.clone();
    }
    (subdirs, files)
}

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTS.iter().any(|known| e.eq_ignore_ascii_case(known)))
}

/// Returns the track and whether its tags could be parsed. Fields the tags
/// leave empty are filled from the folder structure and file name. With an
/// `art` store, the cover embedded in the file (or else the folder's cover
/// image) is saved as a thumbnail.
fn read_track(root: &Path, f: &Found, art: Option<&ArtStore>) -> (Track, bool) {
    let mut t = Track { path: f.rel.clone(), size: f.size, mtime: f.mtime, ..Track::default() };
    // Embedded cover art can be megabytes per file; skip it unless wanted.
    let options = ParseOptions::new().read_cover_art(art.is_some()).parsing_mode(ParsingMode::Relaxed);
    let parsed = Probe::open(root.join(&f.rel)).and_then(|probe| probe.options(options).read());
    let readable = parsed.is_ok();
    if let Ok(file) = parsed {
        let props = file.properties();
        t.dur_ms = props.duration().as_millis() as u32;
        t.bitrate = props.audio_bitrate().unwrap_or(0);

        // The primary tag wins; older tag formats in the same file fill the gaps.
        let primary = file.primary_tag();
        let others = file.tags().iter().filter(|tag| primary.is_none_or(|p| p.tag_type() != tag.tag_type()));
        for tag in primary.into_iter().chain(others) {
            apply_tag(&mut t, tag);
        }
        t.tags = file.tags().iter().map(|tag| tag_letter(tag.tag_type())).collect();
        if let Some(art) = art {
            t.art = embedded_cover(&file).and_then(|bytes| art.add(bytes)).unwrap_or_default();
        }
    }
    if let (Some(art), true, Some(cover)) = (art, t.art.is_empty(), &f.cover) {
        t.art = art.add_file(cover).unwrap_or_default();
    }
    fill_from_path(&mut t, f.base);
    (t, readable)
}

/// The front cover stored in the file's tags, or failing that any picture.
fn embedded_cover(file: &TaggedFile) -> Option<&[u8]> {
    let pictures = || file.tags().iter().flat_map(|tag| tag.pictures());
    pictures().find(|p| p.pic_type() == PictureType::CoverFront).or_else(|| pictures().next()).map(|p| p.data())
}

fn tag_letter(kind: TagType) -> char {
    match kind {
        TagType::Id3v2 => '2',
        TagType::Id3v1 => '1',
        TagType::Ape => 'a',
        TagType::Mp4Ilst => 'm',
        TagType::VorbisComments => 'v',
        TagType::RiffInfo => 'r',
        _ => '?',
    }
}

fn apply_tag(t: &mut Track, tag: &Tag) {
    fn set(field: &mut String, value: Option<impl AsRef<str>>) {
        if let (true, Some(value)) = (field.is_empty(), value) {
            *field = value.as_ref().trim_matches(|c: char| c.is_whitespace() || c == '\0').to_owned();
        }
    }
    set(&mut t.title, tag.title());
    set(&mut t.artist, tag.artist());
    set(&mut t.album, tag.album());
    set(&mut t.genre, tag.genre());
    set(&mut t.album_artist, tag.get_string(ItemKey::AlbumArtist));
    set(&mut t.mb_release, tag.get_string(ItemKey::MusicBrainzReleaseId));
    set(&mut t.mb_recording, tag.get_string(ItemKey::MusicBrainzRecordingId));
    t.compilation |= tag.get_string(ItemKey::FlagCompilation) == Some("1");
    if t.track == 0 {
        t.track = tag.track().unwrap_or(0);
    }
    if t.track_total == 0 {
        t.track_total = tag.track_total().unwrap_or(0);
    }
    if t.disc == 0 {
        t.disc = tag.disk().unwrap_or(0);
    }
    if t.year == 0 {
        t.year = tag
            .date()
            .map(|d| d.year as u32)
            .or_else(|| tag.get_string(ItemKey::Year).and_then(leading_number))
            .unwrap_or(0);
    }
}

/// Fallbacks for untagged files laid out as `Artist/Album/NN Title.ext`.
fn fill_from_path(t: &mut Track, base: usize) {
    let parts: Vec<&str> = t.path.split('/').skip(base).collect();
    let Some((file, dirs)) = parts.split_last() else { return };
    let stem = file.rsplit_once('.').map_or(*file, |(stem, _)| stem);
    if t.title.is_empty() {
        let (number, name) = split_track_number(stem);
        t.title = name.to_owned();
        t.guessed.push('t');
        if t.track == 0 && number != 0 {
            t.track = number;
            t.guessed.push('n');
        }
    }
    if t.artist.is_empty() {
        t.artist = dirs.first().copied().unwrap_or_default().to_owned();
        t.guessed.push('a');
    }
    // A file sitting directly in the artist folder has no album folder.
    if t.album.is_empty() && dirs.len() >= 2 {
        t.album = dirs[dirs.len() - 1].to_owned();
        t.guessed.push('l');
    }
}

/// Splits "07 - Song", "07. Song" or "07 Song" into (7, "Song"). Names that
/// are only a number, or start with a year-like number, are left alone.
fn split_track_number(stem: &str) -> (u32, &str) {
    let digits = stem.bytes().take_while(u8::is_ascii_digit).count();
    let rest = stem[digits..].trim_start_matches([' ', '.', '-', '_']);
    match stem[..digits].parse::<u32>() {
        Ok(n) if (1..=3).contains(&digits) && !rest.is_empty() && rest.len() < stem.len() - digits => (n, rest),
        _ => (0, stem),
    }
}

fn leading_number(s: &str) -> Option<u32> {
    let digits = s.trim_start().bytes().take_while(u8::is_ascii_digit).count();
    s.trim_start()[..digits].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_numbers_are_split_from_file_names() {
        assert_eq!(split_track_number("07 - Song"), (7, "Song"));
        assert_eq!(split_track_number("07. Song"), (7, "Song"));
        assert_eq!(split_track_number("7 Song"), (7, "Song"));
        assert_eq!(split_track_number("1979"), (0, "1979"));
        assert_eq!(split_track_number("50cent"), (0, "50cent"));
        assert_eq!(split_track_number("Song"), (0, "Song"));
    }

    #[test]
    fn untagged_files_take_names_from_folders() {
        let mut t = Track { path: "music/The Example Band/First Album/03 Third Song.wma".into(), ..Track::default() };
        fill_from_path(&mut t, 1);
        assert_eq!((t.artist.as_str(), t.album.as_str(), t.title.as_str(), t.track), ("The Example Band", "First Album", "Third Song", 3));
        assert_eq!(t.guessed, "tnal");

        let mut loose = Track { path: "music/The Example Band/A Loose Song.mp3".into(), ..Track::default() };
        fill_from_path(&mut loose, 1);
        assert_eq!((loose.artist.as_str(), loose.album.as_str(), loose.title.as_str()), ("The Example Band", "", "A Loose Song"));

        let mut tagged = Track { path: "music/x/y/01 z.mp3".into(), title: "Real".into(), artist: "Who".into(), ..Track::default() };
        fill_from_path(&mut tagged, 1);
        assert_eq!((tagged.artist.as_str(), tagged.album.as_str(), tagged.title.as_str(), tagged.track), ("Who", "y", "Real", 0));
        assert_eq!(tagged.guessed, "l");
    }

    /// A folder of files that are not really audio: the walk finds them, their tags cannot be read, the folders name
    /// them, and a second scan takes them all from the first.
    #[test]
    fn scans_a_folder_and_reuses_what_did_not_change() {
        let dir = std::env::temp_dir().join(format!("wax-scan-test-{}", std::process::id()));
        let music = dir.join("music");
        std::fs::create_dir_all(music.join("The Example Band/First Album")).unwrap();
        std::fs::write(music.join("The Example Band/First Album/01 Opening.mp3"), b"not audio").unwrap();
        std::fs::write(music.join("The Example Band/First Album/notes.txt"), b"not music").unwrap();
        std::fs::create_dir_all(music.join(".hidden")).unwrap();
        std::fs::write(music.join(".hidden/02 Secret.mp3"), b"not audio").unwrap();
        let options = Options { root: music.clone(), include: Vec::new(), out: dir.join("index/library.jsonl.gz"), art: None, audio_key: false, reuse: None, jobs: 2 };

        let seen = std::sync::Mutex::new(Vec::new());
        let first = scan(&options, &|progress| seen.lock().unwrap().push(progress)).unwrap();
        assert_eq!((first.tracks, first.reused, first.unreadable), (1, 0, 1));
        assert!(first.changed());
        assert_eq!(seen.lock().unwrap().last(), Some(&Progress { found: 1, read: 1 }));

        let (header, tracks) = read_index_file(&options.out).unwrap();
        assert_eq!(header.root, std::fs::canonicalize(&music).unwrap().to_string_lossy());
        assert_eq!(tracks[0].path, "The Example Band/First Album/01 Opening.mp3");
        assert_eq!((tracks[0].artist.as_str(), tracks[0].album.as_str(), tracks[0].title.as_str()), ("The Example Band", "First Album", "Opening"));

        let again = scan(&options, &|_| {}).unwrap();
        assert_eq!((again.tracks, again.reused, again.before), (1, 1, 1));
        assert!(!again.changed(), "nothing on disk changed");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
