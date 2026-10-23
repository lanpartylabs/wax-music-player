"""The ready catalogue: the releases that are finished, and nothing else.

    python3 -m librarian ready

A release is ready when the folder holding it was matched with certainty and
is complete, each of its tracks is a file that is certain and plays, every
track has its details, the album has a genre, and it has a good cover:
square, or a sleeve of another shape such as a cassette's, and at least 300
pixels a side, about as large as the player draws a cover. Those releases, and only those, are written to the ready folder as
the index the player reads, with a thumbnail of each cover. A release whose
data is all good but whose cover is missing or poor is listed apart, as
waiting for a cover; every other record held with certainty is listed with
what holds it back. A record whose folder lacks some of its tracks is still
whole when files elsewhere in the collection are those very tracks, a deluxe
edition's second disc beside the album it extends, say: they are counted in.
One that still lacks tracks goes in all the same, so long as it holds a
third of itself, with what it lacks listed in the index as missing, for the
player to show but not play. An album held in two folders, or in two
editions, goes in once, and is held back once: the fuller edition, then the
better copy. An artist whom different records print differently goes under
one name throughout: the one most of their records in the catalogue print.
A single held in several formats with different songs, CD1, CD2 and the 12",
shows as one release with a disc for each format, named for it; so does an
album some pressings split in two, held as two folders that differ only by
the disc each is.

Everything else in the collection goes into a second index beside the first,
in the folder "unfinished", for the player's developer view: each file with
the names it has so far and, as its state, what it still wants. A song of an
unfinished record that the ready catalogue has already, as well or better,
can go. The music is only read.
"""

import argparse
import collections
import csv
import gzip
import json
import os
import re
import subprocess
import sys
import time
import unicodedata

from . import catalogue, claim, config, match, reference
from .match import PART, SURE_SECONDS
from .text import DISC_NOISE, fold, is_various

RELEASE = "https://musicbrainz.org/release/"
THUMBNAIL = 320  # pixels a side, as the player draws covers
SHARP = 300  # pixels a side; a cover this large looks as sharp as any at THUMBNAIL (the report still lists those under artwork.SMALL)
SLEEVE = 1.7  # longer side over shorter; a cover up to this far from square is a sleeve of another shape, a promo CD's card or a
# cassette's J-card, and is drawn whole; beyond it, a picture of something else, both sides of a sleeve opened out, say
GAPS = 2  # tracks a record may lack and still borrow them from unrelated records in the collection
HOLDS = 3  # a record that holds at least one in this many of its tracks goes in, the rest of it listed as missing
SAME_SONGS = 0.8  # the share of songs two editions must share, edits and mixes aside, to be copies of one album
VERSION = re.compile(r"\s*[(\[][^)\]]*[)\]]")  # "(radio edit)", "[club mix]": which take of a song a title names
OTHER_MUSIC = re.compile(r"instrumental|karaoke|a ?cappella|acapella|commentary", re.I)  # takes that are other music, not other edits
FORMATS = ("Single", "EP")  # kinds whose several formats, CD1, CD2 and the 12" with their different songs, show as one release
DETAILS = ("title", "artist", "album", "album_artist", "year", "disc", "position", "kind", "release", "recording")
GUEST = re.compile(r"\s+(?:feat\.?|featuring|ft\.?)\s+", re.IGNORECASE)  # in an artist credit, divides the artist from the guests


def cover_fault(cover):
    """What is wrong with a release's cover, or None when it is good."""
    if cover is None:
        return "not looked for yet"
    if not cover["file"]:
        return "none anywhere"
    if max(cover["width"], cover["height"]) < SHARP:
        return f"too small ({cover['width']}x{cover['height']})"
    if max(cover["width"], cover["height"]) > SLEEVE * min(cover["width"], cover["height"]):
        return f"not square ({cover['width']}x{cover['height']})"
    return None


def borrow(mb, record, known):
    """Fill the tracks a record's folder lacks with files elsewhere in the collection that are those tracks. Returns the
    release's tracks that are still not held, as the reference lists them.

    A file is that track when it is the very recording, or the same song by the same artist to within a few seconds of
    its length. It is listed under this record with the track's own place and name, and stays where it is. From
    another edition of the same record any number may come: the album beside its deluxe second disc. From other
    records only the track or two a nearly whole one lacks; a record is not put together out of a compilation's worth of strays.
    """
    unit, own = record["unit"], record["tracks"]
    have = {(f["disc"], f["position"]) for f in own}
    lacking = [track for track in match.tracks_of(mb.release(unit["release"])) if (track["disc"], track["pos"]) not in have]
    if not own or not lacking:
        return lacking
    near, used, lent, still = len(lacking) <= GAPS, {f["file"] for f in own}, [], []
    for track in lacking:
        found = [f for f in known.get(track["recording"], []) if track["recording"]]
        found += [f for f in known.get((fold(track["title"]), fold(track["artist"])), [])
                  if track["seconds"] and f["seconds"] and abs(f["seconds"] - track["seconds"]) <= SURE_SECONDS]
        found = [f for f in found if f["file"] not in used and f["opens"] and f["reader"] is not None and f["seconds"]
                 and (near or f["release_group"] == unit["release_group"])]
        if not found:
            still.append(track)
            continue
        # A copy from another edition of the same record before one from anywhere else; then the better sounding.
        pick = max(found, key=lambda f: (f["release_group"] == unit["release_group"], bool(f["lossless"]), f["bitrate"] or 0))
        used.add(pick["file"])
        lent.append({**pick, "release": unit["release"], "release_group": unit["release_group"], "album": own[0]["album"],
                     "album_artist": own[0]["album_artist"], "year": own[0]["year"], "kind": own[0]["kind"], "disc": track["disc"],
                     "position": track["pos"], "title": track["title"], "artist": track["artist"] or pick["artist"], "copy": 0})
    record["tracks"] = sorted(own + lent, key=lambda f: (f["disc"] or 0, f["position"] or 0))
    record["lent"] = len(lent)
    return still


