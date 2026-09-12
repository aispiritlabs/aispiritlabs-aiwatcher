"""What a web search is to an agent: a port, the rules of scope, one adapter.

Four implementations of this existed across two repositories — SearXNG,
LangSearch, Tavily, Valyu — and the transports were the only part that
differed. Everything else was the same four sentences written four times: a
query is bounded, a result may only come from a domain the caller allowed, only
so many come back, and a search that found nothing is a failure rather than an
empty success.

The last one is the reason this is not just a typed dict. A search tool that
returns `[]` hands the model an answer, and a model handed an empty list writes
a plausible one from memory — which is precisely the failure the citations were
there to prevent. So "no sources" leaves here as a refusal.

**The scope is enforced twice, and that is deliberate.** :meth:`SearchScope.for_query`
narrows what is asked for, and :meth:`SearchScope.permits` drops what came back
anyway. The first is a request to the provider and the second is a check on it:
a provider that ignores `site:` — or is compromised — must not be able to put a
URL the caller never allowed in front of the model. One without the other is a
rule the search engine enforces on our behalf.

The transport is a callable the caller supplies, as in
:mod:`~aiwatcher_agentic.openai_chat`, so an application reaches the engine
through the HTTP client it already has and this module keeps its dependencies.
"""

from __future__ import annotations

from collections.abc import Callable, Iterable, Mapping, Sequence
from dataclasses import dataclass
from typing import Protocol, runtime_checkable
from urllib.parse import urlsplit

import orjson

__all__ = [
    "Fetch",
    "SearchError",
    "SearchProvider",
    "SearchResult",
    "SearchScope",
    "SearxngSearch",
]

#: Fetch one URL with query parameters, and hand back the body. Anything that
#: is not a 2xx is the caller's client to raise, as it is for a model call.
type Fetch = Callable[[str, Mapping[str, str], float], bytes]


class SearchError(ValueError):
    """A search that cannot be handed to a model as an answer.

    `ValueError`, because this is raised inside a tool and the toolset turns a
    tool's exception into a tool result the model reads. A refusal it can act
    on — "search again, differently" — rather than a failure of the run.
    """


@dataclass(frozen=True, slots=True)
class SearchResult:
    """One page a search engine offered, before anyone decided to trust it."""

    url: str
    title: str = ""
    snippet: str = ""
    published_at: str | None = None

    @property
    def host(self) -> str:
        return urlsplit(self.url).hostname or ""


@runtime_checkable
class SearchProvider(Protocol):
    """One search engine.

    `domains` is passed down rather than applied afterwards because engines
    express it natively and differently — a `site:` term here, an
    `include_domains` field there — and an engine told what is wanted returns
    more usable results than one filtered after the fact.
    """

    def search(
        self, query: str, *, count: int, domains: Sequence[str] = ()
    ) -> list[SearchResult]: ...


@dataclass(frozen=True, slots=True)
class SearchScope:
    """What one search may ask for, and what may come back from it."""

    #: Hosts a result may come from. A leading `*.` is accepted and stripped —
    #: a subdomain always matches — so a caller's allowlist can be written the
    #: way allowlists usually are. Empty means the whole web.
    allowed_domains: tuple[str, ...] = ()
    max_results: int = 5
    max_query_chars: int = 1000
    max_title_chars: int = 500
    max_snippet_chars: int = 1000

    @property
    def domains(self) -> tuple[str, ...]:
        return tuple(domain.removeprefix("*.") for domain in self.allowed_domains)

    def for_query(self, text: str) -> str:
        """The query as it goes to the engine, or a refusal naming the bound."""
        query = text.strip()
        if not query or len(text) > self.max_query_chars:
            raise SearchError(
                f"A search query must be between 1 and {self.max_query_chars} characters."
            )
        if not self.domains:
            return query
        return f"{query} ({' OR '.join('site:' + domain for domain in self.domains)})"

    def permits(self, url: str) -> bool:
        """Whether a result's host is one the caller allowed."""
        if not self.domains:
            return True
        host = urlsplit(url).hostname or ""
        return any(host == domain or host.endswith("." + domain) for domain in self.domains)

    def accept(self, results: Iterable[SearchResult]) -> tuple[SearchResult, ...]:
        """Drop what is out of scope, trim what is left, stop at the limit."""
        kept: list[SearchResult] = []
        for result in results:
            if not result.url or not self.permits(result.url):
                continue
            kept.append(
                SearchResult(
                    url=result.url,
                    title=result.title[: self.max_title_chars],
                    snippet=result.snippet[: self.max_snippet_chars],
                    published_at=result.published_at,
                )
            )
            if len(kept) >= self.max_results:
                break
        if not kept:
            raise SearchError("The search returned no sources within the allowed domains.")
        return tuple(kept)


class SearxngSearch:
    """SearXNG's JSON API: `GET /search?q=…&format=json`.

    A private, stateless endpoint an application runs beside its worker, which
    is why there is no key here and why the whole configuration is one URL.
    """

    def __init__(self, url: str, fetch: Fetch, *, timeout: float = 30.0) -> None:
        self._url = url
        self._fetch = fetch
        self._timeout = timeout

    def search(self, query: str, *, count: int, domains: Sequence[str] = ()) -> list[SearchResult]:
        del count, domains  # SearXNG takes both in the query itself.
        body = self._fetch(self._url, {"q": query, "format": "json"}, self._timeout)
        try:
            rows = orjson.loads(body)["results"]
        except (orjson.JSONDecodeError, LookupError, TypeError) as error:
            raise SearchError("The search engine answered with no result list.") from error
        results: list[SearchResult] = []
        for row in rows if isinstance(rows, list) else ():
            if not isinstance(row, dict):
                continue
            url = row.get("url")
            if not isinstance(url, str) or not url:
                continue
            results.append(
                SearchResult(
                    url=url,
                    title=str(row.get("title") or ""),
                    snippet=str(row.get("content") or ""),
                    published_at=(
                        str(row["publishedDate"]) if row.get("publishedDate") is not None else None
                    ),
                )
            )
        return results
