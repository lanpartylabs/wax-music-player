"""Compare what a group of files claims to be with what a release holds.

Nothing here asks the reference anything: it takes catalogued files and a
release already fetched, and says which file is which track and how sure
that is.
"""

import collections
import hashlib
import re

from . import config, reference
from .text import GENERIC_TITLE, LEADING_NUMBER, TRAILING_YEAR, consensus, fold, is_various, lead_artist, plain_title, similar

SURE_SECONDS = 4  # a file and a track this close in length are the same recording, given the name or place also fits
HEARD = 0.8  # fingerprint score from which a file is taken to be that recording
SAME_RIP = 1.5  # two files of the same audio are this close in length
OTHER_TAKE, OTHER_SHARE = 20, 0.05  # a file longer or shorter than a track by this many seconds, and by this share of it, is another take
PRESSING_SHARE = 0.04  # one off by up to this share, with the track's name and place, is another pressing of it
MAX_RECORD = 150  # bigger groups are piles, not records
SCATTERED = 3  # folders, and artists, a compilation's songs must lie in to be put back together as one
FEW_TRACKS = 6  # a release with fewer is a single or an EP: it is what a folder holds only when it is at least half of it
VERDICTS = ("certain", "likely", "unclear")
PART = "\x1f"  # divides a folder from the part of it a unit covers

DISC_FOLDER = re.compile(r"^(cd|disc|disk)[ _.-]*(\d{1,2})$", re.I)
TRACK_IN_NAME = re.compile(r"^\s*(?:(\d)[-.])?(\d{1,3})(?=[\s._)-])")
MBID = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
SHORT_NAME = re.compile(r"[A-Z0-9_$-]{1,8}~[A-Z0-9]{1,3}")  # what a server gives for a name it cannot pass on
VIDEO = ("DVD", "DVD-Video", "Blu-ray", "VHS", "VCD", "SVCD", "LaserDisc", "HD-DVD")


def mbid(value):
    found = MBID.search((value or "").lower())
    return found.group(0) if found else ""


def as_file(row, heard=None):
    """One catalogued file as the matcher sees it: what its tags say, and what its name and folder suggest."""
    folder, _, filename = row["name"].rpartition("/")
    stem = filename.rsplit(".", 1)[0] if "." in filename else filename
    parent, _, last = folder.rpartition("/")
    disc = DISC_FOLDER.match(last)
    if disc and "/" in parent:  # "Album/CD 2/..." belongs to the album folder above it
        folder = parent
    numbered = TRACK_IN_NAME.match(stem)
    seconds = row["duration"] if row["duration"] and not row["doubtful"] else row["fp_duration"] or row["duration"]
    heard = heard or {}
    return {
        "id": row["id"],
        "path": row["name"],
        "dir": folder,
        "stem": stem,
        "title": (row["title"] or "").strip(),
        "artist": (row["artist"] or "").strip(),
        "albumartist": (row["albumartist"] or "").strip(),
        "album": (row["album"] or "").strip(),
        "track": row["track"] or (int(numbered.group(2)) if numbered else 0),
        "disc": row["disc"] or (int(disc.group(2)) if disc else 0) or (int(numbered.group(1)) if numbered and numbered.group(1) else 0),
        "seconds": seconds or 0,
        "mb_recording": mbid(row["mb_recording"]),
        "mb_release": mbid(row["mb_release"]),
        "mb_track": mbid(row["mb_track"]),
        "heard": heard.get("recordings", {}),  # {recording id: fingerprint score}
        "heard_releases": heard.get("releases", []),
        "names": [],  # titles proposed for it from outside, by a model
        "stamp": f"{row['id']}:{row['size']}:{row['mtime']}",
    }


