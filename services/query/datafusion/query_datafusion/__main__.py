"""`python -m query_datafusion`: DataFusion on the six /query routes (`just query-serve`)."""

from aiwatcher_query import serve
from query_datafusion import REFERENCE

if __name__ == "__main__":
    serve(REFERENCE)
