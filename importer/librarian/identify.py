"""Identify every file: which release it is from and which track of it.

    python3 -m librarian identify [--only FOLDER] [--limit N] [--redo]

Takes the collection a unit at a time. A unit that looks like one record is
compared with candidate releases, found from the MusicBrainz ids in its
tags, from its fingerprints, from a search on what it claims to be, and from
looking its songs up one by one. Loose files are looked up as single songs
and given the release they first came out on. Nothing is decided on a name
alone: a file is "certain" only when its length or its audio agrees too.

Safe to stop and run again; units already settled are skipped.
"""

import argparse
import collections
import json
import sys
import time

from . import acoustid, catalogue, config, match, reference
from .match import PART, SURE_SECONDS
from .reference import credit, credited, phrase
from .text import fold, is_various, plain_title, similar, tidy_album

LOOKUPS = 4  # releases compared in full from one search
FORMATS = 3  # editions of each size tried for a single or an EP, whose formats differ in their songs
SAMPLE = 5  # songs looked up to find a record that a search by name missed
NEAR_SECONDS = 30  # a file this far in length from a song of the name it gives may be another version of it; further is no match
HOME_ORDER = ("Album", "EP", "Single", "Soundtrack", "Live", "Remix", "Compilation", "Mixtape/Street", "DJ-mix", "Demo", "Other")


def artist_fits(artist, entity):
    return any(similar(artist, name) >= 0.7 for name in credited(entity) if name)


def find_releases(mb, artist, album, various, wanted):
    """Releases worth comparing track by track, the best fit for `wanted` tracks first."""
    tidy = tidy_album(album, artist)
    by = f" AND artist:{phrase(artist)}" if artist and not various else ""
    queries = [(f"release:{phrase(album)}{by}", 25), (f"release:{phrase(tidy)}{by}", 25), (f"release:({fold(tidy)}){by}", 25)]
    if by:  # the artist may be misspelt or run together: search on the album alone and compare the artists found
        queries.append((f"release:{phrase(tidy)}", 100))
    found, asked = [], set()
    for query, limit in queries:
        if not fold(tidy) or query in asked:
            continue
        asked.add(query)
        found = [
            r for r in mb.search_releases(query, limit)
            if similar(tidy, r.get("title", "")) >= 0.55 and (various or not artist or artist_fits(artist, r))
        ]
        if found:
            break

    if not found and by and fold(tidy):  # the reference may spell the artist another way, beyond what likeness catches
        known = find_artist(mb, artist)
        if known:
            for query in (f"release:{phrase(tidy)} AND arid:{known[1]}", f"release:({fold(tidy)}) AND arid:{known[1]}"):
                found = [r for r in mb.search_releases(query, 25) if similar(tidy, r.get("title", "")) >= 0.55]
                if found:
                    break

    def fit(r):
        count = r.get("track-count") or 0
        return (count != wanted, count < wanted, abs(count - wanted), -int(r.get("score") or 0), r.get("date") or "9999")

    seen, best = set(), []
    for r in sorted(found, key=fit):  # pressings with the same track count share a track list; one will do
        key = ((r.get("release-group") or {}).get("id"), r.get("track-count"))
        if key not in seen:
            seen.add(key)
            best.append(r["id"])
    return best[:LOOKUPS]


def other_editions(mb, group, wanted, seen):
    """Other releases of the same record, those nearest `wanted` tracks first, one of each size; of a single or an EP,
    up to three of each size, since its formats hold different songs at one length, CD1 and CD2 four tracks each."""
    def size(r):
        return sum(m.get("track-count") or 0 for m in r.get("media") or [] if m.get("format") not in match.VIDEO)

    found = sorted(
        (r for r in mb.editions(group) if r.get("id") not in seen),
        key=lambda r: (abs(size(r) - wanted), r.get("status") != "Official", r.get("date") or "9999"),
    )
    sizes, ids = collections.Counter(), []
    for r in found:
        if sizes[size(r)] < (FORMATS if size(r) < match.FEW_TRACKS else 1):
            sizes[size(r)] += 1
            ids.append(r["id"])
    return ids[:3 if wanted >= match.FEW_TRACKS else 2 * FORMATS]


