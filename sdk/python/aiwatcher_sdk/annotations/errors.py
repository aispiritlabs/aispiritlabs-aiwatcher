"""What a refusal from the annotation registry is.

The bottom of this package, and it depends on nothing in it. A rule, a sample
and a manifest all raise, and none of them may reach for the client that talks
HTTP in order to do it.
"""

from __future__ import annotations

from typing import Final

from aiwatcher_sdk.api import ApiError

__all__ = ["DISABLED", "RegistryError"]


class RegistryError(ApiError):
    """The registry refused, or could not be reached.

    ``code`` is the machine-readable discriminator; switch on it rather than on
    the message. ``registry_disabled`` means the instance was started without an
    object store, which is a deployment problem rather than a missing project.
    ``annotation_rejected`` means a drawing did not validate, and ``details``
    holds one line per problem.
    """


#: What a ``registry_disabled`` refusal tells somebody to do about it.
#:
#: A 501 from these routes is an instance built without the store, not a
#: project that does not exist, and the two have entirely different fixes —
#: which is why the hint is attached to the code rather than left to the
#: reader of a 501.
DISABLED: Final = (
    "this aiwatcher instance was started without an annotation store; set AIWATCHER_PROMPT_STORE"
)