def as_single(mb, unit, files):
    """A folder of a song or two, too few to be looked up as a record, as the release they are between them when they
    are each certain, all on one release, and every track of it: a single, say. None when they are not."""
    if not files or any(f["state"] != "certain" or f["copy"] or not f["release"] for f in files) or len({f["release"] for f in files}) != 1:
        return None
    tracks = match.tracks_of(mb.release(files[0]["release"]))
    if len(tracks) != len(files) or {(t["disc"], t["pos"]) for t in tracks} != {(f["disc"], f["position"]) for f in files}:
        return None
    first = files[0]
    return {**dict(unit), "shape": "record", "state": "certain", "how": "its songs", "release": first["release"], "release_group": first["release_group"],
            "title": first["album"], "credited": first["album_artist"], "year": first["year"], "kind": first["kind"], "tracks": len(tracks),
            "missing": 0, "shelf": "complete", "copies": 0}


def unconfirmed(how, f, gap):
    """Why a track of an unverified record is not confirmed, in a few words; empty when the reference was sure of it."""
    off = f"{abs(gap)} s {'longer' if gap > 0 else 'shorter'} than the release's" if gap else "its length unknown"
    return {"sure": "", "named": f"placed by its name; {off}", "name": f"placed by its name; {off}",
            "length": f"placed by its length; its own name says \u201c{f['title'] or f['stem']}\u201d"}[how]


def sounds_better(copy, f):
    """Whether one copy of a song sounds clearly better than another: lossless against lossy, or half as many bits again."""
    if f["lossless"]:
        return False
    return bool(copy["lossless"]) or (copy["bitrate"] or 0) >= 1.5 * (f["bitrate"] or 0) > 0


def better_copies(records, known):
    """In each record that is in, or waits only for a cover, put in place of a track the best copy of it in the collection
    when that sounds clearly better and is in no such record already. The copy stays where it is, as a borrowed track
    does; the file it stands in for is left out, and can go. Returns how many were put in."""
    finished = [record for record in records if not record["why"]]
    used = {f["file"] for record in finished for f in record["tracks"]}
    swapped = 0
    for record in finished:
        for n, f in enumerate(record["tracks"]):
            copies = known.get(f["recording"], []) if f["recording"] else []
            copies += [c for c in known.get((fold(f["title"]), fold(f["artist"])), [])
                       if f["seconds"] and c["seconds"] and abs(c["seconds"] - f["seconds"]) <= SURE_SECONDS]
            copies = [c for c in copies if c["file"] not in used and c["opens"] and c["reader"] is not None and c["seconds"] and sounds_better(c, f)]
            if not copies:
                continue
            pick = max(copies, key=lambda c: (bool(c["lossless"]), c["bitrate"] or 0))
            used.add(pick["file"])
            keep = ("release", "release_group", "recording", "album", "album_artist", "year", "kind", "disc", "position", "title", "artist")
            record["tracks"][n] = {**pick, **{name: f[name] for name in keep}, "copy": 0}
            record.setdefault("replaced", []).append(f)
            swapped += 1
    return swapped


def same_songs(record, other):
    """Whether two records of one release group hold mostly the same songs: copies of one album, not an album and its
    instrumentals. By recording, or by title where two pressings list the same song as different recordings; and, for
    all but singles, by title less its edit or mix ("A Song (radio edit)" is "A Song" mixed into a DJ set).
    A remix single's songs are all one song, so singles are told apart by their exact titles only."""
    def alike(key, share=0.5):
        mine, theirs = collections.Counter(key(f) for f in record["tracks"]), collections.Counter(key(f) for f in other["tracks"])
        mine.pop("", None)
        theirs.pop("", None)
        if not mine or not theirs:
            return True
        return sum((mine & theirs).values()) >= share * min(sum(mine.values()), sum(theirs.values()))
    if alike(lambda f: f["recording"] or "") or alike(lambda f: fold(f["title"] or "")):
        return True
    singles = "Single" in (record["unit"]["kind"], other["unit"]["kind"])
    return not singles and alike(lambda f: song(f["title"]), share=SAME_SONGS)


def song(title):
    """A track's title less what says which edit or mix of the song it is; what makes it other music, an instrumental, stays."""
    other = sorted({mark.group(0).lower().replace(" ", "") for mark in OTHER_MUSIC.finditer(title or "")})
    return " ".join([fold(VERSION.sub("", title or ""))] + other).strip()


def one_place_each(records):
    """Count each file in one record of the catalogue only. A file two records that are in, or wait only for a cover,
    both count (one its own folder's, the other borrowing it) stays with its own folder's record, or else the first;
    the other lists the track as missing, and is held back if that leaves it too little of itself."""
    holders = collections.defaultdict(list)
    for record in records:
        if not record["why"]:
            for f in record["tracks"]:
                holders[f["file"]].append(record)
    for file, records_of in holders.items():
        if len(records_of) < 2:
            continue
        keep = next((r for r in records_of if any(f["file"] == file and f["unit"] == r["unit"]["key"] for f in r["tracks"])), records_of[0])
        for record in records_of:
            if record is keep:
                continue
            track = next(f for f in record["tracks"] if f["file"] == file)
            record["tracks"].remove(track)
            record["gaps"].append({"disc": track["disc"], "pos": track["position"], "title": track["title"], "artist": track["artist"],
                                   "recording": track["recording"], "seconds": track["seconds"]})
            record["gaps"].sort(key=lambda t: (t["disc"] or 0, t["pos"] or 0))
            tracks = record["unit"]["tracks"]
            if not few(record["gaps"], tracks):
                record["why"].append(f"not complete ({tracks - len(record['gaps'])} of {tracks} tracks)")
                record["state"] = "held back"