def form_units(files):
    """Group files into what should be looked up together, as (key, shape, files).

    A "record" is a folder, or the part of a folder sharing an album tag, that looks like one release;
    "loose" files are looked up one song at a time. A compilation whose songs lie one or two to a folder side by side,
    as an iPod leaves them in "Artist - Album" folders under one, is put back together: its songs, by their album tag,
    are one record across those folders.
    """
    folders = collections.defaultdict(list)
    for f in files:
        folders[f["dir"]].append(f)
    units = {}
    for folder in sorted(folders):
        members = folders[folder]
        albums = collections.defaultdict(list)
        for f in members:
            albums[fold(f["album"])].append(f)
        untagged = albums.pop("", [])
        if not albums:  # no album tags: the folder itself is the only claim
            units[folder] = ("record" if 3 <= len(members) <= MAX_RECORD else "loose", members)
            continue
        top = max(albums.values(), key=len)
        tagged = len(members) - len(untagged)
        # One album among the files that name one: the folder is that record, and its untagged files belong with it.
        if len(top) >= 0.6 * tagged and 3 <= len(members) <= MAX_RECORD:
            units[folder] = ("record", members)
            continue
        loose = untagged
        for name in sorted(albums):
            if 3 <= len(albums[name]) <= MAX_RECORD:
                units[folder + PART + name] = ("record", albums[name])
            else:
                loose = loose + albums[name]
        if loose:
            units[folder + PART] = ("loose", loose)

    # Folders of loose songs and nothing else, side by side: their songs regrouped by album tag across the folders.
    scattered, per_folder = collections.defaultdict(list), collections.Counter(key.split(PART)[0] for key in units)
    for key, (shape, members) in units.items():
        folder = key.split(PART)[0]
        if shape == "loose" and per_folder[folder] == 1 and "/" in folder:
            for f in members:
                if fold(f["album"]) and not GENERIC_TITLE.search(fold(f["album"])):
                    scattered[(folder.rpartition("/")[0], fold(f["album"]))].append(f)
    for (parent, album), members in sorted(scattered.items()):
        # A compilation, as an iPod lays one out: songs by several artists, each in a folder of its artist's. One artist's
        # songs a folder each are singles that name their album, and are left to be the singles they are.
        if (3 <= len(members) <= MAX_RECORD and len({f["dir"] for f in members}) >= SCATTERED and parent + PART + album not in units
                and len({fold(f["albumartist"] or f["artist"]) for f in members} | {fold(f["artist"]) for f in members}) >= SCATTERED):
            units[parent + PART + album] = ("record", members)
            moved = {id(f) for f in members}
            for key in {k for f in members for k in (f["dir"], f["dir"] + PART) if k in units and units[k][0] == "loose"}:
                left = [other for other in units[key][1] if id(other) not in moved]
                if left:
                    units[key] = ("loose", left)
                else:
                    del units[key]
    for key in sorted(units):
        yield key, units[key][0], units[key][1]


def signature(files):
    """Changes when a unit's files do."""
    return hashlib.sha1("|".join(sorted(f["stamp"] for f in files)).encode()).hexdigest()


def describe(key, files):
    """What a unit claims to be: (artist, album, whether it looks like a various-artists release)."""
    album, _ = consensus([f["album"] for f in files if f["album"]])
    artist, _ = consensus([f["albumartist"] or f["artist"] for f in files if f["albumartist"] or f["artist"]])
    parts = config.inner(key.split(PART)[0]).split("/")
    if not album:
        guess = TRAILING_YEAR.sub("", parts[-1])
        head, dash, tail = guess.partition(" - ")
        if dash and not artist:  # "Artist - Album"
            artist, album = head, tail
        else:
            album = tail if dash and fold(head) == fold(artist) else guess
    if not artist and len(parts) >= 2 and not parts[-2].startswith("_"):  # "Artist/Album"
        artist = parts[-2]
    credited = collections.Counter(fold(f["artist"]) for f in files if f["artist"])
    various = is_various(artist) or (
        not any(f["albumartist"] for f in files) and len(credited) >= 4 and credited.most_common(1)[0][1] < len(files) / 2
    )
    return artist, album, various


