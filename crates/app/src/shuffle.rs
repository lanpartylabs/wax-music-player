//! Shuffle: a list in a random order, with nothing weighing on it. No song is favoured for being new, liked or often
//! played, and no order is likelier than another.
//!
//! A list is dealt like a deck of cards: every song in it plays once before any comes round again. The deck is kept
//! between runs. wax remembers when each list's round began, and a song played since then, from that list or any
//! other, waits until the round is over; once everything has had its turn, a new round begins.
//!
//! It shuffles songs, one at a time, never the same artist twice running where the list allows it; albums, each
//! played through; or artists, all of one artist's songs in a random order before the next artist's.

use std::collections::HashMap;
use std::hash::{BuildHasher, DefaultHasher, Hash, Hasher};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use wax_core::Library;
use wax_core::library::fold;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Off,
    Songs,
    Albums,
    Artists,
}

impl Mode {
    const ALL: [Mode; 4] = [Mode::Off, Mode::Songs, Mode::Albums, Mode::Artists];

    /// The mode `steps` along, going round.
    pub fn step(self, steps: isize) -> Mode {
        let at = Self::ALL.iter().position(|&mode| mode == self).unwrap_or(0) as isize;
        Self::ALL[(at + steps).rem_euclid(Self::ALL.len() as isize) as usize]
    }

    /// What it shuffles, in a word; empty when off.
    pub fn word(self) -> &'static str {
        match self {
            Mode::Off => "",
            Mode::Songs => "songs",
            Mode::Albums => "albums",
            Mode::Artists => "artists",
        }
    }
}

/// Random numbers: SplitMix64, seeded from the system's randomness. Plenty for putting songs in order; not for secrets.
pub struct Dice(u64);

impl Dice {
    pub fn new() -> Self {
        // The standard library draws its hash keys from the operating system's randomness.
        let seed = std::collections::hash_map::RandomState::new().build_hasher().finish();
        Dice(seed ^ crate::history::now() as u64)
    }

    #[cfg(test)]
    pub fn seeded(seed: u64) -> Self {
        Dice(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number from 0 up to, not including, `n`, each as likely as the next.
    fn below(&mut self, n: usize) -> usize {
        // Multiply and shift (Lemire): for a list of a million songs one number is likelier than another by less
        // than one part in ten million million.
        ((u128::from(self.next()) * n as u128) >> 64) as usize
    }

    /// Fisher–Yates: every order of `items` as likely as any other.
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            items.swap(i, self.below(i + 1));
        }
    }
}

/// When each list's round began, kept in `shuffle.json` in wax's state folder. A list is known by a name the player
/// gives it, as "songs:library?jazz||||": what is shuffled, and what it is shuffled from.
pub struct Rounds {
    file: PathBuf,
    lists: HashMap<String, Round>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
struct Round {
    /// Unix milliseconds.
    began: i64,
    /// When the list was last dealt from: those dealt from longest ago are forgotten first.
    used: i64,
}

/// How many lists' rounds are kept; every search shuffled from is a list of its own.
const KEPT: usize = 200;

impl Rounds {
    pub fn load() -> Self {
        Self::at(crate::state::file("shuffle.json"))
    }

    /// The rounds kept in `file`; none when it is missing or cannot be read, which only means each list starts afresh.
    pub fn at(file: PathBuf) -> Self {
        let lists = std::fs::read(&file).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        Rounds { file, lists }
    }

    /// When the list's round began; a list never dealt from before begins one at `now`.
    pub fn began(&mut self, list: &str, now: i64) -> i64 {
        let round = self.lists.entry(list.to_owned()).or_insert(Round { began: now, used: now });
        round.used = round.used.max(now);
        round.began
    }

    /// Begins a new round of the list at `now`.
    pub fn renew(&mut self, list: &str, now: i64) {
        self.lists.insert(list.to_owned(), Round { began: now, used: now });
    }

    /// Writes them down, through a temporary file, forgetting the lists dealt from longest ago beyond the number kept.
    pub fn save(&mut self) -> Result<(), String> {
        if self.lists.len() > KEPT {
            let mut used: Vec<i64> = self.lists.values().map(|round| round.used).collect();
            used.sort_unstable_by(|a, b| b.cmp(a));
            let last_kept = used[KEPT - 1];
            self.lists.retain(|_, round| round.used >= last_kept);
        }
        let written = (|| -> std::io::Result<()> {
            if let Some(folder) = self.file.parent() {
                std::fs::create_dir_all(folder)?;
            }
            let tmp = self.file.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec(&self.lists).unwrap_or_default())?;
            std::fs::rename(&tmp, &self.file)
        })();
        written.map_err(|e| format!("where each shuffle stands cannot be saved to {}: {e}", self.file.display()))
    }
}

