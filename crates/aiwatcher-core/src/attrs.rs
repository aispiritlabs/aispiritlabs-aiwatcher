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

    pub mod source {
        pub const SERVICE: &str = "aiwatcher.source.service";
        pub const INSTANCE: &str = "aiwatcher.source.instance";
        pub const SDK: &str = "aiwatcher.source.sdk";
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
    }
}
