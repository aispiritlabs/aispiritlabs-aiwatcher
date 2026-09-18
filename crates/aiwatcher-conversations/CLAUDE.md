# The conversation archive

The rules for `aiwatcher-conversations`: the one authored store that is off by
default, whose content is sealed and whose deletions delete. Read
[ADR_0021](../../docs/ADR/ADR_0021_CONVERSATION_ARCHIVE.md) before changing this
area. Export jobs also obey `aiwatcher_jobs::ORDERING` and the root file's one
ordering rule.

- **Never put conversation content on the event log.** It was there once, as
  `data.input` and `data.output` on `llm.completed`, writing somebody's words
  into the durable log the Collector's redaction exists to keep them out of.
  `conversation.turn` is not in the event catalog and adding it means re-reading
  ADR_0021. The archive names the log — `run_id`, `trace_id`, `span_id`,
  `model`, `prompt` — and the log does not know the archive exists.
- **Never retain conversation content by default.**
  `AIWATCHER_CONVERSATION_ARCHIVE` is off unless a deployment says otherwise,
  and it is the only default chosen so that doing nothing keeps nothing. The
  routes answer 501 naming the variable, never an empty list.
- **Never run the archive without a key.** `AIWATCHER_CONVERSATION_KEYS` is
  required whenever the archive is on and the server refuses to start without
  it: an archive with no key is a plaintext archive in a bucket that prompts,
  datasets, annotations and training already read. Object-store encryption
  protects the disk, and every process holding the bucket's credentials still
  reads the content in the clear.
- **Never put content in a turn's head.** The head is plaintext by design so a
  review queue, a finding count and an exclusion report need no decryption; the
  body is sealed. A finding carries a part index, a byte range and a rule id and
  **never the text it matched**, or a finding would put the secret it found in
  every list response.
- **Never authenticate a sealed object by its ciphertext alone.** The key path
  is HKDF `info` and the AEAD's associated data, so a ciphertext copied from one
  turn to another does not open. Without it, anyone who can write to the bucket
  substitutes one person's words for another's and every digest still checks
  out.
- **Never let a turn's approval survive an edit.** Re-sending the same
  `message_id` with different content resets the review to pending — carrying it
  across is how reviewed text becomes unreviewed text with a tick beside it. A
  *human's* findings do survive a re-scan, because a scanner replacing a
  reviewer's judgement is the same mistake the other way.
- **Never infer a preference pair from a review rejection.** A rejection has
  several reasons and only one is "the other answer was better".
  `TurnReview::preference` is explicit, and a DPO export pairs only siblings a
  reviewer labelled — otherwise a turn rejected for holding somebody's address
  becomes the rejected half of a pair and puts that address in the corpus.
- **Never ship an unsafe-output classifier and call it a scan.**
  `conversations::redaction::scan` matches credential and identifier *shapes*
  and nothing else; `FindingKind::Unsafe` exists so a human can record one and
  is never produced by the scanner. A keyword list produces a green tick nobody
  should trust, and an entropy heuristic at any threshold that catches real keys
  also catches base64 images — after which a reviewer who has learned to dismiss
  findings dismisses the true one.
- **Never erase a turn and leave the corpus that already has it.** An erasure —
  and the retention sweep, which is the same problem arriving more quietly —
  deletes the shards of every published corpus whose pinned conversation list it
  touched. The manifest survives with its counts and digests, so the reference
  answers 410 rather than 404.
- **Never write an export shard without re-checking the lease.** A worker claims
  for five minutes and renews per shard; `interrupted` re-reads the record at
  each boundary and stops the worker that no longer holds it. This is not about
  the common case: with the archive changing underneath two workers, the last
  job record would name shard digests that do not describe the stored shards,
  and the version would stop being a content address of anything.
- **Never let a conversation export decide it is finished early.** A cancelled
  or failed job has no manifest and therefore no version, so an interrupted
  export never appears as a completed dataset. The shards it wrote stay written
  and are re-read by the resume; they are never indexed.
- **Never make a producer mint a span id to find its own words.** The server
  derives a span id from the run, the agent and the call, so a producer that
  minted none cannot name it. The archive is asked by **run**, showing the turns
  that name this span or, where none name any, the run's own — the same answer
  for a one-call run without a second copy of the derivation in every SDK.
