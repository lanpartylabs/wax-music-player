//! Painting. Nothing here changes what is selected or playing; the only
//! state touched is the artwork cache, which notes which covers were shown.

use eframe::egui::{Align2, Color32, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, pos2, vec2};
use wax_core::Library;

use super::*;
use crate::draw;
use crate::format::{bytes, clock, count, quality, span};

/// Whose albums a browser shows. It decides the second line under each
/// cover and where the back link of an opened album leads.
#[derive(Clone, Copy)]
enum Scope<'a> {
    /// Every album of a section: tiles name the artist. Carries the section's name.
    Library(&'a str),
    /// One artist's albums: tiles show the year instead.
    Artist(&'a str),
}

/// The parts of the app that painting an album grid or an opened album needs.
struct Shelf<'a> {
    lib: &'a Library,
    info: &'a [AlbumInfo],
    art: &'a mut Art,
    fonts: &'a Fonts,
    playing: Option<u32>,
    /// The smallest a cover is drawn in a grid.
    tile: f32,
    /// Per track: whether it is liked. Empty where songs cannot be liked, and no hearts are drawn.
    liked: &'a [bool],
    pointer: Option<Pos2>,
    /// What an empty grid says.
    empty: &'a str,
}

/// The verified badge across, beside an album's name in a grid and on its page, and the space before it.
const BADGE: f32 = 10.0;
const TITLE_BADGE: f32 = 14.0;
const BADGE_GAP: f32 = 6.0;

const HELP: &[(&str, &str)] = &[
    ("1  2  3", "Albums, Artists, Songs"),
    ("← ↓ ↑ →   h j k l", "move"),
    ("Enter", "open the album, play the song"),
    ("Esc", "close the album, then clear the search"),
    ("/", "search as you type; Enter acts on the top hit"),
    ("Tab", "switch between sidebar and content"),
    ("g  G", "top, bottom"),
    ("Ctrl-d  Ctrl-u", "half a page down, up"),
    ("s  S", "Albums: next, previous order; Songs: next column, reverse"),
    ("f", "filter by genre, kind, sound and decade; sort the albums"),
    ("Space", "pause"),
    ("Shift-↑ ↓   Ctrl-a", "Songs and albums: pick several, pick all; drag them onto a playlist"),
    ("Delete", "Songs: drop what is picked from the library; in a playlist, take it out"),
    ("Shift-L", "like or unlike the song, or the song playing"),
    ("p", "add to a playlist, and more; in the sidebar, rename or delete it"),
    ("Ctrl-n", "new playlist"),
    ("F2", "sidebar: rename the playlist"),
    ("n  N", "next track, previous track"),
    ("x  X", "shuffle songs, albums, artists, or not; the other way round"),
    ("[  ]", "back, forward ten seconds"),
    ("-  =", "volume down, up"),
    ("m", "sound off, on"),
    ("Ctrl-,", "settings"),
    ("?", "show or hide this list"),
    ("Ctrl-q", "quit"),
];

pub(super) fn all(app: &mut App, p: &Painter, l: &Layout, screen: Rect) {
    p.rect_filled(screen, 0.0, theme::BG);
    match app.form() {
        Form::Grid => {
            let focused = app.focus == Focus::Main;
            let (name, playing, empty) = (app.label(app.section).to_owned(), app.playing_here(), app.empty_words("No albums"));
            let App { lib, info, art, fonts, albums, albums_browser, settings, liked, pointer, rest_shown, .. } = app;
            let liked: &[bool] = if *rest_shown { &[] } else { liked };
            let mut shelf = Shelf { lib, info, art, fonts, playing, tile: settings.cover, liked, pointer: *pointer, empty };
            browser(p, l.main, albums_browser, albums, &mut shelf, focused, Scope::Library(&name));
        }
        Form::Artists => artists(app, p, l),
        Form::Table => songs(app, p, l),
        Form::Settings => settings::paint(app, p, l),
    }
    scroll::paint(app, &p.with_clip_rect(l.main), l);
    if l.filters.height() > 0.0 {
        filters::paint(app, p, l.filters);
    }
    sidebar(app, p, l);
    top_bar(app, p, l);
    status(app, p, l);

    let line = Stroke::new(1.0, theme::LINE);
    p.hline(screen.x_range(), l.bar.bottom() + 0.5, line);
    p.hline(screen.x_range(), l.status.top() - 0.5, line);
    p.vline(l.sidebar.right() + 0.5, l.sidebar.y_range(), line);
    // Menus over everything, and songs being dragged over those.
    filters::paint_menu(app, p, l.filters);
    actions::paint(app, p, screen);
    carry::paint(app, p, l);
    if app.help {
        help(app, p, screen);
    }
}

