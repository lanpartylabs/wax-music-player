"""MusicBrainz, the reference the collection is checked against.

Every answer is kept in a local cache, so nothing is asked twice. Requests
are paced to the one a second MusicBrainz allows.
"""

import json
import os
import sqlite3
import time
import urllib.error
import urllib.parse
import urllib.request
import zlib

from . import config

API = "https://musicbrainz.org/ws/2/"
PROJECT = "https://github.com/lanpartylabs/wax-music-player"
# MusicBrainz asks every program to name itself and say how to reach whoever runs it.
AGENT = f"wax-librarian/0.3 ( {config.load()['contact'] or PROJECT} )"
PAUSE = 1.1  # seconds between requests
SEARCH_KEEPS = 60 * 86400  # a search is asked again after this long; a lookup by id is kept for good
RELEASE_INC = "recordings+artist-credits+release-groups+media"
RECORDING_INC = "releases+release-groups+artist-credits+media"


class Unavailable(Exception):
    """MusicBrainz would not answer a question after several minutes of asking; what is in hand is saved."""


class MusicBrainz:
    def __init__(self, cache):
        self.db = sqlite3.connect(os.path.expanduser(cache), timeout=60)
        self.db.execute("PRAGMA journal_mode = WAL")
        self.db.execute("CREATE TABLE IF NOT EXISTS fetched (url TEXT PRIMARY KEY, at INTEGER NOT NULL, body BLOB NOT NULL)")
        self.last, self.requests = 0.0, 0

    def get(self, path, **params):
        url = API + path + "?" + urllib.parse.urlencode({**params, "fmt": "json"})
        row = self.db.execute("SELECT at, body FROM fetched WHERE url = ?", (url,)).fetchone()
        if row and ("query" not in params or time.time() - row[0] < SEARCH_KEEPS):
            return json.loads(zlib.decompress(row[1]))
        why = ""
        for attempt in range(12):
            time.sleep(max(0.0, self.last + PAUSE - time.monotonic()))
            self.last = time.monotonic()
            try:
                request = urllib.request.Request(url, headers={"User-Agent": AGENT, "Accept": "application/json"})
                with urllib.request.urlopen(request, timeout=40) as response:
                    data = json.load(response)
                break
            except urllib.error.HTTPError as e:
                if e.code in (400, 404):  # a query the server cannot parse, or an entry since removed
                    data = {}
                    break
                why = f"HTTP {e.code}"
                if e.code == 503:  # the search servers shed load when busy, sometimes for minutes at a stretch
                    try:
                        asked = min(float(e.headers.get("Retry-After") or 0), 120.0)
                    except ValueError:
                        asked = 0.0
                    time.sleep(max(asked, min(2 ** attempt, 60)))
                    continue
            except (urllib.error.URLError, TimeoutError, ConnectionError, json.JSONDecodeError) as e:
                why = f"{type(e).__name__}: {e}"
            time.sleep(min(5 * (attempt + 1), 30))
        else:
            raise Unavailable(f"{why} from {url}")
        self.requests += 1
        self.db.execute("INSERT OR REPLACE INTO fetched VALUES (?, ?, ?)", (url, int(time.time()), zlib.compress(json.dumps(data).encode())))
        self.db.commit()
        return data

    def release(self, mbid):
        """A release with its full track list."""
        return self.get("release/" + mbid, inc=RELEASE_INC)

    def labels(self, mbid):
        """A release's labels and catalogue numbers."""
        return self.get("release/" + mbid, inc="labels")

    def recording(self, mbid):
        """A recording with the releases it appears on."""
        return self.get("recording/" + mbid, inc=RECORDING_INC)

    def search_releases(self, query, limit=25):
        return self.get("release/", query=query, limit=limit).get("releases", [])

    def search_recordings(self, query, limit=25):
        return self.get("recording/", query=query, limit=limit).get("recordings", [])

    def editions(self, group):
        """Every release of a release group, with the size of each disc."""
        return self.get("release", **{"release-group": group, "inc": "media", "limit": 100}).get("releases", [])


def phrase(text):
    return '"' + text.replace("\\", "\\\\").replace('"', '\\"') + '"'


def credit(entity):
    """The artist credit as printed: "A feat. B"."""
    return "".join(c.get("name", "") + c.get("joinphrase", "") for c in entity.get("artist-credit", []) if isinstance(c, dict))


def credited(entity):
    """Each credited artist's name, with the full credit first."""
    return [credit(entity)] + [c.get("name", "") for c in entity.get("artist-credit", []) if isinstance(c, dict)]


def kind(group):
    """What the release is in Apple Music's and Spotify's terms."""
    secondary = group.get("secondary-types") or []
    for special in ("Compilation", "Soundtrack", "Live", "Remix", "DJ-mix", "Mixtape/Street", "Demo"):
        if special in secondary:
            return special
    return group.get("primary-type") or "Other"
