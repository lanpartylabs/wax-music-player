"""Covers: the front cover of every release the collection holds with certainty, or holds as an unverified record.

    python3 -m librarian artwork [--only FOLDER] [--limit N] [--again] [--jobs N]

For each release that files settled as certain sit on, the Cover Art Archive
is asked for the front cover: of that very release first, then of the same
album in another edition, and if neither is to be had, of each other edition
it holds one of. When the archive has none, or only one that is narrow or
not square, the collection's own art is looked at too: a front cover lying
in the record's folder, and the picture carried inside one of its files.
Failing those, and where librarian.toml allows it (deezer = true), Deezer
is asked for the album: one it holds by the same artist, of the same title,
with as many tracks, is that release, and its cover is taken. Only a record
kept out of the ready catalogue by the want of a cover alone, or let in on a
sleeve that is not square, is asked about there, by its artist and title. The first of these that is square and at
least 500 pixels a side is kept; failing that, the best the archive has, and
only for want of that the collection's own. A cover kept that is small or not square is then
looked for in every other edition the archive holds a front cover of, and
a larger scan of the same picture taken in its place. Covers go in the
covers folder, one JPEG per release, no more than 1200 pixels a side. The
music is only read. A release with no cover anywhere, or only a poor one,
is looked for again a month on, and one the archive would not answer for,
on the next run.
"""

import argparse
import collections
import concurrent.futures
import http.client
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

from . import catalogue, claim, config, match, reference
from .reference import AGENT
from .text import BRACKETED, fold, lead_artist, similar

ARCHIVE = "https://coverartarchive.org/"
DEEZER = "https://api.deezer.com/"
NAMED = 0.9  # the least likeness of artist and of title for an album Deezer holds to be the release, as many tracks long
OTHER_TAKE = re.compile(r"mix|edit|live|acoustic|instrumental|dub|rework|version|demo|session", re.I)  # in brackets: another release
SIDE = 1200  # pixels; the largest size the archive serves ready-made, and the most any kept cover measures
SMALL = 500  # pixels; a cover smaller than this is kept only for want of a better, and listed in the report as small
SMALLEST = 200  # pixels; one of the collection's own pictures narrower than this is not kept at all
SQUARE = 1.25  # longer side over shorter; beyond this a picture is a scan of something other than a square sleeve
LARGEST = 40 * 2**20  # bytes read of any one picture
LOOK_AGAIN = 30 * 86400  # a release without a cover, or with a poor one, is asked about again after this long
EDITIONS = 20  # other editions of a release looked at, at most, for a cover it lacks or a larger scan of a poor one
GLANCE = 16  # pixels a side two pictures are shrunk to, in grey, to tell whether they show the same cover
LIKENESS = 25  # the most two such glances may differ, on average, out of 255, and still show the same cover: one scan against
# another of the same sleeve came to 0-24 (a sticker, a logo, a crop, the colour), another edition's other sleeve to 30 and more
FRONT = ("cover", "folder", "front", "albumart")  # in an image file's name: it is the front cover
NOT_FRONT = ("back", "inlay", "inside", "booklet", "tray", "spine", "disc", "label")  # in its name: it is something else
SOURCES = {"release": "archive (this release)", "release group": "archive (another edition)", "edition": "archive (another edition)",
           "folder": "the record's folder", "embedded": "inside a file", "deezer": "Deezer", None: "none anywhere"}


class Unreached(Exception):
    """The archive did not answer; the release is left to be asked about on the next run."""


def get(url):
    """What is at an address, or None when nothing is there."""
    why = ""
    for attempt in range(4):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers={"User-Agent": AGENT}), timeout=60) as response:
                return response.read(LARGEST)
        except urllib.error.HTTPError as e:
            if e.code in (400, 403, 404):  # no such entry, one withdrawn, or no art for it
                return None
            why = f"HTTP {e.code}"
        except (urllib.error.URLError, TimeoutError, ConnectionError, http.client.HTTPException) as e:
            why = f"{type(e).__name__}: {e}"
        time.sleep(5 * (attempt + 1))
    raise Unreached(f"{why} from {url}")


