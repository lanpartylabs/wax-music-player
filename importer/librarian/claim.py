"""Claims: the record a folder names, held as that record although the reference could not settle every file in it.

A folder that looks like one record, whose files the reference matched only likely, or to no one release for
certain, is still that record when its own names say so and most of its files bear them out. Each file is placed on
the release by what fits best: the reference's own pairing first; then a length within a second or two of one track
still unfilled, when no other file is nearer it; then the file's name alone. A file placed by anything but the
reference's sure pairing is unconfirmed, and a record that holds one is not verified.
"""

import collections
import json

from . import identify, match, reference
from .text import GENERIC_TITLE, LEADING_NUMBER, similar

CLAIM = 0.8  # the least likeness of the album a folder names to a release's title for the folder to claim that release
NEAR = 2.5  # seconds; a file this near in length to a track no file fills, and nearer it than any other file, is placed there
NAMED = 0.9  # the least likeness of a file's name to a track's title for the name alone to place the file
HELD = 0.5  # the least share of a folder's files that must find a place for the folder to be the record it names
FITS = 0.6  # the share of a folder's files a release must fit, by the reference's pairing, to be claimed whatever the folder calls itself
CANDIDATES = 6  # releases weighed, at most, for one folder


def place(files, release, artist=""):
    """Where each file of a folder goes on a release, as [(file, track, how)]; a file that fits nowhere is left out.

    `how` is "sure" for the reference's sure pairing, "named" for its pairing by name at another length, "length" for a
    length that fits, "name" for the name alone.
    """
    outcome = match.compare(files, release, artist)
    tracks = match.tracks_of(release)
    placed, filled, used = [], set(), set()
    for row in outcome["rows"]:
        where = (row["track"]["disc"], row["track"]["pos"]) if row["grade"] else None
        if where and not row["copy"] and where not in filled:
            placed.append((row["file"], row["track"], "sure" if row["grade"] == "sure" else "named"))
            filled.add(where)
            used.add(row["file"]["id"])

    def free():
        return [t for t in tracks if (t["disc"], t["pos"]) not in filled]

    def take(f, t, how):
        placed.append((f, t, how))
        filled.add((t["disc"], t["pos"]))
        used.add(f["id"])

    # By length: a file and a track each nearest the other, and near enough. Only a file whose name says nothing, or names
    # some song of this release (a rip whose names are shuffled against its audio): one that names another song is that song.
    titles = [t["title"] for t in tracks]

    def shuffled(f):
        names = [name for name in match.names_of(f, artist, proposed=False)
                 if not GENERIC_TITLE.search(LEADING_NUMBER.sub("", name).strip())]  # "08 Track 8" says nothing
        return not names or any(similar(name, title) >= NAMED for name in names for title in titles)

    rest = [f for f in files if f["id"] not in used and f["seconds"] and shuffled(f)]
    timed = [t for t in free() if t["seconds"]]
    if rest and timed:
        nearest_track = {f["id"]: min(timed, key=lambda t: abs(t["seconds"] - f["seconds"])) for f in rest}
        for t in timed:
            f = min(rest, key=lambda f: abs(t["seconds"] - f["seconds"]))
            if nearest_track[f["id"]] is t and abs(t["seconds"] - f["seconds"]) <= NEAR:
                take(f, t, "length")
    # By name: the likest pairs first.
    pairs = sorted(((max((similar(name, t["title"]) for name in match.names_of(f, artist, proposed=False)), default=0.0), n, f, t)
                    for n, f in enumerate(f for f in files if f["id"] not in used) for t in free()), key=lambda p: (-p[0], p[1]))
    for likeness, _, f, t in pairs:
        if likeness >= NAMED and f["id"] not in used and (t["disc"], t["pos"]) not in filled:
            take(f, t, "name")
    return placed


def claims(db, mb):
    """{unit key: {"unit", "release", "placed"}} for every folder that is the record it names without the reference being sure of it.

    Of the releases the folder was compared with, those that bear the name it gives, or that most of its files fit
    whatever it calls itself; the release most of its songs were found on one by one; and for a folder matched likely
    the release it was matched to: the one where most files
    find a place less the tracks still lacking, then most are confirmed. A bigger edition does not win by taking in a stray.
    """
    dropped = {row[0] for row in db.execute("SELECT name FROM discarded")}  # by their owner: as if the collection did not hold them
    files = {f["id"]: f for f in identify.load_files(db) if f["path"] not in dropped}
    members, own, ids = collections.defaultdict(list), collections.defaultdict(collections.Counter), {}
    for row in db.execute("SELECT file, unit, state, release FROM identity"):
        if row["file"] in files:
            members[row["unit"]].append(files[row["file"]])
            if row["state"] == "certain" and row["release"]:
                own[row["unit"]][row["release"]] += 1
                ids[row["file"]] = row["release"]
    found = {}
    for unit in db.execute("SELECT * FROM units WHERE shape = 'record' AND state IN ('likely', 'unclear') ORDER BY key"):
        held = members.get(unit["key"])
        if not held:
            continue
        artist, album, _ = match.describe(unit["key"], held)
        tried = sorted(json.loads(unit["tried"] or "[]"), key=lambda t: (-(t["sure"] + t["named"]), t["tracks"]))
        candidates = [t["release"] for t in tried
                      if (album and similar(album, t["title"]) >= CLAIM) or t["sure"] + t["named"] >= max(3, FITS * len(held))]
        if unit["state"] == "likely" and unit["release"] and unit["release"] not in candidates:
            candidates.insert(0, unit["release"])
        # The release most of its songs were found on one by one: an album's instrumentals, filed by length beside the album.
        for release_id, count in own[unit["key"]].most_common(1):
            if count >= 3 and release_id not in candidates:
                candidates.insert(0, release_id)
        best = None
        for release_id in candidates[:CANDIDATES]:
            try:
                release = mb.release(release_id)
            except reference.Unavailable:
                continue
            placed = place(held, release, artist)
            tracks = len(match.tracks_of(release))
            if len(placed) < min(3, tracks) or len(placed) < HELD * len(held):
                continue
            # Files placed, less tracks lacking, and each that was found on this very release by itself counts once more:
            # an album's instrumentals are as long as its songs, and only their own identities tell them apart.
            agree = sum(ids.get(f["id"]) == release_id for f, _, _ in placed)
            fit = (2 * len(placed) - tracks + agree, sum(how == "sure" for _, _, how in placed))
            if best is None or fit > best[0]:
                best = (fit, {"unit": unit, "release": release, "placed": placed})
        if best:
            found[unit["key"]] = best[1]
    return found
