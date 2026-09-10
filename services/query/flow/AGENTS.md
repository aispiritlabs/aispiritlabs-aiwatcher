# Flow data preparation

Prefer native DataFrame expressions (`withEntry`, `ref`, scalar functions,
aggregations, windows, joins). Do not turn Rows into PHP records, transform those
records, then rebuild Rows. For a missing column operation, add a narrow named
ScalarFunction and compose it with `withEntry`.

Fit and transform are separate. A custom fitting transformer is an exception for
a documented cross-row gap: read typed Rows directly, retain only needed state,
return the original Rows, and apply results through scalar expressions. Explicit
fitted state must allow streaming inference without collection or refitting.
Preserve entry types, metadata, null semantics and training/validation isolation.

Use `$php-flow-expressions` when available. Verify via real DataFrames, including
multi-page fitting, lazy inference, serialization and untouched entry preservation.