def size(jpeg):
    """(width, height) from a JPEG's frame header, or None when it has none."""
    at = 2
    while jpeg[:2] == b"\xff\xd8" and at + 9 <= len(jpeg) and jpeg[at] == 0xFF:
        marker = jpeg[at + 1]
        if marker == 0xFF:  # padding before a marker
            at += 1
        elif marker == 0x01 or 0xD0 <= marker <= 0xD8:  # markers that stand alone
            at += 2
        elif 0xC0 <= marker <= 0xCF and marker not in (0xC4, 0xC8, 0xCC):  # a start of frame
            return int.from_bytes(jpeg[at + 7:at + 9], "big"), int.from_bytes(jpeg[at + 5:at + 7], "big")
        else:
            at += 2 + int.from_bytes(jpeg[at + 2:at + 4], "big")
    return None


def as_cover(picture=None, path=None):
    """A picture as a JPEG no more than SIDE a side, from its bytes or from a file: an image, or music carrying one. None if nothing decodes."""
    if picture and (measured := size(picture)) and max(measured) <= SIDE:
        return picture
    fit = f"scale='min({SIDE},iw)':'min({SIDE},ih)':force_original_aspect_ratio=decrease"
    try:
        out = subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-i", path or "pipe:0", "-an", "-map", "0:v:0", "-frames:v", "1", "-vf", fit,
                              "-q:v", "2", "-f", "mjpeg", "pipe:1"], input=picture or b"", capture_output=True, timeout=120)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return out.stdout if out.returncode == 0 and size(out.stdout) else None


def shape(jpeg):
    """(whether it is near enough square, its longer side in pixels). The longer, so that a scan a few pixels off square is not called small."""
    width, height = size(jpeg)
    return max(width, height) <= SQUARE * min(width, height), max(width, height)


def glance(jpeg):
    """A picture shrunk to GLANCE pixels a side in grey, its brightness taken out, as a list of numbers; None if it does not decode."""
    try:
        out = subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-i", "pipe:0", "-vf", f"scale={GLANCE}:{GLANCE}:flags=area,format=gray",
                              "-f", "rawvideo", "pipe:1"], input=jpeg, capture_output=True, timeout=60)
    except (OSError, subprocess.TimeoutExpired):
        return None
    if out.returncode != 0 or len(out.stdout) != GLANCE * GLANCE:
        return None
    mean = sum(out.stdout) / len(out.stdout)
    return [value - mean for value in out.stdout]


def unlikeness(one, other):
    """How far apart two pictures look, on average, out of 255; None when either does not decode."""
    a, b = glance(one), glance(other)
    return sum(abs(x - y) for x, y in zip(a, b)) / len(a) if a and b else None


def from_archive(kind, mbid):
    """(JPEG, its address) for the front cover the archive holds of a release or a release group; (None, None) when it holds none."""
    listing = get(f"{ARCHIVE}{kind}/{mbid}")
    try:
        images = json.loads(listing).get("images") or [] if listing else []
    except ValueError:
        images = []
    unreached = None
    for image in images:
        if image.get("front"):
            sizes = image.get("thumbnails") or {}
            # The size kept, then the picture as it was sent in, then the next size down: the archive at times fails on one and not another.
            for address in filter(None, (sizes.get(str(SIDE)), image.get("image"), sizes.get("large"))):
                address = address.replace("http://", "https://", 1)
                try:
                    picture = get(address)
                except Unreached as e:
                    unreached = e
                    continue
                jpeg = as_cover(picture) if picture else None
                if jpeg:
                    return jpeg, address
    if unreached:
        raise unreached
    return None, None


def editions(mb, want):
    """The other editions of a release that the archive holds a front cover of, those with as many tracks first."""
    try:
        releases = mb.editions(want["group"]) if want["group"] else []
    except reference.Unavailable:
        return []
    tracks = lambda release: sum(medium.get("track-count") or 0 for medium in release.get("media") or [])
    own = next((tracks(release) for release in releases if release["id"] == want["release"]), None)
    others = [release for release in releases if release["id"] != want["release"] and (release.get("cover-art-archive") or {}).get("front")]
    return [release["id"] for release in sorted(others, key=lambda release: tracks(release) != own)][:EDITIONS]