def few(gaps, tracks):
    """Whether a record lacking these tracks of its `tracks` goes in all the same, with them listed as missing: when it
    holds at least a third of itself. A box set's few strays do not make the box set."""
    return (tracks - len(gaps)) * HOLDS >= tracks


def assess(db, mb=None):
    """Every record held with certainty, as {"unit", "tracks", "state", "why", ...}: state is ready, waiting for a cover, or held back.

    With the reference to hand, a record that is not whole in its own folder is completed from the rest of the collection where it can be.
    """
    members, known, strays, rows = collections.defaultdict(list), collections.defaultdict(list), collections.defaultdict(list), {}
    for row in db.execute(
        "SELECT i.*, f.path, f.name, f.size, f.mtime, e.opens, e.reader, e.fingerprint, e.lossless, e.bitrate, u.shape, u.state AS unit_state, "
        "coalesce(e.duration, e.fp_duration) AS seconds FROM identity i JOIN files f ON f.id = i.file JOIN evidence e ON e.file = i.file "
        "JOIN units u ON u.key = i.unit WHERE f.seen = (SELECT max(seen) FROM files) "
        "AND f.name NOT IN (SELECT name FROM discarded)"  # dropped by their owner: as if the collection did not hold them
    ):
        row = dict(row)
        rows[row["file"]] = row
        if row["shape"] == "record" and row["unit_state"] == "certain":
            members[row["unit"]].append(row)
        if row["shape"] == "loose":
            strays[row["unit"]].append(row)
        if row["state"] == "certain" and row["title"]:  # what the collection holds for certain, by recording and by name
            if row["recording"]:
                known[row["recording"]].append(row)
            known[(fold(row["title"]), fold(row["artist"]))].append(row)
    covers = {row["release"]: row for row in db.execute("SELECT * FROM covers")}
    # The genre that counts is the one a model has filed the album under; the first reading from MusicBrainz alone does not.
    genres = {row["release_group"]: (row["filed"], row["style"]) for row in db.execute("SELECT release_group, filed, style FROM genres")}

    units = db.execute("SELECT * FROM units WHERE shape = 'record' AND state = 'certain' ORDER BY key").fetchall()
    if mb:
        # Only the songs of a folder of their own: not those left over beside a record, nor a pile's.
        shared = {row[0] for row in db.execute("SELECT folder FROM units GROUP BY folder HAVING count(*) > 1")}
        for unit in db.execute("SELECT * FROM units WHERE shape = 'loose' ORDER BY key"):
            single = unit["folder"] not in shared and as_single(mb, unit, strays.get(unit["key"]))
            if single:
                units.append(single)
                members[unit["key"]] = strays[unit["key"]]

    # A folder that is the record it names, though the reference was not sure of every file: an unverified record.
    unverified = set()
    for key, found in (claim.claims(db, mb) if mb else {}).items():
        release = found["release"]
        group = release.get("release-group") or {}
        unit = {**dict(found["unit"]), "state": "claimed", "release": release["id"], "release_group": group.get("id"), "title": release.get("title"),
                "credited": reference.credit(release), "year": (group.get("first-release-date") or release.get("date") or "")[:4],
                "kind": reference.kind(group), "tracks": len(match.tracks_of(release))}
        members[key] = []
        for f, t, how in found["placed"]:
            if f["id"] in rows:
                gap = round(f["seconds"] - t["seconds"]) if f["seconds"] and t["seconds"] else None
                members[key].append({**rows[f["id"]], "state": "certain", "copy": 0, "release": unit["release"], "release_group": unit["release_group"],
                                     "recording": t["recording"], "track": t["id"], "disc": t["disc"], "position": t["pos"], "title": t["title"],
                                     "artist": t["artist"] or unit["credited"], "album": unit["title"], "album_artist": unit["credited"],
                                     "year": unit["year"], "kind": unit["kind"], "unconfirmed": unconfirmed(how, f, gap)})
        units.append(unit)
        unverified.add(key)

    records = []
    for unit in units:
        if not members[unit["key"]]:  # every file of it dropped by its owner: no record any more
            continue
        # The record's tracks are the files settled as being on this very release. A certain file beside them that is
        # some other song (a bonus track of another edition, say) is not part of the record; it is left out of it.
        tracks = sorted((f for f in members[unit["key"]] if f["state"] == "certain" and not f["copy"] and f["release"] == unit["release"]),
                        key=lambda f: (f["disc"] or 0, f["position"] or 0, f["name"]))
        record = {"unit": unit, "tracks": tracks, "lent": 0, "gaps": [], "unverified": unit["key"] in unverified}
        why = []
        if mb:
            record["gaps"] = borrow(mb, record, known)
            if record["gaps"] and not few(record["gaps"], unit["tracks"]):
                why.append(f"not complete ({unit['tracks'] - len(record['gaps'])} of {unit['tracks']} tracks)")
        elif unit["shelf"] != "complete":
            why.append(f"not complete ({unit['shelf']})")
        tracks, held = record["tracks"], unit["tracks"] - len(record["gaps"])
        if not why and (len({(f["disc"], f["position"]) for f in tracks}) != held or len(tracks) != held):
            why.append("its tracks do not add up to the release's")
        if any(not f["opens"] or f["reader"] is None or not f["seconds"] for f in tracks):
            why.append("a track cannot be played")
        lacking = {name for name in DETAILS for f in tracks if f[name] in (None, "")} | {"year" for f in tracks if not str(f["year"])[:4].isdigit()}
        if lacking:
            why.append("a track lacks " + ", ".join(sorted(lacking)))
        if any("\ufffd" in f["name"] for f in tracks):
            why.append("a file's name is not valid text, so the player cannot ask for it")
        records.append({**record, "left out": unit["files"] - (len(tracks) - record["lent"]), "why": why,
                        "genre": genres.get(unit["release_group"], (None, None))[0],
                        "style": genres.get(unit["release_group"], (None, None))[1], "cover": covers.get(unit["release"])})

    # An album goes in once, and is held back once: of all the folders that hold it, the one with nothing else against
    # it, then one the reference is sure of, then the edition with most tracks, then lossless, then the higher bitrate.
    # The rest are duplicates of that one.
    held = collections.defaultdict(list)
    for record in records:
        held[record["unit"]["release_group"]].append(record)
    # Each is set against every better one kept before it: a group may hold several records of other songs, a single's
    # formats, each of which may have its own copies.
    for copies in held.values():
        kept = []
        for record in sorted(copies, key=lambda r: (not r["why"], not r["unverified"], len(r["tracks"]), all(f["lossless"] for f in r["tracks"]),
                                                    sum(f["bitrate"] or 0 for f in r["tracks"]) / max(1, len(r["tracks"]))), reverse=True):
            # An edition of other recordings, the album's instrumentals, say, is no copy of it, though MusicBrainz files it beside it.
            best = next((other for other in kept if same_songs(record, other)), None)
            if best is None:
                kept.append(record)
                continue
            same = record["unit"]["release"] == best["unit"]["release"]
            record["why"].insert(0, ("a better copy of this release" if same else "another edition of this album") + " is in from " + best["unit"]["folder"])
    for record in records:
        if not record["why"] and not record["genre"]:
            record["why"].append("no genre")
        record["cover fault"] = cover_fault(record["cover"])
        record["state"] = "held back" if record["why"] else "waiting for a cover" if record["cover fault"] else "ready"
    better_copies(records, known)
    one_place_each(records)
    return records


