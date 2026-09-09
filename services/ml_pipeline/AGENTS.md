# Tabular data preparation

Use native DataFrame expressions, grouped operations and vectorized/batch sklearn
calls. Do not unpack a DataFrame to dictionaries, loop over records and rebuild it
to implement column logic. `apply(axis=1)` and equivalent row UDFs are fallbacks
only for a documented missing native operation, not the default implementation.

Records belong at notebook transport boundaries. Keep transformations between
those boundaries columnar, preserve provenance and order, and serialize missing
values as JSON null. Fit preprocessing on training data only; reuse state on
validation/inference. Keep categorical types and feature order stable across PHP
and Python. Do not claim that a local pandas pipeline is distributed or lazy.

Use `$python-dataframe-expressions` when available. Run the notebook and portable
bundle tests and regenerate `examples/titanic/build_bundle.py` and
`examples/build_seed.py` artifacts after notebook changes.
