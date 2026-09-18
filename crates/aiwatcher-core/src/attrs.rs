//! Attribute and metric names, in one place.
//!
//! Three namespaces, and it matters which one a value goes in:
//!
//! * [`genai`] — OpenTelemetry GenAI semantic conventions. Anything that a
//!   generic OTel backend or Grafana dashboard should understand without
//!   knowing about aiwatcher belongs here.
//! * [`messaging`] — OpenTelemetry messaging conventions. Emmett's `almanac`
//!   puts `correlation_id` / `causation_id` here rather than inventing its own
//!   names, and so do we.
//! * [`aiwatcher`] — everything specific to this system, mirroring the shape of
//!   Emmett's `EmmettAttributes`.
//!
//! Hard-coding a string at a call site is how two dashboards end up disagreeing
//! about a field name. Add it here instead.

/// OpenTelemetry GenAI semantic conventions.
pub mod genai {
    /// `chat`, `execute_tool`, `invoke_agent`.
    pub const OPERATION_NAME: &str = "gen_ai.operation.name";
    /// Current spelling of the provider attribute.
    pub const PROVIDER_NAME: &str = "gen_ai.provider.name";
    /// Older spelling, still emitted so pre-existing dashboards keep working.
    pub const SYSTEM: &str = "gen_ai.system";
    pub const REQUEST_MODEL: &str = "gen_ai.request.model";
    pub const REQUEST_TEMPERATURE: &str = "gen_ai.request.temperature";
    pub const REQUEST_MAX_TOKENS: &str = "gen_ai.request.max_tokens";
    pub const REQUEST_TOP_P: &str = "gen_ai.request.top_p";
    pub const REQUEST_TOP_K: &str = "gen_ai.request.top_k";
    pub const REQUEST_SEED: &str = "gen_ai.request.seed";
    pub const REQUEST_STOP_SEQUENCES: &str = "gen_ai.request.stop_sequences";
    pub const REQUEST_FREQUENCY_PENALTY: &str = "gen_ai.request.frequency_penalty";
    pub const REQUEST_PRESENCE_PENALTY: &str = "gen_ai.request.presence_penalty";
    pub const RESPONSE_MODEL: &str = "gen_ai.response.model";
    pub const RESPONSE_ID: &str = "gen_ai.response.id";
    pub const RESPONSE_FINISH_REASONS: &str = "gen_ai.response.finish_reasons";
    pub const USAGE_INPUT_TOKENS: &str = "gen_ai.usage.input_tokens";
    pub const USAGE_OUTPUT_TOKENS: &str = "gen_ai.usage.output_tokens";
    pub const CONVERSATION_ID: &str = "gen_ai.conversation.id";
    pub const AGENT_ID: &str = "gen_ai.agent.id";
    pub const AGENT_NAME: &str = "gen_ai.agent.name";
    pub const TOOL_NAME: &str = "gen_ai.tool.name";
    pub const TOOL_CALL_ID: &str = "gen_ai.tool.call.id";

    pub mod operation {
        pub const CHAT: &str = "chat";
        pub const EXECUTE_TOOL: &str = "execute_tool";
        pub const INVOKE_AGENT: &str = "invoke_agent";
    }

    pub mod metrics {
        pub const OPERATION_DURATION: &str = "gen_ai.client.operation.duration";
        pub const TOKEN_USAGE: &str = "gen_ai.client.token.usage";
        pub const TIME_TO_FIRST_TOKEN: &str = "gen_ai.server.time_to_first_token";
        /// Not in the spec: cached prompt tokens are what makes cost analysis
        /// possible and no standard attribute covers them yet.
        pub const CACHED_TOKENS: &str = "gen_ai.client.cached_token.usage";
    }

    pub mod token_type {
        pub const KEY: &str = "gen_ai.token.type";
        pub const INPUT: &str = "input";
        pub const OUTPUT: &str = "output";
        pub const CACHED: &str = "cached";
    }
}

/// OpenTelemetry messaging semantic conventions, as Emmett's `almanac` uses
/// them.
pub mod messaging {
    pub const SYSTEM: &str = "messaging.system";
    pub const MESSAGE_ID: &str = "messaging.message.id";
    pub const CORRELATION_ID: &str = "messaging.message.correlation_id";
    pub const CAUSATION_ID: &str = "messaging.message.causation_id";
    pub const CONVERSATION_ID: &str = "messaging.message.conversation_id";
    pub const OPERATION_TYPE: &str = "messaging.operation.type";
    pub const DESTINATION_NAME: &str = "messaging.destination.name";
    pub const BATCH_MESSAGE_COUNT: &str = "messaging.batch.message_count";