fn top_bar(app: &mut App, p: &Painter, l: &Layout) {
    p.rect_filled(l.bar, 0.0, theme::BAR);
    let mid = l.bar.center().y;

    let play = l.play.center();
    draw::skip(p, l.previous.center(), 9.0, -1.0, theme::MUTED);
    draw::skip(p, l.next.center(), 9.0, 1.0, theme::MUTED);
    // Lit while on, with what it shuffles written under it.
    let shuffling = app.settings.shuffle.word();
    let shuffle = l.shuffle.center();
    draw::shuffle(p, shuffle, if shuffling.is_empty() { theme::MUTED } else { theme::ACCENT });
    draw::text(p, pos2(shuffle.x, shuffle.y + 11.0), Align2::CENTER_TOP, 56.0, shuffling, &app.fonts.label, theme::ACCENT);
    volume::paint(app, p, l.volume);
    if app.playing.is_some() && !app.paused {
        draw::pause_bars(p, play, 14.0, theme::TEXT);
    } else {
        draw::triangle(p, play, 18.0, theme::TEXT);
    }

    if l.display.width() > 0.0 {
        let display = l.display;
        p.rect_filled(display, 6.0, theme::WELL);
        p.rect_stroke(display, 6.0, Stroke::new(1.0, theme::LINE), StrokeKind::Inside);
        // The track playing may belong to the index that is not on screen.
        let here = app.playing_rest == app.rest_shown;
        let (lib, art) = match &mut app.shelved {
            Some(other) if !here => (&other.lib, &mut other.art),
            _ => (&app.lib, &mut app.art),
        };
        match app.playing.map(|id| &lib.tracks[id as usize]) {
            Some(track) => {
                let thumb = Rect::from_min_size(display.min + vec2(5.0, 5.0), vec2(display.height() - 10.0, display.height() - 10.0));
                draw::cover(p, thumb, art.get(cover_id(&track.art)), 3.0);

                let center = (thumb.right() + display.right()) / 2.0;
                let width = display.right() - thumb.right() - 24.0;
                // The second line says what the player is doing when that is not simply playing.
                let status = (app.status.serial == app.serial).then_some(&app.status);
                let (under, ink) = match status.map(|status| &status.state) {
                    Some(player::State::Fetching(got, of)) if *of > 0 => (format!("Fetching  {}%", got * 100 / of), theme::MUTED),
                    Some(player::State::Fetching(..)) => ("Fetching".to_owned(), theme::MUTED),
                    Some(player::State::Failed(why)) => (why.clone(), theme::WARN),
                    _ => match (track.artist.is_empty(), track.album.is_empty()) {
                        (false, false) => (format!("{} — {}", track.artist, track.album), theme::MUTED),
                        _ => (format!("{}{}", track.artist, track.album), theme::MUTED),
                    },
                };
                let position = status.map_or(0, |status| status.position.as_millis() as u32);
                let times = format!("{} / {}", if position < 1000 { "0:00".to_owned() } else { clock(position) }, clock(track.dur_ms));
                let timed = draw::text(p, pos2(display.right() - 12.0, display.top() + 7.0), Align2::RIGHT_TOP, 96.0, &times, &app.fonts.small, theme::FAINT);
                draw::text(p, pos2(center, display.top() + 6.0), Align2::CENTER_TOP, width - 2.0 * timed, &track.title, &app.fonts.strong, theme::TEXT);
                draw::text(p, pos2(center, display.top() + 24.0), Align2::CENTER_TOP, width - 64.0, &under, &app.fonts.small, ink);
                // The song playing can be liked from here, unless it is of the unfinished part.
                if !app.playing_rest {
                    let loved = app.store.is_liked(track);
                    draw::heart(p, actions::display_heart(display).center(), 13.0, if loved { theme::LOVE } else { theme::FAINT }, loved);
                }
                // Progress runs along the bottom edge.
                let track_line = progress_line(display);
                p.rect_filled(track_line, 1.0, theme::LINE);
                if track.dur_ms > 0 {
                    let share = (position as f32 / track.dur_ms as f32).clamp(0.0, 1.0);
                    let played = Rect::from_min_max(track_line.min, pos2(track_line.left() + share * track_line.width(), track_line.bottom()));
                    p.rect_filled(played, 1.0, theme::ACCENT);
                }
            }
            None => draw::note(p, display.center(), 9.0, theme::FAINT),
        }
    }

    let field = l.search;
    let edge = if app.searching { Stroke::new(1.5, theme::ACCENT) } else { Stroke::new(1.0, theme::LINE) };
    p.rect_filled(field, 14.0, theme::WELL);
    p.rect_stroke(field, 14.0, edge, StrokeKind::Inside);
    draw::magnifier(p, pos2(field.left() + 15.0, mid), theme::MUTED);
    let left = field.left() + 30.0;
    let width = field.right() - 14.0 - left;
    if app.search.is_empty() && !app.searching {
        draw::text(p, pos2(left, mid), Align2::LEFT_CENTER, width, "Search", &app.fonts.body, theme::FAINT);
    } else {
        draw::text(p, pos2(left, mid), Align2::LEFT_CENTER, width, &app.search, &app.fonts.body, theme::TEXT);
        if app.searching {
            let x = left + draw::width(p, &app.search, &app.fonts.body).min(width) + 1.0;
            p.rect_filled(Rect::from_min_max(pos2(x, mid - 8.0), pos2(x + 1.5, mid + 8.0)), 0.0, theme::ACCENT);
        }
    }
}

