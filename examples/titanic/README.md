# Titanic from scratch with data curation and FlowAI

An executable example from raw passengers to validation predictions. The canvas
names the work: **Missing values**, **Visualization**, **OneHotEncoder**,
**LabelEncoder**, and **Model training & evaluation**. Engine selection stays
inside each block's spec; Python blocks use the existing notebook executor.

Inspired by [Gunes Evitan's Titanic tutorial](https://www.kaggle.com/code/gunesevitan/titanic-advanced-feature-engineering-tutorial)
([author's readable notebook](https://github.com/gunesevitan/kaggle-notebooks/blob/master/Titanic%20-%20Advanced%20Feature%20Engineering%20Tutorial.ipynb)).
This is an original implementation, not a notebook conversion or a reproduction
of its leaderboard score. It covers missing values, titles, family size, decks,
ticket frequencies, quantile bands, encoders, and a Random Forest baseline.

## Recommended: FlowPHP + FlowAI

Start with **[the illustrated Polish walkthrough](WALKTHROUGH.md)**: every block
has a screenshot, its purpose and the expected result.
Use **[Notebook view or create your own notebook](NOTEBOOK.md)** to edit the
whole flow as sequential cells with per-cell output, PHP/Python code and publication.

[`titanic-php.flow.json`](titanic-php.flow.json) is the PHP-first flow. Source,
projection, splitting, imputation, feature engineering and both encoders run in
PHP. Only visualization and Random Forest training use Python.

```sh
composer install --working-dir=services/flow
uv sync --project services/ml_pipeline --locked
uv run --project services/ml_pipeline python examples/titanic/run.py --php --download
# Reuse Kaggle train.csv without network access:
uv run --project services/ml_pipeline python examples/titanic/run.py --php --csv /path/to/train.csv
# Native PHP preparation only, output as JSON:
php examples/titanic/prepare.php /path/to/train.csv > /tmp/titanic-prepared.json
```

In **Data Curation → Pipeline**, import `titanic-php.flow.json` or select
**curation/titanic-flowphp** from **Saved pipelines**, then Preview, Run and Export flow with code.
The full PHP query is in [`preparation.flow`](preparation.flow); the canonical
graph is [`pipeline-php.json`](pipeline-php.json). Encoders preserve their fitted
state in `feature_encoder` and `label_encoder`, and the local runner saves both
as separate JSON files. Inline PHP code is included in the portable bundle.
Install the current `services/flow` on the receiving instance: importing a flow
does not install its PHP or Python libraries.

Native PHP classes also work independently of the editor:

```php
use Aiwatcher\Flow\FlowAI\OneHotEncoder;
use Aiwatcher\Flow\FlowAI\LabelEncoder;

$encoder = (new OneHotEncoder(['Sex', 'Pclass']))->fit($trainingRows);
$features = $encoder->transform($validationRows);
$state = json_decode(json_encode($encoder->toState()), true);
$restored = OneHotEncoder::fromState($state);
$labels = (new LabelEncoder())->fit(array_column($trainingRows, 'Survived'));
```

The data-curation DSL admits these finite operations:

```php
->trainTestSplit('PassengerId', target: 'Survived', fraction: 0.2, seed: 42)
->imputeMissing('Age', output: 'AgeFilled', groupBy: 'Sex,Pclass', fitOn: '_split')
->oneHotEncode('Sex', 'Pclass', fitOn: '_split', stateOutput: 'feature_encoder')
->labelEncode('Survived', fitOn: '_split', stateOutput: 'label_encoder')
```

`fitOn` filters training rows (`fitValue` defaults to `train`); omit it only when
all input is training data. Encoder `state: '<JSON>'` switches to transform-only
mode using an exported vocabulary. `handleUnknown` is `ignore` or `error`.
Imputation supports `median`, `most_frequent`, comma-separated `groupBy` and
`missingIndicator`; it falls back to the global **training** statistic for unseen
groups. A completely missing training column is rejected.

Column changes use native `withEntry`, `ref`, `when` and `coalesce` expressions.
Named scalar functions implement encoder lookup and fitted-statistic lookup.
They do not unpack and reconstruct `Rows`. Fitting is a separate, explicit
collection boundary: `FitPreparation` reads the required typed entries, stores
vocabularies/statistics or split IDs, and returns the original `Rows` unchanged.
Exact fitting/splitting is intended for bounded curation datasets. Supplying
encoder state skips fitting and collection, allowing streaming inference.

PHP applications using FlowPHP directly can use
`(new Preparation($operation, $columns, $options))->apply($frame)`, or compose
the scalar functions themselves with an exported state:

```php
use Aiwatcher\Flow\FlowAI\FittedState;
use Aiwatcher\Flow\FlowAI\Scalar\OneHot;
use function Flow\ETL\DSL\ref;

$state = new FittedState();
$state->set($savedEncoderState);
$frame = $frame->withEntry('model_features', new OneHot([ref('Sex'), ref('Pclass')], $state));
```

The DSL names above are service extensions, not methods added to vendor
DataFrame. Array-based encoder methods remain boundary adapters for callers
without a DataFrame; the Flow pipeline uses scalar evaluation directly.

The PHP split hashes seed + passenger ID independently within each target class.
It is stable under input reordering and differs from sklearn's random split.
For the pinned 891-row CSV: **712 train / 179 validation**, accuracy **0.837989**,
ROC AUC **0.885310**. This is a reproducibility check, not a comparison between
PHP and Python model quality: splits and feature sets differ. The PHP variant
uses titles, decks and family/ticket features; quantile bands remain in the
Python reference below. State interchange PHP → Python was verified for every
passenger's string/integer categories and labels. Numeric JSON representations
such as scientific-notation floats need a shared canonicalization before
claiming general cross-language category compatibility.

The upstream Hub may return transient HTTP 502 responses; the panel now shows
the actual upstream error. The local CSV runner is the reproducible offline path.

## Python reference: run without servers

The editable Python blocks use pandas column expressions, grouped statistics,
string operations and batch sklearn calls. `frame_of` and `records_of` handle
the notebook's record transport boundary; feature preparation stays in the
DataFrame between those boundaries. `OneHotEncoder.fit_frame` and
`transform_frame` provide the direct DataFrame API and preserve the index.
Canonical JSON conversion of individual category values is a narrow adapter
needed for the portable PHP/Python vocabulary, not a row-level feature UDF.
The exported state format is unchanged. Repository rules in
`services/flow/AGENTS.md` and `services/ml_pipeline/AGENTS.md` preserve this style
for subsequent work.

From the repository root:

```sh
uv sync --project services/ml_pipeline --locked
uv run --project services/ml_pipeline python examples/titanic/run.py --download
```

The download is the public `phihung/titanic` CSV mirror pinned to revision
`9753139e0b9d454ab4fd22e884290260db5fc7b6` and checked against SHA-256
`4a437fde05fe5264e1701a7387ac6fb75393772ba38bb2c9c566405af5af4bd7`.
You can instead use Kaggle's `train.csv`:

```sh
uv run --project services/ml_pipeline python examples/titanic/run.py --csv /path/to/train.csv
# A wiring check using the same Python code and a smaller sample:
uv run --project services/ml_pipeline python examples/titanic/run.py --download --limit 25 --output /tmp/titanic-preview
```

`output/` receives predictions, encoder vocabularies, imputation statistics,
feature thresholds, validation metrics, and `visualization.png`. The local
runner uses CSV parsing instead of the Flow source/projection and executes the
Python sources embedded in `titanic.flow.json`. The live path below also tests
Flow's data query, HTTP pagination, and the runtime's subprocess execution.

## Run in data curation

Start the API, Flow query service, Python runtime, and panel in separate terminals:

```sh
just run-execution
just flow-serve
just ml-pipeline-serve
just panel
```

If ports are occupied, select others with the recipes' port arguments and point
the panel's `AIWATCHER_FLOW_URL` and `AIWATCHER_ML_PIPELINE_URL` to those services.
The server's managed execution also needs both URLs configured; see
[the notebook runtime](../../services/ml_pipeline/README.md).

1. Open **Data Curation → Pipeline**.
2. Open **curation/titanic-from-scratch** from **Saved pipelines**, or **Import flow** and choose
   [`titanic.flow.json`](titanic.flow.json).
3. **Preview 25 rows** checks the wiring. **Run** processes all 891 passengers.
4. Click **Missing values**, **Visualization**, or an encoder to edit its code
   and settings. Visualization displays plots and passes the rows onward.
5. **Save** pins the code. **Export flow with code** downloads the graph and
   sources. **Publish** saves predictions in `curation/titanic-from-scratch`.
6. **Run on the server** uses the existing managed execution path with the
   saved notebook revisions; configure the services in that server first.

## The blocks

| Block | Behavior |
| --- | --- |
| Source / Select raw columns | Read 891 passengers through `hub_rows`, retaining the original 12 columns. |
| Train / validation split | Stratified 80/20 split with seed 42, before any fitting. |
| Missing values | Training medians by sex/class for age and class for fare; global fallback; training mode for embarkation. Preserve raw columns and missingness flags. |
| Feature engineering | Titles, decks, family size, isolation, fare per person, training ticket frequencies, training quantile thresholds. |
| Visualization | Plot missing measurements and survival by sex on training rows only. Preserve every row. |
| OneHotEncoder | Fit training categories; unseen validation categories produce zero indicators. Named features live under `model_features`. |
| LabelEncoder | Fit target labels on training rows; reject unknown classes. |
| Model training & evaluation | Fit a fixed Random Forest on explicitly selected features; evaluate validation only; return predictions and probabilities for all rows. |
| Publish predictions | Publish the resulting dataset with provenance. |

Measured on the pinned full CSV with the committed dependency lock:
712 training rows, 179 validation rows, accuracy **0.821229**, ROC AUC **0.847694**.
These are a single split's smoke-test baseline, not a Kaggle test-set score.
Passenger-level splitting allows related family/ticket groups across partitions;
use group-aware evaluation before claiming generalization to unseen families.

## Import/export contract

`aiwatcher.curation`, version `1`, contains:

- Pipeline name, description, block IDs, thematic titles, positions, parameters,
  connections, and inline Flow transformation code.
- Python notebook source text and its SHA-256, with every block pinned to the
  matching revision. Exporting unavailable pinned history fails explicitly.
- Runtime/package requirements. They are descriptive; import does not install
  packages or execute the imported code.

Import validates the format, sizes, source hashes, references and chain before
copying sources. Copies use names derived from the original name and source
digest. Existing edited copies are refused rather than overwritten. The pipeline
loads as an unsaved draft; a failed import leaves the draft unchanged. A service
failure partway through copying can leave unreferenced source files, and retrying
reuses identical copies.

The file is a source bundle, not a hermetic environment or a dataset/model
archive. Custom module imports must be installed in the receiving runtime. The
included FlowAI package ships with `services/ml_pipeline`; the dependency lock
is the reproducible installation source. API tokens, staged rows and trained
model objects are not exported. Review code before pressing Run.

## FlowAI's first API

FlowAI also has a Python package independent of the canvas and notebook format. Its
initial encoders wrap scikit-learn and support reusable fitted JSON state:

```python
import json
from flowai import OneHotEncoder, LabelEncoder

encoder = OneHotEncoder(["Sex", "Embarked"], handle_unknown="ignore")
encoder.fit(training_rows)
features = encoder.transform(validation_rows)  # no fitting here
state = json.loads(json.dumps(encoder.to_state()))
restored = OneHotEncoder.from_state(state)
assert restored.transform(validation_rows) == features

target = LabelEncoder().fit([row["Survived"] for row in training_rows])
labels = target.transform([row["Survived"] for row in validation_rows])
assert target.inverse_transform(labels) == [row["Survived"] for row in validation_rows]
```

Categorical values are JSON scalars, with `None` a distinct feature category.
Missing targets are rejected. Vocabulary order is deterministic, based on JSON
scalar representations; encoded labels imply no numeric ordering of classes.

[NuMojo](https://github.com/Mojo-Numerics-and-Algorithms-group/NuMojo) provides
Mojo-native numerical arrays and routines. A Mojo backend would need its own
categorical preprocessing, model library, and execution adapter. Model training in this first
version uses Python/scikit-learn; it does not claim Mojo execution. The encoder
state format gives a future adapter a concrete interoperability target.

## What this exercise found

Fixed here:

- Native FlowAI encoders, grouped imputation and deterministic splitting now run
  in PHP; adding a PHP block inserts it before the first Python block.
- Ticket strings containing `/` cannot be Flow partition values. The PHP example
  groups on a hashed ticket key while retaining the original ticket.
- Structured upstream 502 errors were reported as an offline Flow service.
  Their actual message now reaches the panel.

- `hub_rows(limit: 891)` silently stopped at the API's first 100-row page.
  It now fetches bounded offset pages, preserving the requested total and offset.
  Preview bounds hub input too, including nested reads.
- Browser execution ignored a saved notebook revision. It now sends the
  runtime's `code_revision`, matching managed execution.
- Saving a graph alone could not move its Python sources. The portable bundle
  carries the exact source and verifies hashes.
- Engine names made poor processing block titles. The palette now exposes the
  work, with separate Python and data-query implementations underneath.

Next extensions, deliberately beyond this first FlowAI slice:

- Typed ports for fitted transformers, model artifacts and reports. The current
  notebook chain carries rows; the local runner saves fitted state separately.
- Load a fitted model and preprocessing state for Kaggle's separate test.csv;
  model registration and submission export.
- Cross-validation fitting preprocessing afresh inside each fold, group-aware
  splits, and out-of-fold target-based family features.
- Generic form controls and schemas for thematic blocks; the missing-values,
  feature-engineering, visualization and baseline templates here expect Titanic
  columns. Encoders themselves work on arbitrary named columns.
- Branches/joins and mixed Python → Flow execution. The current chain contract
  still requires data-query transforms before Python and Publish at the end.
- A backend transaction for importing all sources plus a saved pipeline, and a
  dependency/artifact archive for fully self-contained distribution.

## Rebuild and verify

Edit `pipeline-php.json` or `pipeline.json` for the graph and `services/ml_pipeline/notebooks/flowai_*.py`
for the block implementations. Then rebuild the bundles and service seed:

```sh
python3 examples/titanic/build_bundle.py
python3 examples/build_seed.py
python3 examples/titanic/build_bundle.py --check
just ml-pipeline-check
just flow-check
cd apps/panel && npm test && npm run build
```

Tests cover source round trips and corruption, restored encoder vocabularies,
unknown categories, validation-data isolation, the complete bundled Python
chain, imported pinned-source execution through the HTTP service, and multi-page
hub reads. Real-data checks are opt-in through the runner and live panel.