/// A queue's order, as shuffle deals it.
pub struct Dealt {
    /// Starting with `first` when one was given.
    pub order: Vec<u32>,
    /// How many of its songs this round had still to play: `first`, and the rest of its album, aside. None means the
    /// round is over.
    pub fresh: usize,
}

/// Deals `pool`, a list's songs, the way `mode` shuffles, starting with `first` when one is given: a song chosen,
/// or the one playing. Songs `heard` already this round are left out; when albums are shuffled, an album any of
/// whose songs was heard is, but the rest of `first`'s album follows it all the same.
pub fn deal(lib: &Library, mode: Mode, first: Option<u32>, pool: &[u32], heard: impl Fn(u32) -> bool, dice: &mut Dice) -> Dealt {
    let others = pool.iter().copied().filter(|&song| Some(song) != first);
    let mut order: Vec<u32> = first.into_iter().collect();
    let mut fresh = 0;
    match mode {
        Mode::Off => order.extend(others),
        Mode::Songs => {
            let mut songs: Vec<u32> = others.filter(|&song| !heard(song)).collect();
            dice.shuffle(&mut songs);
            fresh = songs.len();
            order.extend(songs);
            let mut artists: Vec<u64> = order.iter().map(|&song| artist(lib, song)).collect();
            spread(&mut order, &mut artists);
        }
        Mode::Albums => {
            let place = |song: &u32| lib.album_place(*song);
            let (mut albums, index) = groups(others, |song| lib.album_of[song as usize]);
            if let Some(first) = first
                && let Some(&own) = index.get(&lib.album_of[first as usize])
            {
                let mut rest = std::mem::take(&mut albums[own]);
                rest.retain(|song| place(song) > place(&first));
                rest.sort_unstable_by_key(place);
                order.extend(rest);
            }
            albums.retain(|songs| !songs.is_empty() && !songs.iter().any(|&song| heard(song)));
            dice.shuffle(&mut albums);
            for mut songs in albums {
                songs.sort_unstable_by_key(place);
                fresh += songs.len();
                order.extend(songs);
            }
        }
        Mode::Artists => {
            let (mut artists, index) = groups(others.filter(|&song| !heard(song)), |song| artist(lib, song));
            if let Some(first) = first
                && let Some(&own) = index.get(&artist(lib, first))
            {
                let mut rest = std::mem::take(&mut artists[own]);
                dice.shuffle(&mut rest);
                fresh += rest.len();
                order.extend(rest);
            }
            artists.retain(|songs| !songs.is_empty());
            dice.shuffle(&mut artists);
            for mut songs in artists {
                dice.shuffle(&mut songs);
                fresh += songs.len();
                order.extend(songs);
            }
        }
    }
    Dealt { order, fresh }
}

/// The songs by `key`, each group in the order the songs came, the groups in the order their first songs came; and
/// where each key's group is.
fn groups<K: Hash + Eq>(songs: impl Iterator<Item = u32>, key: impl Fn(u32) -> K) -> (Vec<Vec<u32>>, HashMap<K, usize>) {
    let (mut groups, mut index) = (Vec::<Vec<u32>>::new(), HashMap::new());
    for song in songs {
        let at = *index.entry(key(song)).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[at].push(song);
    }
    (groups, index)
}

/// Who a song is by, as one number: its own artist, else its album's, written any way that folds to the same.
fn artist(lib: &Library, song: u32) -> u64 {
    let track = &lib.tracks[song as usize];
    let name = if track.artist.is_empty() { &track.album_artist } else { &track.artist };
    let mut hasher = DefaultHasher::new();
    fold(name).hash(&mut hasher);
    hasher.finish()
}

