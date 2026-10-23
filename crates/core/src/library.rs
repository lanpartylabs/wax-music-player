//! In-memory library: everything the UI needs to filter, sort and browse
//! tens of thousands of tracks in well under a frame.
//!
//! All expensive work (folding text, interning names, ranking sort keys) is
//! done once in [`Library::new`]; a [`Library::view`] call is then a linear
//! pass over small integer arrays plus one sort of `u64` keys.

use std::collections::HashMap;

use memchr::memmem::Finder;

use crate::index::Track;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Column {
    Title,
    Artist,
    Album,
    Genre,
    Year,
    Track,
    Time,
    Added,
    /// How good the file sounds: lossless above lossy, then by bitrate.
    Quality,
    /// Why the file is in its state.
    Why,
}

impl Column {
    pub const ALL: [Column; 10] = [
        Column::Title,
        Column::Artist,
        Column::Album,
        Column::Genre,
        Column::Year,
        Column::Track,
        Column::Time,
        Column::Added,
        Column::Quality,
        Column::Why,
    ];
}

/// How good a file sounds, in the bands a query can ask for.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Sound {
    Lossless,
    /// 320 kbit/s and up.
    Top,
    /// 256 to 319 kbit/s.
    High,
    /// 192 to 255 kbit/s.
    Fair,
    /// Under 192 kbit/s, or unknown.
    Low,
}

impl Sound {
    pub const ALL: [Sound; 5] = [Sound::Lossless, Sound::Top, Sound::High, Sound::Fair, Sound::Low];

    pub fn of(track: &Track) -> Sound {
        match track.bitrate {
            _ if track.lossless() => Sound::Lossless,
            320.. => Sound::Top,
            256.. => Sound::High,
            192.. => Sound::Fair,
            _ => Sound::Low,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Sound::Lossless => "Lossless",
            Sound::Top => "320 kbit/s",
            Sound::High => "256 to 319",
            Sound::Fair => "192 to 255",
            Sound::Low => "Under 192",
        }
    }
}

/// What the user is currently looking at: a search string, filters that hold
/// for the whole window, and the column-browser selection.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Query {
    pub text: String,
    /// Only tracks in this state; see [`Library::states`].
    pub state: Option<u32>,
    /// Only tracks on this kind of release; see [`Library::kinds`].
    pub kind: Option<u32>,
    pub sound: Option<Sound>,
    /// Only tracks from the ten years starting here, as in 1990.
    pub decade: Option<u32>,
    pub genre: Option<u32>,
    pub artist: Option<u32>,
    pub album: Option<u32>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Facet {
    pub id: u32,
    pub count: u32,
}

