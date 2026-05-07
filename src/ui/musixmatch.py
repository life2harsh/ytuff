import json
import sys
import unicodedata

import requests


LRCLIB_SEARCH = "https://lrclib.net/api/search"
STOP_WORDS = {
    "a",
    "an",
    "and",
    "clean",
    "edit",
    "explicit",
    "feat",
    "featuring",
    "ft",
    "live",
    "official",
    "remaster",
    "remastered",
    "the",
    "version",
}


def normalize(value, strip_brackets=False):
    out = []
    depth = 0
    for char in unicodedata.normalize("NFKC", value or ""):
        if strip_brackets:
            if char in "([{":
                depth += 1
                continue
            if char in ")]}":
                depth = max(0, depth - 1)
                continue
            if depth:
                continue
        out.append(char.lower() if char.isalnum() else " ")
    return " ".join("".join(out).split())


def tokenize_title(value):
    return [
        token
        for token in normalize(value, True).split()
        if token and token not in STOP_WORDS
    ]


def artist_aliases(value):
    aliases = [normalize(value)]
    text = (
        (value or "")
        .replace(" featuring ", "|")
        .replace(" feat. ", "|")
        .replace(" feat ", "|")
        .replace(" ft. ", "|")
        .replace(" ft ", "|")
        .replace(" & ", "|")
        .replace(" and ", "|")
        .replace(" x ", "|")
        .replace("/", "|")
        .replace(",", "|")
    )
    for part in text.split("|"):
        alias = normalize(part)
        if alias and alias not in aliases:
            aliases.append(alias)
    return aliases


def overlap_score(left, right):
    if not left or not right:
        return 0
    right_set = set(right)
    hits = sum(1 for token in left if token in right_set)
    return hits * 100 // max(len(left), len(right))


def compare_title(expected, candidate):
    expected_raw = normalize(expected)
    candidate_raw = normalize(candidate)
    if not expected_raw or not candidate_raw:
        return 0
    if expected_raw == candidate_raw:
        return 100
    expected_clean = normalize(expected, True)
    candidate_clean = normalize(candidate, True)
    if expected_clean and expected_clean == candidate_clean:
        return 97
    expected_tokens = tokenize_title(expected)
    candidate_tokens = tokenize_title(candidate)
    if expected_tokens and expected_tokens == candidate_tokens:
        return 95
    return overlap_score(expected_tokens, candidate_tokens)


def compare_artist(expected, candidate):
    expected_raw = normalize(expected)
    candidate_raw = normalize(candidate)
    if not expected_raw or not candidate_raw:
        return 0
    if expected_raw == candidate_raw:
        return 100
    expected_alias_list = artist_aliases(expected)
    candidate_alias_list = artist_aliases(candidate)
    if any(alias in candidate_alias_list for alias in expected_alias_list):
        return 94
    return overlap_score(
        sorted(set(" ".join(expected_alias_list).split())),
        sorted(set(" ".join(candidate_alias_list).split())),
    )


def compare_duration(expected, candidate):
    if expected is None or candidate is None:
        return 0
    diff = abs(int(expected) - int(candidate))
    if diff <= 2:
        return 18
    if diff <= 5:
        return 10
    if diff <= 10:
        return 4
    if diff <= 20:
        return -8
    return -18


def score_item(title, artist, duration, item):
    title_score = compare_title(title, item.get("trackName", ""))
    if title_score < 65:
        return None
    artist_score = compare_artist(artist, item.get("artistName", "")) if artist else 70
    if artist and artist_score < 35:
        return None
    total = title_score * 7 + artist_score * 3 + compare_duration(
        duration, item.get("duration")
    )
    if total < 520:
        return None
    return {
        "score": total,
        "title_score": title_score,
        "artist_score": artist_score,
        "item": item,
    }


def search_tracks(title, artist=None):
    params = {"track_name": title}
    if artist:
        params["artist_name"] = artist
    response = requests.get(
        LRCLIB_SEARCH,
        params=params,
        headers={"User-Agent": "ytuff-dev"},
        timeout=10,
    )
    response.raise_for_status()
    return response.json()


def best_match(title, artist=None, duration=None):
    results = search_tracks(title, artist)
    scored = [
        score
        for score in (
            score_item(title, artist, duration, item)
            for item in results
        )
        if score is not None
    ]
    scored.sort(
        key=lambda item: (
            item["score"],
            item["title_score"],
            item["artist_score"],
        ),
        reverse=True,
    )
    return scored[0] if scored else None


if __name__ == "__main__":
    title = sys.argv[1] if len(sys.argv) > 1 else "ivy"
    artist = sys.argv[2] if len(sys.argv) > 2 else "Taylor Swift"
    match = best_match(title, artist)
    print(json.dumps(match, indent=2, ensure_ascii=False))
