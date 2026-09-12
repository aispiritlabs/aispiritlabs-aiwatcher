# Governed conversation evidence fixture

The executable fixture lives in
`crates/aiwatcher-server/tests/evaluation/conversations.rs`. Run from the root:

```sh
rtk cargo test -p aiwatcher-server --test evaluation conversations
```

It records four distinct synthetic turns, grants explicit `evaluate` consent
with a one-day TTL, approves each turn, and runs a native `prompt_response`
export. Two question/answer cases are derived from the owner's ordinary export
API. The approved local bundle stores only case IDs and input/expected hashes;
question/answer schemas and deterministic exact-string scorer reuse
`contracts/fixtures/evaluation-v1`. Native versions include review timestamps,
so the test generates real pins rather than committing fabricated manifests.

The tests exercise memory and filesystem stores, encrypted retry/GC and paging,
source withdrawal and shorter retention, current review/consent, corrupt owner
bytes, missing keys, plaintext downgrade and real HTTP roles/legacy reads.
The private-word sentinel must never occur in raw Evaluation objects or local
bundle files. The fixture cleans its own directory, binds only an ephemeral
localhost port, and never seeds an existing server.

This is a contract regression fixture, not evidence of held-out model quality.
See `crates/aiwatcher-evaluation/README.md` for the operator bundle format.