def art_id(release):
    """A release's thumbnail id as the player wants it: sixteen hex digits."""
    return release.replace("-", "")[:16]


def credits(release):
    """A release's artist credits as MusicBrainz gives them: the album's, then each track's with its (disc, position)."""
    yield None, release.get("artist-credit") or []
    for disc in release.get("media") or []:
        for track in disc.get("tracks") or []:
            yield (disc.get("position"), track.get("position")), track.get("artist-credit") or (track.get("recording") or {}).get("artist-credit") or []


def one_name_each(releases):
    """{artist id: name} for every artist on these releases: the name most of the credits print, so that each artist has one entry.

    A record may print "M. Brook" where the rest print "Mila Brook", or a Japanese pressing give the name in
    katakana. The artist's own name as MusicBrainz has it counts as one more credit, and a spelling in another script
    gives way to one in Latin letters where there is one.
    """
    printed, own = collections.defaultdict(collections.Counter), {}
    for release in releases:
        for _, parts in credits(release):
            for part in parts:
                printed[part["artist"]["id"]][part.get("name") or part["artist"]["name"]] += 1
                own[part["artist"]["id"]] = part["artist"]["name"]
    latin = lambda name: all(unicodedata.name(c, "").startswith("LATIN") for c in name if c.isalpha())
    names = {}
    for artist, seen in printed.items():
        seen[own[artist]] += 1
        names[artist] = max([name for name in seen if latin(name)] or seen, key=lambda name: (seen[name], name == own[artist], name))
    return names


def named(release, names):
    """(the album's artists, {(disc, position): that track's artists}) for a release, each artist under their one name."""
    credit = lambda parts: "".join(names[part["artist"]["id"]] + part.get("joinphrase", "") for part in parts)
    by_track = {where: credit(parts) for where, parts in credits(release)}
    return by_track.pop(None), by_track


def as_one(records, releases):
    """The ready records the player shows as one release, as [[record, ...]], each group in the order its parts came out.

    The formats of one single or EP whose songs differ, CD1, CD2 and the 12", say, are one release a disc a format. So are
    the halves of an album that some pressings split in two, where the collection holds them as two folders side by side
    whose names differ only by the disc each is.
    """
    by_group = collections.defaultdict(list)
    for record in records:
        by_group[record["unit"]["release_group"]].append(record)
    def disc(record):  # the disc a folder's name says it is, "... [White Album] CD2 (1968)"; 0 for none
        marker = DISC_NOISE.search(record["unit"]["folder"].rpartition("/")[2])
        return int(re.search(r"\d+", marker.group(0)).group(0)) if marker else 0

    def beside(record):  # the folder above, and the folder's name without its disc
        parent, _, name = record["unit"]["folder"].rpartition("/")
        return parent, fold(DISC_NOISE.sub("", name))

    for parts in by_group.values():
        if len(parts) < 2:
            continue
        if all(part["unit"]["kind"] in FORMATS for part in parts):
            yield sorted(parts, key=lambda r: (releases[r["unit"]["release"]].get("date") or "9999", r["unit"]["folder"]))
            continue
        halves = collections.defaultdict(list)
        for part in parts:
            if disc(part):
                halves[beside(part)].append(part)
        for group in halves.values():
            if len(group) > 1 and len({disc(part) for part in group}) == len(group):
                yield sorted(group, key=disc)


