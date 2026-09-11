"""Deterministic fixture score over complete COCO expectations; not mAP."""


def score(actual: object, expected: object) -> float:
    return float(actual == expected)
