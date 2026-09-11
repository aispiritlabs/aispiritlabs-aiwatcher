"""`python -m query_duckdb`: DuckDB on the six /query routes (`just query-serve`)."""

from aiwatcher_query import serve
from query_duckdb import REFERENCE

if __name__ == "__main__":
    serve(REFERENCE)