    /// The value of [`SYSTEM`] for everything this project emits.
    pub const SYSTEM_NAME: &str = "aiwatcher";
}

/// aiwatcher's own namespace.
pub mod aiwatcher {
    pub mod run {
        pub const ID: &str = "aiwatcher.run.id";
        pub const STATUS: &str = "aiwatcher.run.status";
    }

    pub mod stream {
        pub const NAME: &str = "aiwatcher.stream.name";
        pub const POSITION: &str = "aiwatcher.stream.position";
        pub const GLOBAL_POSITION: &str = "aiwatcher.stream.global_position";
    }

    pub mod event {
        pub const TYPE: &str = "aiwatcher.event.type";
        pub const SCHEMA_VERSION: &str = "aiwatcher.event.schema_version";
        pub const SEQUENCE: &str = "aiwatcher.event.sequence";
    }

    /// What a gateway saw of a call's words, as keyed digests only a
    /// deployment holding the gateway's credential can test — see
    /// [`crate::witness`]. On a witness's own call, never on the
    /// application's.
    pub mod witness {
        /// The texts the request held: each message, and each value the
        /// named template was found rendered with.
        pub const ASKED: &str = "aiwatcher.witness.asked";
        /// The same texts normalised (`witness::normalized`) before they were
        /// digested: what finds a question asked again in another case,
        /// spacing or punctuation.
        pub const ASKED_NORMALIZED: &str = "aiwatcher.witness.asked_normalized";
        /// The texts the provider replied with.
        pub const REPLIED: &str = "aiwatcher.witness.replied";
        /// Each value the named template was found rendered with, digested as
        /// a reply is — so a value that is what a model already replied reads
        /// as that reply, and one the application made reads as neither it nor
        /// the case's input.
        pub const RENDERED: &str = "aiwatcher.witness.rendered";
        /// Each value the caller took out of another of its values in steps
        /// the witness repeated, as `value:source` — so a value cut out of the
        /// case's input is accounted as that input.
        pub const DERIVED: &str = "aiwatcher.witness.derived";
        /// Where each of those values was placed, as `name:value` — the
        /// digest of the placeholder's name and of the value, each made as a
        /// reply is — so a judging call's reply naming a placeholder reads back
        /// as the value it holds.
        pub const PLACED: &str = "aiwatcher.witness.placed";
        /// What a way of taking an answer that knows more than the reply — a
        /// label's word — took out of each reply, digested as a reply is.
        pub const TAKEN: &str = "aiwatcher.witness.taken";
        /// The digest of the way the caller said it takes its answer out.
        pub const TAKING: &str = "aiwatcher.witness.taking";
        /// That way took nothing out of any reply: one the caller could not
        /// read, and so none it chose its answer against.
        pub const TOOK_NOTHING: &str = "aiwatcher.witness.took_nothing";
        /// On a tool call a witness relayed: each part of its arguments, and
        /// what the tool returned, digested as a reply is.
        pub const ARGUMENTS: &str = "aiwatcher.witness.arguments";
        pub const RETURNED: &str = "aiwatcher.witness.returned";
        /// On a tool call a witness answered with a function of its own: the
        /// sha256 of the source that function is defined in, which a variant
        /// may pin (`tool_code`).
        pub const TOOL_CODE: &str = "aiwatcher.witness.tool_code";
    }

    pub mod source {
        pub const SERVICE: &str = "aiwatcher.source.service";
        pub const INSTANCE: &str = "aiwatcher.source.instance";
        pub const SDK: &str = "aiwatcher.source.sdk";
        /// The credential ingest authenticated the span's events under — see
        /// `EventEnvelope::published_by`. Only where one credential sent both
        /// ends: a span two credentials wrote is neither one's word.
        pub const PUBLISHED_BY: &str = "aiwatcher.source.published_by";
    }

    pub mod span {
        /// How a span was closed. Distinguishes a real completion from one the
        /// orphan sweeper had to finish — a metric on this is the fastest way
        /// to notice a producer that stopped sending end events.
        pub const CLOSED_BY: &str = "aiwatcher.span.closed_by";
        pub const CHUNK_COUNT: &str = "aiwatcher.span.chunk_count";
        /// `retriever`, `embedding`, `guardrail`… — whatever the producer
        /// called it. Deliberately free-form: a new step kind must not need a
        /// backend release.
        pub const STEP_TYPE: &str = "aiwatcher.span.step_type";
        pub const STEP_NAME: &str = "aiwatcher.span.step_name";
    }