/// The line along the foot of the now-playing display that fills as the track plays.
pub(super) fn progress_line(display: Rect) -> Rect {
    let left = display.left() + display.height() + 5.0;
    Rect::from_min_max(pos2(left, display.bottom() - 4.0), pos2(display.right() - 10.0, display.bottom() - 2.0))
}

fn sidebar(app: &App, p: &Painter, l: &Layout) {
    p.rect_filled(l.sidebar, 0.0, theme::SIDEBAR);
    let heading = pos2(l.sidebar.left() + 18.0, l.sidebar.top() + 16.0);
    draw::text(p, heading, Align2::LEFT_TOP, SIDEBAR_WIDTH - 36.0, "LIBRARY", &app.fonts.label, theme::FAINT);

    for &(at, heading) in &l.headings {
        draw::text(p, at, Align2::LEFT_TOP, SIDEBAR_WIDTH - 36.0, heading, &app.fonts.label, theme::FAINT);
    }
    for (i, section) in app.sections().into_iter().enumerate() {
        let item = l.items[i];
        let selected = app.section == section;
        let active = selected && app.focus == Focus::Sidebar;
        let mid = item.center().y;
        // A playlist being named shows the name typed so far, in a field.
        let named = |naming: &&actions::Naming| matches!((naming.what, section), (actions::Named::Playlist(id), Section::Playlist(this)) if id == this);
        if let Some(naming) = app.renaming.as_ref().filter(named) {
            let field = item.shrink2(vec2(0.0, 1.0));
            p.rect_filled(field, 6.0, theme::WELL);
            p.rect_stroke(field, 6.0, Stroke::new(1.5, theme::ACCENT), StrokeKind::Inside);
            section_icon(p, section, pos2(item.left() + 19.0, mid), theme::MUTED);
            let left = item.left() + 38.0;
            let room = item.right() - 12.0 - left;
            // An empty name shows what the playlist is called now, faintly; a selected one sits on the selection's tint.
            let (shown, ink) = if naming.name.is_empty() { (app.label(section), theme::FAINT) } else { (naming.name.as_str(), theme::TEXT) };
            if naming.selected {
                let width = draw::width(p, shown, &app.fonts.body).min(room);
                p.rect_filled(Rect::from_min_max(pos2(left - 2.0, mid - 10.0), pos2(left + width + 2.0, mid + 10.0)), 3.0, theme::PICKED);
            }
            let used = draw::text(p, pos2(left, mid), Align2::LEFT_CENTER, room, shown, &app.fonts.body, ink);
            if !naming.selected {
                let x = left + if naming.name.is_empty() { 0.0 } else { used.min(room) } + 1.0;
                p.rect_filled(Rect::from_min_max(pos2(x, mid - 8.0), pos2(x + 1.5, mid + 8.0)), 0.0, theme::ACCENT);
            }
            continue;
        }
        if selected {
            p.rect_filled(item, 6.0, if active { theme::ACCENT } else { theme::IDLE });
        }
        let (ink, icon) = if active { (theme::ON_ACCENT, theme::ON_ACCENT) } else { (theme::TEXT, theme::MUTED) };
        section_icon(p, section, pos2(item.left() + 19.0, mid), icon);
        // An unfinished state says how many files are in it, a playlist how many songs.
        let files = match section {
            Section::Rest(id) => app.rest.iter().find(|state| state.id == id).map_or(String::new(), |state| count(state.tracks as usize)),
            Section::Liked => app.counts.get(&0).map_or(String::new(), |&n| count(n)),
            Section::Playlist(id) => app.counts.get(&id).map_or(String::new(), |&n| count(n)),
            _ => String::new(),
        };
        let counted = draw::text(p, pos2(item.right() - 10.0, mid), Align2::RIGHT_CENTER, 60.0, &files, &app.fonts.small, if active { ink } else { theme::FAINT });
        let label_ink = if section == Section::NewPlaylist && !active { theme::MUTED } else { ink };
        draw::text(p, pos2(item.left() + 38.0, mid), Align2::LEFT_CENTER, item.width() - 54.0 - counted, app.label(section), &app.fonts.body, label_ink);
    }
}