def format_name(release, labels):
    """A format of a single as a heading names it: "CD · SKINT 100CDP", "12\" Vinyl · SKINT 100", "CD 2 · 672502 5"."""
    kind = " + ".join(dict.fromkeys(filter(None, (disc.get("format") for disc in release.get("media") or []))))
    told = release.get("disambiguation") or ""
    if kind and fold(told).startswith(fold(kind)):  # "CD 2" says "CD" already
        kind = ""
    numbers = {}  # one of each, however it is spaced or dotted: "672304 2" and "672304.2"
    for label in labels.get("label-info") or []:
        number = label.get("catalog-number") or ""
        if fold(number) and number != "[none]":
            numbers.setdefault(fold(number).replace(" ", ""), number)
    return " · ".join(part for part in (kind, told, ", ".join(numbers.values())) if part)


def shown_as_one(mb, ready, releases):
    """{id(record): what changes in each of its index lines} for the records shown as one release with others: the name,
    year, cover and release id of the whole, and where its own discs fall among the whole's, with each named for its format."""
    joined = {}
    for parts in as_one(ready, releases):
        whole = max(parts, key=lambda r: (len(r["tracks"]), -len(r["gaps"])))
        group = releases[whole["unit"]["release"]].get("release-group") or {}
        year = min(int(str(r["unit"]["year"])[:4]) for r in parts if str(r["unit"]["year"])[:4].isdigit())
        day = min((r["released"] for r in parts if r.get("released", "")[:4] == str(year)), default="")
        before = 0
        for record in parts:
            release = releases[record["unit"]["release"]]
            discs = max(1, len(release.get("media") or []))
            named = format_name(release, mb.labels(record["unit"]["release"])) if record["unit"]["kind"] in FORMATS else ""
            joined[id(record)] = {"l": group.get("title") or whole["unit"]["title"], "y": year, "Y": day, "i": art_id(whole["unit"]["release"]),
                                  "R": whole["unit"]["release"], "before": before, "D": named, "discs": discs, "parts": len(parts)}
            before += discs
    return joined


def join(track, joined):
    """An index line of a record shown as one release with others, as part of that whole."""
    if not joined:
        return track
    disc = track.get("c") or 1
    named = joined["D"] + (f" · disc {disc}" if joined["D"] and joined["discs"] > 1 else "")
    return spare({**track, "l": joined["l"], "y": joined["y"], "Y": joined["Y"], "i": joined["i"], "R": joined["R"], "c": joined["before"] + disc,
                  "D": named})


def released(release):
    """The day a release first came out, as the reference gives it: "2003-09-23", or "2003-09" or "2003" when it knows no more."""
    group = release.get("release-group") or {}
    return group.get("first-release-date") or release.get("date") or ""


def as_track(record, f):
    """One file as a line of the player's index, with the names the reference gives it."""
    unit = record["unit"]
    year = str(f["year"])[:4]
    album_artist, by_track = record["names"]
    artist, guests = (GUEST.split(by_track.get((f["disc"], f["position"])) or f["artist"], maxsplit=1) + [""])[:2]  # guests go in the title
    title = f["title"] if not guests or re.search(r"\bfeat", f["title"], re.IGNORECASE) else f"{f['title']} (feat. {guests})"
    track = {
        "p": f["name"], "s": f["size"], "m": f["mtime"], "d": int(f["seconds"] * 1000), "t": title, "a": artist, "A": album_artist or f["album_artist"],
        "l": f["album"], "g": record["genre"], "j": record["style"], "y": int(year), "n": f["position"], "c": f["disc"], "b": f["bitrate"] or 0,
        "Y": record.get("released", "") if record.get("released", "")[:4] == year else "",  # only a day of the year it is filed under
        "i": art_id(unit["release"]), "N": unit["tracks"], "k": is_various(album_artist or f["album_artist"]) or unit["kind"] == "Compilation",
        "R": f["release"], "T": f["recording"], "K": unit["kind"], "u": bool(f.get("unconfirmed")), "e": f.get("unconfirmed") or "",
    }
    return spare(track)


def spare(track):
    """An index line without the fields that say nothing; the path, size and date always stay."""
    return {key: value for key, value in track.items() if key in "psm" or value not in (0, "", False, None)}


def wants(record):
    """What a record that is not ready still wants, as the developer view names it."""
    if record["state"] == "waiting for a cover":
        return "Missing artwork"
    first = record["why"][0]
    return ("Incomplete" if first.startswith("not complete") else "Duplicates" if " is in from " in first
            else "No genre" if first == "no genre" else "Problems")