    /// Attributes a step carries when it is a retrieval-shaped one. There is no
    /// settled OpenTelemetry convention for retrieval yet, so these live in the
    /// aiwatcher namespace rather than squatting on a `gen_ai.*` name that may
    /// come to mean something else.
    pub mod step {
        pub const DOCUMENT_COUNT: &str = "aiwatcher.step.document_count";
        pub const TOP_K: &str = "aiwatcher.step.top_k";
        pub const CANDIDATE_COUNT: &str = "aiwatcher.step.candidate_count";
        pub const SCORE: &str = "aiwatcher.step.score";
    }

    /// Which registered prompt a call ran on. Not `gen_ai.*`: there is no
    /// convention for it, and the thing being named is *this* registry's
    /// content address rather than a provider's id.
    ///
    /// A reference, never the text — see `PromptRef`, ADR_0011 and ADR_0021.
    pub mod prompt {
        pub const NAME: &str = "aiwatcher.prompt.name";
        pub const VERSION_ID: &str = "aiwatcher.prompt.version_id";
        /// Whether a host that saw the request's text found the named
        /// version's template in it, with its variables filled — what a
        /// gateway reports, and never the application about itself.
        pub const VERIFIED: &str = "aiwatcher.prompt.verified";
        /// Whether that host found the request's text to be nothing but the
        /// template rendered and the values it was rendered with.
        pub const EXACT: &str = "aiwatcher.prompt.exact";
    }

    /// What a call cost, where the provider says so.
    ///
    /// A price table can only estimate a call's cost from its tokens; a
    /// provider reporting one is reporting what it charged. The two are
    /// different claims, so this is its own attribute rather than a figure
    /// mixed into an estimate, and its unit is in the name because a sum has
    /// to be in one currency to be a sum.
    pub mod usage {
        pub const COST_USD: &str = "aiwatcher.usage.cost_usd";
    }

    /// Which declared variant answered in the run: Evaluation's content
    /// address of its pins. On every span of a run whose producer named one.
    pub mod variant {
        pub const ID: &str = "aiwatcher.variant.id";
    }

    /// Which project a span belongs to (ADR_0033), from the credential that
    /// published the event that **opened** it — never repointed by what closed
    /// it, so a span's project is its run's.
    ///
    /// Two attributes rather than one key, because a trace store is where this
    /// is read by somebody who wants to filter on an organization: the scope
    /// rides out as an ordinary attribute and neither VictoriaTraces nor Perses
    /// is tenanted by it. Absent on a global span, which is every span this
    /// build has written.
    pub mod project {
        pub const ORGANIZATION: &str = "aiwatcher.project.organization";
        pub const ID: &str = "aiwatcher.project.id";
    }

    /// Which version of a registered model served a call — the training
    /// registry's version, beside `gen_ai.request.model`'s name. What a
    /// variant pinning a model is held to.
    pub mod model {
        pub const VERSION: &str = "aiwatcher.model.version";
    }

    pub mod processor {
        pub const ID: &str = "aiwatcher.processor.id";
        pub const CHECKPOINT: &str = "aiwatcher.processor.checkpoint";
        pub const STATUS: &str = "aiwatcher.processor.status";
    }

    pub mod metrics {
        pub const EVENTS_INGESTED: &str = "aiwatcher.events.ingested";
        pub const EVENTS_DEDUPLICATED: &str = "aiwatcher.events.deduplicated";
        pub const EVENTS_DEAD_LETTERED: &str = "aiwatcher.events.dead_lettered";
        pub const SPANS_WRITTEN: &str = "aiwatcher.spans.written";
        pub const SPANS_ORPHANED: &str = "aiwatcher.spans.orphaned";
        pub const OPEN_SPANS: &str = "aiwatcher.spans.open";
        pub const LIVE_SUBSCRIBERS: &str = "aiwatcher.live.subscribers";
        pub const PROCESSING_DURATION: &str = "aiwatcher.processor.processing.duration";
        /// How far behind the log the observation journal is, in seconds: the
        /// age of the oldest event it read and did not keep, or of the last it
        /// read when it has not caught up.
        pub const JOURNAL_LAG: &str = "aiwatcher.journal.lag";
        /// Positions the journal read and has not kept a page of.
        pub const JOURNAL_UNKEPT: &str = "aiwatcher.journal.unkept_positions";
        /// How long, in seconds, before the log's retention removes the oldest
        /// event the journal has not kept: nought or less is a gap nobody can
        /// refill. Only where the deployment says what that retention is.
        pub const JOURNAL_MARGIN: &str = "aiwatcher.journal.retention_margin";
    }
}
