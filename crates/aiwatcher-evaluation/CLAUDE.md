# Measuring: scorecards, judges, generated answers and evidence

The rules for `aiwatcher-evaluation` and the scoring executors in
`aiwatcher-server/src/execution/scoring`.
[ADR_0030](../../docs/ADR/ADR_0030_EVALUATION_EVIDENCE.md) is the long form and
is worth reading before changing anything here;
[ADR_0010](../../docs/ADR/ADR_0010_EVALUATION_REPORTS.md) is the older folded
report that still rides the log. The root `CLAUDE.md` holds what reaches past
this crate — who published an event, what a variant was observed doing, and the
one ordering every durable write obeys.

## Scorecards, cases and cohorts

- **Never let an `eval.*` event reach span assembly.** `EventType::forms_span`
  is checked first in `SpanAssembler::ingest`. A report has a start, an end and
  a duration and is still not a trace: its payload is a document, and a
  twenty-minute batch job is noise in a waterfall. The phase is kept because the
  evaluation fold reads it.
- **An evaluation report is not redacted.** The Collector strips `gen_ai.prompt`
  and `gen_ai.completion` from spans, and an evaluation forms no span, so
  nothing strips `data.report`. A producer that puts model output there is
  making a retention decision, deliberately or not at all.
- **Never compare two evaluation reports across datasets.** `baseline_for`
  matches on suite *and* dataset: two scores measured on different cases are two
  facts, and a delta between them claims they are one.
- **Never answer comparability twice in two vocabularies.** A folded report
  compares five optional strings a producer may not have sent, so most of that
  rule is about absence; published evidence compares one `context_id`, the
  content address of cohort, split, suite, scorer and metric definitions
  together. Collapsing them would cost the second everything that makes it
  stronger. What a reader *does* with the answer is the same either way, so the
  three words live once in `aiwatcher_core::comparability` and the panel draws
  one control. A delta is withheld rather than shown wrong, and `same_variant`
  is a field rather than a reason.
- **Never make a comparison read two results.** The metrics it subtracts are in
  each side's header, so it costs what two summaries cost however many cases are
  behind them. Which cases regressed is a full read of both sides and is
  therefore a **route of its own** (`/comparison/cases`) that pages and is
  narrowed by the server. A diff row carries **where** its case is — the cursor
  the case route would want, which the merge already knows — and never what that
  case said.
- **Never let a scorecard carry code, or its author decide which way is
  better.** A scorer is a name from the vocabulary this deployment implements
  and the parameters that name takes; the enum *is* the implementation, so the
  vocabulary cannot grow without an arm that knows what a new word means.
  Direction and aggregation are derived — `forbidden` counts a phrase, and a
  card declaring it higher-is-better would invert every comparison — while the
  unit is the author's one word for a quantity, and `absolute_error` requires it
  because the scorer sees two numbers and never what they count. A scorer
  compares numbers as written (exact decimals from the JSON each side was kept
  in; a case keeps that JSON as `actual_spelled`) and turns a distance into a
  double only to publish it. When an existing scorer's answer changes,
  `SCORING_VERSION` moves: `context.scorer` names the code that read the card
  and `context.suite` names the card.
- **Never let a framework's metric in, except by name and pinned.** DeepEval's
  and Opik's metrics are Python a card must never carry, so they run in
  `services/scorers` and `Scorer::External` names an adapter, a metric and its
  parameters — nothing in `aiwatcher-evaluation` names a framework. Direction is
  still not the author's: the work role records the service's catalog and
  publishing a card copies its description into the card version as `declared`,
  so a later catalog changes no published card and an upgrade is publishing the
  card again. An `external_evaluation` step reads the live catalog before asking
  anything and fails naming both when the service runs another release or model;
  the service refuses the same request with a 409. A reply is a number or the
  adapter's own sentence, kept per declaration and question as a judge's is. A
  metric a model graded carries `measured_by` and reads as not reproducible, and
  is warned about as uncalibrated unless the card holds it against people. The
  service turns every framework's phoning home off before importing it, wants a
  bearer token on both routes — off localhost it refuses to start without one —
  and in a cluster reaches DNS and its model alone.
- **Never publish a case that answered some of the metrics.** A scored case
  carries every declared metric or it is a failure with a reason and none of
  them: a case in three averages out of four gives each metric its own
  denominator and nothing in the numbers says so. A selected case nobody
  answered is unscored rather than zero, and a case a recording answered twice
  is not scored at all.