fn section_icon(p: &Painter, section: Section, center: Pos2, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    match section {
        // A record sleeve.
        Section::Albums => {
            p.rect_stroke(Rect::from_center_size(center, vec2(13.0, 13.0)), 2.5, stroke, StrokeKind::Inside);
            p.circle_filled(center, 2.0, color);
        }
        // Head and shoulders.
        Section::Artists => {
            p.circle_filled(pos2(center.x, center.y - 3.5), 3.0, color);
            p.rect_filled(Rect::from_center_size(pos2(center.x, center.y + 4.5), vec2(12.0, 5.0)), 2.5, color);
        }
        Section::Songs => draw::note(p, center, 5.5, color),
        Section::Liked => draw::heart(p, center, 13.0, color, false),
        // Three lines, a list.
        Section::Playlist(_) => {
            for dy in [-4.0, 0.0, 4.0] {
                p.line_segment([pos2(center.x - 6.0, center.y + dy), pos2(center.x + 6.0, center.y + dy)], stroke);
            }
        }
        // A plus.
        Section::NewPlaylist => {
            p.line_segment([pos2(center.x - 6.0, center.y), pos2(center.x + 6.0, center.y)], stroke);
            p.line_segment([pos2(center.x, center.y - 6.0), pos2(center.x, center.y + 6.0)], stroke);
        }
        // A ring, not yet filled in.
        Section::Rest(_) => {
            p.circle_stroke(center, 5.0, stroke);
        }
        // Two sliders.
        Section::Settings => {
            for (dy, knob) in [(-3.5, 3.0), (3.5, -3.0)] {
                p.line_segment([pos2(center.x - 6.5, center.y + dy), pos2(center.x + 6.5, center.y + dy)], stroke);
                p.circle_filled(pos2(center.x + knob, center.y + dy), 2.6, color);
            }
        }
    }
}

fn status(app: &App, p: &Painter, l: &Layout) {
    p.rect_filled(l.status, 0.0, theme::BAR);
    let songs = app.result.rows.len();
    let plural = |n: usize, word: &str| format!("{} {word}{}", count(n), if n == 1 { "" } else { "s" });
    let leading = match app.form() {
        Form::Grid => plural(app.albums.len(), "album") + "  ·  ",
        Form::Artists => plural(app.result.artists.len(), "artist") + "  ·  ",
        Form::Table | Form::Settings => String::new(),
    };
    let summary = format!(
        "{leading}{}  ·  {}  ·  {}",
        plural(songs, "song"),
        span(app.result.total_ms),
        bytes(app.result.total_bytes),
    );
    // What was just done is said for a few seconds, in place of the totals; then how many songs are picked, if several are.
    let said = app.notice.as_ref().filter(|(_, since)| since.elapsed() < Duration::from_secs(8)).map(|(text, _)| text.clone());
    let picked = (app.picked.len() > 1).then(|| {
        let delete = if app.listing().is_some() { "Delete takes them out of this playlist" } else { "Delete drops them from the library" };
        format!("{} songs picked  ·  drag them onto a playlist, or right-click for more  ·  {delete}", count(app.picked.len()))
    });
    let scanning = app.scanning.as_ref().map(settings::scanning_words);
    let (line, ink) = match (said, picked, scanning) {
        (Some(said), ..) => (said, theme::TEXT),
        (None, Some(picked), _) => (picked, theme::TEXT),
        (None, None, Some(scanning)) => (scanning, theme::TEXT),
        (None, None, None) => (summary, theme::MUTED),
    };
    draw::text(p, l.status.center(), Align2::CENTER_CENTER, l.status.width() - 220.0, &line, &app.fonts.small, ink);

    let took = format!("{:.2} ms", app.query_time.as_secs_f64() * 1000.0);
    draw::text(p, pos2(l.status.right() - 12.0, l.status.center().y), Align2::RIGHT_CENTER, 90.0, &took, &app.fonts.small, theme::FAINT);
}

fn browser(p: &Painter, area: Rect, browser: &Browser, albums: &[u32], shelf: &mut Shelf, focused: bool, scope: Scope) {
    let p = &p.with_clip_rect(area);
    match &browser.detail {
        Some(detail) => {
            let (Scope::Library(back) | Scope::Artist(back)) = scope;
            album_detail(p, area, detail, shelf, focused, back);
        }
        None => album_grid(p, area, &browser.grid, albums, shelf, focused, scope),
    }
}

