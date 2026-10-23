"""The catalogue: one SQLite file holding everything known about every file.

Evidence is only ever added here; the music itself is never touched. Paths
are kept as the bytes the file system gives, because some names in the
collection are not valid text.
"""

import os
import sqlite3

SCHEMA = """
CREATE TABLE IF NOT EXISTS files (
    id INTEGER PRIMARY KEY,
    path BLOB NOT NULL UNIQUE,  -- relative to the root, exactly as the file system gives it
    name TEXT NOT NULL,         -- the path as text, for display and search
    kind TEXT NOT NULL,         -- audio | image | playlist | other
    size INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    seen INTEGER NOT NULL       -- start of the last pass that found the file
);

-- What a file is and what it says about itself, as of `gathered`.
-- Regathered when the file's size or mtime no longer match.
CREATE TABLE IF NOT EXISTS evidence (
    file INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    gathered INTEGER NOT NULL,
    size INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    opens INTEGER NOT NULL,     -- 0: listed in its folder but cannot be opened
    error TEXT,                 -- why it could not be opened or read
    reader TEXT,                -- what read the tags: mutagen | ffprobe
    container TEXT,             -- what the file really is, whatever its extension says
    codec TEXT,
    lossless INTEGER,
    bitrate INTEGER,            -- kbit/s
    sample_rate INTEGER,
    channels INTEGER,
    bits INTEGER,
    duration REAL,              -- seconds, as the container states or the reader estimates
    doubtful INTEGER,           -- the reader was unsure it parsed the audio correctly
    tag_kinds TEXT,             -- "id3v2.3 id3v1 ape"
    title TEXT, artist TEXT, albumartist TEXT, album TEXT,
    track INTEGER, tracktotal INTEGER, disc INTEGER, disctotal INTEGER,
    date TEXT, originaldate TEXT, genre TEXT, compilation INTEGER, isrc TEXT,
    mb_recording TEXT, mb_release TEXT, mb_releasegroup TEXT, mb_track TEXT, acoustid TEXT,
    pictures INTEGER,           -- embedded images
    lyrics INTEGER,             -- carries embedded lyrics
    tags TEXT,                  -- every tag as read, JSON {key: [values]}
    fp_duration REAL,           -- seconds, measured by decoding
    fingerprint TEXT,           -- Chromaprint of the first two minutes
    fp_error TEXT
);

-- What AcoustID knows of a file's fingerprint.
CREATE TABLE IF NOT EXISTS heard (
    file INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    looked INTEGER NOT NULL,
    fingerprint_of INTEGER NOT NULL,  -- the evidence the lookup was made from, by its `gathered`
    error TEXT,
    results BLOB                      -- the answer, zlib-compressed JSON
);

-- A unit is a group of files looked up together: a folder, the part of a folder sharing an album tag,
-- or a folder's loose songs. This is what it claims to be and what it turned out to be.
CREATE TABLE IF NOT EXISTS units (
    key TEXT PRIMARY KEY,       -- the folder, then for a part of one: \x1f and the album, or \x1f alone for its loose songs
    folder TEXT NOT NULL,
    shape TEXT NOT NULL,        -- record | loose
    signature TEXT NOT NULL,    -- its files as they were when it was looked at
    files INTEGER NOT NULL,
    artist TEXT, album TEXT, various INTEGER,
    state TEXT NOT NULL,        -- certain | likely | unclear | not found | loose
    how TEXT,                   -- what led to the release: ids | fingerprint | search | songs | edition | judge
    release TEXT, release_group TEXT, title TEXT, credited TEXT, year TEXT, kind TEXT,
    tracks INTEGER, missing INTEGER, shelf TEXT, copies INTEGER,
    tried TEXT,                 -- JSON: each release compared and how it fitted
    proposal TEXT,              -- JSON: what a model proposed that the reference did not confirm
    judged INTEGER NOT NULL DEFAULT 0,  -- passes a model has made over it
    decided INTEGER NOT NULL
);

-- What each file is. Every row says how it was settled and on what evidence.
CREATE TABLE IF NOT EXISTS identity (
    file INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    unit TEXT NOT NULL,
    state TEXT NOT NULL,        -- certain | likely | unknown
    how TEXT,                   -- ids | fingerprint | tags | judge | listen
    release TEXT, release_group TEXT, recording TEXT, track TEXT,
    disc INTEGER, position INTEGER,
    title TEXT, artist TEXT, album TEXT, album_artist TEXT, year TEXT, kind TEXT,
    copy INTEGER NOT NULL DEFAULT 0,  -- a second copy of a track another file in the unit already fills
    evidence TEXT,              -- JSON
    judged INTEGER NOT NULL DEFAULT 0,
    decided INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS identity_unit ON identity (unit);
CREATE INDEX IF NOT EXISTS identity_recording ON identity (recording);

-- Tracks a record on the shelf lacks.
CREATE TABLE IF NOT EXISTS wanted (
    unit TEXT NOT NULL,
    release TEXT NOT NULL,
    disc INTEGER NOT NULL,
    position INTEGER NOT NULL,
    recording TEXT, title TEXT, artist TEXT, seconds REAL,
    held INTEGER REFERENCES files(id) ON DELETE SET NULL,  -- a file elsewhere in the collection that is this track
    PRIMARY KEY (unit, disc, position)
);

-- The front cover of each release certain files sit on; the picture itself is a file in the covers folder.
CREATE TABLE IF NOT EXISTS covers (
    release TEXT PRIMARY KEY,
    release_group TEXT,
    looked INTEGER NOT NULL,
    source TEXT,                -- release | release group | folder | embedded; NULL when there is none anywhere
    origin TEXT,                -- its address at the archive, or the file in the collection it was taken from
    file TEXT,                  -- its name in the covers folder
    width INTEGER, height INTEGER, bytes INTEGER
);

-- The genre of each album certain files sit on, out of the short list the shelves go by.
CREATE TABLE IF NOT EXISTS genres (
    release_group TEXT PRIMARY KEY,
    looked INTEGER NOT NULL,
    genre TEXT,                 -- the first reading, from MusicBrainz; NULL when it says nothing of the album or its artists
    source TEXT,                -- album | artist | kind
    basis TEXT,                 -- JSON: the MusicBrainz genres, with their votes, that it rests on
    filed TEXT,                 -- the genre it is filed under, out of Apple's list, once a model has checked the first reading
    style TEXT,                 -- the subgenre under that, where one clearly fits
    checked INTEGER,            -- when a model last looked; NULL until one has
    judge TEXT                  -- the model that did
);

-- Files their owner has dropped from the library in the player. Nothing is deleted by that: they are left out of what
-- the player is given, and listed in the report for removal.
CREATE TABLE IF NOT EXISTS discarded (
    name TEXT PRIMARY KEY,      -- the file's path as the index gives it
    at INTEGER NOT NULL
);

-- Every question put to a model, and what it cost.
CREATE TABLE IF NOT EXISTS asked (
    at INTEGER NOT NULL, stage TEXT NOT NULL, model TEXT NOT NULL,
    tokens_in INTEGER, tokens_out INTEGER, usd REAL, seconds REAL, error TEXT
);
"""

