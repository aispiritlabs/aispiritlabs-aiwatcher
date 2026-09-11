# What is authored, and therefore outside retention

**The one decision underneath all of these:** [everything folded from the
log](OBSERVABILITY.md) is bounded by that log's retention, and some things must
not be. The version of a prompt a run used has to be readable after that run has
been evicted; so does the corpus a model was trained on, and the licence that
permitted it. So five registries — prompts, annotations, datasets, conversations
and training — live in an object store instead, sharing one rule: **identity is
content**, and a head that indexes an object is written *after* the object.

| ADR | Decided | Where it stands |
|---|---|---|
| [0011](../ADR/ADR_0011_PROMPT_REGISTRY.md) | A prompt is authored, not observed, and lives in an object store | Accepted, one amendment. Sets the two orderings every other registry copies: the version object before the head, and the shard before the cursor |
| [0015](../ADR/ADR_0015_DATASET_EXPLORATION.md) | Dataset exploration uses slices and immutable dataset references | Accepted |
| [0017](../ADR/ADR_0017_IMAGE_ANNOTATION.md) | An annotation is authored, vector-first, and split by family rather than by image | Accepted, **amended by [0020](../ADR/ADR_0020_GENERIC_VISION_ANNOTATION.md)** |
| [0018](../ADR/ADR_0018_TRAINING_RUNS.md) | A training run is a record, not a trace, and it has its own module | Accepted. The one registry whose contents never came from the log |
| [0019](../ADR/ADR_0019_DATASET_HUB_DISCOVERY.md) | A hub is searched for what exists and never asked what is permitted | Accepted, **amended by [0020](../ADR/ADR_0020_GENERIC_VISION_ANNOTATION.md)** |
| [0020](../ADR/ADR_0020_GENERIC_VISION_ANNOTATION.md) | The annotation tool ships no vocabulary; the schema carries the domain | Accepted. It is what amended 0017 and 0019 |
| [0021](../ADR/ADR_0021_CONVERSATION_ARCHIVE.md) | Conversation content is an encrypted archive with its own retention, not events on the log | Accepted. The only registry that is **off by default**, and the only one whose deletions delete |
| [0022](../ADR/ADR_0022_STAGED_IMPORT_JOBS.md) | A long job over an object store is one primitive, and a corpus is staged before it is imported | Accepted. `aiwatcher-jobs` holds the rules; each caller keeps its own records |
| [0023](../ADR/ADR_0023_MODEL_PACKAGE.md) | A serving runtime is handed a declared package, and a checkpoint URI is not one | Accepted. Every artifact carries its `sha256`, because an address is not an identity |

| [0030](../ADR/ADR_0030_EVALUATION_EVIDENCE.md) | Evaluation owns pinned variants and durable evidence | Contract, durable registry and legacy read bridge implemented for synthetic evidence; native source adapters and orphan collection remain |

## The three rules that repeat

**Identity is content.** A prompt version is `sha256(text)`, an image id is the
digest of its bytes, an export version is a digest over its shards. Publishing
twice is therefore idempotent, and a content address supplied by a *caller* is
refused everywhere — two different images under one key is a training set whose
labels belong to a different picture, and no metric detects it.

**The object before the head, the shard before the cursor.** 0011 stated it and
0022 named it once for everybody (`aiwatcher_jobs::ORDERING`). A crash the right
way round re-does work and writes identical bytes; the wrong way round leaves an
index whose rows 404, or a corpus missing rows nothing can tell you about.

**A claim by an outside party is data, never truth.** 0019 is the sharpest form:
a hub's licence field becomes `claimed_license`, never a verdict, because the
first live search returned a corpus declaring `cc-by-sa-4.0` for drawings its
authors say are not theirs to license. 0023 is the same rule for a package's
declared shapes, which are cross-checked against the artifact rather than
believed.

## What has moved

**0020 generalised the annotation tool** and amended two ADRs doing it: 0017's
vocabulary and 0019's matching both belonged to one domain, and the schema
carries the domain now. Nothing else here has been superseded.

## What would reopen one

0021's is written into its defaults: if retaining conversation content ever
becomes the common case rather than the deliberate one, the off-by-default
posture is the thing to revisit — and it is the one decision in this group whose
reversal is not a refactor but a promise to somebody. 0023's is a framework
whose artifacts cannot describe themselves, which would leave a declaration with
nothing to cross-check it against.
