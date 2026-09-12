"""The model a graded metric asks, as this deployment configures it.

One OpenAI-compatible endpoint for every adapter, so a card measured with
DeepEval's relevancy and Opik's hallucination is two frameworks asking one
model, and the catalog can say which. Declared, never detected: the name and
the revision are pinned into every card measured with a graded metric, and a
model nobody named is no graded metric at all.
"""

from __future__ import annotations

import os
from dataclasses import dataclass

from aiwatcher_scorers.contract import ModelReference


@dataclass(frozen=True)
class ModelSettings:
    url: str
    name: str
    revision: str
    token: str | None = None
    #: ``llamacpp`` asks a thinking model not to think, because a reply that
    #: spends its tokens reasoning never reaches the JSON a metric asked for;
    #: ``openai`` sends requests as the framework wrote them. aiwatcher's judge
    #: profiles, for the same reason.
    profile: str = "openai"

    @property
    def reference(self) -> ModelReference:
        return ModelReference(self.name, self.revision)

    @property
    def extra_body(self) -> dict[str, object]:
        if self.profile == "llamacpp":
            return {"chat_template_kwargs": {"enable_thinking": False}}
        return {}

    @classmethod
    def from_env(cls) -> ModelSettings | None:
        url = os.environ.get("AIWATCHER_SCORERS_MODEL_URL")
        name = os.environ.get("AIWATCHER_SCORERS_MODEL")
        if not url and not name:
            return None
        revision = os.environ.get("AIWATCHER_SCORERS_MODEL_REVISION")
        if not url or not name or not revision:
            raise ValueError(
                "a graded metric's model is AIWATCHER_SCORERS_MODEL_URL, AIWATCHER_SCORERS_MODEL "
                "and AIWATCHER_SCORERS_MODEL_REVISION together: it is pinned into every card "
                "measured with it"
            )
        profile = os.environ.get("AIWATCHER_SCORERS_MODEL_PROFILE", "openai")
        if profile not in ("openai", "llamacpp"):
            raise ValueError("AIWATCHER_SCORERS_MODEL_PROFILE is openai or llamacpp")
        return cls(
            url=url.rstrip("/"),
            name=name,
            revision=revision,
            token=os.environ.get("AIWATCHER_SCORERS_MODEL_TOKEN"),
            profile=profile,
        )