def from_editions(want, jpeg):
    """A larger or squarer scan of a poor cover from another edition, as (JPEG, its address, "edition"); None when there is none.

    Another edition's cover is taken only when it shows the same picture: a remaster or a deluxe edition may wear another.
    Of those good enough to keep outright the likest is taken; failing any, the best of the rest.
    """
    held, scans = shape(jpeg), []
    for mbid in want["editions"]:
        try:
            other, address = from_archive("release", mbid)
        except Unreached:
            continue
        if other and shape(other) > held and (apart := unlikeness(jpeg, other)) is not None and apart <= LIKENESS:
            scans.append((shape(other) >= (True, SMALL), apart, shape(other), other, address))
    if not scans:
        return None
    good = [scan for scan in scans if scan[0]]
    best = min(good, key=lambda scan: scan[1]) if good else max(scans, key=lambda scan: scan[2])
    return best[3], best[4], "edition"


def from_deezer(want):
    """(JPEG, its address) for the cover of the album Deezer holds that is this release: by the same artist, of the same
    title, with as many tracks. (None, None) when it holds no such album.

    A title that adds a take in brackets, "Some Song (Some DJ Remix)", is another release, though like the name. Deezer
    is asked by artist and title first, then, where that finds nothing, in plain words, which forgive a misspelt title.
    """
    artist, title, tracks = want["named"]
    likeness = lambda album: (similar(album.get("title") or "", title),
                              max(similar((album.get("artist") or {}).get("name") or "", name) for name in (artist, lead_artist(artist))))
    added = lambda album: " ".join(part for part in BRACKETED.findall(album.get("title") or "") if fold(part) not in fold(title))
    fielded = " ".join(f'{field}:"{value.replace(chr(34), "")}"' for field, value in (("artist", lead_artist(artist)), ("album", title)))
    for query in (fielded, f"{lead_artist(artist)} {title}"):
        listing = get(DEEZER + "search/album?" + urllib.parse.urlencode({"q": query, "limit": 10}))
        try:
            answer = json.loads(listing) if listing else {}
        except ValueError:
            answer = {}
        if answer.get("error"):  # it answers a burst of questions, or a fault, with an error in place of albums
            raise Unreached(f"Deezer: {answer['error'].get('message') or answer['error']}")
        fits = [album for album in answer.get("data") or [] if album.get("nb_tracks") == tracks and album.get("cover_xl")
                and min(likeness(album)) >= NAMED and not OTHER_TAKE.search(added(album))]
        if fits:
            break
    if not fits:
        return None, None
    address = max(fits, key=likeness)["cover_xl"]
    picture = get(address)
    jpeg = as_cover(picture) if picture else None
    return (jpeg, address) if jpeg else (None, None)


def from_collection(root, want):
    """(JPEG, the file it came from, "folder" or "embedded") for the largest square picture of a release in the collection itself."""
    best = (0, None, None, None)
    for source, files in (("folder", want["images"]), ("embedded", want["songs"])):
        for path, name in files:
            jpeg = as_cover(path=os.path.join(root, path))
            square, side = shape(jpeg) if jpeg else (False, 0)
            if square and side >= SMALLEST and side > best[0]:
                best = (side, jpeg, name, source)
    return best[1:]


def find(root, want):
    """One release's cover as (JPEG, where it came from, which kind of source); the JPEG is None when there is none anywhere."""
    found, unreached = [], None
    for source, kind, mbid in (("release", "release", want["release"]), ("release group", "release-group", want["group"])):
        try:
            jpeg, address = from_archive(kind, mbid) if mbid else (None, None)
        except Unreached as e:  # one edition's entry failing does not stop the other being asked for
            unreached = e
            continue
        if jpeg:
            found.append((jpeg, address, source))
            if shape(jpeg) >= (True, SMALL):
                return found[-1]
    # Neither to be had, the edition the archive shows for the album failing to come, say: each other edition's own.
    for mbid in want["editions"] if not found else ():
        try:
            jpeg, address = from_archive("release", mbid)
        except Unreached as e:
            unreached = e
            continue
        if jpeg:
            found.append((jpeg, address, "edition"))
            if shape(jpeg) >= (True, SMALL):
                return found[-1]
    own = from_collection(root, want)
    if own[0] and shape(own[0]) >= (True, SMALL):
        return own
    try:
        jpeg, address = from_deezer(want) if want.get("deezer") else (None, None)
    except Unreached as e:
        jpeg, unreached = None, unreached or e
    if jpeg and shape(jpeg) >= (True, SMALL):
        return jpeg, address, "deezer"
    if found:
        return max(found, key=lambda cover: shape(cover[0]))  # the archive's squarest, then largest
    if unreached:  # nothing good in hand and the archive not heard from: ask again next run, do not settle for less yet
        raise unreached
    return own