# Columns added since a table was first made: (table, column, type).
ADDED = (("genres", "filed", "TEXT"), ("genres", "style", "TEXT"), ("genres", "checked", "INTEGER"), ("genres", "judge", "TEXT"))

EVIDENCE = (
    "file gathered size mtime opens error reader container codec lossless bitrate sample_rate channels bits duration "
    "doubtful tag_kinds title artist albumartist album track tracktotal disc disctotal date originaldate genre "
    "compilation isrc mb_recording mb_release mb_releasegroup mb_track acoustid pictures lyrics tags fp_duration "
    "fingerprint fp_error"
).split()


def connect(path):
    db = sqlite3.connect(os.path.expanduser(path), timeout=120)
    db.row_factory = sqlite3.Row
    db.execute("PRAGMA journal_mode = WAL")
    db.execute("PRAGMA foreign_keys = ON")
    db.executescript(SCHEMA)
    for table, column, kind in ADDED:
        if column not in {row["name"] for row in db.execute(f"PRAGMA table_info({table})")}:
            db.execute(f"ALTER TABLE {table} ADD COLUMN {column} {kind}")
    return db


def save_evidence(db, row):
    """Store one file's evidence, replacing what was gathered before."""
    marks = ", ".join("?" for _ in EVIDENCE)
    db.execute(f"INSERT OR REPLACE INTO evidence ({', '.join(EVIDENCE)}) VALUES ({marks})", [row.get(column) for column in EVIDENCE])
