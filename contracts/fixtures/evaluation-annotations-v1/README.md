# Annotation evaluation fixture

Schemas, suite and deterministic exact-equality scorer for native COCO cases.
This is a regression fixture, not detection mAP or promotion evidence.

The native project, uploaded SVGs, human revisions, export and operator-approved
manifest are assembled in `crates/aiwatcher-server/tests/evaluation/annotations.rs`.
Run `rtk cargo test -p aiwatcher-server --test evaluation annotations::`.
That test uses memory and its own temporary directory/listener; no demo instance
is contacted. The Evaluation README describes the case mapping and approval.

Native IDs are computed by their owners, so no fabricated project/export digest
is provided here. Pin the bytes of these files and the generated cases in both
operator and producer manifests. Do not use the short-answer schemas or scorer
for annotation cases.