def unfinished(db, records):
    """Every audio file outside the ready catalogue as (index lines, {thumbnail: cover}): each with the names it has so far,
    its state, and in a few words why it is in that state."""
    # A file counted into a ready record from another folder is in the ready catalogue, whatever its own folder's record wants;
    # one counted into a record that waits for a cover waits with it, whatever the record of its own folder is held back for.
    # A file a better copy stands in for can go.
    rank = lambda record: (record["state"] == "ready", record["state"] == "waiting for a cover", not record["why"] or " is in from " not in record["why"][0])
    placed = {f["file"]: (None, 0, "") if record["state"] == "ready" else (wants(record), record["unit"]["tracks"], record_reason(record))
              for record in sorted(records, key=rank) for f in record["tracks"]}
    for record in records:
        for f in record.get("replaced", []):
            if placed.get(f["file"], ("",))[0] is not None:
                placed[f["file"]] = ("Can go", record["unit"]["tracks"], f"a better-sounding copy plays in its place on {record['unit']['title']}")
    # What the ready catalogue holds, song by song, for the songs of unfinished records that it holds already.
    finished = collections.defaultdict(list)
    for record in records:
        if record["state"] == "ready":
            for f in record["tracks"]:
                if f["recording"]:
                    finished[f["recording"]].append(f)
                finished[(fold(f["title"]), fold(f["artist"]))].append(f)
    genres = dict(db.execute("SELECT release_group, coalesce(filed, genre) FROM genres").fetchall())  # the first reading, until it is checked
    covers = {row["release"]: row["file"] for row in db.execute("SELECT release, file FROM covers WHERE file IS NOT NULL")}
    dropped = {row["name"] for row in db.execute("SELECT name FROM discarded")}  # by their owner, in the player
    units = {row["key"]: row for row in db.execute("SELECT key, folder, shape, state, title, album, files, tried FROM units")}
    shared = {row[0] for row in db.execute("SELECT folder FROM units GROUP BY folder HAVING count(*) > 1")}
    claimed = {record["unit"]["key"] for record in records if record["unverified"]}
    tracks, art = [], {}
    for f in db.execute(
        "SELECT i.*, f.name, f.size, f.mtime, e.bitrate, e.lossless, coalesce(e.duration, e.fp_duration) AS seconds, e.title AS said_title, "
        "e.artist AS said_artist, e.albumartist AS said_albumartist, e.album AS said_album, e.track AS said_track, e.disc AS said_disc, "
        "e.date AS said_date, e.genre AS said_genre FROM identity i JOIN files f ON f.id = i.file JOIN evidence e ON e.file = i.file "
        "WHERE f.kind = 'audio' AND f.seen = (SELECT max(seen) FROM files)"
    ):
        if f["name"] in dropped:
            continue
        if f["file"] in placed:
            state, of, reason = placed[f["file"]]
        else:
            (state, reason), of = on_its_own(f, units.get(f["unit"]), claimed, shared), 0
        if state is None:
            continue
        if state == "Incomplete" and (copy := held_already(f, finished)):
            state, reason = "Can go", f"the ready list has this song, as good or better, on {copy['album']}"
        if f["state"] == "unknown":  # only what the file says of itself, or failing that its name
            artist, guests, title = f["said_artist"], "", f["said_title"] or f["name"].rpartition("/")[2].rpartition(".")[0]
            album, album_artist, year, genre = f["said_album"], f["said_albumartist"], f["said_date"], f["said_genre"]
            disc, position = f["said_disc"], f["said_track"]
        else:
            artist, guests = (GUEST.split(f["artist"] or "", maxsplit=1) + [""])[:2]
            title, album, album_artist, year, genre = f["title"], f["album"], f["album_artist"], f["year"], genres.get(f["release_group"])
            disc, position = f["disc"], f["position"]
        if guests and not re.search(r"\bfeat", title or "", re.IGNORECASE):
            title = f"{title} (feat. {guests})"
        if f["release"] in covers:
            art[art_id(f["release"]) + ".jpg"] = covers[f["release"]]
        track = {
            "p": f["name"], "s": f["size"], "m": f["mtime"], "d": int((f["seconds"] or 0) * 1000), "t": title, "a": artist, "A": album_artist,
            "l": album, "g": genre, "y": int(str(year)[:4]) if str(year or "")[:4].isdigit() else 0, "n": position, "c": disc,
            "b": f["bitrate"] or 0, "i": art_id(f["release"]) if f["release"] in covers else "", "R": f["release"], "T": f["recording"],
            "K": f["kind"], "N": of, "z": state, "e": reason,
        }
        tracks.append(spare(track))
    return tracks, art


def record_reason(record):
    """Why a record is held back or waits, in the words the player shows."""
    if record["state"] == "waiting for a cover":
        fault = record["cover fault"]
        size = fault.partition("(")[2].rstrip(")").replace("x", "\u00d7")
        text = ("no cover anywhere: not in the Cover Art Archive, its folder or its files" if fault == "none anywhere"
                else "its cover has not been looked for yet" if fault == "not looked for yet"
                else f"the only cover is {size}, under {SHARP} pixels" if fault.startswith("too small")
                else f"the only cover is not square ({size})")
    else:
        text = "; ".join(re.sub(r"^not complete \((\d+) of (\d+) tracks\)$", r"has \1 of its \2 tracks", why)
                         .replace(" is in from ", " is in, from ").replace("this release", "this album") for why in record["why"])
    return ("an unverified album: " if record["unverified"] else "") + text


def on_its_own(f, unit, claimed, shared):
    """(state, why) for a file no record counts in: by what it is alone, and the folder it is in."""
    evidence = json.loads(f["evidence"]) if f["evidence"] else {}
    album = (unit["album"] or unit["folder"].rpartition("/")[2]) if unit else ""
    if f["copy"]:
        return "Duplicates", "a second copy of a track another file in this folder already is"
    if unit and unit["shape"] == "record" and unit["key"] not in claimed and unit["state"] == "unclear":
        tried = sorted(json.loads(unit["tried"] or "[]"), key=lambda t: -(t["sure"] + t["named"]))
        if tried:
            t = tried[0]
            return "Unclear", (f"its folder names \u201c{album}\u201d, but no release fits it: the nearest, {t['title']} ({t['tracks']} tracks), "
                               f"fits {t['sure'] + t['named']} of its {unit['files']} files")
        return "Unclear", f"its folder names \u201c{album}\u201d, but MusicBrainz has no release by that name that fits"
    if unit and unit["shape"] == "record" and unit["key"] not in claimed and unit["state"] == "likely":
        return "Likely", f"in a folder matched only likely to {unit['title']}, and on none of its tracks"
    if f["state"] == "likely":
        if f["how"] == "judge":
            return "Likely", "a model's guess at what it is, which the file's own tags and name do not bear out"
        gap = evidence.get("gap")
        return "Likely", (f"named \u201c{f['title']}\u201d, but {gap:.0f} s off that song's length: perhaps another version" if gap
                          else f"named \u201c{f['title']}\u201d, but its length could not be compared")
    if f["state"] == "certain":
        if unit and unit["shape"] == "record" and unit["state"] == "not found":
            return "Unknown albums", f"the song is known, but its folder, \u201c{album}\u201d, is no album MusicBrainz knows"
        if unit and (unit["shape"] == "record" or unit["folder"] in shared):
            return "Loose songs", "beside an album in its folder, but not on it"
        if unit and unit["files"] > 2:
            return "Loose songs", f"one of {unit['files']} songs from different albums in one folder"
        return "Loose songs", "a song in a folder of its own, not part of an album"
    if "proposal" in evidence:
        proposal = evidence["proposal"]
        return "Unknown", f"a model took it for \u201c{proposal.get('title')}\u201d by {proposal.get('artist')}, which MusicBrainz could not confirm"
    if "heard" in evidence:
        return "Unknown", "MusicBrainz knows no song by its tags or name; a listen to it placed no song either"
    if not (f["said_title"] or f["said_artist"]):
        return "Unknown", "it has no tags, and its file name finds no song in MusicBrainz"
    return "Unknown", "MusicBrainz knows no song by its tags or name"