def shown_as(name, title):
    """Whether a file name with "?" where the file system could not show a character spells this title."""
    if "?" not in name:
        return False
    pattern = "".join(".{0,2}" if c == "?" else re.escape(c) for c in name.strip())
    return re.fullmatch(pattern, title.strip(), re.I | re.S) is not None


def nameless(f):
    """Whether a file says nothing about what it is: no title tag, and a file name that is a number or a short-name stub."""
    if f["title"] and not GENERIC_TITLE.search(f["title"]):
        return False
    stem = LEADING_NUMBER.sub("", f["stem"]).strip()
    return not fold(stem) or bool(GENERIC_TITLE.search(stem) or SHORT_NAME.fullmatch(f["stem"]))


def names_of(f, artist="", proposed=True):
    """What a file's title may be: its tag, its file name, and each with a leading number or artist taken off."""
    names = []
    for raw in [f["title"], f["stem"].replace("_", " "), *(f["names"] if proposed else [])]:
        raw = (raw or "").strip()
        if not raw or GENERIC_TITLE.search(raw):
            continue
        bare = LEADING_NUMBER.sub("", raw)
        head, dash, tail = bare.partition(" - ")
        if dash and fold(head) in (fold(artist), fold(f["artist"])):
            bare = tail
        names += [raw, bare]
        if " - " in bare:  # "Artist - Album - Title"
            names.append(bare.rsplit(" - ", 1)[1])
    return [name for name in dict.fromkeys(names) if fold(name)]


def song_claims(f, artist=""):
    """Guesses at (title, artist), best first, from the tags and then from the file name."""
    by = f["artist"] or artist
    guesses = []
    title = f["title"] if f["title"] and not GENERIC_TITLE.search(f["title"]) else ""
    if title and by:
        guesses += [(title, by), (plain_title(LEADING_NUMBER.sub("", title) or title), lead_artist(by))]
    if title and artist and fold(artist) != fold(by):  # the folder's artist, where the file spells its own differently
        guesses.append((plain_title(title), lead_artist(artist)))
    parts = [part.strip() for part in LEADING_NUMBER.sub("", f["stem"].replace("_", " ")).split(" - ") if part.strip()]
    if len(parts) >= 2:  # "Artist - Title"
        guesses.append((plain_title(parts[-1]), lead_artist(parts[0])))
    if parts and by:
        guesses.append((plain_title(parts[-1]), lead_artist(by)))
    for name in f["names"]:  # a title proposed from outside comes with the artist proposed for it
        if f.get("by") or by:
            guesses.insert(0, (name, f.get("by") or by))
    seen, kept = set(), []
    for name, who in guesses:
        key = (fold(name), fold(who))
        if all(key) and key not in seen and not GENERIC_TITLE.search(name):
            seen.add(key)
            kept.append((name, who))
    return kept


def tracks_of(release, video=False):
    """A release's tracks in order; discs that are films rather than music are left out."""
    return [
        {
            "disc": medium.get("position", 1),
            "pos": track.get("position", 0),
            "id": track.get("id", ""),
            "title": track.get("title", ""),
            "artist": reference.credit(track) or reference.credit(track.get("recording") or {}),
            "recording": (track.get("recording") or {}).get("id", ""),
            "seconds": (track.get("length") or (track.get("recording") or {}).get("length") or 0) / 1000,
        }
        for medium in release.get("media", [])
        if video or medium.get("format") not in VIDEO
        for track in medium.get("tracks", [])
    ]