- **Never ask anybody to stage a cohort this deployment can derive.** `POST
  /evaluation-cohorts` derives the three files a cohort pins as canonical bytes
  from a curation version, an annotation export or a conversation corpus, and
  admission derives them again when a pinned member is not in the bundle. A
  `limit` takes the owner's first cases, never a sample, and a cohort of some of
  the cases is its own context — which is what keeps a smoke run from comparing
  with a full one. A conversation corpus's cohort is derived by an admin and
  holds digests, never words; an external cohort is not derivable.
- **Never let a scoring step read the archive on the strength of who pressed
  start.** A conversation cohort's cases are content, which a request reads only
  for an admin, and a step has no session. The executor asks the gate first and
  grants itself content access only for a pair an admin admitted, and
  `Registry::cohort` checks the content gate before the adapter. Such a cohort
  is answered from the archive itself (`"answers": "archive"`), never from a
  staged recording, and a card over it reads no expectation — the expectation
  *is* the response being measured.
- **Never make a judgement a case on its own.** Feedback on a trace, a low
  judgement or a complaint is a *proposal* (`POST /evaluation-reviews`),
  addressed by the dataset it would join and the thing it was seen on, so
  proposing the same trace twice lands on the review under way. It becomes a
  case only through people: somebody writes the expected answer, somebody
  approves it — an edit takes the approval away, and approving somebody else's
  words takes admin — and `publish` writes the approved cases as a new version
  of that dataset. A published case does not change. A result's case holds its
  own words, read from the result by the position its row carries, so nothing is
  retyped; a trace holds none, so its proposal writes them. The conversation
  archive is not a source: its words leave the seal only through a corpus
  export.
- **Never record a judgement without the form it was given on.** `3` is
  excellent on one team's form and a failure on another's, so a rubric is a
  resource — the question, the words a person and a judge are both given, the
  answers it admits and which end is better — versioned by its content. An
  assessment names the **concrete version**, never the head, or a rewritten set
  of levels silently re-reads every score given under the old one. Repeating
  what the current revision says lands on that revision rather than writing a
  second, which is what makes a redelivered write safe and stops a nightly judge
  writing a revision a night for ever.
- **Never let one author's judgement stand in for another's.** A standing
  judgement is identified by target, rubric **and** source and author together,
  so a person's score and a judge's about one case are two records and both are
  returned. The author never comes from the body for a person — a client that
  could name the reviewer could file somebody else's judgement — and a judge is
  named explicitly with the session beside it. Nothing checks the target exists:
  a judgement outlives the trace it is about.
- **Never let a quality judgement authorise conversation reuse.** A turn's
  review answers whether this content may be trained on at all; an assessment
  answers whether the answer was any good. Two decisions, two owners — a
  reviewer may say "keep this, it was wrong" — and an assessment carries no
  expected answer either, because a better answer is a change to a dataset, made
  through review.
