"""Deterministic synthetic responses for the contract fixture, not a model."""

def answer(question: str) -> str:
    return {
        "What is the capital of Poland?": "Warsaw",
        "What is two plus two?": "4",
        "Return an empty string.": "",
    }[question]
