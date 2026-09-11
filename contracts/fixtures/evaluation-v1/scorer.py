"""Exact Unicode string equality; no trimming, case folding or model call."""

def score(actual: str, expected: str) -> float:
    return float(actual == expected)