def wanted(db, only=None, claimed=()):
    """Every release that certain files sit on, or that a folder claims (as (release, release group, folder)), with the
    collection's own pictures of it, as {release: {...}}."""
    releases = {}
    for row in db.execute(
        "SELECT i.release, max(i.release_group) AS grp FROM identity i JOIN files f ON f.id = i.file "
        "WHERE i.state = 'certain' AND i.release IS NOT NULL AND f.name LIKE ? ESCAPE '\\' GROUP BY i.release ORDER BY i.release",
        ((only or "").replace("\\", "\\\\").replace("%", "\\%").replace("_", "\\_") + "%",),
    ):
        releases[row["release"]] = {"release": row["release"], "group": row["grp"], "images": [], "songs": []}

    for row in db.execute(  # the first files of each release that carry a picture
        "SELECT i.release, f.path, f.name FROM identity i JOIN files f ON f.id = i.file JOIN evidence e ON e.file = i.file "
        "WHERE i.state = 'certain' AND i.release IS NOT NULL AND e.opens = 1 AND e.pictures > 0 ORDER BY i.release, i.disc, i.position, f.name"
    ):
        want = releases.get(row["release"])
        if want and len(want["songs"]) < 2:
            want["songs"].append((row["path"], row["name"]))

    lying = collections.defaultdict(list)  # folder: the front covers lying in it, largest file first
    for row in db.execute("SELECT path, name, size FROM files WHERE kind = 'image' AND seen = (SELECT max(seen) FROM files) ORDER BY size DESC"):
        folder, _, name = row["name"].rpartition("/")
        name = name.lower()
        if any(word in name for word in FRONT) and not any(word in name for word in NOT_FRONT):
            lying[folder].append((row["path"], row["name"]))
    for release, group, folder in claimed:
        if folder.startswith(only or ""):
            releases.setdefault(release, {"release": release, "group": group, "images": [], "songs": []})
    for row in [*db.execute("SELECT release, folder FROM units WHERE shape = 'record' AND state = 'certain' AND release IS NOT NULL ORDER BY key"),
                *({"release": release, "folder": folder} for release, _, folder in claimed)]:
        want = releases.get(row["release"])
        if want:
            want["images"] = (want["images"] + lying[row["folder"]])[:3]
    return releases