/// Result of a query: the track rows in display order and the browser columns that go with them.
#[derive(Clone, Default, Debug)]
pub struct View {
    pub rows: Vec<u32>,
    /// Genres containing at least one track that matches the search text.
    pub genres: Vec<Facet>,
    /// Artists within the selected genre.
    pub artists: Vec<Facet>,
    /// Albums within the selected genre and artist.
    pub albums: Vec<Facet>,
    pub total_ms: u64,
    pub total_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct Album {
    pub name: String,
    pub artist: u32,
}

pub struct Library {
    pub tracks: Vec<Track>,
    /// Display names, indexed by id. Ids ascend in alphabetical order.
    pub genres: Vec<String>,
    pub artists: Vec<String>,
    pub albums: Vec<Album>,
    /// Kinds of release, such as "Album" and "Single"; empty where the index does not say.
    pub kinds: Vec<String>,
    /// What tracks still want before they are finished; "" for those that want nothing.
    pub states: Vec<String>,
    /// Every decade with a track in it, ascending, as in 1990.
    pub decades: Vec<u32>,
    pub kind_of: Vec<u32>,
    pub state_of: Vec<u32>,
    pub genre_of: Vec<u32>,
    pub artist_of: Vec<u32>,
    pub album_of: Vec<u32>,
    /// Folded searchable text of every track, concatenated; `hay_off[i]..hay_off[i + 1]` is track `i`.
    hay: Vec<u8>,
    hay_off: Vec<u32>,
    /// Position of each track in artist / album / disc / track order, used to break sort ties.
    natural: Vec<u32>,
    ranks: [Vec<u32>; Column::ALL.len()],
}

impl Library {
    pub fn new(tracks: Vec<Track>) -> Self {
        let n = tracks.len();

        let genre_keys: Vec<String> = tracks.iter().map(|t| sort_key(&t.genre)).collect();
        let group_keys: Vec<String> = tracks.iter().map(|t| sort_key(t.group_artist())).collect();
        let artist_keys: Vec<String> = tracks.iter().map(|t| sort_key(&t.artist)).collect();
        let album_keys: Vec<String> = tracks.iter().map(|t| sort_key(&t.album)).collect();
        let title_keys: Vec<String> = tracks.iter().map(|t| sort_key(&t.title)).collect();

        let (genre_of, genres) = intern(&genre_keys, tracks.iter().map(|t| t.genre.as_str()));
        let kind_keys: Vec<String> = tracks.iter().map(|t| sort_key(&t.kind)).collect();
        let (kind_of, kinds) = intern(&kind_keys, tracks.iter().map(|t| t.kind.as_str()));
        let state_keys: Vec<String> = tracks.iter().map(|t| sort_key(&t.state)).collect();
        let (state_of, states) = intern(&state_keys, tracks.iter().map(|t| t.state.as_str()));
        let mut decades: Vec<u32> = tracks.iter().filter(|t| t.year > 0).map(|t| t.year / 10 * 10).collect();
        decades.sort_unstable();
        decades.dedup();
        let (artist_of, artists) = intern(&group_keys, tracks.iter().map(|t| t.group_artist()));
        // An album is identified by its name together with its artist, so two
        // different "Greatest Hits" stay separate, and by its release where the
        // index names one, so that an artist's two records of one name do too.
        // Sorting by name first keeps ids alphabetical for the browser.
        let album_ids: Vec<(&str, u32, &str)> =
            (0..n).map(|i| (album_keys[i].as_str(), artist_of[i], tracks[i].mb_release.as_str())).collect();
        let (album_of, album_names) = intern(&album_ids, tracks.iter().map(|t| t.album.as_str()));
        let mut albums: Vec<Album> = album_names.into_iter().map(|name| Album { name, artist: 0 }).collect();
        for i in 0..n {
            albums[album_of[i] as usize].artist = artist_of[i];
        }

        let mut order: Vec<u32> = (0..n as u32).collect();
        order.sort_unstable_by(|&a, &b| {
            let (a, b) = (a as usize, b as usize);
            group_keys[a]
                .cmp(&group_keys[b])
                .then_with(|| album_keys[a].cmp(&album_keys[b]))
                .then_with(|| tracks[a].mb_release.cmp(&tracks[b].mb_release))
                .then_with(|| tracks[a].disc.cmp(&tracks[b].disc))
                .then_with(|| tracks[a].track.cmp(&tracks[b].track))
                .then_with(|| title_keys[a].cmp(&title_keys[b]))
                .then_with(|| tracks[a].path.cmp(&tracks[b].path))
        });
        let mut natural = vec![0u32; n];
        for (pos, &i) in order.iter().enumerate() {
            natural[i as usize] = pos as u32;
        }

        let or_last = |v: u32| if v == 0 { LAST } else { v };
        let ranks = [
            rank_text(&title_keys),
            rank_text(&artist_keys),
            rank_text(&album_keys),
            rank_text(&genre_keys),
            // By the day where the index knows it: one year's releases in the order they came out.
            tracks.iter().map(|t| or_last(t.released())).collect(),
            tracks.iter().map(|t| or_last(t.track)).collect(),
            tracks.iter().map(|t| or_last(t.dur_ms)).collect(),
            dense_rank(&tracks.iter().map(|t| t.mtime).collect::<Vec<_>>()),
            tracks.iter().map(|t| or_last(t.quality())).collect(),
            rank_text(&tracks.iter().map(|t| sort_key(&t.reason)).collect::<Vec<_>>()),
        ];

        let mut hay = Vec::with_capacity(n * 64);
        let mut hay_off = Vec::with_capacity(n + 1);
        for t in &tracks {
            hay_off.push(hay.len() as u32);
            for field in [&t.title, &t.artist, &t.album_artist, &t.album, &t.genre] {
                if !field.is_empty() {
                    hay.extend_from_slice(fold(field).as_bytes());
                    hay.push(b'\n');
                }
            }
            if t.year != 0 {
                hay.extend_from_slice(t.year.to_string().as_bytes());
                hay.push(b'\n');
            }
        }
        hay_off.push(hay.len() as u32);

        Library {
            tracks,
            genres,
            artists,
            albums,
            kinds,
            states,
            decades,
            kind_of,
            state_of,
            genre_of,
            artist_of,
            album_of,
            hay,
            hay_off,
            natural,
            ranks,
        }
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    /// Runs a query and returns rows sorted by `sort`. Within equal sort keys
    /// rows always stay in album order, and untagged values always come last,
    /// whichever direction is chosen.
    pub fn view(&self, q: &Query, sort: Column, descending: bool) -> View {
        let n = self.len();
        let text_match = self.match_text(&q.text);

        let mut genre_counts = vec![0u32; self.genres.len()];
        let mut artist_counts = vec![0u32; self.artists.len()];
        let mut album_counts = vec![0u32; self.albums.len()];
        let mut view = View::default();

        for i in 0..n {
            if text_match.as_ref().is_some_and(|mask| !mask[i]) {
                continue;
            }
            let track = &self.tracks[i];
            if track.missing {
                // A track the collection lacks is listed only with its own album, and counts toward nothing.
                if q.album == Some(self.album_of[i]) && q.sound.is_none() && q.state.is_none_or(|s| s == self.state_of[i]) {
                    view.rows.push(i as u32);
                }
                continue;
            }
            if q.state.is_some_and(|s| s != self.state_of[i])
                || q.kind.is_some_and(|k| k != self.kind_of[i])
                || q.sound.is_some_and(|s| s != Sound::of(track))
                || q.decade.is_some_and(|d| track.year < d || track.year >= d + 10)
            {
                continue;
            }
            genre_counts[self.genre_of[i] as usize] += 1;
            if q.genre.is_some_and(|g| g != self.genre_of[i]) {
                continue;
            }
            artist_counts[self.artist_of[i] as usize] += 1;
            if q.artist.is_some_and(|a| a != self.artist_of[i]) {
                continue;
            }
            album_counts[self.album_of[i] as usize] += 1;
            if q.album.is_some_and(|a| a != self.album_of[i]) {
                continue;
            }
            view.rows.push(i as u32);
            view.total_ms += self.tracks[i].dur_ms as u64;
            view.total_bytes += self.tracks[i].size;
        }

        view.genres = facets(&genre_counts);
        view.artists = facets(&artist_counts);
        view.albums = facets(&album_counts);
        self.sort(&mut view.rows, sort, descending);
        view
    }

    pub fn sort(&self, rows: &mut [u32], col: Column, descending: bool) {
        let rank = &self.ranks[col as usize];
        rows.sort_unstable_by_key(|&i| {
            let primary = match rank[i as usize] {
                LAST => LAST,
                r if descending => LAST - 1 - r,
                r => r,
            };
            ((primary as u64) << 32) | self.natural[i as usize] as u64
        });
    }

    /// `None` when the text is blank (everything matches); otherwise one flag
    /// per track. Every whitespace-separated word must occur somewhere in the
    /// track's title, artist, album, genre or year.
    fn match_text(&self, text: &str) -> Option<Vec<bool>> {
        let folded = fold(text);
        let mut words = folded.split_whitespace().peekable();
        words.peek()?;

        let n = self.len();
        let mut mask = vec![true; n];
        for word in words {
            let finder = Finder::new(word.as_bytes());
            let mut hit = vec![false; n];
            let (mut track, mut pos) = (0usize, 0usize);
            // One search over the whole buffer instead of one per track: jump
            // to the next occurrence, mark its track, resume after that track.
            while let Some(rel) = finder.find(&self.hay[pos..]) {
                let at = (pos + rel) as u32;
                while self.hay_off[track + 1] <= at {
                    track += 1;
                }
                hit[track] = true;
                pos = self.hay_off[track + 1] as usize;
                track += 1;
                if track == n {
                    break;
                }
            }
            for i in 0..n {
                mask[i] &= hit[i];
            }
        }
        Some(mask)
    }
}

fn facets(counts: &[u32]) -> Vec<Facet> {
    counts
        .iter()
        .enumerate()
        .filter(|&(_, &count)| count > 0)
        .map(|(id, &count)| Facet { id: id as u32, count })
        .collect()
}

/// Lowercased, accent-stripped form used for searching, so "rokkur kor" finds "Rökkur Kór".
pub fn fold(s: &str) -> String {
    let mut out = deunicode::deunicode(s);
    out.make_ascii_lowercase();
    out
}

/// Sort rank given to untagged values so they end up after everything else.
const LAST: u32 = u32::MAX;
/// Sort key standing in for an empty string; greater than any folded text.
const EMPTY_KEY: &str = "\x7f";

/// Folded form with a leading "the " dropped.
fn sort_key(s: &str) -> String {
    let folded = fold(s.trim());
    if folded.is_empty() {
        return EMPTY_KEY.into();
    }
    match folded.strip_prefix("the ") {
        Some(rest) if !rest.is_empty() => rest.to_owned(),
        _ => folded,
    }
}

/// Rank of each key among the distinct keys, in ascending order.
fn dense_rank<K: Ord>(keys: &[K]) -> Vec<u32> {
    let mut order: Vec<u32> = (0..keys.len() as u32).collect();
    order.sort_unstable_by(|&a, &b| keys[a as usize].cmp(&keys[b as usize]));
    let mut ranks = vec![0u32; keys.len()];
    let mut rank = 0u32;
    for w in 0..order.len() {
        if w > 0 && keys[order[w] as usize] != keys[order[w - 1] as usize] {
            rank += 1;
        }
        ranks[order[w] as usize] = rank;
    }
    ranks
}

fn rank_text(keys: &[String]) -> Vec<u32> {
    let mut ranks = dense_rank(keys);
    for (rank, key) in ranks.iter_mut().zip(keys) {
        if key == EMPTY_KEY {
            *rank = LAST;
        }
    }
    ranks
}

/// Assigns one id per distinct key (ids ascend with the key) and picks the
/// most common spelling as that id's display name, so "MiLa BrooK" and
/// "Mila Brook" end up as a single entry.
fn intern<'a, K: Ord>(keys: &[K], names: impl Iterator<Item = &'a str>) -> (Vec<u32>, Vec<String>) {
    let ids = dense_rank(keys);
    let count = ids.iter().max().map_or(0, |&m| m as usize + 1);
    let mut spellings: Vec<HashMap<&str, u32>> = vec![HashMap::new(); count];
    for (name, &id) in names.zip(&ids) {
        *spellings[id as usize].entry(name.trim()).or_default() += 1;
    }
    let display = spellings
        .into_iter()
        .map(|seen| {
            // Ties resolve to the alphabetically first spelling so the result is deterministic.
            seen.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(a.0))).map_or(String::new(), |(s, _)| s.to_owned())
        })
        .collect();
    (ids, display)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(artist: &str, album: &str, n: u32, title: &str, genre: &str, year: u32) -> Track {
        Track {
            path: format!("{artist}/{album}/{n:02} {title}.mp3"),
            size: 1_000,
            dur_ms: 60_000 * n,
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            genre: genre.into(),
            year,
            track: n,
            ..Track::default()
        }
    }

    fn sample() -> Library {
        Library::new(vec![
            track("The Night Owls", "Late Bloom", 2, "Brightwater Steps", "Rock", 2004),
            track("Rökkur Kór", "Ljós...", 3, "Hvítur Snjór", "Post-Rock", 2005),
            track("Mila Brook", "Under the Moon", 1, "Halfway Lantern", "Hip Hop", 2009),
            track("MiLa BrooK", "Under the Moon", 2, "Stay Up 2 Late", "Hip Hop", 2009),
            track("Mila Brook", "Under the Moon", 3, "Easy As...", "Hip Hop", 2009),
            track("The Night Owls", "Late Bloom", 1, "First Light", "Rock", 2004),
            track("", "", 0, "untagged", "", 0),
        ])
    }

    fn titles(lib: &Library, view: &View) -> Vec<String> {
        view.rows.iter().map(|&i| lib.tracks[i as usize].title.clone()).collect()
    }

    #[test]
    fn default_order_is_artist_album_track_with_untagged_last() {
        let lib = sample();
        let view = lib.view(&Query::default(), Column::Artist, false);
        assert_eq!(
            titles(&lib, &view),
            [
                "Halfway Lantern",
                "Stay Up 2 Late",
                "Easy As...",
                "First Light",
                "Brightwater Steps",
                "Hvítur Snjór",
                "untagged"
            ]
        );
        assert_eq!(view.total_bytes, 7_000);
    }

    #[test]
    fn two_releases_of_one_name_by_one_artist_are_two_albums() {
        let on = |release: &str, n: u32, title: &str, year: u32| Track { mb_release: release.into(), ..track("The Example Band", "The Example Band", n, title, "Punk", year) };
        let lib = Library::new(vec![
            on("2000", 1, "Once Again", 2000),
            on("1993", 2, "Second", 1993),
            on("2000", 2, "Twice Again", 2000),
            on("1993", 1, "Opening", 1993),
        ]);
        assert_eq!(lib.albums.len(), 2);
        assert_eq!(lib.album_of, [1, 0, 1, 0]);
        let view = lib.view(&Query::default(), Column::Artist, false);
        assert_eq!(titles(&lib, &view), ["Opening", "Second", "Once Again", "Twice Again"]);
    }

    #[test]
    fn a_missing_track_is_listed_only_with_its_album() {
        let gap = Track { path: String::new(), size: 0, missing: true, ..track("The Night Owls", "Late Bloom", 3, "Kettle Weather", "Rock", 2004) };
        let mut tracks = sample().tracks;
        tracks.push(gap);
        let lib = Library::new(tracks);
        let everything = lib.view(&Query::default(), Column::Artist, false);
        assert!(!titles(&lib, &everything).contains(&"Kettle Weather".to_owned()));
        assert_eq!(everything.total_bytes, 7_000);
        let owls = everything.artists.iter().find(|f| lib.artists[f.id as usize] == "The Night Owls").unwrap();
        assert_eq!(owls.count, 2);
        let album = lib.album_of[lib.tracks.iter().position(|t| t.album == "Late Bloom").unwrap()];
        let opened = lib.view(&Query { album: Some(album), ..Query::default() }, Column::Album, false);
        assert_eq!(titles(&lib, &opened), ["First Light", "Brightwater Steps", "Kettle Weather"]);
        assert_eq!(opened.total_ms, 180_000);
        let searched = lib.view(&Query { text: "kettle".into(), ..Query::default() }, Column::Artist, false);
        assert!(searched.rows.is_empty());
    }

    #[test]
    fn quality_puts_lossless_first_then_the_higher_bitrate() {
        let file = |name: &str, bitrate: u32| Track { path: name.into(), title: name.into(), bitrate, ..Track::default() };
        let lib = Library::new(vec![file("a.mp3", 128), file("b.flac", 900), file("c.mp3", 320), file("d.mp3", 0), file("e.m4a", 256)]);
        let view = lib.view(&Query::default(), Column::Quality, true);
        assert_eq!(titles(&lib, &view), ["b.flac", "c.mp3", "e.m4a", "a.mp3", "d.mp3"]);
    }

    #[test]
    fn filters_hold_whatever_else_is_asked() {
        let file = |name: &str, bitrate: u32, year: u32, kind: &str, state: &str| Track {
            path: name.into(),
            title: name.into(),
            bitrate,
            year,
            kind: kind.into(),
            state: state.into(),
            ..Track::default()
        };
        let lib = Library::new(vec![
            file("a.flac", 900, 1994, "Album", ""),
            file("b.mp3", 320, 1999, "Single", ""),
            file("c.mp3", 128, 2004, "Album", "Incomplete"),
            file("d.mp3", 256, 0, "", "Unknown"),
        ]);
        let found = |q: Query| titles(&lib, &lib.view(&q, Column::Title, false));
        let id = |names: &[String], name: &str| names.iter().position(|n| n == name).map(|i| i as u32);
        assert_eq!(lib.decades, [1990, 2000]);
        assert_eq!(found(Query { decade: Some(1990), ..Query::default() }), ["a.flac", "b.mp3"]);
        assert_eq!(found(Query { sound: Some(Sound::Lossless), ..Query::default() }), ["a.flac"]);
        assert_eq!(found(Query { sound: Some(Sound::Low), ..Query::default() }), ["c.mp3"]);
        assert_eq!(found(Query { kind: id(&lib.kinds, "Album"), ..Query::default() }), ["a.flac", "c.mp3"]);
        assert_eq!(found(Query { kind: id(&lib.kinds, "Album"), state: id(&lib.states, ""), ..Query::default() }), ["a.flac"]);
        assert_eq!(found(Query { state: id(&lib.states, "Unknown"), ..Query::default() }), ["d.mp3"]);
    }

    #[test]
    fn spellings_merge_and_articles_are_ignored() {
        let lib = sample();
        assert_eq!(lib.artists, ["Mila Brook", "The Night Owls", "Rökkur Kór", ""]);
        assert_eq!(lib.genres, ["Hip Hop", "Post-Rock", "Rock", ""]);
    }

    #[test]
    fn search_is_accent_insensitive_and_needs_every_word() {
        let lib = sample();
        let find = |text: &str| {
            let q = Query { text: text.into(), ..Query::default() };
            titles(&lib, &lib.view(&q, Column::Artist, false))
        };
        assert_eq!(find("rokkur kor"), ["Hvítur Snjór"]);
        assert_eq!(find("owls bright"), ["Brightwater Steps"]);
        assert_eq!(find("  2009  moon "), ["Halfway Lantern", "Stay Up 2 Late", "Easy As..."]);
        assert_eq!(find("owls moon"), Vec::<String>::new());
        assert_eq!(find("").len(), 7);
    }

    #[test]
    fn browser_narrows_left_to_right() {
        let lib = sample();
        let rock = lib.genres.iter().position(|g| g == "Rock").unwrap() as u32;
        let view = lib.view(&Query { genre: Some(rock), ..Query::default() }, Column::Artist, false);
        // Every genre stays listed; artists and albums shrink to the selection.
        assert_eq!(view.genres.len(), 4);
        assert_eq!(view.artists.len(), 1);
        assert_eq!(lib.artists[view.artists[0].id as usize], "The Night Owls");
        assert_eq!(view.albums, [Facet { id: lib.album_of[0], count: 2 }]);
        assert_eq!(view.rows.len(), 2);
    }

    #[test]
    fn descending_sort_keeps_album_order_inside_ties() {
        let lib = sample();
        let view = lib.view(&Query::default(), Column::Year, true);
        assert_eq!(
            titles(&lib, &view)[..4],
            ["Halfway Lantern", "Stay Up 2 Late", "Easy As...", "Hvítur Snjór"]
        );
    }
}
