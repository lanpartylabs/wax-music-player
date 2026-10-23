"""Comparing and tidying the names found in tags, file names and the reference."""

import collections
import difflib
import functools
import re
import unicodedata

# "(Remastered)", "[Bonus Tracks]"; not a leading one, which is part of a name: "(the) Night Owls"
BRACKETED = re.compile(r"(?<=\S)\s*[\(\[][^\)\]]*[\)\]]")
LEADING_NUMBER = re.compile(r"^\s*(?:\d[-.])?\d{1,3}\s*[-._)]+\s*|^\s*\d{2,3}\s+(?=\S)")
LEADING_YEAR = re.compile(r"^\s*[\(\[]?(19|20)\d\d[\)\]]?\s*[-.]?\s*")
TRAILING_YEAR = re.compile(r"\s*[\(\[](19|20)\d\d[\)\]]\s*$")
DISC_NOISE = re.compile(r"\b(cd|disc|disk)[ _.-]*\d{1,2}\b", re.I)
GENERIC_TITLE = re.compile(r"^(audio ?)?(track|titel|piste|pista|spur)[ _.-]*\d+$|^untitled|^unknown|^no title|^\d{1,3}$"
                           r"|^(hidden|bonus|secret) ?track( ?\d+)?$", re.I)
FEATURING = re.compile(r"\s+[\(\[]?(?:feat\.?|ft\.?|featuring)\s+.*$", re.I)
SECOND_ARTIST = re.compile(r"\s+(?:feat\.?|ft\.?|featuring|with|vs\.?|x)\s+|\s*[,;/&]\s*", re.I)
VARIOUS = ("various artists", "various", "va", "v a", "varios artistas", "verschiedene interpreten")


@functools.lru_cache(maxsize=200_000)
def fold(text):
    """Form for comparing names: no case, accents or punctuation."""
    text = unicodedata.normalize("NFKD", text or "")
    text = "".join(c for c in text if not unicodedata.combining(c)).casefold().replace("&", " and ")
    return " ".join(re.sub(r"[\W_]+", " ", text).split())


NUMBERS = {word: str(n) for n, word in enumerate(
    "zero one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty".split())}
NUMBERS.update({word: str(n * 10) for n, word in enumerate("thirty forty fifty sixty seventy eighty ninety".split(), 3)})


def in_figures(folded):
    """A folded name with its numbers in figures, as the reference may write them: "the seven seas" is "the 7 seas"."""
    return " ".join(NUMBERS.get(word, word) for word in folded.split())


def similar(a, b):
    """Likeness of two names from 0 to 1; bracketed extras such as "(Remastered)" count for little, and so does writing a number out."""
    fa, fb = in_figures(fold(a)), in_figures(fold(b))
    if not fa or not fb:
        return 0.0
    if fa == fb:
        return 1.0
    best = difflib.SequenceMatcher(None, fa, fb).ratio()
    sa, sb = in_figures(fold(BRACKETED.sub("", a))), in_figures(fold(BRACKETED.sub("", b)))
    if sa and sb and (sa, sb) != (fa, fb):
        best = max(best, difflib.SequenceMatcher(None, sa, sb).ratio() - 0.05)
    return best


def consensus(values):
    """The most common value, ignoring differences of case and punctuation, and the share holding it."""
    groups = collections.defaultdict(collections.Counter)
    for value in values:
        groups[fold(value)][value] += 1
    if not groups:
        return "", 0.0
    top = max(groups.values(), key=lambda spellings: sum(spellings.values()))
    return top.most_common(1)[0][0], sum(top.values()) / len(values)


def tidy_album(album, artist=""):
    """An album name without what rippers add: "Artist - 2003 - Album [CD1]" becomes "Album"."""
    name = album
    head, dash, tail = name.partition(" - ")
    if dash and artist and fold(head) == fold(artist):
        name = tail
    name = DISC_NOISE.sub("", BRACKETED.sub("", LEADING_YEAR.sub("", name)))
    return name.strip(" -_.,") or album


def is_various(name):
    return fold(name) in VARIOUS


def lead_artist(artist):
    """The first-named artist of a credit: "A feat. B" and "A & B" give "A"."""
    return SECOND_ARTIST.split(artist or "", maxsplit=1)[0].strip()


def plain_title(title):
    """A title without bracketed extras or a featuring credit."""
    return FEATURING.sub("", BRACKETED.sub("", title or "")).strip()
