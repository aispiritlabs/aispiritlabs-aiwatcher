//! The single error type the domain layer raises.
//!
//! Adapters wrap their own transport failures in [`ports::PortError`]; this
//! type covers only what can go wrong while parsing or promoting an event.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, CoreError>;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid {kind} id {value:?}: {reason}")]
    InvalidId {
        kind: &'static str,
        value: String,
        reason: &'static str,
    },

    #[error("invalid stream name {value:?}: expected `<type>:<id>`")]
    InvalidStreamName { value: String },

    #[error("invalid checkpoint {value:?}: {reason}")]
    InvalidCheckpoint { value: String, reason: &'static str },

    #[error("unsupported schema_version {found}, this build understands up to {supported}")]
    UnsupportedSchemaVersion { found: u16, supported: u16 },

    #[error("event {event_id} of type {event_type} is missing required field `{field}`")]
    MissingField {
        event_id: String,
        event_type: String,
        field: &'static str,
    },

    #[error("malformed payload for {event_type}: {source}")]
    MalformedPayload {
        event_type: String,
        #[source]
        source: serde_json::Error,
    },
}