/// Nudges a shuffled order so that no artist plays twice running, wherever the list allows it. Each place takes the
/// next song the dice gave that is by someone other than the song before; the first song stays first, every song still
/// plays once, and the order is otherwise left as the dice made it.
///
/// Taking the next such song is not always enough: an artist with half the songs left must come now or end up beside
/// itself later. So when one artist has more than half of the songs still to place, its next song comes first.
fn spread(order: &mut [u32], artists: &mut [u64]) {
    let n = order.len();
    if n < 3 {
        return; // with the first song staying first, there is nothing to choose
    }
    // How many songs each artist has still to place, and how many artists have each count.
    let mut left: HashMap<u64, usize> = HashMap::new();
    for &artist in &artists[1..] {
        *left.entry(artist).or_default() += 1;
    }
    let mut with = vec![0usize; n];
    for &count in left.values() {
        with[count] += 1;
    }
    let mut most = left.values().copied().max().unwrap_or(0);

    for i in 1..n {
        let (before, remaining) = (artists[i - 1], n - i);
        if left.get(&before) == Some(&remaining) {
            break; // all that is left is by the artist just played
        }
        let next = if most > remaining / 2 {
            (i..n).find(|&j| artists[j] != before && left[&artists[j]] == most)
        } else {
            None
        };
        let take = next.or_else(|| (i..n).find(|&j| artists[j] != before)).unwrap_or(i);
        order[i..=take].rotate_right(1);
        artists[i..=take].rotate_right(1);

        let count = left.get_mut(&artists[i]).expect("counted above");
        with[*count] -= 1;
        *count -= 1;
        with[*count] += 1;
        while most > 0 && with[most] == 0 {
            most -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use wax_core::index::Track;

    use super::*;

    /// A library of `albums` albums by `artists` artists in turn, of `songs` tracks each.
    fn library(artists: usize, albums: usize, songs: u32) -> Library {
        let tracks = (0..albums)
            .flat_map(|album| {
                (1..=songs).map(move |n| Track {
                    path: format!("{album}/{n}.flac"),
                    title: format!("Song {n}"),
                    artist: format!("Artist {}", album % artists),
                    album_artist: format!("Artist {}", album % artists),
                    album: format!("Album {album}"),
                    track: n,
                    ..Track::default()
                })
            })
            .collect();
        Library::new(tracks)
    }

    fn all(lib: &Library) -> Vec<u32> {
        (0..lib.len() as u32).collect()
    }

    #[test]
    fn every_song_comes_once_and_no_order_is_favoured() {
        let lib = library(10, 10, 1);
        let mut dice = Dice::seeded(7);
        let mut first_place = [0u32; 10];
        let deals = 20_000;
        for _ in 0..deals {
            let dealt = deal(&lib, Mode::Songs, None, &all(&lib), |_| false, &mut dice);
            assert_eq!(dealt.order.len(), 10);
            assert_eq!(dealt.order.iter().collect::<HashSet<_>>().len(), 10, "each song once");
            first_place[dealt.order[0] as usize] += 1;
        }
        // Each song leads a tenth of the deals: 2000, give or take what chance alone gives (about 42).
        for (song, &led) in first_place.iter().enumerate() {
            assert!((1800..=2200).contains(&led), "song {song} led {led} of {deals} deals");
        }
    }

    #[test]
    fn an_artist_never_plays_twice_running_while_another_can_come_between() {
        // Three artists, one with half the songs.
        let lib = library(3, 4, 6);
        let mut dice = Dice::seeded(1);
        for _ in 0..200 {
            let dealt = deal(&lib, Mode::Songs, Some(5), &all(&lib), |_| false, &mut dice);
            assert_eq!(dealt.order[0], 5, "the song chosen plays first");
            assert_eq!(dealt.order.iter().collect::<HashSet<_>>().len(), lib.len());
            let by = |song: u32| artist(&lib, song);
            let runs = dealt.order.windows(2).filter(|pair| by(pair[0]) == by(pair[1])).count();
            assert_eq!(runs, 0, "{:?}", dealt.order.iter().map(|&song| &lib.tracks[song as usize].artist).collect::<Vec<_>>());
        }
        // With nobody else left, an artist's songs do follow one another.
        let one = library(1, 1, 4);
        assert_eq!(deal(&one, Mode::Songs, None, &all(&one), |_| false, &mut dice).order.len(), 4);
    }

    #[test]
    fn spreading_leaves_no_artist_beside_itself_whenever_that_can_be_done() {
        let mut dice = Dice::seeded(11);
        let mut checked = 0;
        for _ in 0..5000 {
            let n = 2 + dice.below(30);
            let names = 1 + dice.below(5) as u64;
            // Lopsided: half the songs by one artist, give or take.
            let mut artists: Vec<u64> = (0..n).map(|_| if dice.below(2) == 0 { 0 } else { dice.below(names as usize) as u64 }).collect();
            let mut order: Vec<u32> = (0..n as u32).collect();
            let mut songs: Vec<(u32, u64)> = order.iter().copied().zip(artists.iter().copied()).collect();
            spread(&mut order, &mut artists);

            assert_eq!(order[0], 0, "the first song stays first");
            let mut after: Vec<(u32, u64)> = order.iter().copied().zip(artists.iter().copied()).collect();
            (songs.sort_unstable(), after.sort_unstable());
            assert_eq!(songs, after, "the same songs, each by its own artist");

            // It can be done when no artist has more than half of the songs after the first, the first's artist
            // counting as one more since it has played already.
            let rest = n - 1;
            let mut count: HashMap<u64, usize> = HashMap::new();
            for &artist in &artists[1..] {
                *count.entry(artist).or_default() += 1;
            }
            let can = count.values().all(|&c| c <= rest.div_ceil(2)) && count.get(&artists[0]).copied().unwrap_or(0) <= rest / 2;
            if can {
                checked += 1;
                assert!(artists.windows(2).all(|pair| pair[0] != pair[1]), "{artists:?}");
            }
        }
        assert!(checked > 500, "only {checked} lists could be spread");
    }

    #[test]
    fn albums_play_through_and_the_chosen_songs_album_goes_on_from_it() {
        let lib = library(5, 5, 4);
        let album = |song: u32| lib.album_of[song as usize];
        let mut dice = Dice::seeded(3);
        let first = lib.tracks.iter().position(|t| t.album == "Album 2" && t.track == 2).unwrap() as u32;
        let dealt = deal(&lib, Mode::Albums, Some(first), &all(&lib), |_| false, &mut dice);
        let tracks: Vec<(String, u32)> = dealt.order.iter().map(|&s| (lib.tracks[s as usize].album.clone(), lib.tracks[s as usize].track)).collect();
        assert_eq!(&tracks[..3], &[("Album 2".into(), 2), ("Album 2".into(), 3), ("Album 2".into(), 4)], "the rest of its album");
        assert_eq!(tracks.len(), 3 + 4 * 4, "track 1 of the chosen album is passed, as when it plays in order");
        for chunk in dealt.order[3..].chunks(4) {
            assert!(chunk.iter().all(|&song| album(song) == album(chunk[0])), "an album at a time");
            assert_eq!(chunk.iter().map(|&s| lib.tracks[s as usize].track).collect::<Vec<_>>(), [1, 2, 3, 4], "in its own order");
        }
        assert_eq!(dealt.fresh, 16);
    }

    #[test]
    fn artists_come_one_at_a_time() {
        let lib = library(3, 6, 3);
        let by = |song: u32| artist(&lib, song);
        let dealt = deal(&lib, Mode::Artists, Some(0), &all(&lib), |_| false, &mut Dice::seeded(9));
        assert_eq!(dealt.order.len(), lib.len());
        let changes = dealt.order.windows(2).filter(|pair| by(pair[0]) != by(pair[1])).count();
        assert_eq!(changes, 2, "three artists, so two changes of artist");
        assert_eq!(by(dealt.order[1]), by(0), "the chosen song's artist goes on");
    }

    #[test]
    fn what_was_heard_this_round_waits_and_a_finished_round_says_so() {
        let lib = library(4, 4, 2);
        let mut dice = Dice::seeded(5);
        let heard: HashSet<u32> = [0, 1, 2, 7].into();
        let dealt = deal(&lib, Mode::Songs, None, &all(&lib), |song| heard.contains(&song), &mut dice);
        assert_eq!(dealt.order.iter().copied().collect::<HashSet<_>>(), (0..8).filter(|s| !heard.contains(s)).collect());
        assert_eq!(dealt.fresh, 4);
        // Album 0 (songs 0, 1) and album 1 (2, 3) were heard from, so only albums 2 and 3 are left.
        let albums = deal(&lib, Mode::Albums, None, &all(&lib), |song| heard.contains(&song), &mut dice);
        assert_eq!(albums.order.iter().copied().collect::<HashSet<_>>(), [4, 5].into(), "album 3 was heard from too");
        let over = deal(&lib, Mode::Songs, Some(3), &all(&lib), |_| true, &mut dice);
        assert_eq!((over.order, over.fresh), (vec![3], 0), "nothing new: the round is over");
    }

    #[test]
    fn rounds_are_kept_between_runs() {
        let folder = std::env::temp_dir().join(format!("wax-rounds-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let file = folder.join("shuffle.json");
        let mut rounds = Rounds::at(file.clone());
        assert_eq!(rounds.began("songs:library", 100), 100, "a first shuffle begins a round");
        assert_eq!(rounds.began("songs:library", 500), 100);
        rounds.renew("albums:liked", 300);
        rounds.save().unwrap();
        let mut again = Rounds::at(file);
        assert_eq!(again.began("songs:library", 900), 100);
        assert_eq!(again.began("albums:liked", 900), 300);
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn the_button_goes_round_the_modes() {
        assert_eq!(Mode::Off.step(1), Mode::Songs);
        assert_eq!(Mode::Artists.step(1), Mode::Off);
        assert_eq!(Mode::Off.step(-1), Mode::Artists);
    }
}