def find_artist(mb, name):
    """The reference's artist for a name the files spell their own way ("milabrook", "the night-owls"): (name, id), or None."""
    squashed = fold(name).replace(" ", "")
    for found in mb.get("artist/", query=phrase(name), limit=5).get("artists", []):
        names = [found.get("name", "")] + [alias.get("name", "") for alias in found.get("aliases") or []]
        if squashed and found.get("id") and any(fold(known).replace(" ", "") == squashed for known in names):
            return found.get("name", ""), found["id"]
    return None


def find_recordings(mb, f, artist="", queries=4):
    """Recordings whose name and artist fit a file: (those of its length too, all of them)."""
    claims = match.song_claims(f, artist)[:queries]
    named = []

    def search(title, by, fits_artist):
        found = mb.search_recordings(f"recording:{phrase(title)} AND {by}", limit=100)
        fits = [r for r in found if similar(title, r.get("title", "")) >= 0.8 and fits_artist(r)]
        exact = [r for r in fits if f["seconds"] and r.get("length") and abs(r["length"] / 1000 - f["seconds"]) <= SURE_SECONDS]
        # A recording titled just as the file says beats one with a "(live)" or "(remix)" the file never mentions.
        plain = [r for r in exact if fold(r.get("title", "")) in (fold(title), fold(plain_title(title)))]
        return plain or exact, fits

    for title, who in claims:
        exact, fits = search(title, f"artist:{phrase(who)}", lambda r: artist_fits(who, r))
        if exact:
            return exact, fits
        named += fits
    if not named and claims:  # nothing by that artist name at all: the reference may spell the artist another way
        title, who = claims[0]
        known = find_artist(mb, who)
        if known and fold(known[0]) != fold(who):
            return search(title, f"arid:{known[1]}", lambda r: True)
    return [], named