fn album_grid(p: &Painter, area: Rect, grid: &Grid, albums: &[u32], shelf: &mut Shelf, focused: bool, scope: Scope) {
    if albums.is_empty() {
        draw::text(p, area.center(), Align2::CENTER_CENTER, area.width(), shelf.empty, &shelf.fonts.body, theme::FAINT);
        return;
    }
    let cells = Cells::new(area, shelf.tile);
    for index in cells.shown(grid.scroll, albums.len()) {
        let id = albums[index] as usize;
        let (album, info) = (&shelf.lib.albums[id], &shelf.info[id]);
        let cover = cells.cover(index, grid.scroll);
        if index == grid.cursor {
            let ring = Stroke::new(3.0, if focused { theme::ACCENT } else { theme::IDLE });
            p.rect_stroke(cover.expand(5.0), 9.0, ring, StrokeKind::Inside);
        }
        draw::cover(p, cover, shelf.art.get(info.art), 5.0);

        let title = if album.name.is_empty() { "Unknown Album" } else { &album.name };
        let room = if info.verified { cells.tile - BADGE - BADGE_GAP } else { cells.tile };
        let used = draw::text(p, pos2(cover.left(), cover.bottom() + 7.0), Align2::LEFT_TOP, room, title, &shelf.fonts.body, theme::TEXT);
        if info.verified {
            let middle = cover.bottom() + 7.0 + draw::cap_middle(p, &shelf.fonts.body);
            draw::verified(p, pos2(cover.left() + used + BADGE_GAP + BADGE / 2.0, middle), BADGE, theme::FAINT);
        }
        let under = pos2(cover.left(), cover.bottom() + 25.0);
        let mut caption = match scope {
            Scope::Library(_) => artist_name(shelf.lib, album.artist).to_owned(),
            Scope::Artist(_) if info.year > 0 => info.year.to_string(),
            Scope::Artist(_) => String::new(),
        };
        // A single or an EP says so, as it often bears the name of the album it comes from.
        if let Some(kind) = small_release(shelf.lib, info) {
            caption = if caption.is_empty() { kind.to_owned() } else { format!("{caption} · {kind}") };
        }
        draw::text(p, under, Align2::LEFT_TOP, cells.tile, &caption, &shelf.fonts.small, theme::MUTED);
    }
}

/// "Single" or "EP" for an album that is one; None for anything else.
fn small_release<'a>(lib: &'a Library, info: &AlbumInfo) -> Option<&'a str> {
    lib.kinds.get(info.kind as usize).map(String::as_str).filter(|kind| matches!(*kind, "Single" | "EP"))
}

