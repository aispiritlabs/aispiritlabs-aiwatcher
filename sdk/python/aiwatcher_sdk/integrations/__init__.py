"""Adapters that plug aiwatcher into an existing agent framework.

Each one implements whatever tracing interface the framework already has, so
wiring aiwatcher in is a one-line change at the framework's own seam rather than
instrumentation sprinkled through call sites.
"""