def grade(likeness, gap, placed, same, seconds=0):
    """How well one file fits one track of `seconds`: "sure" by the audio or by length plus name or place, "named" by name alone.

    A file much longer or shorter than the track is a live take, an edit or a remix of the song, or has a hidden track
    run on: it is not this track, whatever its name. One a little off, with the track's name and in its place, is
    another pressing or master of it.
    """
    if same and (gap is None or gap <= 3 * SURE_SECONDS):
        return "sure"
    if gap is not None and gap > max(OTHER_TAKE, OTHER_SHARE * seconds):
        return ""
    if gap is not None and gap <= SURE_SECONDS and (likeness >= 0.75 or placed):
        return "sure"
    if gap is not None and gap <= max(2 * SURE_SECONDS, PRESSING_SHARE * seconds) and likeness >= 0.9 and placed:
        return "sure"
    if likeness >= 0.9 or (likeness >= 0.8 and placed):
        return "named"
    return ""


def compare(files, release, artist=""):
    """Say which file is which track of a release, and how well the whole group fits it."""
    tracks = tracks_of(release)
    one_disc = len({t["disc"] for t in tracks}) == 1
    pairs = []
    prompted = set()  # (file, track) pairs where only a title proposed from outside makes the names agree
    for i, f in enumerate(files):
        names, own = names_of(f, artist), set(names_of(f, artist, proposed=False))
        for j, t in enumerate(tracks):
            likeness, name = max(((1.0 if shown_as(name, t["title"]) else similar(name, t["title"]), name) for name in names), default=(0.0, ""))
            if likeness >= 0.75 and name not in own and max((similar(mine, t["title"]) for mine in own), default=0.0) < 0.75:
                prompted.add((i, j))
            gap = abs(f["seconds"] - t["seconds"]) if f["seconds"] and t["seconds"] else None
            placed = bool(f["track"]) and f["track"] == t["pos"] and (f["disc"] == t["disc"] or (one_disc and f["disc"] in (0, 1)))
            same = bool(t["id"] and f["mb_track"] == t["id"]) or bool(
                t["recording"] and (f["mb_recording"] == t["recording"] or f["heard"].get(t["recording"], 0) >= HEARD)
            )
            cost = (1 - likeness) + (min(gap, 15) / 15 if gap is not None else 0.5) - (0.25 if placed else 0) - (1 if same else 0)
            pairs.append((cost, i, j, likeness, gap, placed, same, t["seconds"]))
    pairs.sort()

    paired, taken, cheapest = {}, set(), {}
    for pair in pairs:
        _, i, j = pair[:3]
        cheapest.setdefault(i, pair)
        if i not in paired and j not in taken:
            paired[i] = pair
            taken.add(j)

    filler = {pair[2]: i for i, pair in paired.items()}  # track: the file paired with it
    rows, matched = [], set()
    for i, f in enumerate(files):
        pair, copy = paired.get(i), False
        fits = grade(*pair[3:]) if pair else ""
        # A second copy of a track another file already fills. It must say by its name or its audio that it is that track,
        # or be as long as the file already there: "Song A", numbered 13 and near the length of "Song B", 13th on a
        # reissue, is no copy of it when the file that is "Song B" is four seconds longer.
        if not fits and i in cheapest and grade(*cheapest[i][3:]) and (
                cheapest[i][3] >= 0.75 or cheapest[i][6] or abs(f["seconds"] - files[filler[cheapest[i][2]]]["seconds"]) <= SAME_RIP):
            pair, copy = cheapest[i], True
            fits = grade(*pair[3:])
            if fits != "sure":  # only named like a track a file already fills: another version of the song, and no part of the record
                pair, copy, fits = None, False, ""
        if fits == "sure" and (i, pair[2]) in prompted and not pair[5] and not pair[6]:
            # The name came from a model, so the length must be what tells this track from the rest of the release.
            rivals = sum(1 for t in tracks if f["seconds"] and t["seconds"] and abs(f["seconds"] - t["seconds"]) <= SURE_SECONDS)
            fits = "named" if rivals > 1 else fits
        if fits:
            matched.add(pair[2])
        rows.append({
            "file": f,
            "grade": fits,
            "track": tracks[pair[2]] if fits else None,
            "copy": copy,
            "likeness": round(pair[3], 2) if fits else None,
            "gap": round(pair[4], 1) if fits and pair[4] is not None else None,
            "placed": bool(fits and pair[5]),
            "same": bool(fits and pair[6]),
            "left": False,
        })

    # A file that says nothing about itself is placed by what is left over: the one unfilled track of its length,
    # or, when it cannot be read at all, the one unfilled track there is.
    for row in rows:
        spare = [j for j in range(len(tracks)) if j not in matched]
        f = row["file"]
        if row["grade"] or not spare or not nameless(f):
            continue
        def near(seconds, other):
            return bool(seconds and other and abs(seconds - other) <= SURE_SECONDS)

        fitting = [j for j in spare if near(f["seconds"], tracks[j]["seconds"])]
        unplaced = [r["file"] for r in rows if not r["grade"]]
        if len(fitting) == 1 and sum(near(other["seconds"], tracks[fitting[0]]["seconds"]) for other in unplaced) == 1:
            j, fits = fitting[0], "sure"  # the only file of that length, and the only unfilled track of it
        elif not f["seconds"] and len(spare) == 1 and sum(not r["grade"] for r in rows) == 1:
            j, fits = spare[0], "left"
        else:
            continue
        matched.add(j)
        row.update(grade=fits, track=tracks[j], left=True, gap=round(abs(f["seconds"] - tracks[j]["seconds"]), 1) if f["seconds"] else None)

    sure = sum(r["grade"] == "sure" for r in rows)
    graded = sum(bool(r["grade"]) for r in rows)
    enough = min(3, len(tracks))

    missing = [t for j, t in enumerate(tracks) if j not in matched]
    touched = {tracks[j]["disc"] for j in matched}
    if not missing:
        shelf = "complete"
    elif matched and all(t["disc"] not in touched for t in missing):
        shelf = "whole discs of a set"
    else:
        shelf = "missing 1-2" if len(missing) <= 2 else "thinner"

    # A file may lack only a length to compare (unreadable, or the reference gives none); it may not contradict.
    solid = all(r["grade"] in ("sure", "left") or (r["grade"] == "named" and r["placed"] and r["gap"] is None) for r in rows if r["grade"])
    # Files that fill a release, or all but a track or two of it, are that release whatever else shares their folder; but
    # a release of a few tracks only when they are at least half the folder: three songs of a single are not a folder of thirty.
    filled = shelf in ("complete", "missing 1-2") and (len(tracks) >= FEW_TRACKS or 2 * graded >= len(files))
    if solid and sure >= enough and len(matched) >= enough and (graded == len(files) or filled):
        verdict = "certain"
    elif len(matched) >= enough and (graded >= 0.8 * len(files) or (filled and sure >= enough)):
        verdict = "likely"
    else:
        verdict = "unclear"

    group = release.get("release-group") or {}
    return {
        "verdict": verdict,
        "key": (VERDICTS.index(verdict), -graded, len(missing), release.get("status") not in (None, "Official"), release.get("date") or "9999"),
        "release": release.get("id", ""),
        "group": group.get("id", ""),
        "title": release.get("title", ""),
        "artist": reference.credit(release),
        "year": (group.get("first-release-date") or release.get("date") or "")[:4],
        "kind": reference.kind(group),
        "tracks": len(tracks),
        "sure": sure,
        "named": graded - sure,
        "unmatched": len(files) - graded,
        "missing": missing,
        "shelf": shelf,
        "copies": sum(r["copy"] for r in rows),
        "rows": rows,
    }


def wanted_count(files):
    """How many different tracks a group of files seems to hold."""
    numbered = {(f["disc"], f["track"]) for f in files if f["track"]}
    if len(numbered) >= 0.6 * len(files):
        return len(numbered)
    return len({fold(f["title"] or f["stem"]) for f in files})