fn album_detail(p: &Painter, area: Rect, detail: &Detail, shelf: &mut Shelf, focused: bool, back: &str) {
    let rects = detail_rects(area);
    let (album, info) = (&shelf.lib.albums[detail.album as usize], &shelf.info[detail.album as usize]);
    let artist = artist_name(shelf.lib, album.artist);
    let fonts = shelf.fonts;

    draw::text(p, rects.back.left_center(), Align2::LEFT_CENTER, rects.back.width(), &format!("‹  {back}"), &fonts.body, theme::ACCENT);
    draw::cover(p, rects.cover, shelf.art.get(info.art), 6.0);

    let (left, top, width) = (rects.heading.left(), rects.heading.top(), rects.heading.width());
    let title = if album.name.is_empty() { "Unknown Album" } else { &album.name };
    let room = if info.verified { width - TITLE_BADGE - BADGE_GAP } else { width };
    let used = draw::text(p, pos2(left, top), Align2::LEFT_TOP, room, title, &fonts.title, theme::TEXT);
    if info.verified {
        draw::verified(p, pos2(left + used + BADGE_GAP + TITLE_BADGE / 2.0, top + draw::cap_middle(p, &fonts.title)), TITLE_BADGE, theme::FAINT);
    }
    draw::text(p, pos2(left, top + 33.0), Align2::LEFT_TOP, width, artist, &fonts.body, theme::ACCENT);
    let (genre, subgenre) = detail.tracks.first().map_or(("", ""), |&t| {
        let track = &shelf.lib.tracks[t as usize];
        (track.genre.as_str(), track.subgenre.as_str())
    });
    let mut facts = Vec::new();
    if !genre.is_empty() {
        facts.push(if subgenre.is_empty() { genre.to_owned() } else { format!("{genre} / {subgenre}") });
    }
    if info.year > 0 {
        facts.push(info.year.to_string());
    }
    if let Some(kind) = small_release(shelf.lib, info) {
        facts.push(kind.to_owned());
    }
    // Counted from the tracks listed, which in an unfinished state are not always the whole album; those it lacks are listed but not held.
    let tracks = detail.tracks.iter().map(|&t| &shelf.lib.tracks[t as usize]).filter(|t| !t.missing);
    let (held, whole) = (tracks.clone().count() as u32, tracks.clone().map(|t| t.track_total).max().unwrap_or(0));
    let songs = if held == 1 && whole <= 1 { "song" } else { "songs" };
    facts.push(if whole > held { format!("{held} of {whole} {songs}") } else { format!("{held} {songs}") });
    facts.push(span(tracks.clone().map(|t| t.dur_ms as u64).sum()));
    draw::text(p, pos2(left, top + 56.0), Align2::LEFT_TOP, width, &facts.join("  ·  "), &fonts.small, theme::MUTED);
    // Why the album is where it is: the reason most of its unfinished songs give, or what keeps it from being verified.
    let mut reasons: HashMap<&str, usize> = HashMap::new();
    for track in tracks.clone().filter(|t| !t.state.is_empty() && !t.reason.is_empty()) {
        *reasons.entry(track.reason.as_str()).or_default() += 1;
    }
    let unconfirmed = tracks.filter(|t| t.unconfirmed).count();
    let why = match reasons.into_iter().max_by_key(|&(reason, n)| (n, std::cmp::Reverse(reason))) {
        Some((reason, _)) => format!("Why: {reason}"),
        None if unconfirmed > 0 => format!("Unverified: {unconfirmed} of its songs placed by their name or length alone"),
        None => String::new(),
    };
    draw::text(p, pos2(left, top + 78.0), Align2::LEFT_TOP, width, &why, &fonts.small, theme::FAINT);

    p.hline(rects.rows.x_range(), rects.rows.top() - 0.5, Stroke::new(1.0, theme::LINE));
    for row in detail.list.shown(detail.lines.len()) {
        let rect = row_rect(rects.rows, row - detail.list.top, TRACK_ROW);
        let mid = rect.center().y;
        let id = match &detail.lines[row] {
            Line::Track(id) => *id,
            Line::Disc(name) => {
                // Set low in its row, nearer the tracks it heads than those above.
                draw::text(p, pos2(rect.left() + 42.0, rect.bottom() - 6.0), Align2::LEFT_BOTTOM, rect.width() - 54.0, name, &fonts.small, theme::MUTED);
                continue;
            }
        };
        let track = &shelf.lib.tracks[id as usize];
        let selected = row == detail.list.cursor;
        let playing = shelf.playing == Some(id);
        let lit = selected && focused;
        if selected {
            p.rect_filled(rect, 5.0, if focused { theme::ACCENT } else { theme::IDLE });
        } else if detail.picked.contains(&id) {
            p.rect_filled(rect, 5.0, theme::PICKED);
        }
        let (ink, soft, mark) = if lit {
            (theme::ON_ACCENT, theme::ON_ACCENT, theme::ON_ACCENT)
        } else if track.missing {
            (theme::FAINT, theme::FAINT, theme::ACCENT)
        } else {
            (if playing { theme::ACCENT } else { theme::TEXT }, theme::MUTED, theme::ACCENT)
        };

        if playing {
            draw::triangle(p, pos2(rect.left() + 20.0, mid), 9.0, mark);
        } else if track.track > 0 {
            draw::text(p, pos2(rect.left() + 28.0, mid), Align2::RIGHT_CENTER, 26.0, &track.track.to_string(), &fonts.small, if lit { soft } else { theme::FAINT });
        }
        draw::text(p, pos2(rect.right() - 12.0, mid), Align2::RIGHT_CENTER, 56.0, &clock(track.dur_ms), &fonts.small, soft);
        if !track.missing && !shelf.liked.is_empty() {
            let shown = shelf.liked[id as usize] || selected || shelf.pointer.is_some_and(|at| rect.contains(at));
            if shown {
                heart(p, actions::track_heart(rect).center(), shelf.liked[id as usize], lit);
            }
        }

        let title_left = rect.left() + 42.0;
        let room = rect.right() - 96.0 - title_left;
        let used = draw::text(p, pos2(title_left, mid), Align2::LEFT_CENTER, room, &track.title, &fonts.body, ink);
        if track.missing {
            draw::text(p, pos2(title_left + used + 12.0, mid), Align2::LEFT_CENTER, room - used - 12.0, "missing", &fonts.small, soft);
        } else if track.unconfirmed {
            draw::text(p, pos2(title_left + used + 12.0, mid), Align2::LEFT_CENTER, room - used - 12.0, &track.reason, &fonts.small, if lit { soft } else { theme::FAINT });
        } else if !track.artist.is_empty() && !track.artist.eq_ignore_ascii_case(artist) {
            // On compilations each track names its own artist.
            draw::text(p, pos2(title_left + used + 12.0, mid), Align2::LEFT_CENTER, room - used - 12.0, &track.artist, &fonts.small, soft);
        }
    }
}