def held_already(f, finished):
    """The copy the ready catalogue {recording or (title, artist): [files]} has of this song already, if one sounds at least
    as good; None when it has none: the same recording, or the same song by the same artist to within a few seconds of its length.

    Such a file adds nothing that is not there; a box set's few strays whose songs are on the albums, say.
    """
    copies = finished.get(f["recording"], []) if f["recording"] else []
    copies += [c for c in finished.get((fold(f["title"]), fold(f["artist"])), [])
               if f["seconds"] and c["seconds"] and abs(c["seconds"] - f["seconds"]) <= SURE_SECONDS]
    return next((c for c in copies if c["file"] != f["file"] and (c["lossless"] or (not f["lossless"] and (c["bitrate"] or 0) >= 0.9 * (f["bitrate"] or 0)))), None)


def as_gap(record, track):
    """A track of a ready record that the collection does not have, as a line of the player's index: listed, not playable."""
    first = record["tracks"][0]
    stand_in = {"name": "", "size": 0, "mtime": 0, "seconds": track["seconds"], "title": track["title"], "artist": track["artist"],
                "album": first["album"], "album_artist": first["album_artist"], "year": first["year"], "position": track["pos"],
                "disc": track["disc"], "bitrate": 0, "release": record["unit"]["release"], "recording": track["recording"]}
    return {**as_track(record, stand_in), "w": True}


def write_index(index, tracks, ready=False):
    """Write the index the player reads: a heading line, then a line a track, in album order. The ready catalogue's heading
    says it is that, so the player may mark the albums in it that are whole as verified. Music read from a folder here,
    not from a remote, is named in the heading too: a player on this machine plays the files from there."""
    tracks.sort(key=lambda t: (fold(t.get("A") or t.get("a", "")), fold(t.get("l", "")), t.get("R", ""), t.get("c", 0), t.get("n", 0), t["p"]))
    settings = config.load()
    here = {} if settings["remote"] else {"root": os.path.abspath(os.path.expanduser(settings["root"]))}
    with gzip.open(index + ".part", "wt", encoding="utf-8") as f:
        f.write(json.dumps({"v": 1, "generated": int(time.time()), "count": len(tracks), "art": True, **({"ready": True} if ready else {}), **here}) + "\n")
        for track in tracks:
            f.write(json.dumps(track, ensure_ascii=False, separators=(",", ":")) + "\n")
    os.replace(index + ".part", index)


def thumbnails(wanted, covers, art):
    """Bring the art folder to exactly the wanted {thumbnail: cover in the covers folder}; returns how many were drawn."""
    os.makedirs(art, exist_ok=True)
    drawn = 0
    for name in os.listdir(art):
        if name not in wanted:
            os.remove(os.path.join(art, name))
    # The whole cover, on itself blurred to fill the square: a sleeve that is not square is drawn whole, not cut down.
    fill = (f"split[a][b];[a]scale={THUMBNAIL}:{THUMBNAIL}:force_original_aspect_ratio=increase,crop={THUMBNAIL}:{THUMBNAIL},"
            f"boxblur=16:2,eq=brightness=-0.06[back];[b]scale={THUMBNAIL}:{THUMBNAIL}:force_original_aspect_ratio=decrease[front];"
            "[back][front]overlay=(W-w)/2:(H-h)/2")
    for name, cover in wanted.items():
        cover, out = os.path.join(covers, cover), os.path.join(art, name)
        if not os.path.exists(cover) or (os.path.exists(out) and os.path.getmtime(out) >= os.path.getmtime(cover)):
            continue
        done = subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-y", "-i", cover, "-vf", fill, "-frames:v", "1", "-q:v", "3", "-f", "mjpeg",
                               out + ".part"], capture_output=True, timeout=120)
        if done.returncode != 0:
            raise OSError(f"no thumbnail could be drawn from {cover}: {done.stderr.decode('utf-8', 'replace').strip()[:200]}")
        os.replace(out + ".part", out)
        drawn += 1
    return drawn


def write(folder, name, header, rows):
    with open(os.path.join(folder, name), "w", newline="", encoding="utf-8") as f:
        writer = csv.writer(f, delimiter="\t")
        writer.writerow(header)
        writer.writerows(rows)