def run(db, mb, root, folder, only=None, limit=None, again=False, jobs=4, log=print):
    os.makedirs(folder, exist_ok=True)
    claimed = [(found["release"]["id"], (found["release"].get("release-group") or {}).get("id"), found["unit"]["folder"])
               for found in claim.claims(db, mb).values()]
    releases = wanted(db, only, claimed)
    now = int(time.time())
    for row in db.execute("SELECT release, file, looked, width, height FROM covers"):
        if row["release"] in releases:
            held = bool(row["file"]) and os.path.exists(os.path.join(folder, row["file"]))
            good = held and max(row["width"], row["height"]) >= SMALL and max(row["width"], row["height"]) <= SQUARE * min(row["width"], row["height"])
            recent = not again and now - row["looked"] < LOOK_AGAIN
            releases[row["release"]]["settled"] = good or (recent and (held or not row["file"]))
    todo = [want for want in releases.values() if not want.get("settled")]
    deezer = config.load()["deezer"]
    todo = todo[:limit] if limit else todo
    log(f"{len(releases):,} releases held with certainty; {len(todo):,} to find a cover for")
    # Deezer is asked only about the records that the want of a cover alone keeps out of the ready catalogue, or that it
    # lets in only on a sleeve that is not square, a promo's card with its track list on it, say.
    from . import ready  # here, for ready draws on this module
    sleeve = lambda cover: bool(cover and cover["file"]) and max(cover["width"], cover["height"]) > SQUARE * min(cover["width"], cover["height"])
    waiting = {r["unit"]["release"] for r in ready.assess(db, mb) if r["state"] == "waiting for a cover"
               or (r["state"] == "ready" and sleeve(r["cover"]))} if todo else set()
    for want in todo:  # here, not in the pool: the reference's cache is one connection
        release = mb.release(want["release"])
        want["named"] = (reference.credit(release), release.get("title") or "", len(match.tracks_of(release)))
        want["editions"] = editions(mb, want)
        want["deezer"] = deezer and want["release"] in waiting

    def attempt(want):
        try:
            return find(root, want)
        except Unreached as e:
            return e

    def keep(want, found):
        jpeg, origin, source = found
        name = want["release"] + ".jpg" if jpeg else None
        if jpeg:
            with open(os.path.join(folder, name + ".part"), "wb") as f:
                f.write(jpeg)
            os.replace(os.path.join(folder, name + ".part"), os.path.join(folder, name))
        width, height = size(jpeg) if jpeg else (None, None)
        db.execute("INSERT OR REPLACE INTO covers (release, release_group, looked, source, origin, file, width, height, bytes) "
                   "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                   (want["release"], want["group"], int(time.time()), source, origin, name, width, height, len(jpeg) if jpeg else None))
        db.commit()  # at once: a write left open while the next cover is fetched locks the other stages out of the catalogue

    outcomes, poor, began = collections.Counter(), [], time.time()
    with concurrent.futures.ThreadPoolExecutor(jobs) as pool:
        for n, (want, found) in enumerate(zip(todo, pool.map(attempt, todo)), 1):
            if isinstance(found, Unreached):
                outcomes["archive not reached"] += 1
                if outcomes["archive not reached"] <= 5:
                    log(f"not reached: {found}")
            else:
                keep(want, found)
                outcomes[SOURCES[found[2]]] += 1
                if found[0] and shape(found[0]) < (True, SMALL):
                    poor.append((want, found))
            if n % 250 == 0:
                log(f"{n:,} of {len(todo):,} releases, about {(len(todo) - n) * (time.time() - began) / n / 60:.0f} min left | "
                    + ", ".join(f"{name} {count:,}" for name, count in sorted(outcomes.items())))

        bettered = 0
        for (want, found), better in zip(poor, pool.map(lambda pair: from_editions(pair[0], pair[1][0]), poor)):
            if better:
                keep(want, better)
                outcomes[SOURCES[found[2]]] -= 1
                outcomes[SOURCES[better[2]]] += 1
                bettered += 1
    db.commit()
    log(f"looked for {len(todo):,} covers in {(time.time() - began) / 60:.1f} min: "
        + (", ".join(f"{name} {count:,}" for name, count in sorted(outcomes.items()) if count) or "nothing to do")
        + (f"; of {len(poor):,} small or not square, {bettered:,} found larger in another edition" if poor else ""))


def main(argv=None):
    parser = argparse.ArgumentParser(prog="librarian artwork", description=__doc__.split("\n\n")[0])
    parser.add_argument("--only", help="look only for covers of what is under this folder")
    parser.add_argument("--limit", type=int, help="look for at most this many covers")
    parser.add_argument("--again", action="store_true", help="look again for the releases found without a cover, or with a poor one")
    parser.add_argument("--jobs", type=int, default=4, help="covers fetched at once")
    args = parser.parse_args(argv)
    db = catalogue.connect(config.path("catalogue.db"))
    run(db, reference.MusicBrainz(config.path("reference.db")), os.fsencode(os.path.abspath(config.library())), config.path("covers"),
        args.only, args.limit, args.again, args.jobs,
        log=lambda text: print(text, flush=True))


if __name__ == "__main__":
    sys.exit(main())