fn artists(app: &mut App, p: &Painter, l: &Layout) {
    let (list_rect, pane) = artists_rects(l.main);
    let (header, body) = pane_rects(pane);
    let in_main = app.focus == Focus::Main;
    let playing = app.playing_here();
    let App { lib, info, art, fonts, result, artist_list, artist_albums, artist_browser, artist_in_grid, settings, liked, pointer, rest_shown, .. } = app;
    let liked: &[bool] = if *rest_shown { &[] } else { liked };

    {
        let p = &p.with_clip_rect(list_rect);
        for row in artist_list.shown(result.artists.len()) {
            let rect = row_rect(list_rect, row - artist_list.top, ARTIST_ROW).shrink2(vec2(8.0, 1.0));
            let selected = row == artist_list.cursor;
            let lit = selected && in_main && !*artist_in_grid;
            if selected {
                p.rect_filled(rect, 5.0, if lit { theme::ACCENT } else { theme::IDLE });
            }
            let name = artist_name(lib, result.artists[row].id);
            let ink = if lit { theme::ON_ACCENT } else { theme::TEXT };
            draw::text(p, pos2(rect.left() + 10.0, rect.center().y), Align2::LEFT_CENTER, rect.width() - 20.0, name, &fonts.body, ink);
        }
    }
    p.vline(list_rect.right() + 0.5, l.main.y_range(), Stroke::new(1.0, theme::LINE));

    let Some(artist) = result.artists.get(artist_list.cursor) else {
        draw::text(p, pane.center(), Align2::CENTER_CENTER, pane.width(), "No artists", &fonts.body, theme::FAINT);
        return;
    };
    let name = artist_name(lib, artist.id);
    let left = header.left() + 26.0;
    let width = header.width() - 52.0;
    draw::text(p, pos2(left, header.top() + 18.0), Align2::LEFT_TOP, width, name, &fonts.title, theme::TEXT);
    let facts = format!(
        "{} {}  ·  {} {}",
        count(artist_albums.len()),
        if artist_albums.len() == 1 { "album" } else { "albums" },
        count(artist.count as usize),
        if artist.count == 1 { "song" } else { "songs" },
    );
    draw::text(p, pos2(left, header.top() + 48.0), Align2::LEFT_TOP, width, &facts, &fonts.small, theme::MUTED);

    let mut shelf = Shelf { lib, info, art, fonts, playing, tile: settings.cover, liked, pointer: *pointer, empty: "No albums" };
    browser(p, body, artist_browser, artist_albums, &mut shelf, in_main && *artist_in_grid, Scope::Artist(name));
}

fn songs(app: &App, p: &Painter, l: &Layout) {
    let (head, rows) = songs_rects(l.main);
    let columns = song_columns(head, matches!(app.section, Section::Rest(_)));
    let focused = app.focus == Focus::Main;
    let fonts = &app.fonts;

    // A playlist keeps its own order: no column is sorted.
    let listing = app.listing();
    for spec in &columns {
        let sorted = spec.column == app.sort && listing.is_none();
        let ink = if sorted { theme::TEXT } else { theme::MUTED };
        let (pos, anchor) = cell_anchor(spec, head.center().y);
        // The arrow sits after a left-aligned label and before a right-aligned one.
        let arrow_room = if sorted { 14.0 } else { 0.0 };
        let shift = if spec.align_right { -arrow_room } else { 0.0 };
        let used = draw::text(p, pos2(pos.x + shift, pos.y), anchor, spec.right - spec.left - arrow_room, spec.label, &fonts.label, ink);
        if sorted {
            let x = if spec.align_right { spec.right - 5.0 } else { spec.left + used + 9.0 };
            sort_arrow(p, pos2(x, head.center().y), app.descending, theme::TEXT);
        }
    }
    p.hline(head.x_range(), head.bottom() + 0.5, Stroke::new(1.0, theme::LINE));

    let p = &p.with_clip_rect(rows);
    let len = app.result.rows.len();
    if len == 0 {
        let empty = match listing {
            _ if !app.search.is_empty() || app.filters != Filters::default() => "No songs",
            Some(actions::Listing::Liked) => "No liked songs yet: click a song's heart, or press Shift-L on it",
            Some(actions::Listing::Playlist(_)) => "Nothing in this playlist yet: right-click songs, or press p on them, to add them here",
            None => app.empty_words("No songs"),
        };
        draw::text(p, rows.center(), Align2::CENTER_CENTER, rows.width(), empty, &fonts.body, theme::FAINT);
    }
    for row in app.songs.shown(len) {
        let id = app.result.rows[row];
        let track = &app.lib.tracks[id as usize];
        let rect = row_rect(rows, row - app.songs.top, SONG_ROW);
        let mid = rect.center().y;
        let selected = row == app.songs.cursor;
        let playing = app.playing_here() == Some(id);
        let lit = selected && focused;
        if selected {
            p.rect_filled(rect, 0.0, if focused { theme::ACCENT } else { theme::IDLE });
        } else if app.picked.contains(&id) {
            p.rect_filled(rect, 0.0, theme::PICKED);
        } else if row % 2 == 1 {
            p.rect_filled(rect, 0.0, theme::STRIPE);
        }
        if playing {
            draw::triangle(p, pos2(rect.left() + 18.0, mid), 8.0, if lit { theme::ON_ACCENT } else { theme::ACCENT });
        }
        // Liked songs show their heart; any other shows an empty one under the pointer or the cursor.
        if let (false, false, Some(time)) = (app.rest_shown, track.missing, columns.last()) {
            let loved = app.liked[id as usize];
            if loved || selected || app.pointer.is_some_and(|at| rect.contains(at)) {
                heart(p, actions::table_heart(time, rect).center(), loved, lit);
            }
        }
        for spec in &columns {
            let number = |n: u32| if n == 0 { String::new() } else { n.to_string() };
            let (value, ink) = match spec.column {
                Column::Title => (track.title.clone(), if playing { theme::ACCENT } else { theme::TEXT }),
                Column::Artist => (track.artist.clone(), theme::MUTED),
                Column::Album => (track.album.clone(), theme::MUTED),
                Column::Genre => (track.genre.clone(), theme::MUTED),
                Column::Year => (number(track.year), theme::MUTED),
                Column::Time => (clock(track.dur_ms), theme::MUTED),
                Column::Quality => (quality(&track.ext(), track.lossless(), track.bitrate), theme::MUTED),
                Column::Why => (track.reason.clone(), theme::MUTED),
                Column::Track | Column::Added => continue,
            };
            // A track the collection lacks, listed with its album: greyed, its quality saying so.
            let (value, ink) = match spec.column {
                _ if !track.missing => (value, ink),
                Column::Quality => ("Missing".to_owned(), theme::FAINT),
                _ => (value, theme::FAINT),
            };
            let (pos, anchor) = cell_anchor(spec, mid);
            draw::text(p, pos, anchor, spec.right - spec.left, &value, &fonts.body, if lit { theme::ON_ACCENT } else { ink });
        }
    }
}