def releases_by_songs(mb, files, artist):
    """Releases holding several of a group's songs, found by looking a few of the songs up one at a time."""
    sample = files[:: max(1, len(files) // SAMPLE)][:SAMPLE]
    votes = collections.Counter()
    for f in sample:
        exact, _ = find_recordings(mb, f, artist, queries=2)
        votes.update({release["id"] for r in exact for release in r.get("releases") or []})
    need = max(2, (len(sample) + 1) // 2)
    return [release for release, count in votes.most_common(3) if count >= need]


def home(recordings, album=""):
    """Where a song belongs, as (recording, release): the release its own album tag names, else its original
    album, else the EP or single it came out on, and a compilation only when it appeared nowhere else."""
    best = None
    for recording in recordings:
        for release in recording.get("releases") or []:
            group = release.get("release-group") or {}
            kind = reference.kind(group)
            key = (
                not (album and similar(tidy_album(album), release.get("title", "")) >= 0.8),
                release.get("status") not in (None, "Official"),
                is_various(credit(release)),
                HOME_ORDER.index(kind) if kind in HOME_ORDER else len(HOME_ORDER),
                release.get("date") or "9999",
                release.get("id", ""),
            )
            if best is None or key < best[0]:
                best = (key, recording, release)
    return best[1:] if best else None


def settle(mb, f, recordings, state, how):
    """The identity of a file known to be one of `recordings`."""
    gaps = [abs(r["length"] / 1000 - f["seconds"]) for r in recordings if r.get("length") and f["seconds"]]
    evidence = {"gap": round(min(gaps), 1) if gaps else None, "recordings": len(recordings)}
    chosen = home(recordings, f["album"])
    if not chosen:  # known to the reference, but on no release
        recording = recordings[0]
        return {"state": state, "how": how, "recording": recording.get("id"), "title": recording.get("title"),
                "artist": credit(recording), "evidence": evidence}
    recording, release = chosen
    full = mb.release(release["id"])
    track = next((t for t in match.tracks_of(full, video=True) if t["recording"] == recording.get("id")), None) or {}
    group = full.get("release-group") or release.get("release-group") or {}
    return {
        "state": state, "how": how, "recording": recording.get("id"),
        "release": release["id"], "release_group": group.get("id"),
        "track": track.get("id"), "disc": track.get("disc"), "position": track.get("pos"),
        "title": track.get("title") or recording.get("title"),
        "artist": track.get("artist") or credit(recording),
        "album": full.get("title") or release.get("title"),
        "album_artist": credit(full) or credit(release) or credit(recording),
        "year": (group.get("first-release-date") or full.get("date") or release.get("date") or "")[:4],
        "kind": reference.kind(group),
        "evidence": evidence,
    }


def close(recording, f, seconds):
    return not recording.get("length") or not f["seconds"] or abs(recording["length"] / 1000 - f["seconds"]) <= seconds


def identify_song(mb, f, artist=""):
    """One file looked up as a single song; None when the reference does not know it by what the file offers."""
    if f["mb_recording"]:
        recording = mb.recording(f["mb_recording"])
        if recording.get("id") and close(recording, f, 3 * SURE_SECONDS):
            return settle(mb, f, [recording], "certain", "ids")
    for heard, score in sorted(f["heard"].items(), key=lambda item: -item[1])[:2]:
        if score >= match.HEARD:
            recording = mb.recording(heard)
            if recording.get("id") and close(recording, f, 2 * SURE_SECONDS):
                return settle(mb, f, [recording], "certain", "fingerprint")
    proposed = bool(f["names"])  # a title put forward by a model is among the claims
    exact, named = find_recordings(mb, f, artist)
    if exact:
        if not proposed:
            return settle(mb, f, exact, "certain", "tags")
        # A model's guess that happens to fit some recording's length is not proof. It is certain only when
        # something of the file's own, its tags, name or folder, says the same.
        return settle(mb, f, exact, "certain" if backed(f, artist, exact) else "likely", "judge")
    if named:  # the file's own name for a known song, at another length: some other version of it
        def gap(r):
            return abs(r["length"] / 1000 - f["seconds"]) if r.get("length") and f["seconds"] else 1e9
        nearest = min(named, key=gap)
        own = match.names_of(f, artist, proposed=False)
        if gap(nearest) <= NEAR_SECONDS and (not proposed or any(similar(name, nearest.get("title", "")) >= 0.75 for name in own)):
            return settle(mb, f, [nearest], "likely", "tags")
    return None


def backed(f, artist, recordings):
    """Whether the file itself, by its tags, name or folder, agrees with any of the recordings a model's proposal led to."""
    own_names = match.names_of(f, artist, proposed=False)
    own_artists = [name for name in (f["artist"], f["albumartist"], artist, *config.inner(f["dir"]).split("/")) if name]
    for recording in recordings:
        if any(similar(name, recording.get("title", "")) >= 0.75 for name in own_names):
            return True
        if any(artist_fits(name, recording) for name in own_artists):
            return True
    return False


def identify_record(mb, files, artist, album, various, extra=()):
    """Find the release a group of files is. Returns (the best comparison or None, every comparison in brief)."""
    wanted = match.wanted_count(files)
    best, tried, seen, outcomes = None, [], set(), []

    def consider(release_id, source):
        nonlocal best
        if not release_id or release_id in seen:
            return
        seen.add(release_id)
        release = mb.release(release_id)
        if not match.tracks_of(release):
            return
        outcome = match.compare(files, release, artist)
        outcome["source"] = source
        outcomes.append(outcome)
        tried.append({"release": release_id, "title": outcome["title"], "artist": outcome["artist"], "tracks": outcome["tracks"],
                      "sure": outcome["sure"], "named": outcome["named"], "verdict": outcome["verdict"], "from": source})
        if best is None or outcome["key"] < best["key"]:
            best = outcome

    def whole():
        return best is not None and best["verdict"] == "certain" and not best["missing"] and not best["unmatched"]

    def settled():
        return best is not None and best["verdict"] in ("certain", "likely")

    tagged = collections.Counter(f["mb_release"] for f in files if f["mb_release"])
    for release_id, count in tagged.most_common(2):
        if count >= max(2, 0.3 * len(files)):
            consider(release_id, "ids")
    if not whole():
        heard = collections.Counter(release for f in files for release in set(f["heard_releases"]))
        for release_id, count in heard.most_common(3):
            if count >= max(2, 0.5 * len(files)):
                consider(release_id, "fingerprint")
    for release_id in extra:
        consider(release_id, "judge")
    if not whole() and album:
        for release_id in find_releases(mb, artist, album, various, wanted):
            consider(release_id, "search")
            if whole():
                break
    if not settled():
        for release_id in releases_by_songs(mb, files, artist):
            consider(release_id, "songs")
    # Other editions of the record that fits best, if it fits at all: a promo CD with the songs of a pressing the search did not find.
    if (settled() or (best and best["sure"] + best["named"] >= 2)) and (best["missing"] or best["unmatched"]) and best["group"]:
        for release_id in other_editions(mb, best["group"], wanted, seen):
            consider(release_id, "edition")
            if whole():
                break
    fuller = whole_edition(mb, best, outcomes, artist)
    if fuller:
        best = fuller
        next(t for t in tried if t["release"] == fuller["release"]).update(verdict="certain", sure=fuller["sure"], named=fuller["named"])
    return best, tried


def whole_edition(mb, best, outcomes, artist):
    """An edition of the record held whole, where the one settled on is not: the comparison made certain, or None.

    A deluxe edition may have every one of its tracks in the folder and still rank below the standard one, because a
    bonus track or two matched it by name while the reference's length for them is missing or a little off. If each
    of those files is, looked up on its own, certainly that very song, nothing is left in doubt and the fuller edition
    is the record. The same goes for a shorter edition held whole against a longer one that lacks a track.
    """
    if best is None or best["verdict"] != "certain" or not (best["missing"] or best["unmatched"]):
        return None
    placed = lambda outcome: outcome["sure"] + outcome["named"]
    filled = lambda outcome: outcome["tracks"] - len(outcome["missing"])  # tracks of the release that some file is
    # An edition held whole is the record when it fills more tracks than the one settled on; and also when it fills a
    # few fewer than an edition that is not held whole: fourteen of fourteen, not fifteen of nineteen. (Only nearly
    # as many, or a three-track single would claim the folder of an album it was cut from.)
    rivals = [o for o in outcomes if o is not best and o["group"] == best["group"] and o["verdict"] in ("certain", "likely") and not o["missing"]
              and (filled(o) > filled(best) or (best["missing"] and filled(o) >= 0.8 * filled(best)))]
    for outcome in sorted(rivals, key=lambda o: (-filled(o), o["verdict"] != "certain")):
        if outcome["verdict"] == "certain":
            return outcome
        doubted = [row for row in outcome["rows"] if row["grade"] and row["grade"] not in ("sure", "left")]
        for row in doubted:
            song = identify_song(mb, row["file"], artist)
            if not (song and song["state"] == "certain"
                    and (song.get("recording") == row["track"]["recording"] or fold(plain_title(song.get("title") or "")) == fold(plain_title(row["track"]["title"])))):
                break
            row["grade"] = "sure"
        else:
            outcome.update(verdict="certain", sure=placed(outcome), named=0)
            return outcome
    return None


def from_row(row, verdict):
    """The identity of a file matched to a track of its unit's release."""
    f, track = row["file"], row["track"]
    sure = row["grade"] == "sure" or (verdict == "certain" and row["grade"] != "left")
    how = "tags"
    if row["same"]:
        how = "fingerprint" if f["heard"].get(track["recording"], 0) >= match.HEARD else "ids"
    return {
        "state": "certain" if sure else "likely", "how": how, "recording": track["recording"], "track": track["id"],
        "disc": track["disc"], "position": track["pos"], "title": track["title"], "artist": track["artist"], "copy": int(row["copy"]),
        "evidence": {"likeness": row["likeness"], "gap": row["gap"], "placed": row["placed"], "same": row["same"],
                     **({"left_over": True} if row["left"] else {})},
    }


IDENTITY = ("unit state how release release_group recording track disc position title artist album album_artist year kind "
            "copy evidence judged decided").split()
UNIT = ("key folder shape signature files artist album various state how release release_group title credited year kind "
        "tracks missing shelf copies tried proposal judged decided").split()


RANK = {"unknown": 0, "likely": 1, "certain": 2}


def save_identity(db, file, unit, found, judged=0, keep=True):
    """Record what a file is. With `keep`, what a model settled for it earlier stands unless this is better.

    The reference looks at a whole folder again whenever one file in it changes. That must not undo, for the files
    that did not change, what a model worked out or heard: asking again costs money and may not come out the same.
    """
    found = dict(found or {"state": "unknown"}, unit=unit, judged=judged, decided=int(time.time()))
    if keep and not judged:
        before = db.execute("SELECT state, judged FROM identity WHERE file = ?", (file,)).fetchone()
        if before and before["judged"] and RANK[before["state"]] >= RANK[found["state"]]:
            db.execute("UPDATE identity SET unit = ? WHERE file = ?", (unit, file))
            return
    found["evidence"] = json.dumps(found.get("evidence")) if found.get("evidence") is not None else None
    found.setdefault("copy", 0)
    db.execute(f"INSERT OR REPLACE INTO identity (file, {', '.join(IDENTITY)}) VALUES (?, {', '.join('?' for _ in IDENTITY)})",
               [file] + [found.get(column) for column in IDENTITY])


def save_unit(db, unit):
    unit = dict(unit, decided=int(time.time()))
    for column in ("tried", "proposal"):
        unit[column] = json.dumps(unit[column]) if unit.get(column) else None
    unit.setdefault("judged", 0)
    db.execute(f"INSERT OR REPLACE INTO units ({', '.join(UNIT)}) VALUES ({', '.join('?' for _ in UNIT)})", [unit.get(column) for column in UNIT])


def save_record(db, mb, key, files, artist, album, various, best, tried, judged=0, proposal=None):
    """Store what a record unit turned out to be, and what each of its files is."""
    unit = {"key": key, "folder": key.split(PART)[0], "shape": "record", "signature": match.signature(files), "files": len(files),
            "artist": artist, "album": album, "various": int(various), "tried": tried, "judged": judged, "proposal": proposal}
    settled = best is not None and best["verdict"] != "unclear"
    # Everything is asked of the reference before anything is written, so the catalogue is never held mid-change.
    if settled:
        shared = {"release": best["release"], "release_group": best["group"], "album": best["title"], "album_artist": best["artist"],
                  "year": best["year"], "kind": best["kind"]}
        found, placed = {}, set()
        for row in best["rows"]:
            if row["grade"]:
                placed.add(row["file"]["id"])  # on the release itself: that outranks whatever was thought of the file alone
                found[row["file"]["id"]] = {**shared, **from_row(row, best["verdict"])}
                if best["source"] == "judge" and found[row["file"]["id"]]["how"] == "tags":
                    found[row["file"]["id"]]["how"] = "judge"
            else:  # in the folder but not on the release
                found[row["file"]["id"]] = identify_song(mb, row["file"], artist)
        unit.update(state=best["verdict"], how=best["source"], release=best["release"], release_group=best["group"],
                    title=best["title"], credited=best["artist"], year=best["year"], kind=best["kind"], tracks=best["tracks"],
                    missing=len(best["missing"]), shelf=best["shelf"], copies=best["copies"])
    else:  # no record to file them under yet; each may still be known as a song
        found, placed = {f["id"]: identify_song(mb, f, artist) for f in files}, set()
        unit["state"] = "unclear" if best else "not found"

    save_unit(db, unit)
    for file, identity in found.items():
        save_identity(db, file, key, identity, judged, keep=file not in placed)
    db.execute("DELETE FROM wanted WHERE unit = ?", (key,))
    if settled and best["shelf"] == "missing 1-2":
        for t in best["missing"]:
            db.execute("INSERT OR REPLACE INTO wanted (unit, release, disc, position, recording, title, artist, seconds) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                       (key, best["release"], t["disc"], t["pos"], t["recording"], t["title"], t["artist"], t["seconds"]))
    return unit["state"]


def load_files(db):
    """Every audio file in the collection as the matcher sees it."""
    heard = acoustid.load(db)
    rows = db.execute(
        "SELECT f.id, f.name, f.size, f.mtime, e.duration, e.doubtful, e.fp_duration, e.title, e.artist, e.albumartist, e.album, "
        "e.track, e.disc, e.mb_recording, e.mb_release, e.mb_track FROM files f JOIN evidence e ON e.file = f.id "
        "WHERE f.kind = 'audio' AND f.seen = (SELECT max(seen) FROM files) ORDER BY f.name"
    )
    return [match.as_file(row, heard.get(row["id"])) for row in rows]


def find_held(db):
    """For each track a record lacks, look for a file elsewhere in the collection that is that track."""
    known = collections.defaultdict(list)
    for row in db.execute("SELECT file, unit, recording, title, artist FROM identity WHERE state = 'certain' AND title IS NOT NULL"):
        if row["recording"]:
            known[row["recording"]].append(row)
        known[(fold(row["title"]), fold(row["artist"]))].append(row)
    lengths = dict(db.execute("SELECT file, coalesce(duration, fp_duration) FROM evidence"))
    held = 0
    for want in db.execute("SELECT rowid, * FROM wanted").fetchall():
        found = [row for row in known.get(want["recording"], []) if row["unit"] != want["unit"]]
        if not found:  # the same song by name and length, from another pressing
            found = [
                row for row in known.get((fold(want["title"]), fold(want["artist"])), [])
                if row["unit"] != want["unit"] and want["seconds"] and lengths.get(row["file"])
                and abs(lengths[row["file"]] - want["seconds"]) <= SURE_SECONDS
            ]
        db.execute("UPDATE wanted SET held = ? WHERE rowid = ?", (found[0]["file"] if found else None, want["rowid"]))
        held += bool(found)
    db.commit()
    return held


def placed_otherwise(db, mb, units):
    """The record units whose files the matcher, as it now is, would place differently on the release they were settled on.

    Asks the reference nothing new: each release is in its cache since it was matched. What a model led to or settled is left be.
    """
    stored = collections.defaultdict(dict)  # unit: {file: (disc, position, copy)} for the files on its own release
    for row in db.execute("SELECT i.file, i.unit, i.disc, i.position, i.copy FROM identity i JOIN units u ON u.key = i.unit "
                          "WHERE i.release = u.release AND i.judged = 0"):
        stored[row["unit"]][row["file"]] = (row["disc"], row["position"], row["copy"])
    judged = {row["file"] for row in db.execute("SELECT file FROM identity WHERE judged > 0")}
    settled = {row["key"]: row for row in db.execute(
        "SELECT key, release, state, shelf FROM units WHERE shape = 'record' AND state IN ('certain', 'likely') AND how != 'judge' AND judged = 0")}
    changed = set()
    for key, shape, members in units:
        unit = settled.get(key)
        if shape != "record" or not unit:
            continue
        outcome = match.compare(members, mb.release(unit["release"]), match.describe(key, members)[0])
        now = {row["file"]["id"]: (row["track"]["disc"], row["track"]["pos"], int(row["copy"]))
               for row in outcome["rows"] if row["grade"] and row["file"]["id"] not in judged}
        if (outcome["verdict"], outcome["shelf"]) != (unit["state"], unit["shelf"]) or now != stored[key]:
            changed.add(key)
    return changed


def run(db, mb, only=None, limit=None, redo=False, unsettled=False, editions=False, incomplete=False, rematch=False, log=print):
    files = load_files(db)
    units = list(match.form_units(files))
    keys = {key for key, _, _ in units}
    stale = [row["key"] for row in db.execute("SELECT key FROM units") if row["key"] not in keys]
    for key in stale:  # folders that have changed shape or gone
        db.execute("DELETE FROM units WHERE key = ?", (key,))
        db.execute("DELETE FROM wanted WHERE unit = ?", (key,))
    # What is known of files that are no longer listed goes with them.
    db.execute("DELETE FROM identity WHERE file NOT IN (SELECT id FROM files WHERE seen = (SELECT max(seen) FROM files))")
    db.commit()  # never hold the catalogue while waiting on the network
    done = {row["key"]: row["signature"] for row in db.execute("SELECT key, signature FROM units")}
    open_ = {row["key"] for row in db.execute("SELECT key FROM units WHERE state IN ('unclear', 'not found')")} if unsettled else set()
    if editions:  # records settled on one edition though another that was compared held every one of its tracks
        held_whole = lambda tried: any(t["verdict"] == "likely" and t["tracks"] == t["sure"] + t["named"] for t in json.loads(tried or "[]"))
        open_ |= {row["key"] for row in db.execute("SELECT key, tried FROM units WHERE shape = 'record' AND state = 'certain' AND how != 'judge' "
                                                    "AND judged = 0") if held_whole(row["tried"])}
    if incomplete:  # records settled on a release they do not hold the whole of
        # Not those a model led to: the reference alone would not find them again.
        open_ |= {row["key"] for row in db.execute(
            "SELECT key FROM units WHERE shape = 'record' AND state = 'certain' AND shelf != 'complete' AND how != 'judge' AND judged = 0")}
    if rematch:
        open_ |= placed_otherwise(db, mb, units)
    todo = [u for u in units if (redo or u[0] in open_ or done.get(u[0]) != match.signature(u[2])) and (not only or u[0].startswith(only))]
    todo = todo[:limit] if limit else todo
    log(f"{len(files):,} files in {len(units):,} units; {len(todo):,} units to identify")

    counts, began, refused, n = collections.Counter(), time.time(), 0, 0
    for n, (key, shape, members) in enumerate(todo, 1):
        try:
            if shape == "record":
                artist, album, various = match.describe(key, members)
                best, tried = identify_record(mb, members, artist, album, various)
                counts[save_record(db, mb, key, members, artist, album, various, best, tried)] += 1
            else:
                songs = [(f, identify_song(mb, f)) for f in members]
                for f, found in songs:
                    save_identity(db, f["id"], key, found)
                    counts["loose song " + (found or {"state": "unknown"})["state"]] += 1
                save_unit(db, {"key": key, "folder": key.split(PART)[0], "shape": "loose", "signature": match.signature(members),
                               "files": len(members), "state": "loose"})
            db.commit()
            refused = 0
        except reference.Unavailable as e:  # left undecided, so the next run takes it up again
            db.rollback()
            counts["put off, MusicBrainz busy"] += 1
            refused += 1
            if refused >= 5:
                log(f"MusicBrainz has refused five units in a row ({e}); stopping here. The next run carries on from this point.")
                break
        if n % 25 == 0:
            rate = n / (time.time() - began)
            log(f"{n:,} of {len(todo):,} units, {mb.requests:,} requests, about {(len(todo) - n) / rate / 60:.0f} min left | "
                + ", ".join(f"{name} {count}" for name, count in sorted(counts.items())))
    held = find_held(db)
    log(f"looked at {n:,} of {len(todo):,} units in {(time.time() - began) / 60:.1f} min with {mb.requests:,} requests: "
        + (", ".join(f"{name} {count}" for name, count in sorted(counts.items())) or "nothing to do")
        + f"; {held} missing tracks found elsewhere in the collection")
    return counts


def main(argv=None):
    parser = argparse.ArgumentParser(prog="librarian identify", description=__doc__.split("\n\n")[0])
    parser.add_argument("--only", help="identify only units whose folder starts with this")
    parser.add_argument("--limit", type=int, help="identify at most this many units")
    parser.add_argument("--redo", action="store_true", help="look again at every unit, settled or not")
    parser.add_argument("--unsettled", action="store_true", help="look again at the units left unclear or not found")
    parser.add_argument("--editions", action="store_true", help="look again at the records for which another edition was held whole")
    parser.add_argument("--incomplete", action="store_true", help="look again at the records settled on a release they do not hold the whole of")
    parser.add_argument("--rematch", action="store_true", help="look again at the records whose files the matcher now places differently")
    args = parser.parse_args(argv)
    db = catalogue.connect(config.path("catalogue.db"))
    run(db, reference.MusicBrainz(config.path("reference.db")), args.only, args.limit, args.redo, args.unsettled, args.editions, args.incomplete,
        args.rematch, log=lambda text: print(text, flush=True))


if __name__ == "__main__":
    sys.exit(main())
