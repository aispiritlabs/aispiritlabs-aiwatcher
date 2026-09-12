"""What a search is allowed to ask for, and what is allowed back from it.

The rules are tested apart from the engine, because they are the half that is
the same for every engine — and the half that decides whether a URL nobody
allowed can reach the model.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence

import orjson
import pytest

from aiwatcher_agentic.web_search import (
    SearchError,
    SearchProvider,
    SearchResult,
    SearchScope,
    SearxngSearch,
)

SCOPED = SearchScope(allowed_domains=("*.example.org", "shop.example.com"), max_results=2)


def result(url: str, title: str = "t", snippet: str = "s") -> SearchResult:
    return SearchResult(url=url, title=title, snippet=snippet)


def test_a_query_is_scoped_to_the_domains_the_caller_allowed() -> None:
    assert SCOPED.for_query("  fence panels  ") == (
        "fence panels (site:example.org OR site:shop.example.com)"
    )
    assert SearchScope().for_query("fence panels") == "fence panels"


@pytest.mark.parametrize("text", ["", "   ", "x" * 1001])
def test_a_query_outside_its_bounds_is_a_refusal_naming_the_bound(text: str) -> None:
    with pytest.raises(SearchError, match="between 1 and 1000"):
        SearchScope().for_query(text)


@pytest.mark.parametrize(
    ("url", "allowed"),
    [
        ("https://example.org/p", True),
        ("https://deep.sub.example.org/p", True),
        ("https://shop.example.com/p", True),
        ("https://notexample.org/p", False),
        ("https://example.org.evil.test/p", False),
        ("https://example.com/p", False),
        ("not a url", False),
    ],
)
def test_a_host_is_allowed_only_when_it_is_the_domain_or_under_it(url: str, allowed: bool) -> None:
    """`example.org.evil.test` is the case a substring check gets wrong."""
    assert SCOPED.permits(url) is allowed


def test_the_scope_is_checked_again_on_what_came_back() -> None:
    """A provider that ignored `site:` must not put a stranger in front of the model."""
    kept = SCOPED.accept(
        [
            result("https://evil.test/p"),
            result("https://example.org/a"),
            result("https://shop.example.com/b"),
            result("https://example.org/c"),
        ]
    )
    assert [item.url for item in kept] == ["https://example.org/a", "https://shop.example.com/b"]


def test_long_titles_and_snippets_are_trimmed_rather_than_carried() -> None:
    [kept] = SearchScope(max_results=1, max_title_chars=4, max_snippet_chars=6).accept(
        [result("https://any.test/p", title="a" * 20, snippet="b" * 20)]
    )
    assert kept.title == "aaaa" and kept.snippet == "bbbbbb"


def test_finding_nothing_is_a_refusal_and_never_an_empty_answer() -> None:
    """An empty list is an answer, and a model handed one writes its own."""
    with pytest.raises(SearchError, match="no sources within the allowed domains"):
        SCOPED.accept([result("https://evil.test/p")])
    with pytest.raises(SearchError, match="no sources"):
        SCOPED.accept([])


def test_searxng_reads_the_json_api_and_skips_rows_with_no_address() -> None:
    asked: list[tuple[str, Mapping[str, str]]] = []

    def fetch(url: str, params: Mapping[str, str], timeout: float) -> bytes:
        asked.append((url, dict(params)))
        return orjson.dumps(
            {
                "results": [
                    {"url": "https://example.org/a", "title": "A", "content": "cheap"},
                    {"title": "no address"},
                    "junk",
                    {"url": "https://example.org/b", "publishedDate": "2026-01-02"},
                ]
            }
        )

    provider: SearchProvider = SearxngSearch("http://search/search", fetch)
    results = provider.search(SCOPED.for_query("panels"), count=5)

    assert asked == [
        (
            "http://search/search",
            {"q": "panels (site:example.org OR site:shop.example.com)", "format": "json"},
        )
    ]
    assert [item.url for item in results] == ["https://example.org/a", "https://example.org/b"]
    assert results[0].snippet == "cheap" and results[0].host == "example.org"
    assert results[1].published_at == "2026-01-02" and results[1].title == ""


def test_an_engine_that_answers_with_no_result_list_is_a_refusal() -> None:
    def fetch(url: str, params: Mapping[str, str], timeout: float) -> bytes:
        return orjson.dumps({"error": "overloaded"})

    with pytest.raises(SearchError, match="no result list"):
        SearxngSearch("http://search/search", fetch).search("panels", count=5)


def test_a_provider_is_recognised_by_shape_and_not_by_inheritance() -> None:
    class Static:
        def search(
            self, query: str, *, count: int, domains: Sequence[str] = ()
        ) -> list[SearchResult]:
            return [result("https://example.org/a")]

    assert isinstance(Static(), SearchProvider)