/// Where text of a column is anchored on the row centred at `y`.
fn cell_anchor(spec: &ColumnSpec, y: f32) -> (Pos2, Align2) {
    if spec.align_right { (pos2(spec.right, y), Align2::RIGHT_CENTER) } else { (pos2(spec.left, y), Align2::LEFT_CENTER) }
}

fn sort_arrow(p: &Painter, center: Pos2, descending: bool, color: Color32) {
    let tip = if descending { 3.0 } else { -3.0 };
    let points = vec![pos2(center.x - 4.0, center.y - tip), pos2(center.x + 4.0, center.y - tip), pos2(center.x, center.y + tip)];
    p.add(Shape::convex_polygon(points, color, Stroke::NONE));
}

fn help(app: &App, p: &Painter, screen: Rect) {
    const ROW: f32 = 24.0;
    let fonts = &app.fonts;
    let keys_width = HELP.iter().map(|(keys, _)| draw::width(p, keys, &fonts.strong)).fold(0.0, f32::max);
    let what_width = HELP.iter().map(|(_, what)| draw::width(p, what, &fonts.body)).fold(0.0, f32::max);
    let size = vec2(keys_width + what_width + 76.0, HELP.len() as f32 * ROW + 40.0);
    let panel = Rect::from_center_size(screen.center(), size);
    p.rect_filled(panel.translate(vec2(0.0, 3.0)).expand(2.0), 12.0, Color32::from_black_alpha(26));
    p.rect_filled(panel, 10.0, theme::WELL);
    p.rect_stroke(panel, 10.0, Stroke::new(1.0, theme::LINE), StrokeKind::Inside);

    let body = panel.shrink2(vec2(24.0, 20.0));
    for (i, (keys, what)) in HELP.iter().enumerate() {
        let mid = body.top() + (i as f32 + 0.5) * ROW;
        draw::text(p, pos2(body.left(), mid), Align2::LEFT_CENTER, keys_width + 4.0, keys, &fonts.strong, theme::TEXT);
        draw::text(p, pos2(body.left() + keys_width + 28.0, mid), Align2::LEFT_CENTER, what_width + 4.0, what, &fonts.body, theme::MUTED);
    }
}

fn artist_name(lib: &Library, id: u32) -> &str {
    let name = &lib.artists[id as usize];
    if name.is_empty() { "Unknown Artist" } else { name }
}

/// A song's heart in a list: filled when it is liked, an outline otherwise; white on a lit row.
fn heart(p: &Painter, center: Pos2, liked: bool, lit: bool) {
    let color = match (lit, liked) {
        (true, _) => theme::ON_ACCENT,
        (false, true) => theme::LOVE,
        (false, false) => theme::FAINT,
    };
    draw::heart(p, center, 12.0, color, liked);
}