- **Never decide a gate's verdict anywhere but the server.** `POST
  /evaluation-results/{id}/gate` answers `pass`, `regression`, `incomplete` or
  `error` from the comparison the panel draws, held to a policy: a tolerance per
  metric, metrics ignored, critical cases that must be measured and no worse
  whatever the average did, and whether generated answers must all be traced on
  the variant's pins. A scorer that failed or a case nobody answered is
  `incomplete` and never passes; a pair that does not compare is `error`.
  `aiwatcher-gate` stages, declares, starts, follows and asks, and exits 0 to 3.
- **Never publish evidence aiwatcher measures through the producer's route.**
  `POST /evaluation-results` answers 403 `measured_here` for a context scored by
  `aiwatcher.scoring`: the first publication of an ID wins, so anybody with an
  editor's token could otherwise put numbers under a declared run's name before
  the run did.

## Judges

- **Never admit a judge under the bytes rule.** The other five adapters admit a
  source by reading the owner's bytes again; a judge's output is a model call
  nobody can read back, so carrying that rule across would either refuse every
  judge or be relaxed for the five that satisfy it. Its own rule is ADR_0030's:
  configuration pinned by content (`context.judge.configuration`), a recorded
  calibration set as part of the evidence, disagreement with human scores stored
  beside the result, and the result marked as not reproducible. A manifest
  carrying a judge no adapter implements is refused by name.
- **Never publish a judged result without the agreement it measured.** A `judge`
  scorer names a rubric version and takes its scale and direction from it; the
  run declares the profile (`openai | llamacpp`), the model and revision, the
  pinned settings and a **calibration set** — the human judgements of a
  published result's cases, frozen by content through `POST
  /evaluation-calibrations`. Admission refuses a set with no person's judgement
  under a rubric the card asks; publication refuses a judged result without the
  agreement, counted over **every** item so a judge that declines the hard cases
  does not agree its way up, with its 95% Wilson interval, which is what says
  three of three proves little. The evidence says `reproducible: false` and a
  comparison says `judged`. The declared revision stays the author's word: what
  the provider said served each reply is counted into `served` and compared with
  nothing, because a provider's name for a model is an alias, a file or a dated
  snapshot.
- **Never let a judge answer in a shape it was not asked for.** Every call
  carries the scale as a JSON Schema the provider decodes against, and a reply
  that is a value of another kind fails its case rather than being read
  generously — gemma, told "true or false", answered `"false"` in quotes, so
  every case it said no to failed and only its yeses were counted. A reason
  never quotes a reply, nor a provider refusal its body: a reply can repeat the
  answer it was shown.
- **Never show a judge only what a card points at.** What a case asked reaches a
  judge through `input_path`, absent from every card that shows nothing so their
  versions do not move; the source adapter hands it over as
  `SourceEvidence::inputs`, beside the expectations and never into a shard. A
  case without it fails naming the path, and a calibration item without it is
  not asked and counts **against** the judge's agreement rather than out of it.
  On named levels a card may name a `pass_level`: the metric is then the
  fraction at or beyond it rather than a mean of positions, and both the judge's
  reply and the person's judgement go through that one mapping, so the agreement
  is about the number the result publishes.
- **Never send the archive's words to a judge without every reader being told.**
  A judge over a conversation cohort is allowed, and asking it sends what it
  reads to a provider outside the archive's encryption, retention and erasure.
  `JudgeConfiguration::reads_archive` is derived into the context, where an
  admin admitting the pair admits it and a hand-written context saying otherwise
  is refused; `ScoringRunView::warnings` says it in words, the panel holds
  admitting and starting until it is acknowledged, the evidence says it for as
  long as it is kept, and the executor logs it. What stays here holds none of
  it: evidence is sealed and kept replies carry no words.
- **Never ask a judge from the `serve` role, or start a judged run nothing will
  claim.** A judged run is `judge_evaluation`, claimed where
  `AIWATCHER_JUDGE_URL` and `AIWATCHER_JUDGE_PROVIDER` are — a socket and a
  credential. The start route answers 501 `judge_disabled` without one and 422
  naming both profiles on a deployment with another, because a started run
  nobody claims waits for ever.
- **Never ask a judge the same question twice in one run.** Every reply is kept
  under the declaration and the digest of the question
  (`evaluation-judges/replies/`) before the fold reads it, and a retry reads it
  back. Re-asked, a model answers differently, the fold writes different bytes,
  and an attempt whose settlement was lost after publication meets the first
  publication of its own ID as a conflict. What is kept is `JudgeReply::kept`
  and never the reply's words: a kept reply has no seal, no retention and no
  erasure, and a reply can repeat what it was shown.

## Generated answers, and what a witness saw

- **Never hand a generator what a case expected.** A run whose answers a worker
  generates starts with `evaluation_cases`, which reads the cohort under the
  pair's admission and writes each case's `case_id` and `input` and nothing
  else; the expectations stay with the owner and the score step. A generator
  that could read them could answer by copying, and nothing in the numbers would
  say so. Not over the conversation archive, whose questions would reach a
  worker outside its seal.
- **Never ask the application again on a retry.** The score step reads the rows
  the completed generation attempt wrote, from its own input; a case the task
  declined is unscored, never zero, and an answer is compared from the JSON the
  generation wrote, so an integer digit for digit.
- **Never let a worker answer under a variant's name with something else.** The
  task writes `generated_with` — the digests of the code and generation config
  it holds — and the score step refuses answers it is missing from or that
  disagree with the variant's pins.
- **Never take the prompt and the model from the answer.** They are references,
  so their witness is the application's trace: an answer names its `run_id`, and
  `evaluation_traces` — in the `serve` role, where the log's fold is — refuses
  answers whose run names another variant or result, whose call rendered another
  version of the pinned prompt, whose pinned model served at another version, or
  that declared the pinned workflow in another shape or stepped through a node
  its declaration lacks. What the traces do not show is counted rather than
  refused; a gate's `require_traces` is where fewer than all fails.
- **Never take an application's word for its own model or prompt.** A serving
  host's run naming the call it served (`caller_run_id`), published under
  another credential the deployment names a witness, witnesses the model
  version; a gateway in front of a provider witnesses the version the provider
  said served it and whether the request's text holds the pinned prompt's
  template, refusing the answers when it does not. `require_witness` requires
  both for every answer.
- **Never let an answer count as witnessed without an exchange.** The gateway's
  keyed digests say whether an answer is, word for word, a reply it relayed — or
  what the caller said it would take out of one — and whether the request held
  the case's input. One call doing both, for a request that was nothing but the
  pinned prompt with the answer not in it, rendered with values each accounted
  for (the case's input or a part of it, the reply of another call so made, a
  tool the gateway relayed or answered, a tool's host witnessed under its key,
  arguments so accounted for, or a value taken out of one of those in steps the
  gateway repeated) is an **exchange** — which an application answering around
  the gateway, telling the model what to say or handing it a value it made
  cannot show. `require_witnessed_answer` requires one per answer.
- **Never read an answer out of a reply in a way the variant did not pin.** A
  label's word counts only where the generation config pins that `answer_from`;
  an answer made of several replies only where each part the pinned response
  schema names is one; replies joined into one text only in the words it pins as
  `answer_joined`; and a reply chosen among others that went into nothing a
  witness saw only where `answer_chosen` picks it — the first relayed, the most
  of exactly `n`, or the one placed where a call on the judging prompt it pins
  names, shown the candidates in the witnessed order or asked both ways where it
  pins `order`.
- **Never read a question asked elsewhere as an exchange.** A case asked on the
  pinned prompt in another run while the measurement ran — or from as long
  before it as `asked_since_seconds` says, which a gate's policy may require at
  least — as asked or normalised, is no exchange. It is read from the
  projector's `asked` index, which names the date it does not reach back past,
  and a case asked on another prompt is counted apart and denies nothing unless
  a gate says so.
- **Never let a tool answer for code the variant did not pin.** A tool a witness
  answered with code other than the `tool_code` the generation config pins
  refuses the answers, and a value nothing accounted for names the tools the run
  called with no witness as where it may have come from.
- **Never read a missing run as a missing answer.** An answer naming a run the
  log never received is **lost in transport** where its client counted a run
  that never arrived, and unknown where none did.
- **Never let a declared graph go unheld.** A run's steps are held to the order
  the pinned declaration leads and to how often: a node starts once per
  completion leading into it, a failed start gives its turn back, a declared
  loop goes round as often as it completes, a node declared `repeats` runs once
  per item, one declared `at_most` starts no more than that, and an edge
  declared `at_most` is followed no more than that — the rounds of a cycle
  through it, a retry not counted. Edges sharing one of the declaration's
  `bounds` are held between them and must lead back into one loop's head or the
  pinned declaration is refused naming the bound; an edge's own bound no smaller
  than its source can complete is named on the result and measured all the same.
- **Never let a baseline differ from its candidate in more than the variant.** A
  baseline is a second declaration differing in its variant and ID alone, which
  is what gives the two one context.

## Evidence, approvals and retention

- **Never publish evaluation evidence for a pair nobody admitted.** An approval
  is a resource Evaluation owns, addressed by the pair it admits — `approval_id
  = sha256([1, "evaluation.approval", variant_id, context_id])` — recording who
  admitted it, when, and what the adapter verified beyond the manifest's
  digests. As one directory on disk an instance held exactly one pair, so a
  second variant hid the first's results. Admitting and withdrawing are
  **admin**, because `AIWATCHER_AUTH_INGEST_TOKENS` makes a producer an editor
  by construction. The gate is checked *after* the adapter, so a source that is
  gone says so rather than arriving as "nobody approved this".
- **Never let "not yet" and "no" read alike.** A publication and a scoring run's
  start both get 409 `pair_not_admitted` naming the approval — including when
  the adapter found nothing that admits the pair, because admitting it is where
  the adapter's own reason surfaces. A withdrawn pair, a bundle that changed
  underneath an approval and a caller who may not read the source stay 403: none
  is a step somebody still has to take.
- **Never start a scoring run for a pair nobody admitted.** A declaration is the
  run's identity, so a run started without an approval could only fail at
  publication — and that failed run is what every later start of the same
  declaration lands on. `POST /evaluation-runs` declares and answers with the
  derived manifest and its `approval_id`; `/start` refuses until
  `Registry::admission` — asked, never inferred from a record — says yes.
- **Never admit a line without approving each variant by name.** A line admits
  every variant of one experiment in one context at once, so a pipeline
  measuring a new commit needs nobody; each variant is still approved when its
  run starts, from bytes kept by their digest, naming the line. A line is
  refused for evidence a producer measured and over the conversation archive. A
  variant naming a model or a workflow brings what those imply — the model's
  package from the training registry, the weights and the workflow's declaration
  a pipeline sent by digest — and a model the registry does not hold is
  addressed by its own package: the version is the sha256 of the
  `model-package.json`.
- **Never admit evidence this deployment measured by asking for files it has
  none of.** A producer's suite and scorer are files re-read from the operator's
  bundle; a scoring run's suite is a scorecard in this registry and its scorer
  is the binary. The registry admits that kind against those owners and the
  adapter skips `suite.json` and `scorer.py` for it alone.
- **Never let the adapter's bytes be the registry's business.** An operator
  stages a bundle through `PUT /evaluation-approvals/{id}/bundle/{name}` (admin)
  into `evaluation-bundles/`, the adapter's own prefix, because what a bundle
  *is* is the adapter's question and `aiwatcher-evaluation` knows only the
  digest it was told (`ApprovalBundles`, beside `SourceAuthority`). Staging
  admits nothing: the approval resolves the whole bundle and records its digest,
  so bytes arriving after one stop that pair reading until they are what was
  admitted. A member's name is one segment or the bundle's one folder, never
  resolved as a path.
- **Never digest a bundle's `manifest.json` into what an approval admits.** The
  pair it declares is the approval's own address and the rest names the run that
  wrote it, so a digest over its bytes moved with every run and staging the
  second run's manifest hid everything the first published. `bundle_digest`
  covers what a bundle adds beyond the pins, by content, and is `None` when that
  is nothing; a pair admitted over other bytes answers 409
  `admitted_other_bytes` naming it.
- **Never read absence of an approval as a withdrawal.** Publication requires a
  record; a read requires only that no withdrawal marker exists, so evidence
  published before an instance kept approvals stays readable. Withdrawal hides
  every result measured under that pair, is final for that approval ID, and
  moves no retention deadline in either direction.
- **Never make a summary read the whole result.** The header is one
  content-addressed object and every count and metric is in it; walking fifty
  shards to return it made a catalogue page cost the corpus. A shard is verified
  when the page it is on is read, and a damaged shard is *that page's*
  `EvidenceState` rather than an error and never a short page. Collection lists
  what a result holds in order to delete the rest, so the difference from the
  header rides back on the retention report as `damaged`, bounded and dated — as
  old as the last hourly pass, which is why it is not a state.
- **Never resolve one source once per row.** Resolving reads the owner's own
  bytes — a model's artifacts inside a 100 MiB budget, a corpus shard by shard —
  and a catalogue is mostly repetitions of a handful of pairs. One verdict per
  admitted pair for the length of one `list` or `sweep`, and only a verdict
  *about the source*: a store that was briefly unreachable is not one.
- **Never make retention read a result to find its deadline.** `expires_at` on
  the receipt is already the minimum of the instance's clock and the source's,
  recorded at commit, so expiry needs neither the metadata nor the owner.
  Collection — a prefix listing per published result — is the expensive half and
  runs hourly, because it is about a writer that stopped and an hour late is the
  same answer.
- **Never let a sweep's count include what an earlier sweep did.** It counts
  what *that* pass retired, and a pass that failed writes its own failure to
  `evaluations/retention.json` with the number of consecutive failures —
  durable, so it survives a restart and every replica reads the same one. A
  sweep failing for a week otherwise looks exactly like one with nothing to do.
- **Never restore the `evaluations/` prefix from two points in time.**
  Publication and collection race at one immutable key, so a split restore
  resurrects a claim a collector abandoned or drops a tombstone that recorded an
  erasure. Restore it whole, to one moment, and run one collection pass. This,
  the conversation archive and the execution stream are the three stores whose
  contents exist nowhere else.