def summary(records):
    """The few lines the report carries."""
    by = collections.Counter(record["state"] for record in records)
    ready = [record for record in records if record["state"] == "ready"]
    lines = [f"Ready catalogue: {by['ready']:,} releases, {sum(len(record['tracks']) for record in ready):,} tracks.",
             "  what they are: " + (", ".join(f"{kind} {n:,}" for kind, n in collections.Counter(r["unit"]["kind"] for r in ready).most_common()) or "none"),
             f"  good data, waiting for a cover: {by['waiting for a cover']:,}",
             f"  held back: {by['held back']:,}",
             f"  made whole with tracks from elsewhere in the collection: {sum(1 for r in ready if r['lent'] and not r['gaps']):,}",
             f"  in with tracks missing, listed as missing: {sum(1 for r in ready if r['gaps']):,} "
             f"({sum(len(r['gaps']) for r in ready):,} tracks)",
             f"  a better sounding copy put in from elsewhere in the collection: {sum(len(r.get('replaced', [])) for r in ready):,} tracks "
             f"of {sum(1 for r in ready if r.get('replaced')):,} releases",
             f"  singles and EPs held as a folder of a song or two: {sum(1 for r in ready if r['unit']['how'] == 'its songs'):,}",
             f"  verified, every track held and confirmed: {sum(1 for r in ready if not r['gaps'] and not any(f.get('unconfirmed') for f in r['tracks'])):,}; "
             f"unverified records, a folder the reference was not sure of: {sum(1 for r in ready if r['unverified']):,}"]
    reasons = collections.Counter(re.sub(r" from .*| \(.*|(?<= lacks) .*", "", reason) for record in records for reason in record["why"])
    if reasons:
        lines.append("  for: " + ", ".join(f"{reason} {n:,}" for reason, n in reasons.most_common()))
    return "\n".join(lines)


def run(db, mb, folder, covers, log=print):
    os.makedirs(folder, exist_ok=True)
    records = assess(db, mb)
    ready = [record for record in records if record["state"] == "ready"]
    releases = [mb.release(record["unit"]["release"]) for record in ready]  # in the reference's cache since they were matched: nothing is asked
    names, renamed = one_name_each(releases), collections.Counter()
    for record, release in zip(ready, releases):
        record["names"] = named(release, names)
        record["released"] = released(release)
        if record["names"][0] and record["names"][0] != record["unit"]["credited"]:
            renamed[f"{record['unit']['credited']} as {record['names'][0]}"] += 1
    joined = shown_as_one(mb, ready, {release["id"]: release for release in releases})
    dropped = {row["name"] for row in db.execute("SELECT name FROM discarded")}
    tracks = [join(as_track(record, f), joined.get(id(record))) for record in ready for f in record["tracks"]]
    tracks += [join(as_gap(record, track), joined.get(id(record))) for record in ready for track in record["gaps"]]
    shown = {track["i"] for track in tracks}
    drawn = thumbnails({art_id(r["unit"]["release"]) + ".jpg": r["cover"]["file"] for r in ready if art_id(r["unit"]["release"]) in shown},
                       covers, os.path.join(folder, "art"))
    index = os.path.join(folder, "library.jsonl.gz")
    write_index(index, tracks, ready=True)

    rest, rest_art = unfinished(db, records)
    os.makedirs(os.path.join(folder, "unfinished"), exist_ok=True)
    write_index(os.path.join(folder, "unfinished", "library.jsonl.gz"), rest)
    drawn += thumbnails(rest_art, covers, os.path.join(folder, "unfinished", "art"))

    row = lambda r: [(r.get("names") or [None])[0] or r["unit"]["credited"], r["unit"]["title"], r["unit"]["year"], r["unit"]["kind"], r["genre"] or "", len(r["tracks"]),
                     r["unit"]["key"].replace(PART, " # "), RELEASE + r["unit"]["release"]]
    head = ["artist", "release", "year", "kind", "genre", "tracks", "folder", "release link"]
    order = lambda r: (fold(row(r)[0]), fold(r["unit"]["title"]))
    write(folder, "ready.tsv", head + ["other files in the folder left out", "tracks counted in from elsewhere in the collection"],
          [row(r) + [r["left out"], r["lent"]] for r in sorted(ready, key=order)])
    write(folder, "waiting-for-cover.tsv", head + ["cover"],
          [row(r) + [r["cover fault"]] for r in sorted(records, key=order) if r["state"] == "waiting for a cover"])
    write(folder, "missing-tracks.tsv", ["artist", "release", "disc", "track", "title", "length", "release link"],
          [[row(r)[0], r["unit"]["title"], t["disc"], t["pos"], t["title"], f"{int(t['seconds']) // 60}:{int(t['seconds']) % 60:02d}" if t["seconds"] else "",
            RELEASE + r["unit"]["release"]] for r in sorted(ready, key=order) for t in r["gaps"]])
    write(folder, "held-back.tsv", head + ["held back by"],
          [row(r) + ["; ".join(r["why"])] for r in sorted(records, key=order) if r["state"] == "held back"])
    log(summary(records))
    if joined:
        log(f"  shown as one release with others, a disc each: {len(joined):,} releases as {len({j['R'] for j in joined.values()}):,} "
            "(the formats of a single or EP, or the halves of an album held as two folders)")
    if renamed:
        log(f"  {sum(renamed.values()):,} releases filed under the name most of the artist's records print, not as this one prints it: "
            + ", ".join(f"{name} ({n})" if n > 1 else name for name, n in renamed.most_common()))
    log(f"\nIndex of {len(tracks):,} tracks written to {index}; {drawn:,} thumbnails drawn; lists beside it")
    if dropped:
        log(f"{len(dropped):,} files dropped from the library in the player are in neither; they are listed in the report")
    log(f"The other {len(rest):,} files, in the folder unfinished beside it: "
        + ", ".join(f"{state} {n:,}" for state, n in collections.Counter(t["z"] for t in rest).most_common()))


def main(argv=None):
    argparse.ArgumentParser(prog="librarian ready", description=__doc__.split("\n\n")[0]).parse_args(argv)
    run(catalogue.connect(config.path("catalogue.db")), reference.MusicBrainz(config.path("reference.db")), config.path("ready"),
        config.path("covers"), log=lambda text: print(text, flush=True))


if __name__ == "__main__":
    sys.exit(main())
